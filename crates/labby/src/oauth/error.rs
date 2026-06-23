//! Error types for the local OAuth callback relay.

/// Operator-facing relay failures.
#[derive(Debug, thiserror::Error)]
pub enum OauthRelayError {
    #[error("unknown oauth relay machine `{machine_id}`; available machines: {available}")]
    UnknownMachine {
        machine_id: String,
        available: String,
    },
    #[error("invalid oauth relay target URL `{value}`: {source}")]
    InvalidTargetUrl {
        value: String,
        source: url::ParseError,
    },
    #[error("failed to bind local oauth relay on {bind_addr}: {source}")]
    Bind {
        bind_addr: String,
        source: std::io::Error,
    },
    #[error("oauth relay target `{target}` timed out after {timeout_ms}ms")]
    UpstreamTimeout { target: String, timeout_ms: u64 },
}
