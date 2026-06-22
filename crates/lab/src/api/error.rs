//! HTTP error handling.
//!
//! `ToolError` from `crate::dispatch::error` is the canonical error type for
//! all surfaces (MCP, API, CLI). It now lives in the `lab-runtime` crate, so
//! the HTTP status-code mapping cannot hang off an `impl IntoResponse for
//! ToolError` here (orphan rule). Instead, `ApiError` is a thin local newtype
//! around `ToolError` that carries the `IntoResponse` mapping; axum route
//! handlers return `Result<_, ApiError>` and rely on `From<ToolError>` so `?`
//! still propagates dispatch-layer `ToolError`s transparently.
//!
//! The serialized error envelope is byte-identical to the MCP envelope —
//! `IntoResponse` serializes the inner `ToolError` directly and derives the
//! HTTP status from `kind()`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub use crate::dispatch::error::ToolError;

/// HTTP-surface newtype wrapper around the surface-neutral [`ToolError`].
///
/// `ToolError` now lives in the `lab-runtime` crate, so an
/// `impl IntoResponse for ToolError` here would violate Rust's orphan rule
/// (both the trait and the type would be foreign to `lab`). `ApiError` is a
/// thin, local newtype that carries the `IntoResponse` mapping instead. axum
/// route handlers return `Result<_, ApiError>`; `From<ToolError>` lets `?`
/// propagate `ToolError` from the shared dispatch layer transparently.
pub struct ApiError(pub ToolError);

impl From<ToolError> for ApiError {
    fn from(e: ToolError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.kind() {
            "auth_failed" => StatusCode::UNAUTHORIZED,
            "not_found" => StatusCode::NOT_FOUND,
            "rate_limited" | "queue_saturated" | "session_limit_exceeded" | "too_many_subscribers" => StatusCode::TOO_MANY_REQUESTS,
            "sync_in_progress" | "service_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
            "missing_param" | "invalid_param" | "validation_failed" => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            "confirmation_required" => StatusCode::UNPROCESSABLE_ENTITY,
            "ssrf_blocked" | "no_remote_transport" => StatusCode::UNPROCESSABLE_ENTITY,
            // lab-zxx5.18 install hardening kinds. All user-caller errors with
            // a specific remediation; 422 for validation-style, 413 for size,
            // 504 for timeouts.
            "symlink_rejected"
            | "path_traversal"
            | "path_traversal_rejected"
            | "invalid_encoding"
            // marketplace/stash validation-style caller errors
            | "missing_env_values"
            | "unsupported_runtime_hint"
            | "unsupported_registry_type" => StatusCode::UNPROCESSABLE_ENTITY,
            "content_too_large" | "preview_truncated" | "too_many_files" => {
                StatusCode::PAYLOAD_TOO_LARGE
            }
            // Advertised-but-unimplemented actions (artifact.diff/artifact.patch)
            // and unsupported operations.
            "not_implemented" | "not_supported" => StatusCode::NOT_IMPLEMENTED,
            "secrets_export_not_allowed" => StatusCode::FORBIDDEN,
            "install_timeout" | "timeout" | "code_mode_timeout" | "code_mode_fuel_exhausted" => {
                StatusCode::GATEWAY_TIMEOUT
            }
            "oauth_needs_reauth" => StatusCode::UNAUTHORIZED,
            "oauth_state_invalid" => StatusCode::BAD_REQUEST,
            "forbidden" | "dev_preview_read_only" => StatusCode::FORBIDDEN,
            "unknown_action" | "unknown_subaction" | "unknown_instance" | "unknown_target" => {
                StatusCode::BAD_REQUEST
            }
            "network_error"
            | "bad_gateway"
            | "server_error"
            | "upstream_error"
            | "oauth_resource_mismatch"
            | "oauth_issuer_mismatch"
            | "oauth_unsupported_method"
            // Deploy-specific kinds (feature-gated service, HTTP surface pending).
            // Registered here so status codes are correct when the HTTP route is wired.
            | "ssh_unreachable"
            | "build_failed"
            | "preflight_failed"
            | "transfer_failed"
            | "install_failed"
            | "restart_failed"
            | "verify_failed"
            | "arch_mismatch"
            | "integrity_missing"
            | "integrity_mismatch"
            | "deploy_failed"
            | "not_connected" => StatusCode::BAD_GATEWAY,
            "conflict" | "ambiguous_tool" | "restart_required" => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Serialize the inner ToolError directly — byte-identical to the MCP
        // error envelope.
        let body = serde_json::to_value(&self.0).unwrap_or_else(|_| {
            serde_json::json!({"kind": "internal_error", "message": "error serialization failed"})
        });

        // RFC 9728: WWW-Authenticate on 401 responses requires the resolved
        // resource_url from AppState. IntoResponse has no access to state, so
        // the auth middleware in router.rs is responsible for adding the header.
        // We omit it here rather than advertising a wrong (localhost) URL.
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    use super::{ApiError, ToolError};

    fn status_for(kind: &str) -> StatusCode {
        ApiError(ToolError::Sdk {
            sdk_kind: kind.to_string(),
            message: "x".to_string(),
        })
        .into_response()
        .status()
    }

    #[test]
    fn marketplace_stash_kinds_map_to_expected_status() {
        assert_eq!(status_for("not_implemented"), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(status_for("not_supported"), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            status_for("preview_truncated"),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(status_for("too_many_files"), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            status_for("secrets_export_not_allowed"),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_for("missing_env_values"),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(status_for("unknown_target"), StatusCode::BAD_REQUEST);
        assert_eq!(status_for("deploy_failed"), StatusCode::BAD_GATEWAY);
        assert_eq!(status_for("not_connected"), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn confirmation_required_maps_to_422() {
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "confirm".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn restart_required_maps_to_conflict() {
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "restart_required".to_string(),
            message: "restart labby serve".to_string(),
        })
        .into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn queue_saturated_maps_to_429() {
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "queue_saturated".to_string(),
            message: "queue full".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn acp_session_limit_exceeded_maps_to_429() {
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "session_limit_exceeded".to_string(),
            message: "session limit reached".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn acp_too_many_subscribers_maps_to_429() {
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "too_many_subscribers".to_string(),
            message: "subscriber limit reached".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn path_traversal_maps_to_422() {
        // The ACP installer converges on the canonical `path_traversal` kind
        // (Docs-M1). It must map to the same 422 as the legacy
        // `path_traversal_rejected` spelling.
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "path_traversal".to_string(),
            message: "archive entry escapes extract root".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn content_too_large_maps_to_413() {
        // Decompression-bomb / oversized-archive guard (Sec/Test-M3).
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "content_too_large".to_string(),
            message: "uncompressed archive exceeds cap".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn integrity_missing_maps_to_502() {
        let response = ApiError(ToolError::Sdk {
            sdk_kind: "integrity_missing".to_string(),
            message: "missing sha256".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}
