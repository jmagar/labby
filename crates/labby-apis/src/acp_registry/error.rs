//! ACP Registry-specific errors.

use crate::core::error::ApiError;

/// Errors returned by [`AcpRegistryClient`](super::client::AcpRegistryClient).
#[derive(Debug, thiserror::Error)]
pub enum AcpRegistryError {
    /// Upstream HTTP/transport error or non-success status.
    #[error("HTTP request failed: {0}")]
    Request(#[from] ApiError),
    /// The server returned a non-success status with a body.
    #[error("API error {status}: {body}")]
    Api { status: u16, body: String },
}
