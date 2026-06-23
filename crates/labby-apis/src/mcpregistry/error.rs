//! `RegistryError` for the MCP Registry client.

use crate::core::error::ApiError;

/// Errors returned by [`McpRegistryClient`](super::client::McpRegistryClient).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// Caller supplied an invalid input (e.g. blank server name).
    #[error("invalid input: {message}")]
    InvalidInput { message: String },

    /// Upstream HTTP/transport error or non-success status.
    #[error(transparent)]
    Api(#[from] ApiError),
}
