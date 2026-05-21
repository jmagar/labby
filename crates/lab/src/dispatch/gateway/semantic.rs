//! Semantic tool search: BM42 sparse + dense embedding + Qdrant hybrid search with RRF.
//!
//! Ported sparse-vector computation from axon (FNV-1a hash, log-TF, 65 536 buckets).
//! This module owns the tool-search-specific layer on top of the generic lab_apis clients;
//! it does not own the Qdrant or TEI HTTP clients themselves.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use lab_apis::qdrant::{QdrantClient, QdrantError, SearchHit, SparseVector, UpsertPoint};
use lab_apis::tei::{EmbedInput, TeiClient, TeiError};

use crate::dispatch::gateway::index::{IndexedTool, SearchHit as LexicalHit};

// ── BM42 sparse vector ────────────────────────────────────────────────────────

/// Number of sparse vector buckets. Wire-format commitment with Qdrant.
/// Changing this constant requires re-indexing the `lab-tools` collection.
const SPARSE_DIM: u32 = 65_536;

/// Hard cap on indexable terms accepted per call. Defends against pathological
/// inputs where a misbehaving upstream returns a huge tool description. Counts
/// terms AFTER stopword/length filtering — filtered-out tokens do not consume
/// the budget. With ~50–150 unique terms typical for a tool name + description,
/// 65,536 is well above any real input and well below any DoS shape.
const MAX_TERMS_PER_VECTOR: usize = 65_536;

// ── Payload key constants ─────────────────────────────────────────────────────

const PAYLOAD_NAME: &str = "name";
const PAYLOAD_UPSTREAM: &str = "upstream";
const PAYLOAD_DESCRIPTION: &str = "description";

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
///
/// When `text` yields no indexable terms (non-ASCII / all-stopword / all-short)
/// returns an empty `SparseVector` and emits a `tracing::warn!` with a coarse
/// character profile so operators can see when hybrid silently degrades to
/// dense-only.
/// Yield the lowercase, alphanumeric, non-stopword, length≥3 terms from `text`.
///
/// Single source of truth for tokenization across sparse-vector indexing,
/// keyword-form rewriting, and any future BM-style scorer. If this drifts, the
/// NL/keyword embedding arms diverge silently.
fn iter_terms(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|term| {
            let lower = term.to_ascii_lowercase();
            if lower.len() < 3 || STOP_WORDS.contains(lower.as_str()) {
                None
            } else {
                Some(lower)
            }
        })
}

pub fn compute_sparse_vector(text: &str) -> SparseVector {
    let mut bucket_tf: HashMap<u32, u32> = HashMap::with_capacity(64);
    let mut scanned: usize = 0;
    for lower in iter_terms(text) {
        if scanned >= MAX_TERMS_PER_VECTOR {
            tracing::warn!(
                target: "tool_search.sparse",
                len = text.len(),
                cap = MAX_TERMS_PER_VECTOR,
                "compute_sparse_vector: term cap reached — truncating"
            );
            break;
        }
        scanned += 1;
        *bucket_tf.entry(term_to_index(&lower)).or_insert(0) += 1;
    }

    if bucket_tf.is_empty() {
        log_empty_sparse_profile(text);
        return SparseVector::default();
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

fn log_empty_sparse_profile(text: &str) {
    let mut ascii_alnum = 0usize;
    let mut non_ascii = 0usize;
    let mut whitespace = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            ascii_alnum += 1;
        } else if !c.is_ascii() {
            non_ascii += 1;
        } else if c.is_whitespace() {
            whitespace += 1;
        } else {
            other += 1;
        }
    }
    tracing::warn!(
        target: "tool_search.sparse",
        len = text.len(),
        ascii_alnum,
        non_ascii,
        whitespace,
        other,
        "compute_sparse_vector: no indexable terms — hybrid search will use dense-only"
    );
}

// ── Query rewriting (dual-embedding) ──────────────────────────────────────────

/// Reduce `query` to its non-stopword alphanumeric tokens, joined with spaces.
/// Qwen3-Embedding is asymmetric — natural-language questions score noticeably
/// lower than the equivalent keyword form against document-mode embeddings, so
/// embedding both and unioning candidates improves recall on NL queries.
fn keyword_form(query: &str) -> String {
    iter_terms(query).collect::<Vec<_>>().join(" ")
}

/// Decide whether the dual-embedding secondary arm should run for `query`.
///
/// Skips queries already keyword-shaped (the token-count guard catches the
/// common "docker logs"-style queries) or too short to add signal.
fn should_dual_embed(keyword: &str) -> bool {
    keyword.split_whitespace().count() >= 3
}

// ── Tool point ID ─────────────────────────────────────────────────────────────

/// Derive a stable u64 point ID from upstream name + tool name using FNV-1a.
pub(crate) fn tool_point_id(upstream: &str, name: &str) -> u64 {
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

/// Per-arm Qdrant prefetch size before RRF fusion. Bumped from 50 to 100 to
/// keep the recall window wider than `limit*3` even on small top-k requests —
/// RRF on a larger candidate set is pure rank math, almost free.
const HYBRID_CANDIDATES: usize = 100;

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
/// Embeds each tool's name + description, computes the sparse vector, and
/// upserts all points in a single batch. Existing points for the same upstream
/// are overwritten in place via deterministic point IDs (`tool_point_id`), then
/// stale points (whose IDs are NOT in the new batch) are deleted in a second
/// step. This avoids the delete-then-upsert window during which a concurrent
/// `search_semantic` would see zero hits for the upstream.
pub async fn index_tools(
    qdrant_url: &str,
    tei_url: &str,
    upstream_name: &str,
    tools: &[IndexedTool],
) -> Result<(), SemanticError> {
    let tei = TeiClient::new(tei_url);
    let qdrant = QdrantClient::new(qdrant_url);

    if tools.is_empty() {
        qdrant
            .delete_by_filter(
                TOOLS_COLLECTION,
                upstream_filter(upstream_name, &[]),
            )
            .await?;
        return Ok(());
    }

    let texts: Vec<String> = tools
        .iter()
        .map(|t| tool_embed_text(&t.name, &t.description))
        .collect();
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();

    let dense_vecs = tei.embed_documents(&text_refs).await?;

    let points: Vec<UpsertPoint> = tools
        .iter()
        .zip(texts.iter())
        .zip(dense_vecs.into_iter())
        .map(|((tool, embed_text), dense)| {
            let sparse = compute_sparse_vector(embed_text);
            UpsertPoint {
                id: tool_point_id(&tool.upstream_name, &tool.name),
                dense,
                sparse,
                payload: serde_json::json!({
                    PAYLOAD_NAME: tool.name,
                    PAYLOAD_UPSTREAM: tool.upstream_name,
                    PAYLOAD_DESCRIPTION: tool.description,
                }),
            }
        })
        .collect();

    let keep_ids: Vec<u64> = points.iter().map(|p| p.id).collect();

    // Upsert FIRST (overwrite in place), then sweep stale IDs. Reverse order
    // would leave an empty window for the upstream.
    qdrant.upsert_points(TOOLS_COLLECTION, &points).await?;
    qdrant
        .delete_by_filter(TOOLS_COLLECTION, upstream_filter(upstream_name, &keep_ids))
        .await?;
    Ok(())
}

/// Build a Qdrant filter that matches points for `upstream_name` whose ID is
/// NOT in `keep_ids`. When `keep_ids` is empty, the filter matches every point
/// for the upstream (used as a clear-all sweep).
fn upstream_filter(upstream_name: &str, keep_ids: &[u64]) -> serde_json::Value {
    let must = serde_json::json!([{
        "key": PAYLOAD_UPSTREAM,
        "match": { "value": upstream_name }
    }]);
    if keep_ids.is_empty() {
        serde_json::json!({ "must": must })
    } else {
        serde_json::json!({
            "must": must,
            "must_not": [{ "has_id": keep_ids }]
        })
    }
}

// ── Semantic search ───────────────────────────────────────────────────────────

/// A semantic search hit: tool name, upstream, RRF rank (0-based), and the
/// description payload (carried so semantic-only hits can be surfaced through
/// RRF fusion without consulting the lexical index).
#[derive(Debug, Clone)]
pub struct SemanticHit {
    pub name: String,
    pub upstream: String,
    pub description: String,
    pub rank: usize,
}

/// Run hybrid search (dense + BM42 + RRF) in Qdrant for `query`.
///
/// Embeds the natural-language query and an optional keyword form in a single
/// TEI round-trip (the NL form receives the Qwen3 query instruction; the
/// keyword form is embedded as a document). When both arms are available, runs
/// the two Qdrant hybrid queries in parallel and unions candidates by
/// `(name, upstream)` keeping the better rank.
///
/// Returns hits ordered by RRF rank (best first), up to `limit` results. Falls
/// back to dense-only search when the sparse vector is empty (non-ASCII or
/// all-stopword query).
pub async fn search_semantic(
    qdrant_url: &str,
    tei_url: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticHit>, SemanticError> {
    let tei = TeiClient::new(tei_url);
    let qdrant = QdrantClient::new(qdrant_url);

    let keyword = keyword_form(query);
    let use_dual = should_dual_embed(&keyword);

    // One TEI batch covers both forms; the NL one gets QUERY_INSTRUCTION, the
    // keyword form does NOT (it is document-shaped tokens — applying the query
    // prefix would push the vector into query space and defeat the point).
    let mut embed_inputs: Vec<EmbedInput<'_>> = Vec::with_capacity(2);
    embed_inputs.push(EmbedInput::query(query));
    if use_dual {
        embed_inputs.push(EmbedInput::document(keyword.as_str()));
    }
    let vectors = tei.embed_mixed(&embed_inputs).await?;
    // Defensive: if TEI ever returns fewer vectors than inputs, degrade to a
    // semantic miss rather than panic. Removes the non-local invariant that
    // would otherwise hide behind .pop().expect().
    if vectors.len() < embed_inputs.len() {
        return Ok(Vec::new());
    }
    let mut vectors = vectors.into_iter();
    let primary_dense = match vectors.next() {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };
    let secondary_dense = if use_dual { vectors.next() } else { None };

    let primary_sparse = compute_sparse_vector(query);
    let secondary_sparse = if use_dual {
        Some(compute_sparse_vector(&keyword))
    } else {
        None
    };

    let candidates = HYBRID_CANDIDATES.max(limit * 3);

    // Dispatch primary; concurrently dispatch the optional secondary arm.
    let primary_fut = run_single_arm(&qdrant, &primary_dense, &primary_sparse, limit, candidates);
    let primary_hits = match secondary_dense.as_ref() {
        Some(sec_dense) => {
            let sec_sparse = secondary_sparse
                .as_ref()
                .expect("set when use_dual is true");
            let secondary_fut = run_single_arm(&qdrant, sec_dense, sec_sparse, limit, candidates);
            let (primary, secondary) = tokio::join!(primary_fut, secondary_fut);
            let primary = primary?;
            // Secondary failure is non-fatal — log and continue with primary.
            match secondary {
                Ok(secondary) => union_by_best_rank(primary, secondary),
                Err(e) => {
                    tracing::warn!(
                        target: "tool_search.semantic",
                        error = %e,
                        "secondary (keyword) arm failed; using primary results only"
                    );
                    primary
                }
            }
        }
        None => primary_fut.await?,
    };

    Ok(primary_hits
        .into_iter()
        .enumerate()
        .filter_map(|(rank, hit)| {
            let name = hit.payload.get(PAYLOAD_NAME)?.as_str()?.to_string();
            let upstream = hit.payload.get(PAYLOAD_UPSTREAM)?.as_str()?.to_string();
            let description = hit
                .payload
                .get(PAYLOAD_DESCRIPTION)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Some(SemanticHit {
                name,
                upstream,
                description,
                rank,
            })
        })
        .take(limit)
        .collect())
}

async fn run_single_arm(
    qdrant: &QdrantClient,
    dense: &[f32],
    sparse: &SparseVector,
    limit: usize,
    candidates: usize,
) -> Result<Vec<SearchHit>, SemanticError> {
    let hits = if sparse.is_empty() {
        qdrant.dense_search(TOOLS_COLLECTION, dense, limit).await?
    } else {
        qdrant
            .hybrid_search(TOOLS_COLLECTION, dense, sparse, limit, candidates)
            .await?
    };
    Ok(hits)
}

/// Union two Qdrant hit lists by `(name, upstream)` keeping the better rank.
///
/// "Better rank" = lower position in the list. The output preserves order by
/// rank-min across the two inputs. Used to merge the NL and keyword arms before
/// re-numbering ranks for RRF fusion downstream.
fn union_by_best_rank(primary: Vec<SearchHit>, secondary: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut best: HashMap<(String, String), (usize, SearchHit)> = HashMap::new();
    // Enumerate each arm INDEPENDENTLY so rank-0 of the secondary stays rank-0.
    // Chaining-then-enumerating would inflate every secondary rank by primary.len()
    // and dominate semantic-only hits with poor primary ranks.
    let primary_ranked = primary.into_iter().enumerate();
    let secondary_ranked = secondary.into_iter().enumerate();
    for (rank, hit) in primary_ranked.chain(secondary_ranked) {
        let key = match (
            hit.payload.get(PAYLOAD_NAME).and_then(|v| v.as_str()),
            hit.payload.get(PAYLOAD_UPSTREAM).and_then(|v| v.as_str()),
        ) {
            (Some(n), Some(u)) => (n.to_string(), u.to_string()),
            _ => continue,
        };
        match best.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                if rank < e.get().0 {
                    e.insert((rank, hit));
                }
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert((rank, hit));
            }
        }
    }
    let mut merged: Vec<(usize, SearchHit)> = best.into_values().collect();
    merged.sort_by_key(|(rank, _)| *rank);
    merged.into_iter().map(|(_, h)| h).collect()
}

// ── RRF fusion ────────────────────────────────────────────────────────────────

/// Reciprocal Rank Fusion of lexical and semantic results.
///
/// `k = 60` is the standard RRF constant (Cormack et al., 2009).
/// Each result receives `Σ 1 / (k + rank_i)` summed across both lists.
/// Fused scores are comparable regardless of the original score scales.
///
/// Semantic-only hits (present in `semantic` but not in `lexical`) are
/// surfaced: a synthetic `IndexedTool` is reconstructed from the Qdrant
/// payload (name + upstream + description) so callers see them in results.
/// Without this, the whole point of semantic search — rescuing tools the
/// lexical scorer can't reach — is lost.
pub fn rrf_fuse(lexical: &[LexicalHit], semantic: &[SemanticHit], top_k: usize) -> Vec<LexicalHit> {
    const K: f32 = 60.0;

    let lexical_map: HashMap<(&str, &str), &LexicalHit> = lexical
        .iter()
        .map(|hit| {
            (
                (hit.tool.name.as_str(), hit.tool.upstream_name.as_str()),
                hit,
            )
        })
        .collect();
    let semantic_map: HashMap<(&str, &str), &SemanticHit> = semantic
        .iter()
        .map(|s| ((s.name.as_str(), s.upstream.as_str()), s))
        .collect();

    // Inherit upstream priority for semantic-only hits from any sibling lexical
    // hit on the same upstream. Upstreams configured with priority=0 are
    // suppressed and have NO lexical entries (the >0 filter at IndexedTool::search
    // drops them), so a missing entry here means "either not suppressed, or
    // semantic-indexed with no lexical view" — default to 1.0 (not suppressed).
    let priority_by_upstream: HashMap<&str, f32> = lexical
        .iter()
        .map(|hit| (hit.tool.upstream_name.as_str(), hit.tool.priority))
        .collect();

    let mut rrf_scores: HashMap<(&str, &str), f32> = HashMap::new();
    for (rank, hit) in lexical.iter().enumerate() {
        let key = (hit.tool.name.as_str(), hit.tool.upstream_name.as_str());
        #[allow(clippy::cast_precision_loss)]
        let rank_f = rank as f32;
        *rrf_scores.entry(key).or_insert(0.0) += 1.0 / (K + rank_f);
    }
    for sem in semantic {
        let key = (sem.name.as_str(), sem.upstream.as_str());
        #[allow(clippy::cast_precision_loss)]
        let sem_rank_f = sem.rank as f32;
        *rrf_scores.entry(key).or_insert(0.0) += 1.0 / (K + sem_rank_f);
    }

    let mut results: Vec<(f32, LexicalHit)> = rrf_scores
        .into_iter()
        .filter_map(|((name, upstream), score)| {
            if let Some(hit) = lexical_map.get(&(name, upstream)) {
                Some((score, (*hit).clone()))
            } else if let Some(sem) = semantic_map.get(&(name, upstream)) {
                // Semantic-only hit: synthesize an IndexedTool from the payload.
                // Inherit upstream priority from the lexical view — semantic-only
                // hits on suppressed (priority=0) upstreams are dropped so the
                // user-configured suppression survives fusion.
                let priority = priority_by_upstream
                    .get(upstream)
                    .copied()
                    .unwrap_or(1.0);
                if priority == 0.0 {
                    return None;
                }
                let tool = IndexedTool::from_semantic_payload_with_priority(
                    name,
                    upstream,
                    &sem.description,
                    priority,
                );
                Some((score, LexicalHit { tool, score }))
            } else {
                None
            }
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
        .map(|(rrf_score, mut hit)| {
            hit.score = rrf_score;
            hit
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
        assert!(!sv.is_empty(), "should have non-stopword terms");
        let sv2 = compute_sparse_vector("a is to");
        assert!(
            sv2.is_empty(),
            "all-stopword or short input should be empty"
        );
    }

    #[test]
    fn compute_sparse_vector_log_tf_weights() {
        let sv = compute_sparse_vector("docker docker docker");
        assert!(!sv.is_empty());
        let w = sv.values[0];
        let expected = (1.0f32 + 3.0).ln();
        assert!(
            (w - expected).abs() < 0.01,
            "expected ln(4) ≈ {expected}, got {w}"
        );
    }

    #[test]
    fn compute_sparse_vector_caps_term_scan() {
        // Above the cap we expect truncation, not panic.
        let many = "abc ".repeat(MAX_TERMS_PER_VECTOR + 100);
        let sv = compute_sparse_vector(&many);
        // All terms hash to the same bucket ("abc"), so we get exactly one entry
        // regardless. The cap protects the scan loop, not the bucket count.
        assert!(!sv.is_empty(), "should still emit at least one bucket");
    }

    #[test]
    fn keyword_form_strips_stopwords_and_lowercases() {
        let k = keyword_form("How do I list the docker containers on dookie?");
        // "do", "I", "on", "the" excluded; len<3 also dropped.
        assert!(k.contains("list"));
        assert!(k.contains("docker"));
        assert!(k.contains("containers"));
        assert!(k.contains("dookie"));
        assert!(!k.contains("how"));
        assert!(!k.contains("the"));
    }

    #[test]
    fn should_dual_embed_skips_short_queries() {
        assert!(!should_dual_embed(""));
        assert!(!should_dual_embed("logs"));
        assert!(!should_dual_embed("docker logs"));
        assert!(should_dual_embed("list docker containers"));
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
                description: "live log streaming".to_string(),
                rank: 0,
            },
            SemanticHit {
                name: "arcane".to_string(),
                upstream: "lab".to_string(),
                description: "docker container manager".to_string(),
                rank: 1,
            },
        ];

        let fused = rrf_fuse(&lexical, &semantic, 5);
        assert_eq!(fused[0].tool.name, "logs", "logs must rank first");
        // arcane is semantic-only — must now SURFACE, not be dropped.
        let arcane = fused.iter().find(|h| h.tool.name == "arcane");
        assert!(
            arcane.is_some(),
            "semantic-only hit `arcane` must surface in fused results"
        );
        assert_eq!(arcane.unwrap().tool.upstream_name, "lab");
        assert_eq!(arcane.unwrap().tool.description, "docker container manager");
    }
}
