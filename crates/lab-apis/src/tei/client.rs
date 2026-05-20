//! HuggingFace Text Embeddings Inference HTTP client.

use super::error::TeiError;

/// Instruction prefix for Qwen3-Embedding asymmetric query encoding.
///
/// Prepend to query text before embedding. Do NOT apply to document chunks —
/// document text must be embedded raw. Applies `EmbedKind::Query` semantics.
pub const QUERY_INSTRUCTION: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ";

pub struct TeiClient {
    base_url: String,
    http: reqwest::Client,
}

impl TeiClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Embed a batch of raw strings. Callers are responsible for any instruction prefix.
    ///
    /// Returns one float vector per input string.
    pub async fn embed(&self, inputs: &[&str]) -> Result<Vec<Vec<f32>>, TeiError> {
        let url = format!("{}/embed", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "inputs": inputs }))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(TeiError::Api { status, body });
        }
        Ok(resp.json().await?)
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
}
