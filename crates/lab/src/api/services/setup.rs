//! HTTP route group for the `setup` Bootstrap orchestrator.
//!
//! Mounted at `/v1/setup` behind the host-validation Layer (Chunk E):
//! requests with a non-loopback Host header are rejected with 421 before
//! reaching the dispatcher.

use std::net::SocketAddr;

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use serde_json::Value;

use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::setup::ACTIONS;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_setup_admin(&req.action, request_id, auth.as_ref())?;
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
    handle_action_with_meta(
        "setup",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        ACTIONS,
        |action, params| async move { crate::dispatch::setup::dispatch(&action, params).await },
    )
    .await
}

fn setup_action_requires_admin(action: &str) -> bool {
    let bare = action.strip_prefix("setup.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        .unwrap_or(true)
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn require_setup_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if !setup_action_requires_admin(action) || has_admin_scope(auth) {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "setup",
        action,
        request_id,
        kind = "forbidden",
        "setup action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(scopes: &[&str]) -> Extension<AuthContext> {
        Extension(AuthContext {
            sub: "tester@example.com".to_string(),
            actor_key: None,
            issuer: "test".to_string(),
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            via_session: true,
            email: Some("tester@example.com".to_string()),
            csrf_token: None,
        })
    }

    #[test]
    fn setup_settings_mutations_require_admin_scope_on_api_gate() {
        let read_only = auth(&["lab:read"]);
        for action in [
            "settings.update",
            "settings.config.update",
            "settings.env.update",
        ] {
            assert!(require_setup_admin(action, None, Some(&read_only)).is_err());
            assert!(require_setup_admin(action, None, Some(&auth(&["lab:admin"]))).is_ok());
        }
    }
}
