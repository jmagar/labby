use std::sync::Arc;

use axum::{Extension, Json, Router, extract::State, http::HeaderMap, routing::post};
use serde_json::Value;

use crate::api::oauth::AuthContext;
use crate::api::services::helpers::handle_action;
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_none_or(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn gateway_action_requires_admin(action: &str) -> bool {
    !matches!(
        action,
        "help" | "schema" | "gateway.help" | "gateway.schema"
    )
}

fn require_gateway_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if !gateway_action_requires_admin(action) || has_admin_scope(auth) {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "gateway",
        action,
        request_id,
        kind = "forbidden",
        "gateway action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ToolError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_gateway_admin(&req.action, request_id, auth.as_ref())?;
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "gateway manager not wired".to_string(),
        })?;

    handle_action(
        "gateway",
        "api",
        request_id,
        req,
        crate::dispatch::gateway::ACTIONS,
        move |action, params| {
            let manager = Arc::clone(&manager);
            let subject = subject.clone();
            async move {
                let params = inject_gateway_owner(params, subject.as_deref(), request_id);
                crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params).await
            }
        },
    )
    .await
}

fn inject_gateway_owner(params: Value, subject: Option<&str>, request_id: Option<&str>) -> Value {
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    let raw = match (subject, request_id) {
        (Some(sub), Some(request_id)) => Some(format!("api:{sub}:{request_id}")),
        (Some(sub), None) => Some(format!("api:{sub}")),
        (None, Some(request_id)) => Some(format!("api:anonymous:{request_id}")),
        (None, None) => Some("api:anonymous".to_string()),
    };
    object.entry("owner".to_string()).or_insert_with(|| {
        serde_json::json!({
            "surface": "api",
            "subject": subject,
            "request_id": request_id,
            "raw": raw,
        })
    });
    if let Some(origin) = raw {
        object
            .entry("origin".to_string())
            .or_insert_with(|| Value::String(origin));
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::api::oauth::AuthContext;
    use crate::api::{router::build_router_with_bearer, state::AppState};
    use crate::config::{
        LabConfig, UpstreamConfig, VirtualServerConfig, VirtualServerSurfacesConfig,
    };
    use crate::dispatch::gateway::config::{load_gateway_config, write_gateway_config};
    use crate::dispatch::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use crate::registry::build_default_registry;

    fn test_manager_with_path() -> (Arc<GatewayManager>, std::path::PathBuf) {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let path = std::env::temp_dir().join(format!(
            "lab-gateway-api-test-{}-{}.toml",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        (
            Arc::new(GatewayManager::new(
                path.clone(),
                GatewayRuntimeHandle::default(),
            )),
            path,
        )
    }

    fn test_manager() -> Arc<GatewayManager> {
        test_manager_with_path().0
    }

    fn test_app() -> Router {
        test_app_with_manager(test_manager())
    }

    fn test_app_with_manager(manager: Arc<GatewayManager>) -> Router {
        let state = AppState::from_registry(build_default_registry()).with_gateway_manager(manager);
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

    async fn post_gateway(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/gateway")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    async fn post_gateway_fresh(body: serde_json::Value) -> axum::response::Response {
        post_gateway(test_app(), body).await
    }

    async fn get_gateway_actions(app: Router) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/gateway/actions")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
    }

    #[tokio::test]
    async fn gateway_list_route_exists() {
        let response = post_gateway_fresh(json!({"action":"gateway.list","params":{}})).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gateway_sensitive_actions_require_admin_when_authenticated() {
        let app = test_app_with_auth(read_only_auth_context());

        for action in [
            "gateway.list",
            "gateway.status",
            "gateway.service_config.get",
            "gateway.add",
            "gateway.reload",
            "gateway.oauth.probe",
            "gateway.mcp.cleanup",
        ] {
            let response = post_gateway(
                app.clone(),
                json!({
                    "action": action,
                    "params": {
                        "confirm": true,
                        "service": "plex",
                        "url": "https://fixture.example.com/mcp",
                        "name": "fixture-http",
                        "spec": {"name":"fixture-http","url":"https://fixture.example.com/mcp"}
                    }
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{action}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(payload["kind"], "forbidden", "{action}");
        }
    }

    #[tokio::test]
    async fn gateway_list_returns_stale_virtual_server_warning() {
        let manager = test_manager();
        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "stale-registry".to_string(),
                    service: "mcpregistry".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        let response = post_gateway(
            test_app_with_manager(manager),
            json!({"action":"gateway.list","params":{}}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload[0]["id"], "stale-registry");
        assert_eq!(payload[0]["warnings"][0]["code"], "unknown_service");
    }

    #[tokio::test]
    async fn gateway_reload_quarantines_stale_virtual_server_before_list() {
        let (manager, path) = test_manager_with_path();
        write_gateway_config(
            &path,
            &LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "stale-registry".to_string(),
                    service: "mcpregistry".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            },
        )
        .expect("write config");
        let app = test_app_with_manager(manager);

        let reloaded = post_gateway(
            app.clone(),
            json!({"action":"gateway.reload","params":{"confirm":true}}),
        )
        .await;
        assert_eq!(reloaded.status(), StatusCode::OK);

        let listed = post_gateway(app, json!({"action":"gateway.list","params":{}})).await;
        assert_eq!(listed.status(), StatusCode::OK);
        let body = axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload.as_array().expect("array").len(), 0);

        let migrated = load_gateway_config(&path).expect("load migrated config");
        assert!(migrated.virtual_servers.is_empty());
        assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
        assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-registry");
    }

    #[tokio::test]
    async fn gateway_get_returns_not_found_for_missing_gateway() {
        let response =
            post_gateway_fresh(json!({"action":"gateway.get","params":{"name":"fixture-http"}}))
                .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn gateway_test_accepts_proposed_spec() {
        let response = post_gateway_fresh(json!({
            "action":"gateway.test",
            "params":{"spec":{"name":"fixture-stdio","command":"echo","args":["hello"]}}
        }))
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gateway_add_update_remove_reload_routes_exist() {
        let app = test_app();

        let added = post_gateway(app.clone(), json!({
            "action":"gateway.add",
            "params":{"confirm":true,"spec":{"name":"fixture-http","url":"https://fixture.example.com/mcp","bearer_token_env":"FIXTURE_HTTP_TOKEN"}}
        }))
        .await;
        assert_eq!(added.status(), StatusCode::OK);

        let updated = post_gateway(
            app.clone(),
            json!({
                "action":"gateway.update",
                "params":{"confirm":true,"name":"fixture-http","patch":{"proxy_resources":true}}
            }),
        )
        .await;
        assert_eq!(updated.status(), StatusCode::OK);

        let removed = post_gateway(
            app.clone(),
            json!({"action":"gateway.remove","params":{"confirm":true,"name":"fixture-http"}}),
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);

        let reloaded = post_gateway(
            app,
            json!({"action":"gateway.reload","params":{"confirm":true}}),
        )
        .await;
        assert_eq!(reloaded.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gateway_client_config_get_route_matches_advertised_action() {
        let manager = test_manager();
        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("https://fixture.example.com/mcp".to_string()),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            }])
            .await;

        let response = post_gateway(
            test_app_with_manager(manager),
            json!({"action":"gateway.client_config.get","params":{"name":"fixture-http"}}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload["name"], "fixture-http");
        assert_eq!(payload["type"], "http");
        assert_eq!(payload["url"], "https://fixture.example.com/mcp");
    }

    #[tokio::test]
    async fn gateway_destructive_routes_require_confirm() {
        let response = post_gateway_fresh(json!({
            "action":"gateway.add",
            "params":{"spec":{"name":"fixture-http","url":"http://127.0.0.1:9001","bearer_token_env":"FIXTURE_HTTP_TOKEN"}}
        }))
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn gateway_actions_endpoint_is_registered() {
        let response = get_gateway_actions(test_app()).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}
