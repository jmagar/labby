//! HTTP route group for the `setup` Bootstrap orchestrator.
//!
//! Mounted at `/v1/setup` behind the host-validation Layer (Chunk E):
//! requests with a non-loopback Host header are rejected with 421 before
//! reaching the dispatcher.

use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use serde_json::Value;

use crate::api::services::helpers::handle_action;
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::setup::ACTIONS;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    if plugin_lifecycle_action(&req.action) && !http_bind_is_loopback(&state) {
        tracing::info!(
            surface = "api",
            service = "setup",
            action = %req.action,
            bind_host = state.http_bind_host.as_deref().map(String::as_str).unwrap_or("<unknown>"),
            "setup plugin lifecycle action skipped because HTTP bind is non-loopback"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: "setup plugin lifecycle actions are only available over loopback HTTP".into(),
        });
    }
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    handle_action(
        "setup",
        "api",
        request_id,
        req,
        ACTIONS,
        |action, params| async move { crate::dispatch::setup::dispatch(&action, params).await },
    )
    .await
}

fn plugin_lifecycle_action(action: &str) -> bool {
    matches!(
        action,
        "installed_plugins" | "services_status" | "install_plugin" | "uninstall_plugin"
    )
}

fn http_bind_is_loopback(state: &AppState) -> bool {
    let host = state
        .http_bind_host
        .as_deref()
        .map(String::as_str)
        .unwrap_or("127.0.0.1");
    let normalized = host.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(normalized, "127.0.0.1" | "::1" | "localhost")
}
