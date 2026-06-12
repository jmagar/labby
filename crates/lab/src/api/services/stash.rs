//! HTTP route group for the `stash` service.

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
use crate::dispatch::stash::catalog::ACTIONS;

/// Actions that mutate stash state and require `lab:admin` scope.
const STASH_WRITE_ACTIONS: &[&str] = &[
    "component.import",
    "component.save",
    "component.export",
    "component.deploy",
    "component.create",
    "provider.link",
    "provider.push",
    "provider.pull",
    "target.add",
    "target.remove",
];

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
}

async fn handle(
    State(_state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());

    // Scope gate: mutating actions require `lab:admin`.
    // Read-only actions (components.list, component.get, component.workspace,
    // component.revisions, providers.list, targets.list, help, schema) pass through.
    if STASH_WRITE_ACTIONS.contains(&req.action.as_str()) {
        let has_admin = auth
            .as_ref()
            .is_some_and(|ctx| ctx.0.scopes.iter().any(|s| s == "lab:admin"));
        if !has_admin {
            tracing::warn!(
                surface = "api",
                service = "stash",
                action = req.action.as_str(),
                request_id,
                kind = "forbidden",
                "stash write action rejected: lab:admin scope required"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!("action `{}` requires `lab:admin` scope", req.action),
            });
        }
    }

    handle_action_with_meta(
        "stash",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        ACTIONS,
        |action, params| async move {
            crate::dispatch::stash::dispatch::dispatch_for_surface("api", &action, params).await
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::api::{oauth::AuthContext, router::build_router_with_bearer, state::AppState};

    fn test_app() -> Router {
        let state = AppState::new();
        build_router_with_bearer(state, None, None)
    }

    fn test_app_with_auth(auth: AuthContext) -> Router {
        test_app().layer(Extension(auth))
    }

    fn read_only_auth_context() -> AuthContext {
        AuthContext {
            sub: "read-only-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("reader@example.com".to_string()),
        }
    }

    fn admin_auth_context() -> AuthContext {
        AuthContext {
            sub: "admin-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string(), "lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("admin@example.com".to_string()),
        }
    }

    async fn post_stash(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/stash")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    /// Without auth context (no bearer token configured), read-only stash actions
    /// must pass the scope gate and reach dispatch.
    #[tokio::test]
    async fn read_only_actions_pass_scope_gate() {
        let app = test_app();
        for action in &[
            "components.list",
            "component.get",
            "component.workspace",
            "component.revisions",
            "providers.list",
            "targets.list",
            "help",
            "schema",
        ] {
            let response = post_stash(app.clone(), json!({ "action": action, "params": {} })).await;
            // Should not be forbidden (403) — may be 400/404/200 from dispatch
            assert_ne!(
                response.status(),
                StatusCode::FORBIDDEN,
                "read-only action `{action}` must not be blocked by scope gate"
            );
        }
    }

    #[tokio::test]
    async fn write_actions_require_admin_scope() {
        for app in [test_app(), test_app_with_auth(read_only_auth_context())] {
            let response = post_stash(
                app,
                json!({
                    "action": "component.create",
                    "params": {"kind": "settings", "name": "test"}
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }

        let response = post_stash(
            test_app_with_auth(admin_auth_context()),
            json!({
                "action": "component.create",
                "params": {"kind": "settings", "name": "test"}
            }),
        )
        .await;
        assert_ne!(response.status(), StatusCode::FORBIDDEN);
    }
}
