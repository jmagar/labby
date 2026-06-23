//! Deploy-specific error taxonomy.
//!
//! Every variant has a stable `kind()` tag that appears verbatim in MCP and
//! HTTP error envelopes. Adding a new kind is a spec change — update
//! `docs/dev/ERRORS.md` at the same time.

use thiserror::Error;

use crate::core::ApiError;

/// Errors surfaced by the deploy service.
///
/// The `redacted_message()` helper is what escapes to the MCP/HTTP envelope;
/// the `Display` impl (via `thiserror::Error`) includes full structured
/// detail and is safe to log at WARN locally.
#[derive(Debug, Error)]
pub enum DeployError {
    #[error("validation_failed: field={field} reason={reason}")]
    ValidationFailed { field: String, reason: String },

    #[error("ssh_unreachable: host={host}")]
    SshUnreachable { host: String },

    #[error("build_failed: reason={reason}")]
    BuildFailed { reason: String },

    #[error("preflight_failed: host={host} reason={reason}")]
    PreflightFailed { host: String, reason: String },

    #[error("transfer_failed: host={host} reason={reason}")]
    TransferFailed { host: String, reason: String },

    #[error("install_failed: host={host} reason={reason}")]
    InstallFailed { host: String, reason: String },

    #[error("restart_failed: host={host} reason={reason}")]
    RestartFailed { host: String, reason: String },

    #[error("verify_failed: host={host} reason={reason}")]
    VerifyFailed { host: String, reason: String },

    #[error("partial_failure: failed={failed}")]
    PartialFailure { failed: usize },

    #[error("conflict: host={host} already in progress")]
    Conflict { host: String },

    #[error("arch_mismatch: host={host} local={local} remote={remote}")]
    ArchMismatch {
        host: String,
        local: String,
        remote: String,
    },

    #[error("integrity_mismatch: host={host} sha256 mismatch between local and remote artifact")]
    IntegrityMismatch { host: String },

    #[error("auth_failed: {reason}")]
    AuthFailed { reason: String },

    #[error(transparent)]
    Api(#[from] ApiError),
}

impl DeployError {
    /// Stable kind tag exposed to callers via MCP / HTTP.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ValidationFailed { .. } => "validation_failed",
            Self::SshUnreachable { .. } => "ssh_unreachable",
            Self::BuildFailed { .. } => "build_failed",
            Self::PreflightFailed { .. } => "preflight_failed",
            Self::TransferFailed { .. } => "transfer_failed",
            Self::InstallFailed { .. } => "install_failed",
            Self::RestartFailed { .. } => "restart_failed",
            Self::VerifyFailed { .. } => "verify_failed",
            Self::PartialFailure { .. } => "partial_failure",
            Self::Conflict { .. } => "conflict",
            Self::ArchMismatch { .. } => "arch_mismatch",
            Self::IntegrityMismatch { .. } => "integrity_mismatch",
            Self::AuthFailed { .. } => "auth_failed",
            Self::Api(e) => e.kind(),
        }
    }

    /// Produce a redacted description safe to return through MCP/HTTP
    /// envelopes. Full structured detail is still available via `Display`.
    #[must_use]
    pub fn redacted_message(&self) -> String {
        match self {
            Self::ValidationFailed { field, .. } => {
                format!("validation failed for field `{field}`")
            }
            Self::SshUnreachable { .. } => "ssh host unreachable".into(),
            Self::BuildFailed { .. } => "local build failed".into(),
            Self::PreflightFailed { .. } => "preflight check failed".into(),
            Self::TransferFailed { .. } => "artifact transfer failed".into(),
            Self::InstallFailed { .. } => "atomic install failed".into(),
            Self::RestartFailed { .. } => "service restart failed".into(),
            Self::VerifyFailed { .. } => "post-install verification failed".into(),
            Self::PartialFailure { failed } => format!("{failed} host(s) failed"),
            Self::Conflict { .. } => "another deploy is in progress for this host".into(),
            Self::ArchMismatch { .. } => {
                "architecture mismatch between build host and target".into()
            }
            Self::IntegrityMismatch { .. } => "artifact integrity check failed on target".into(),
            Self::AuthFailed { .. } => "authentication failed".into(),
            Self::Api(e) => e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kinds_are_stable_strings() {
        for (err, expected) in [
            (
                DeployError::ValidationFailed {
                    field: "x".into(),
                    reason: "bad".into(),
                },
                "validation_failed",
            ),
            (
                DeployError::SshUnreachable {
                    host: "mini1".into(),
                },
                "ssh_unreachable",
            ),
            (
                DeployError::BuildFailed {
                    reason: "rustc".into(),
                },
                "build_failed",
            ),
            (
                DeployError::PreflightFailed {
                    host: "mini1".into(),
                    reason: "no_disk".into(),
                },
                "preflight_failed",
            ),
            (
                DeployError::TransferFailed {
                    host: "mini1".into(),
                    reason: "drop".into(),
                },
                "transfer_failed",
            ),
            (
                DeployError::InstallFailed {
                    host: "mini1".into(),
                    reason: "rename".into(),
                },
                "install_failed",
            ),
            (
                DeployError::RestartFailed {
                    host: "mini1".into(),
                    reason: "unit".into(),
                },
                "restart_failed",
            ),
            (
                DeployError::VerifyFailed {
                    host: "mini1".into(),
                    reason: "exit".into(),
                },
                "verify_failed",
            ),
            (DeployError::PartialFailure { failed: 1 }, "partial_failure"),
            (
                DeployError::Conflict {
                    host: "mini1".into(),
                },
                "conflict",
            ),
            (
                DeployError::ArchMismatch {
                    host: "mini1".into(),
                    local: "x86_64".into(),
                    remote: "aarch64".into(),
                },
                "arch_mismatch",
            ),
            (
                DeployError::IntegrityMismatch {
                    host: "mini1".into(),
                },
                "integrity_mismatch",
            ),
            (
                DeployError::AuthFailed {
                    reason: "token".into(),
                },
                "auth_failed",
            ),
            (
                DeployError::Api(ApiError::Internal("sdk failure".into())),
                "internal_error",
            ),
        ] {
            assert_eq!(err.kind(), expected);
        }
    }

    #[test]
    fn redacted_message_does_not_leak_host_or_reason_detail() {
        let e = DeployError::TransferFailed {
            host: "sensitive-host-1.internal".into(),
            reason: "rsync: [::1] dropped".into(),
        };
        let m = e.redacted_message();
        assert!(!m.contains("sensitive-host-1"));
        assert!(!m.contains("rsync"));
    }
}
