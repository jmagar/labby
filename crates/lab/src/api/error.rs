//! HTTP error handling.
//!
//! `ToolError` from `crate::dispatch::error` is the canonical error type for
//! all surfaces (MCP, API, CLI). Its `IntoResponse` impl lives here (not in
//! `dispatch/`) because HTTP status code mapping is an API surface concern.
//!
//! `ApiError` was a duplicate type that serialized a bare `{kind, message}`
//! envelope, dropping structured fields. It has been removed — use
//! `ToolError` directly in all HTTP handlers.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub use crate::dispatch::error::ToolError;

impl IntoResponse for ToolError {
    fn into_response(self) -> Response {
        let status = match self.kind() {
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
            | "invalid_encoding" => StatusCode::UNPROCESSABLE_ENTITY,
            "content_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "install_timeout" | "timeout" | "code_mode_timeout" | "code_mode_fuel_exhausted" => {
                StatusCode::GATEWAY_TIMEOUT
            }
            "oauth_needs_reauth" => StatusCode::UNAUTHORIZED,
            "oauth_state_invalid" => StatusCode::BAD_REQUEST,
            "forbidden" | "dev_preview_read_only" => StatusCode::FORBIDDEN,
            "unknown_action" | "unknown_subaction" | "unknown_instance" => {
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
            | "integrity_mismatch" => StatusCode::BAD_GATEWAY,
            "conflict" | "ambiguous_tool" | "restart_required" => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Serialize self directly — byte-identical to the MCP error envelope.
        let body = serde_json::to_value(&self).unwrap_or_else(|_| {
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

    use super::ToolError;

    #[test]
    fn confirmation_required_maps_to_422() {
        let response = ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "confirm".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn restart_required_maps_to_conflict() {
        let response = ToolError::Sdk {
            sdk_kind: "restart_required".to_string(),
            message: "restart labby serve".to_string(),
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn queue_saturated_maps_to_429() {
        let response = ToolError::Sdk {
            sdk_kind: "queue_saturated".to_string(),
            message: "queue full".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn acp_session_limit_exceeded_maps_to_429() {
        let response = ToolError::Sdk {
            sdk_kind: "session_limit_exceeded".to_string(),
            message: "session limit reached".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn acp_too_many_subscribers_maps_to_429() {
        let response = ToolError::Sdk {
            sdk_kind: "too_many_subscribers".to_string(),
            message: "subscriber limit reached".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn path_traversal_maps_to_422() {
        // The ACP installer converges on the canonical `path_traversal` kind
        // (Docs-M1). It must map to the same 422 as the legacy
        // `path_traversal_rejected` spelling.
        let response = ToolError::Sdk {
            sdk_kind: "path_traversal".to_string(),
            message: "archive entry escapes extract root".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn content_too_large_maps_to_413() {
        // Decompression-bomb / oversized-archive guard (Sec/Test-M3).
        let response = ToolError::Sdk {
            sdk_kind: "content_too_large".to_string(),
            message: "uncompressed archive exceeds cap".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn integrity_missing_maps_to_502() {
        let response = ToolError::Sdk {
            sdk_kind: "integrity_missing".to_string(),
            message: "missing sha256".to_string(),
        }
        .into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
}
