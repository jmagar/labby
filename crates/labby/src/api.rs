//! API surface for `lab`.
//!
//! Thin axum layer over `lab-apis` clients. Mirrors the MCP dispatch shape:
//! one route group per service, action + params dispatch, structured JSON
//! error envelopes with stable `kind` tags.
//!
//! The API is an optional surface — the binary can run as CLI-only, MCP-only,
//! HTTP-only, or any combination. Routes are feature-gated per service.

/// Shared application state (service clients, config).
pub mod state;

/// HTTP error re-exports — canonical type is `ToolError` from `dispatch::error`.
pub mod error;

/// Router builder — composes all feature-gated route groups.
pub mod router;

/// Shared auth-route request-id and dispatch logging helpers.
pub mod auth_helpers;

/// `GET /health` and `GET /ready` liveness/readiness probes.
pub mod health;

/// HTTP auth helpers for bearer-or-OAuth mode (metadata, WWW-Authenticate).
pub mod oauth;

/// Browser-facing upstream OAuth routes.
#[cfg(feature = "gateway")]
pub mod upstream_oauth;

/// Browser-session endpoints for the hosted UI.
pub mod browser_session;

/// Node runtime routes mounted under `/v1/nodes/*`.
#[cfg(feature = "nodes")]
pub mod nodes;

/// Static Labby web asset serving helpers.
pub mod web;

/// Host header validation Layer (DNS rebinding mitigation for v1).
pub mod host_validation;

/// Per-service HTTP route handlers (one module per service).
pub mod services;

/// OpenAPI 3.1 schema generation (all utoipa coupling lives here).
#[allow(clippy::needless_for_each)]
pub mod openapi;

/// Shared request type for all service action dispatchers.
///
/// Every service handler deserializes `POST /v1/<service>` bodies into this
/// struct and forwards `action` + `params` to the corresponding dispatch
/// function, keeping HTTP and MCP input semantics aligned while each transport
/// owns its response envelope.
#[derive(Debug, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[allow(unused_imports)]
pub use error::{ApiError, ToolError};
#[allow(unused_imports)]
pub use state::AppState;
