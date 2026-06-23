//! Typed errors for the `setup` service.

use thiserror::Error;

/// Library-side errors. The dispatch layer maps these into stable
/// envelope `kind` strings (see `crates/lab/src/dispatch/setup/`).
#[derive(Debug, Error)]
pub enum SetupError {
    #[error("missing required parameter: {0}")]
    MissingParam(String),

    #[error("invalid value for {field}: {reason}")]
    InvalidValue { field: String, reason: String },

    #[error("unknown service: {0}")]
    UnknownService(String),
}
