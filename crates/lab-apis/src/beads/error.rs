//! Beads error type.

#[derive(Debug, thiserror::Error)]
pub enum BeadsError {
    #[error("dolt connection not configured: {message}")]
    NotConfigured { message: String },

    #[error("dolt connection failed: {message}")]
    Connect { message: String },

    #[error("dolt query failed ({command}): {message}")]
    Query { command: String, message: String },

    #[error("invalid identifier `{value}`: {message}")]
    InvalidIdentifier { value: String, message: String },

    #[error("decode error ({command}): {message}")]
    Decode { command: String, message: String },
}

impl From<mysql_async::Error> for BeadsError {
    fn from(err: mysql_async::Error) -> Self {
        Self::Query {
            command: "mysql".to_string(),
            message: err.to_string(),
        }
    }
}

impl From<mysql_async::UrlError> for BeadsError {
    fn from(err: mysql_async::UrlError) -> Self {
        Self::Connect {
            message: err.to_string(),
        }
    }
}
