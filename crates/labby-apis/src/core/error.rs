//! Canonical error taxonomy.
//!
//! `ApiError::kind()` returns one of a small set of `&'static str` tags
//! so the binary can map any service error into the
//! structured MCP envelope and the CLI can render consistent messages.

use std::time::Duration;

use thiserror::Error;

/// Shared error type returned by every service client.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Authentication failed (401/403).
    #[error("authentication failed")]
    Auth,

    /// Resource not found (404).
    #[error("not found")]
    NotFound,

    /// Rate limited by upstream (429). May carry an upstream `Retry-After`.
    #[error("rate limited")]
    RateLimited {
        /// Suggested wait before the next attempt, from `Retry-After`.
        retry_after: Option<Duration>,
    },

    /// Request was rejected for a domain reason (422 / business rule).
    #[error("validation failed: {field}: {message}")]
    Validation {
        /// Offending field name.
        field: String,
        /// Human-readable reason.
        message: String,
    },

    /// Transport-level failure (DNS, TCP, TLS, body read).
    #[error("network error: {0}")]
    Network(String),

    /// 5xx response from upstream.
    #[error("server error {status}: {body}")]
    Server {
        /// HTTP status code.
        status: u16,
        /// Response body or status text.
        body: String,
    },

    /// Failed to deserialize a response body.
    #[error("decode error: {0}")]
    Decode(String),

    /// Programmer error: invariant violated, unreachable state, etc.
    #[error("internal: {0}")]
    Internal(String),
}

impl ApiError {
    /// Stable string tag for the MCP error envelope.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Auth => "auth_failed",
            Self::NotFound => "not_found",
            Self::RateLimited { .. } => "rate_limited",
            Self::Validation { .. } => "validation_failed",
            Self::Network(_) => "network_error",
            Self::Server { .. } => "server_error",
            Self::Decode(_) => "decode_error",
            Self::Internal(_) => "internal_error",
        }
    }
}
