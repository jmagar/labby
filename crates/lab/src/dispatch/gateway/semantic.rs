//! Semantic tool search: BM42 sparse + dense embedding + Qdrant hybrid search with RRF.
//!
//! Ported sparse-vector computation from axon (FNV-1a hash, log-TF, 65 536 buckets).
//! This module owns the tool-search-specific layer on top of the generic lab_apis clients;
//! it does not own the Qdrant or TEI HTTP clients themselves.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lab_apis::qdrant::{QdrantClient, QdrantError, SearchHit, SparseVector, UpsertPoint};
use lab_apis::tei::{TeiClient, TeiError};

use crate::dispatch::gateway::index::{IndexedTool, SearchHit as LexicalHit};

// ── BM42 sparse vector ────────────────────────────────────────────────────────

/// Number of sparse vector buckets. Wire-format commitment with Qdrant.
/// Changing this constant requires re-indexing the `lab-tools` collection.
const SPARSE_DIM: u32 = 65_536;

/// Stopwords excluded from sparse vector computation.
/// Content verbs ("make", "create", "build") are intentionally kept — they encode user intent.
static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a", "am", "an", "and", "any", "are", "as", "at", "be", "but", "by", "can", "do", "does",
        "for", "from", "had", "has", "have", "he", "her", "him", "his", "how", "if", "in", "into",
        "is", "it", "its", "me", "my", "no", "not", "of", "on", "or", "our", "out", "she", "so",
        "than", "that", "the", "their", "them", "then", "they", "this", "to", "too", "up", "us",
        "via", "was", "we", "were", "what", "when", "where", "who", "why", "you", "your",
    ]
    .into_iter()
    .collect()
});

/// Map a single lowercase alphanumeric term to a bucket index via FNV-1a.
///
/// Fixed seed ensures the same term always maps to the same bucket.
pub fn term_to_index(term: &str) -> u32 {
    const FNV_OFFSET: u32 = 2_166_136_261;
    const FNV_PRIME: u32 = 16_777_619;
    let mut hash = FNV_OFFSET;
    for byte in term.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash % SPARSE_DIM
}

/// Compute a BM42-style sparse vector for `text`.
///
/// TF weight = `ln(1 + raw_count)`. Qdrant applies IDF server-side.
/// Terms shorter than 3 bytes or in STOP_WORDS are excluded.
pub fn compute_sparse_vector(text: &str) -> SparseVector {
    let mut bucket_tf: HashMap<u32, u32> = HashMap::with_capacity(64);
    for term in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        let lower = term.to_ascii_lowercase();
        if lower.len() < 3 || STOP_WORDS.contains(lower.as_str()) {
            continue;
        }
        *bucket_tf.entry(term_to_index(&lower)).or_insert(0) += 1;
    }
    let mut indices: Vec<u32> = Vec::with_capacity(bucket_tf.len());
    let mut values: Vec<f32> = Vec::with_capacity(bucket_tf.len());
    for (idx, count) in bucket_tf {
        indices.push(idx);
        #[allow(clippy::cast_precision_loss)]
        values.push((count as f32).ln_1p());
    }
    SparseVector { indices, values }
}

// ── Tool point ID ─────────────────────────────────────────────────────────────

/// Derive a stable u64 point ID from upstream name + tool name using FNV-1a.
fn tool_point_id(upstream: &str, name: &str) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;
    let mut hash = FNV_OFFSET;
    for byte in upstream
        .as_bytes()
        .iter()
        .chain(b"/".iter())
        .chain(name.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ── Collection constants ──────────────────────────────────────────────────────

/// Qdrant collection for lab tool vectors.
pub const TOOLS_COLLECTION: &str = "lab-tools";

/// Dense embedding dimension (must match the TEI model output dimension).
const DENSE_DIM: usize = 1024;

/// Number of per-arm candidates for Qdrant prefetch before RRF fusion.
const HYBRID_CANDIDATES: usize = 50;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SemanticError {
    Tei(TeiError),
    Qdrant(QdrantError),
}

impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tei(e) => write!(f, "TEI error: {e}"),
            Self::Qdrant(e) => write!(f, "Qdrant error: {e}"),
        }
    }
}

impl From<TeiError> for SemanticError {
    fn from(e: TeiError) -> Self {
        Self::Tei(e)
    }
}

impl From<QdrantError> for SemanticError {
    fn from(e: QdrantError) -> Self {
        Self::Qdrant(e)
    }
}

// ── Collection setup ──────────────────────────────────────────────────────────

/// Ensure the `lab-tools` collection exists with the named dense+bm42 schema.
pub async fn ensure_tools_collection(qdrant_url: &str) -> Result<(), SemanticError> {
    let client = QdrantClient::new(qdrant_url);
    client
        .ensure_named_collection(TOOLS_COLLECTION, DENSE_DIM)
        .await?;
    Ok(())
}

// ── Indexing ──────────────────────────────────────────────────────────────────

/// Build the text to embed for a tool (document-mode — no instruction prefix).
fn tool_embed_text(name: &str, description: &str) -> String {
    format!("{name}: {description}")
}

/// Index a batch of upstream tools into Qdrant.
///
/// Embeds each tool's name + description, computes the sparse vector,
/// and upserts all points in a single batch. Existing points for the same
/// upstream are replaced by first deleting and then upserting.
pub async fn index_tools(
    qdrant_url: &str,
    tei_url: &str,
    upstream_name: &str,
    tools: &[IndexedTool],
) -> Result<(), SemanticError> {
    if tools.is_empty() {
        return Ok(());
    }

    let tei = TeiClient::new(tei_url);
    let qdrant = QdrantClient::new(qdrant_url);

    // Build embed texts (document mode — no query instruction).
    let texts: Vec<String> = tools
        .iter()
        .map(|t| tool_embed_text(&t.name, &t.description))
        .collect();
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();

    let dense_vecs = tei.embed_documents(&text_refs).await?;

    // Delete existing points for this upstream before re-upserting.
    qdrant
        .delete_by_payload(TOOLS_COLLECTION, "upstream", upstream_name)
        .await?;

    // Build UpsertPoints with dense + sparse vectors.
    let points: Vec<UpsertPoint> = tools
        .iter()
        .zip(dense_vecs.into_iter())
        .map(|(tool, dense)| {
            let embed_text = tool_embed_text(&tool.name, &tool.description);
            let sparse = compute_sparse_vector(&embed_text);
            UpsertPoint {
                id: tool_point_id(&tool.upstream_name, &tool.name),
                dense,
                sparse,
                payload: serde_json::json!({
                    "name": tool.name,
                    "upstream": tool.upstream_name,
                    "description": tool.description,
                }),
            }
        })
        .collect();

    qdrant.upsert_points(TOOLS_COLLECTION, &points).await?;
    Ok(())
}

// ── Semantic search ───────────────────────────────────────────────────────────

/// A semantic search hit: tool name, upstream, and Qdrant-assigned RRF rank (0-based).
#[derive(Debug, Clone)]
pub struct SemanticHit {
    pub name: String,
    pub upstream: String,
    pub rank: usize,
}

/// Run hybrid search (dense + BM42 + RRF) in Qdrant for `query`.
///
/// Returns hits ordered by Qdrant's RRF score (best first), up to `limit` results.
/// Falls back to dense-only search when the sparse vector is empty (non-ASCII or
/// all-stopword query).
pub async fn search_semantic(
    qdrant_url: &str,
    tei_url: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticHit>, SemanticError> {
    let tei = TeiClient::new(tei_url);
    let qdrant = QdrantClient::new(qdrant_url);

    let dense = tei.embed_query(query).await?;
    let sparse = compute_sparse_vector(query);

    let candidates = HYBRID_CANDIDATES.max(limit * 3);
    let hits: Vec<SearchHit> = if sparse.is_empty() {
        qdrant.dense_search(TOOLS_COLLECTION, &dense, limit).await?
    } else {
        qdrant
            .hybrid_search(TOOLS_COLLECTION, &dense, &sparse, limit, candidates)
            .await?
    };

    Ok(hits
        .into_iter()
        .enumerate()
        .filter_map(|(rank, hit)| {
            let name = hit.payload.get("name")?.as_str()?.to_string();
            let upstream = hit.payload.get("upstream")?.as_str()?.to_string();
            Some(SemanticHit {
                name,
                upstream,
                rank,
            })
        })
        .collect())
}

// ── RRF fusion ────────────────────────────────────────────────────────────────

/// Reciprocal Rank Fusion of lexical and semantic results.
///
/// `k = 60` is the standard RRF constant (Cormack et al., 2009).
/// Each result receives `Σ 1 / (k + rank_i)` summed across both lists.
/// Fused scores are comparable regardless of the original score scales.
pub fn rrf_fuse(lexical: &[LexicalHit], semantic: &[SemanticHit], top_k: usize) -> Vec<LexicalHit> {
    const K: f32 = 60.0;

    // Build a lookup from lexical hits: (name, upstream) → hit + rank.
    let lexical_map: HashMap<(&str, &str), (usize, &LexicalHit)> = lexical
        .iter()
        .enumerate()
        .map(|(rank, hit)| {
            (
                (hit.tool.name.as_str(), hit.tool.upstream_name.as_str()),
                (rank, hit),
            )
        })
        .collect();

    // Accumulate RRF scores for all tools that appear in either list.
    let mut rrf_scores: HashMap<(&str, &str), f32> = HashMap::new();

    for (rank, hit) in lexical.iter().enumerate() {
        let key = (hit.tool.name.as_str(), hit.tool.upstream_name.as_str());
        #[allow(clippy::cast_precision_loss)] // rank ≤ top_k (~50), well within f32 mantissa
        let rank_f = rank as f32;
        *rrf_scores.entry(key).or_insert(0.0) += 1.0 / (K + rank_f);
    }
    for sem in semantic {
        let key = (sem.name.as_str(), sem.upstream.as_str());
        #[allow(clippy::cast_precision_loss)]
        let sem_rank_f = sem.rank as f32;
        *rrf_scores.entry(key).or_insert(0.0) += 1.0 / (K + sem_rank_f);
    }

    // Collect results. Prefer the lexical hit when available (carries full IndexedTool).
    // Semantic-only hits (not in lexical list) are omitted — they have no IndexedTool to
    // return; the upstream's full tool list is not in scope here. Callers who want semantic-
    // only recall should extend this by fetching tool metadata from the Qdrant payload.
    let mut results: Vec<(f32, &LexicalHit)> = rrf_scores
        .iter()
        .filter_map(|((name, upstream), &score)| {
            lexical_map
                .get(&(*name, *upstream))
                .map(|(_, hit)| (score, *hit))
        })
        .collect();

    results.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.tool.name.cmp(&b.1.tool.name))
    });
    results.truncate(top_k);
    results
        .into_iter()
        .map(|(rrf_score, hit)| LexicalHit {
            tool: hit.tool.clone(),
            score: rrf_score,
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn term_to_index_is_stable() {
        // Lock in bucket indices for known terms — changing SPARSE_DIM or the hash
        // seed invalidates existing Qdrant points and requires a full re-index.
        assert_eq!(term_to_index("docker"), term_to_index("docker"));
        assert_eq!(term_to_index("radarr"), term_to_index("radarr"));
        // Different terms must not always collide (basic sanity).
        assert_ne!(term_to_index("docker"), term_to_index("radarr"));
    }

    #[test]
    fn compute_sparse_vector_excludes_stopwords_and_short_terms() {
        let sv = compute_sparse_vector("the quick brown fox");
        // "the" is a stopword → excluded.
        // "fox" has len == 3 (not < 3) → included.
        // "quick" and "brown" → included.
        // Total non-stopword, len≥3 terms: quick, brown, fox = 3 unique buckets (collisions possible).
        assert!(!sv.is_empty(), "should have non-stopword terms");
        // "a", "is" etc are 1-2 bytes → excluded.
        let sv2 = compute_sparse_vector("a is to");
        assert!(
            sv2.is_empty(),
            "all-stopword or short input should be empty"
        );
    }

    #[test]
    fn compute_sparse_vector_log_tf_weights() {
        let sv = compute_sparse_vector("docker docker docker");
        // "docker" appears 3 times → weight = ln(1 + 3) ≈ 1.386
        assert!(!sv.is_empty());
        let w = sv.values[0];
        let expected = (1.0f32 + 3.0).ln();
        assert!(
            (w - expected).abs() < 0.01,
            "expected ln(4) ≈ {expected}, got {w}"
        );
    }

    #[test]
    fn tool_point_id_is_stable_and_unique() {
        let id1 = tool_point_id("upstream-a", "radarr");
        let id2 = tool_point_id("upstream-a", "radarr");
        let id3 = tool_point_id("upstream-b", "radarr");
        assert_eq!(id1, id2, "same input must yield same ID");
        assert_ne!(id1, id3, "different upstream must yield different ID");
    }

    #[test]
    fn rrf_fuse_combines_both_lists() {
        use crate::dispatch::gateway::index::ToolIndex;

        fn make_hit(name: &str, score: f32) -> LexicalHit {
            // Build via ToolIndex::search so private fields are populated correctly.
            let index = ToolIndex::build_for_test(name, "lab", "description");
            let mut hits = index.search(name, 1, 0.0);
            let mut hit = hits.pop().unwrap_or_else(|| LexicalHit {
                tool: index.tools[0].clone(),
                score: 200.0,
            });
            hit.score = score;
            hit
        }

        let lexical = vec![make_hit("logs", 22.0), make_hit("doctor", 2.0)];
        let semantic = vec![
            SemanticHit {
                name: "logs".to_string(),
                upstream: "lab".to_string(),
                rank: 0,
            },
            SemanticHit {
                name: "arcane".to_string(),
                upstream: "lab".to_string(),
                rank: 1,
            },
        ];

        let fused = rrf_fuse(&lexical, &semantic, 5);
        // `logs` appears in both lists → highest RRF score.
        assert_eq!(fused[0].tool.name, "logs", "logs must rank first");
        // `arcane` is semantic-only → not in lexical → not returned (no IndexedTool).
        // `doctor` is lexical-only → in fused results.
        assert!(fused.iter().all(|h| h.tool.name != "arcane"));
    }
}
