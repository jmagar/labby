//! HuggingFace Text Embeddings Inference HTTP client.

use std::sync::LazyLock;
use std::time::Duration;

use super::error::TeiError;

/// Instruction prefix for Qwen3-Embedding asymmetric query encoding.
///
/// Prepend to query text before embedding. Do NOT apply to document chunks —
/// document text must be embedded raw. Applies `EmbedKind::Query` semantics.
pub const QUERY_INSTRUCTION: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ";

/// Shared HTTP client. `reqwest::Client` owns a connection pool; constructing one
/// per call exhausts sockets and bypasses pooling.
///
/// Builder failure (rustls/TLS init only) falls back to `reqwest::Client::new()`
/// — losing the tuned pool/timeout settings but preserving forward progress.
/// Library code MUST NOT panic at init.
static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Max attempts (initial + retries) for transient TEI failures (429/5xx/transport).
const MAX_ATTEMPTS: usize = 3;

/// Base backoff before retry. Doubles each attempt; jitter added per attempt.
const BACKOFF_BASE_MS: u64 = 250;

pub struct TeiClient {
    base_url: String,
}

impl TeiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Embed a batch of raw strings. Callers are responsible for any instruction prefix.
    ///
    /// Returns one float vector per input string. Retries up to [`MAX_ATTEMPTS`] times
    /// on transport errors, HTTP 429, and HTTP 5xx with exponential backoff + jitter.
    pub async fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, TeiError> {
        let url = format!("{}/embed", self.base_url);
        let body = serde_json::json!({ "inputs": inputs });

        let mut last_err: Option<TeiError> = None;
        for attempt in 0..MAX_ATTEMPTS {
            match HTTP_CLIENT.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(resp.json().await?);
                    }
                    let code = status.as_u16();
                    let body_text = resp.text().await.unwrap_or_default();
                    let retryable = code == 429 || (500..=599).contains(&code);
                    let err = TeiError::Api {
                        status: code,
                        body: body_text,
                    };
                    if !retryable || attempt + 1 == MAX_ATTEMPTS {
                        return Err(err);
                    }
                    tracing::warn!(
                        target: "tei",
                        status = code,
                        attempt = attempt + 1,
                        max_attempts = MAX_ATTEMPTS,
                        "TEI request failed with retryable status, backing off"
                    );
                    last_err = Some(err);
                }
                Err(e) => {
                    if attempt + 1 == MAX_ATTEMPTS {
                        return Err(TeiError::Request(e));
                    }
                    tracing::warn!(
                        target: "tei",
                        attempt = attempt + 1,
                        max_attempts = MAX_ATTEMPTS,
                        error = %e,
                        "TEI transport error, backing off"
                    );
                    last_err = Some(TeiError::Request(e));
                }
            }
            backoff_sleep(attempt).await;
        }
        Err(last_err.expect("loop body sets last_err on every failure path"))
    }

    /// Embed a single query string, prepending the Qwen3 query instruction prefix.
    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>, TeiError> {
        let prefixed = format!("{QUERY_INSTRUCTION}{query}");
        let mut vecs = self.embed(&[prefixed.as_str()]).await?;
        Ok(vecs.pop().unwrap_or_default())
    }

    /// Embed a batch of document strings (no instruction prefix).
    pub async fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, TeiError> {
        self.embed(texts).await
    }

    /// Embed a heterogeneous batch where each input is either a query (prefixed)
    /// or a document (raw). One TEI round-trip; preserves input order.
    ///
    /// Used by the dual-embedding tool-search path where the NL form needs the
    /// Qwen3 query instruction but the keyword form must be embedded raw.
    pub async fn embed_mixed(&self, inputs: &[EmbedInput<'_>]) -> Result<Vec<Vec<f32>>, TeiError> {
        let prepared: Vec<String> = inputs
            .iter()
            .map(|input| match input.kind {
                EmbedKind::Query => format!("{QUERY_INSTRUCTION}{}", input.text),
                EmbedKind::Document => input.text.to_string(),
            })
            .collect();
        let refs: Vec<&str> = prepared.iter().map(String::as_str).collect();
        self.embed(&refs).await
    }
}

/// Tag for [`TeiClient::embed_mixed`] inputs — controls whether the Qwen3 query
/// instruction prefix is applied.
#[derive(Debug, Clone, Copy)]
pub enum EmbedKind {
    Query,
    Document,
}

/// One input for [`TeiClient::embed_mixed`].
#[derive(Debug, Clone, Copy)]
pub struct EmbedInput<'a> {
    pub kind: EmbedKind,
    pub text: &'a str,
}

impl<'a> EmbedInput<'a> {
    pub fn query(text: &'a str) -> Self {
        Self {
            kind: EmbedKind::Query,
            text,
        }
    }

    pub fn document(text: &'a str) -> Self {
        Self {
            kind: EmbedKind::Document,
            text,
        }
    }
}

async fn backoff_sleep(attempt: usize) {
    let base = BACKOFF_BASE_MS.saturating_mul(1u64 << attempt.min(6));
    tokio::time::sleep(Duration::from_millis(base + jitter_ms())).await;
}

/// Deterministic-per-call but high-entropy jitter in `0..BACKOFF_BASE_MS` ms.
///
/// Mixes wall-clock nanos with process id so concurrent retriers on the same
/// host don't all sleep the same amount. `Instant::now().elapsed()` was used
/// previously and is effectively zero — successive calls return ~0ns because
/// `now - now` is just monotonic-clock overhead.
fn jitter_ms() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    // Cheap mix — no PRNG dep needed for jitter quality.
    let mixed = now.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(pid);
    mixed % BACKOFF_BASE_MS
}
