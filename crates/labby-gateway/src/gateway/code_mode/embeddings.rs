//! TEI (Text Embeddings Inference) HTTP client and cosine-similarity ranking
//! for Code Mode's semantic search blend.
//!
//! All vector math lives here, host-side — no raw floats are ever serialized
//! into the QuickJS sandbox. Every function here is designed to be wrapped in
//! a fail-open caller (see `code_mode_host.rs::semantic_rank`); this module
//! itself returns ordinary `Result`s and does not implement the
//! cooldown/fail-open policy — that is the caller's responsibility.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use labby_runtime::error::ToolError;

/// TEI's confirmed hard server-side limit on inputs per `/embed` call
/// (`max_batch_requests` in `GET /info`). `embed_via_tei` chunks any larger
/// input list into batches of at most this size.
pub(crate) const TEI_MAX_BATCH_SIZE: usize = 512;

/// Per-request timeout for one `POST /embed` call. Hardcoded, not
/// configurable — see the plan's YAGNI rationale (the one required knob is
/// `tei_url`; timeout/cooldown are engineering constants).
pub(crate) const TEI_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum accepted TEI response body size before JSON decoding. Guards
/// against a misbehaving or compromised TEI endpoint forcing unbounded
/// memory use.
pub(crate) const TEI_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct TeiEmbedResponse(Vec<Vec<f32>>);

/// Batch-embed `texts` via one or more `POST {url}/embed` calls, chunked to
/// at most `TEI_MAX_BATCH_SIZE` inputs per request (TEI's hard server-side
/// limit). Returns one vector per input text, in input order.
pub(crate) async fn embed_via_tei(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let mut all_vectors = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(TEI_MAX_BATCH_SIZE) {
        let vectors = embed_batch(url, chunk).await?;
        all_vectors.extend(vectors);
    }
    Ok(all_vectors)
}

async fn embed_batch(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError> {
    let client = reqwest::Client::new();
    let endpoint = format!("{}/embed", url.trim_end_matches('/'));
    let response = client
        .post(&endpoint)
        .timeout(TEI_REQUEST_TIMEOUT)
        .json(&json!({ "inputs": texts }))
        .send()
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("TEI request failed: {err}"),
        })?;
    if !response.status().is_success() {
        return Err(ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message: format!("TEI returned HTTP {}", response.status()),
        });
    }
    let body = response.bytes().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "network_error".to_string(),
        message: format!("failed to read TEI response body: {err}"),
    })?;
    if body.len() > TEI_MAX_RESPONSE_BYTES {
        return Err(ToolError::Sdk {
            sdk_kind: "decode_error".to_string(),
            message: format!(
                "TEI response body is {} bytes, exceeding the {} byte cap",
                body.len(),
                TEI_MAX_RESPONSE_BYTES
            ),
        });
    }
    let parsed: TeiEmbedResponse = serde_json::from_slice(&body).map_err(|err| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("failed to decode TEI /embed response: {err}"),
    })?;
    if parsed.0.len() != texts.len() {
        return Err(ToolError::Sdk {
            sdk_kind: "decode_error".to_string(),
            message: format!(
                "TEI returned {} vectors for {} inputs",
                parsed.0.len(),
                texts.len()
            ),
        });
    }
    Ok(parsed.0)
}

/// Cosine similarity between two equal-length vectors. Returns `0.0` for a
/// zero-magnitude vector (rather than dividing by zero / NaN) — this can
/// legitimately happen for a degenerate embedding and should score as "no
/// similarity", not poison the sort with NaN.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(-1.0, 1.0)
}

/// Rank catalog entries by cosine similarity to `query_vector`. Returns
/// `(id, similarity)` pairs sorted descending by similarity — callers decide
/// how many to keep.
pub(crate) fn rank_by_similarity(
    query_vector: &[f32],
    catalog_vectors: &[(String, Vec<f32>)],
) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = catalog_vectors
        .iter()
        .map(|(id, vector)| (id.clone(), cosine_similarity(query_vector, vector)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors_is_negative_one() {
        let v = vec![1.0, 2.0, 3.0];
        let neg: Vec<f32> = v.iter().map(|x| -x).collect();
        assert!((cosine_similarity(&v, &neg) - -1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector_returns_zero_not_nan() {
        let result = cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]);
        assert_eq!(result, 0.0);
        assert!(!result.is_nan());
    }

    #[test]
    fn cosine_similarity_mismatched_lengths_returns_zero() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn rank_by_similarity_sorts_descending() {
        let query = vec![1.0, 0.0];
        let catalog = vec![
            ("low".to_string(), vec![0.0, 1.0]),
            ("high".to_string(), vec![1.0, 0.0]),
            ("mid".to_string(), vec![0.7, 0.7]),
        ];
        let ranked = rank_by_similarity(&query, &catalog);
        assert_eq!(ranked[0].0, "high");
        assert_eq!(ranked[2].0, "low");
    }

    #[tokio::test]
    async fn embed_via_tei_empty_input_returns_empty_without_http_call() {
        let result = embed_via_tei("http://127.0.0.1:1", &[]).await;
        assert_eq!(result.unwrap(), Vec::<Vec<f32>>::new());
    }

    #[tokio::test]
    async fn embed_via_tei_unreachable_server_returns_network_error() {
        // Port 1 is a reserved/unused low port — connection refused, fast.
        let result = embed_via_tei("http://127.0.0.1:1", &["test".to_string()]).await;
        assert!(result.is_err());
    }

    #[test]
    fn tei_max_batch_size_matches_documented_tei_limit() {
        // Regression guard: this constant must track TEI's real
        // max_batch_requests (currently 512, confirmed via GET /info against
        // the live dev TEI server). If TEI's limit changes, update here.
        assert_eq!(TEI_MAX_BATCH_SIZE, 512);
    }
}
