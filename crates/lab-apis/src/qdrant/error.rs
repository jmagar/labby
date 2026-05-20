#[derive(Debug, thiserror::Error)]
pub enum QdrantError {
    #[error("Qdrant request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Qdrant error {status}: {body}")]
    Api { status: u16, body: String },
    #[error("Qdrant URL parse error: {0}")]
    Url(String),
    #[error("Qdrant response parse error: {0}")]
    Parse(#[from] serde_json::Error),
}
