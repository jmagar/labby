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

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
}

fn snippets_action_requires_admin(action: &str) -> bool {
    let bare = action.strip_prefix("snippets.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    crate::dispatch::snippets::ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        .unwrap_or(true)
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn require_snippets_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if !snippets_action_requires_admin(action) || has_admin_scope(auth) {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "snippets",
        action,
        request_id,
        kind = "forbidden",
        "snippets action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_snippets_admin(&req.action, request_id, auth.as_ref())?;
    let manager = state.gateway_manager.clone();

    handle_action_with_meta(
        "snippets",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::snippets::ACTIONS,
        move |action, params| async move {
            if matches!(action.as_str(), "snippets.exec" | "snippets.test") {
                let manager = manager
                    .as_ref()
                    .ok_or_else(|| ToolError::internal_message("gateway manager not wired"))?;
                return crate::dispatch::snippets::dispatch::dispatch_with_manager(
                    manager, &action, params,
                )
                .await;
            }
            crate::dispatch::snippets::dispatch(&action, params).await
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

    use crate::api::{oauth::AuthContext, state::AppState};

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

    fn app_with_auth(auth: AuthContext) -> Router {
        let state = AppState::from_registry(crate::registry::build_default_registry());
        super::routes(state.clone())
            .layer(Extension(auth))
            .with_state(state)
    }

    async fn post_snippets(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    #[tokio::test]
    async fn read_only_actions_do_not_require_admin_scope() {
        let app = app_with_auth(read_only_auth_context());
        for action in ["snippets.list", "help", "schema"] {
            let params = if action == "schema" {
                json!({"action": "snippets.list"})
            } else {
                json!({})
            };
            let response =
                post_snippets(app.clone(), json!({"action": action, "params": params})).await;
            assert_ne!(
                response.status(),
                StatusCode::FORBIDDEN,
                "read-only action `{action}` must not require admin scope"
            );
        }
    }

    #[tokio::test]
    async fn admin_actions_require_admin_scope() {
        let app = app_with_auth(read_only_auth_context());
        for action in [
            "snippets.get",
            "snippets.exec",
            "snippets.create",
            "snippets.remove",
            "snippets.test",
            "snippets.validate",
        ] {
            let response = post_snippets(
                app.clone(),
                json!({
                    "action": action,
                    "params": {
                        "name": "fixture",
                        "body": "async () => ({ ok: true })",
                        "confirm": true
                    }
                }),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "action `{action}` should require lab:admin scope"
            );
        }
    }

    #[tokio::test]
    async fn validate_requires_admin_scope() {
        let app = app_with_auth(read_only_auth_context());
        let response = post_snippets(
            app,
            json!({
                "action": "snippets.validate",
                "params": {
                    "name": "fixture",
                    "body": "async () => ({ ok: true })"
                }
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn remove_requires_confirmation_after_admin_scope_passes() {
        let app = app_with_auth(admin_auth_context());
        let response = post_snippets(
            app,
            json!({
                "action": "snippets.remove",
                "params": {"name": "fixture"}
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
