//! `AcpError` — typed error taxonomy for the ACP domain.

use thiserror::Error;

use super::types::AcpSessionState;

/// Top-level ACP error type.
#[derive(Debug, Error)]
pub enum AcpError {
    #[error("session not found: {session_id}")]
    SessionNotFound { session_id: String },

    #[error("invalid state transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: AcpSessionState,
        to: AcpSessionState,
    },

    #[error("unauthorized: session {session_id} is not owned by caller")]
    Unauthorized { session_id: String },

    #[error("persistence error: {0}")]
    Persistence(#[from] PersistenceError),

    #[error("spawn failed: {0}")]
    SpawnFailed(String),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Persistence-layer errors (SQLite, serialization, I/O).
#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("sqlite error: {0}")]
    Sqlite(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
