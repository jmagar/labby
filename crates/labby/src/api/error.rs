//! HTTP error handling.
//!
//! `ToolError` (now `labby_runtime::error::ToolError`, re-exported from
//! `crate::dispatch::error`) is the canonical error type for all surfaces
//! (MCP, API, CLI). Because `ToolError` is now a foreign type and `axum`'s
//! `IntoResponse` is a foreign trait, we cannot `impl IntoResponse for
//! ToolError` directly (orphan rule). Instead, HTTP handlers return
//! `Result<_, ApiError>`, where `ApiError` is a local newtype wrapping
//! `ToolError`. `From<ToolError> for ApiError` makes `?` work, and the HTTP
//! status-code mapping lives on `impl IntoResponse for ApiError` — still the
//! single place HTTP status codes are assigned for the API surface.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use labby_runtime::agent_error::AgentErrorContext;

pub use crate::dispatch::error::ToolError;

/// Local wrapper around `ToolError` so the API surface can implement
/// axum's `IntoResponse` without violating the orphan rule.
///
/// Handlers return `Result<_, ApiError>`. `?` on a `ToolError`-producing
/// expression converts automatically via `From<ToolError>`. Internal
/// (non-handler) helpers may keep returning `ToolError`; convert at the
/// handler boundary.
///
/// The optional context carries the dispatch `service`/`action` into the
/// serialized body so HTTP error envelopes match what MCP envelopes carry.
/// `handle_action` populates it on the `/v1/<service>` dispatch path; errors
/// raised outside a service dispatch (router/auth middleware) have none.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub error: ToolError,
    context: Option<Box<AgentErrorContext>>,
}

impl ApiError {
    #[must_use]
    pub fn new(error: ToolError) -> Self {
        Self {
            error,
            context: None,
        }
    }

    /// Attach the dispatch `service`/`action` context serialized into the body.
    #[must_use]
    pub fn with_service_action(mut self, service: &str, action: &str) -> Self {
        self.context = Some(Box::new(AgentErrorContext::for_service_action(
            service, action,
        )));
        self
    }

    /// The JSON body this error serializes to — byte-identical to the MCP
    /// error-object shape, with `service`/`action` populated when the error
    /// came from a service dispatch.
    #[must_use]
    pub fn body(&self) -> serde_json::Value {
        match &self.context {
            Some(context) => self.error.to_agent_value_with_context(context),
            None => self.error.to_agent_value(),
        }
    }
}

impl From<ToolError> for ApiError {
    fn from(e: ToolError) -> Self {
        Self::new(e)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.error.kind() {
            "auth_failed" => StatusCode::UNAUTHORIZED,
            "not_found" | "route_not_found" => StatusCode::NOT_FOUND,
            "rate_limited" | "queue_saturated" => StatusCode::TOO_MANY_REQUESTS,
            "busy" => StatusCode::TOO_MANY_REQUESTS,
            "sync_in_progress"
            | "service_unavailable"
            | "provider_unavailable"
            | "source_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
            "missing_param" | "invalid_param" | "validation_failed" | "invalid_hint"
            | "tool_error" => StatusCode::UNPROCESSABLE_ENTITY,
            "relay_invalid_target" => StatusCode::UNPROCESSABLE_ENTITY,
            "relay_registry_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
            "relay_forwarder_init_failed" => StatusCode::BAD_GATEWAY,
            "confirmation_required" => StatusCode::UNPROCESSABLE_ENTITY,
            "ssrf_blocked" | "no_remote_transport" => StatusCode::UNPROCESSABLE_ENTITY,
            "symlink_rejected" | "path_traversal" | "invalid_encoding" => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            "content_too_large" | "response_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
            "quota_exceeded" => StatusCode::PAYLOAD_TOO_LARGE,
            "not_supported" => StatusCode::NOT_IMPLEMENTED,
            "install_timeout"
            | "timeout"
            | "audit_timeout"
            | "code_mode_timeout"
            | "code_mode_fuel_exhausted"
            | "provider_timeout" => StatusCode::GATEWAY_TIMEOUT,
            "oauth_needs_reauth" => StatusCode::UNAUTHORIZED,
            "oauth_state_invalid" => StatusCode::BAD_REQUEST,
            "oauth_scope_upgrade_required" | "forbidden" => StatusCode::FORBIDDEN,
            "oauth_account_ambiguous"
            | "oauth_client_mismatch"
            | "oauth_shared_credential_protected" => StatusCode::CONFLICT,
            "unknown_action" | "unknown_subaction" | "unknown_instance" => StatusCode::BAD_REQUEST,
            "unknown_upstream" | "unknown_tool" => StatusCode::NOT_FOUND,
            "network_error"
            | "bad_gateway"
            | "server_error"
            | "upstream_error"
            | "cancelled"
            | "oauth_resource_mismatch"
            | "oauth_issuer_mismatch"
            | "oauth_unsupported_method"
            | "preflight_failed"
            | "install_failed"
            | "verify_failed"
            | "not_connected"
            | "invalid_provider_output" => StatusCode::BAD_GATEWAY,
            "conflict"
            | "contract_changed"
            | "ambiguous_tool"
            | "restart_required"
            | "stale_suggestion"
            | "merge_write_conflict"
            | "workspace_not_configured" => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Serialize the inner ToolError with the dispatch context (when one
        // was attached) — matching the MCP error envelope, which always
        // carries `service`/`action`.
        let body = self.body();

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
        ApiError::new(ToolError::Sdk {
            sdk_kind: kind.to_string(),
            message: "x".to_string(),
        })
        .into_response()
        .status()
    }

    #[test]
    fn body_includes_service_action_only_when_context_attached() {
        let err = ApiError::new(ToolError::Sdk {
            sdk_kind: "missing_param".to_string(),
            message: "query is required".to_string(),
        });
        let bare = err.body();
        assert!(bare.get("service").is_none());
        assert!(bare.get("action").is_none());

        let contextual = err.with_service_action("gateway", "gateway.list").body();
        assert_eq!(contextual["kind"], "missing_param");
        assert_eq!(contextual["service"], "gateway");
        assert_eq!(contextual["action"], "gateway.list");
    }

    #[test]
    fn file_stash_capacity_errors_have_stable_http_statuses() {
        assert_eq!(status_for("busy"), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(status_for("quota_exceeded"), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            status_for("service_unavailable"),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn confirmation_required_maps_to_422() {
        let response = ApiError::new(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "confirm".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn restart_required_maps_to_conflict() {
        let response = ApiError::new(ToolError::Sdk {
            sdk_kind: "restart_required".to_string(),
            message: "restart labby serve".to_string(),
        })
        .into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn contract_changed_maps_to_conflict() {
        assert_eq!(status_for("contract_changed"), StatusCode::CONFLICT);
    }

    #[test]
    fn queue_saturated_maps_to_429() {
        let response = ApiError::new(ToolError::Sdk {
            sdk_kind: "queue_saturated".to_string(),
            message: "queue full".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn source_unavailable_maps_to_service_unavailable() {
        assert_eq!(
            status_for("source_unavailable"),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn tool_error_maps_to_422_instead_of_bad_gateway() {
        assert_eq!(status_for("tool_error"), StatusCode::UNPROCESSABLE_ENTITY);
        assert_ne!(status_for("tool_error"), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn path_traversal_maps_to_422() {
        // Generic path-safety failures are caller-fixable validation errors.
        let response = ApiError::new(ToolError::Sdk {
            sdk_kind: "path_traversal".to_string(),
            message: "archive entry escapes extract root".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn gateway_enrichment_kinds_map_to_non_500_statuses() {
        assert_eq!(status_for("invalid_hint"), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(status_for("stale_suggestion"), StatusCode::CONFLICT);
        assert_eq!(status_for("unknown_upstream"), StatusCode::NOT_FOUND);
        assert_eq!(
            status_for("provider_unavailable"),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(status_for("provider_timeout"), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            status_for("invalid_provider_output"),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn google_credential_broker_kinds_map_to_actionable_statuses() {
        assert_eq!(
            status_for("oauth_scope_upgrade_required"),
            StatusCode::FORBIDDEN
        );
        for kind in [
            "oauth_account_ambiguous",
            "oauth_client_mismatch",
            "oauth_shared_credential_protected",
        ] {
            assert_eq!(status_for(kind), StatusCode::CONFLICT, "kind={kind}");
        }
    }

    #[test]
    fn setup_and_workspace_kinds_do_not_fall_through_to_500() {
        assert_eq!(status_for("audit_timeout"), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(status_for("merge_write_conflict"), StatusCode::CONFLICT);
        assert_eq!(status_for("workspace_not_configured"), StatusCode::CONFLICT);
        // A post-commit draft cleanup failure really is an internal partial
        // transaction failure and intentionally remains HTTP 500.
        assert_eq!(
            status_for("draft_clear_failed"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn public_relay_kinds_map_to_expected_statuses() {
        assert_eq!(
            status_for("relay_invalid_target"),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            status_for("relay_registry_unavailable"),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            status_for("relay_forwarder_init_failed"),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn content_too_large_maps_to_413() {
        // Decompression-bomb / oversized-archive guard (Sec/Test-M3).
        let response = ApiError::new(ToolError::Sdk {
            sdk_kind: "content_too_large".to_string(),
            message: "uncompressed archive exceeds cap".to_string(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn response_too_large_maps_to_413() {
        // Gateway cap on upstream MCP response bytes.
        assert_eq!(
            status_for("response_too_large"),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[test]
    fn cancelled_maps_to_bad_gateway() {
        // The upstream reported the proxied call was cancelled.
        assert_eq!(status_for("cancelled"), StatusCode::BAD_GATEWAY);
    }
}
