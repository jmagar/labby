#[derive(Debug, thiserror::Error)]
pub enum TeiError {
    #[error("TEI request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("TEI error {status}: {body}")]
    Api { status: u16, body: String },
}
