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

use crate::api::error::ApiError;
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
) -> Result<Json<Value>, ApiError> {
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
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: "setup plugin lifecycle actions are only available over loopback HTTP".into(),
        }));
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
    // Both the canonical dotted names AND the deprecated snake_case aliases
    // must be matched here. The gate is keyed on the action string the
    // dispatcher will execute, so a dotted-named HTTP call (e.g.
    // `plugin.install`) must be loopback-restricted exactly like its legacy
    // `install_plugin` alias — otherwise the dotted form would be a
    // restriction bypass.
    //
    // The name set is owned by `crate::dispatch::setup::PLUGIN_LIFECYCLE_ACTIONS`
    // (the same module that holds the catalog and dispatch arms) so the gate,
    // catalog, and dispatch routing share one source of truth instead of three
    // hand-synced literal lists.
    crate::dispatch::setup::PLUGIN_LIFECYCLE_ACTIONS.contains(&action)
}

fn http_bind_is_loopback(state: &AppState) -> bool {
    host_is_loopback(state.http_bind_host.as_deref().map(String::as_str))
}

/// Pure loopback check over the configured bind host. A missing host defaults
/// to `127.0.0.1` (the loopback default for `labby serve`). Extracted so the
/// gate predicate is testable without constructing a full `AppState`.
fn host_is_loopback(host: Option<&str>) -> bool {
    let host = host.unwrap_or("127.0.0.1");
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

    /// The canonical dotted plugin-lifecycle names and their deprecated
    /// snake_case aliases must all be recognized as loopback-gated. A miss
    /// here means a dotted-named HTTP call could bypass the loopback
    /// restriction that the flat names enforce.
    #[test]
    fn plugin_lifecycle_gate_matches_dotted_and_flat_names() {
        for action in [
            // canonical dotted forms
            "plugins.installed",
            "services.status",
            "plugin.install",
            "plugin.uninstall",
            // deprecated snake_case aliases
            "installed_plugins",
            "services_status",
            "install_plugin",
            "uninstall_plugin",
        ] {
            assert!(
                plugin_lifecycle_action(action),
                "{action} must be treated as a plugin-lifecycle action"
            );
        }
        // Non-lifecycle actions must NOT be gated.
        assert!(!plugin_lifecycle_action("settings.update"));
        assert!(!plugin_lifecycle_action("draft.commit"));
        // Near-miss typos of the lifecycle names must NOT be gated either —
        // these are the realistic mistake classes the dotted/flat split adds,
        // and matching them would over-restrict (or mask a real routing bug).
        for near_miss in [
            "plugin.installed",
            "plugins.install",
            "service.status",
            "services.statu",
            "plugin.uninstal",
        ] {
            assert!(
                !plugin_lifecycle_action(near_miss),
                "{near_miss} is not a real lifecycle action and must not be gated"
            );
        }
    }

    /// `host_is_loopback` is the predicate the `handle` gate uses to decide
    /// whether plugin-lifecycle actions are reachable. Table-test it directly,
    /// including IPv6, bracketed, and whitespace-padded forms.
    #[test]
    fn host_is_loopback_classifies_bind_addresses() {
        for host in [
            Some("127.0.0.1"),
            Some("::1"),
            Some("localhost"),
            Some("[::1]"),
            Some("[127.0.0.1]"),
            Some("  127.0.0.1  "),
            None, // unset → defaults to the loopback bind
        ] {
            assert!(host_is_loopback(host), "{host:?} should be loopback");
        }
        for host in [
            Some("0.0.0.0"),
            Some("10.0.0.5"),
            Some("192.168.1.10"),
            Some("example.com"),
            Some("::"),
            Some(""),
        ] {
            assert!(!host_is_loopback(host), "{host:?} should NOT be loopback");
        }
    }

    /// The `handle` gate rejects a request when
    /// `plugin_lifecycle_action(action) && !http_bind_is_loopback(state)`.
    /// `http_bind_is_loopback` delegates to `host_is_loopback`, so we exercise
    /// that exact composition here without building `AppState` (which needs a
    /// Tokio runtime for ACP registry init). Asserting the computed outcome —
    /// rather than chaining the predicates inside the assertion — keeps the
    /// test from collapsing into a tautology and proves the dotted forms are
    /// rejected/allowed identically to the flat aliases.
    #[test]
    fn dotted_plugin_lifecycle_action_is_loopback_gated() {
        // (action, expected_rejected_on_non_loopback)
        let cases = [
            // dotted canonical forms — gated
            ("plugins.installed", true),
            ("services.status", true),
            ("plugin.install", true),
            ("plugin.uninstall", true),
            // flat aliases — gated identically
            ("installed_plugins", true),
            ("install_plugin", true),
            // non-lifecycle actions — never gated, reachable on any bind
            ("settings.update", false),
            ("draft.commit", false),
        ];

        for (action, expect_rejected_off_loopback) in cases {
            // The exact boolean the `handle` gate computes before dispatch.
            let rejected_on_non_loopback =
                plugin_lifecycle_action(action) && !host_is_loopback(Some("0.0.0.0"));
            let rejected_on_loopback =
                plugin_lifecycle_action(action) && !host_is_loopback(Some("127.0.0.1"));
            let rejected_on_default = plugin_lifecycle_action(action) && !host_is_loopback(None);

            assert_eq!(
                rejected_on_non_loopback, expect_rejected_off_loopback,
                "{action}: non-loopback rejection outcome mismatch"
            );
            // Loopback and the default (unset) bind never reject, for any action.
            assert!(
                !rejected_on_loopback,
                "{action} must be reachable on loopback"
            );
            assert!(
                !rejected_on_default,
                "{action} must be reachable on the default (unset) bind"
            );
        }
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
