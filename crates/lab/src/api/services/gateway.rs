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

/// Returns true when the action requires `lab:admin` scope.
///
/// Single source of truth: reads `ActionSpec.requires_admin` from the gateway
/// catalog (A-H2/S5 fix). No bespoke match arm — adding a new action to the
/// catalog automatically inherits the right scope gate.
fn gateway_action_requires_admin(action: &str) -> bool {
    // Universal built-ins are never admin-gated, whether the caller passes them
    // bare (`help`) or service-prefixed (`gateway.help`). The catalog stores them
    // bare, so strip any `gateway.` prefix before the discovery check.
    let bare = action.strip_prefix("gateway.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    crate::dispatch::gateway::ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        // Unknown actions default to admin-required (fail-safe).
        .unwrap_or(true)
}

/// Returns true when the authenticated context carries `lab:admin`.
///
/// T1 fix: when auth IS configured on the HTTP surface, `None` auth means the
/// request arrived without credentials — it must be DENIED admin actions.
/// `is_none_or(...)` is only safe for stdio (which is handled separately via
/// the MCP surface and never reaches this API handler).
fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
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
    let owner = crate::dispatch::gateway::shared::make_api_runtime_owner(subject, request_id);
    let origin = owner.raw.clone();
    // Serialize the owner struct into its JSON shape for the params object.
    // The fields match the GatewayRuntimeOwnerParams shape consumed by dispatch.
    object.entry("owner".to_string()).or_insert_with(|| {
        serde_json::json!({
            "surface": owner.surface,
            "subject": owner.subject,
            "request_id": owner.request_id,
            "raw": owner.raw,
        })
    });
    if let Some(origin) = origin {
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

    // ── Test fixtures ────────────────────────────────────────────────────────

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

    /// Build a test app WITH bearer auth configured (gateway routes are mounted).
    ///
    /// T1 fix: `test_app()` previously used `build_router_with_bearer(state, None, None)`
    /// which set `needs_auth=false` and, before the fix, mounted gateway routes without
    /// any authentication gate.  Now gateway routes are only mounted when auth IS
    /// configured.  Tests that exercise gateway actions must use an authenticated app.
    fn test_app_with_manager(manager: Arc<GatewayManager>) -> Router {
        let state = AppState::from_registry(build_default_registry()).with_gateway_manager(manager);
        // Use a static bearer token so needs_auth=true and /v1/gateway is mounted.
        build_router_with_bearer(state, Some("test-token".into()), None)
    }

    fn test_app() -> Router {
        test_app_with_manager(test_manager())
    }

    /// App with bearer auth + an injected AuthContext (for scope-gated tests).
    fn test_app_with_auth_context(manager: Arc<GatewayManager>, auth: AuthContext) -> Router {
        test_app_with_manager(manager).layer(Extension(auth))
    }

    /// Mount ONLY the gateway route group with a layered `AuthContext` and no
    /// bearer-auth middleware — exercises the per-action scope gate in isolation.
    ///
    /// The full-router static bearer path always injects `lab:admin`, so it
    /// cannot model a non-admin caller. Mounting `services::gateway::routes`
    /// directly (mirroring `upstream_oauth_routes_require_admin_scope`) lets the
    /// layered read-only context survive to the handler's scope gate.
    fn gateway_routes_with_auth_context(manager: Arc<GatewayManager>, auth: AuthContext) -> Router {
        let state = AppState::from_registry(build_default_registry()).with_gateway_manager(manager);
        super::routes(state.clone())
            .layer(Extension(auth))
            .with_state(state)
    }

    /// POST to a directly-mounted gateway route group (no bearer header).
    async fn post_gateway_routes(app: Router, body: serde_json::Value) -> axum::response::Response {
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

    fn admin_auth_context() -> AuthContext {
        AuthContext {
            sub: "admin-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("admin@example.com".to_string()),
        }
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

    // ── Request helpers ──────────────────────────────────────────────────────

    async fn post_gateway(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/gateway")
                .header(header::CONTENT_TYPE, "application/json")
                // Include the static bearer token so the auth middleware passes.
                .header(header::AUTHORIZATION, "Bearer test-token")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    /// Post to /v1/gateway as admin (bearer token + lab:admin AuthContext injected).
    async fn post_gateway_as_admin(
        manager: Arc<GatewayManager>,
        body: serde_json::Value,
    ) -> axum::response::Response {
        let app = test_app_with_auth_context(manager, admin_auth_context());
        post_gateway(app, body).await
    }

    async fn get_gateway_actions(app: Router) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/gateway/actions")
                .header(header::AUTHORIZATION, "Bearer test-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
    }

    // ── T1: Security posture tests ───────────────────────────────────────────

    /// T1 (Critical): when auth IS configured, a request arriving with NO
    /// AuthContext (no bearer token, no session) must be DENIED on all admin
    /// gateway actions — not silently allowed.
    #[tokio::test]
    async fn gateway_admin_actions_refused_when_no_auth_context_present() {
        // App has bearer auth configured (gateway IS mounted), but the request
        // carries no Authorization header → no AuthContext in extensions.
        let app = test_app();

        for action in [
            "gateway.list",
            "gateway.get",
            "gateway.status",
            "gateway.add",
            "gateway.reload",
            "gateway.oauth.probe",
            "gateway.mcp.cleanup",
            "gateway.service_config.get",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/gateway")
                        .header(header::CONTENT_TYPE, "application/json")
                        // No Authorization header → no AuthContext
                        .body(Body::from(
                            json!({
                                "action": action,
                                "params": {
                                    "confirm": true,
                                    "name": "fixture",
                                    "spec": {"name": "fixture", "url": "https://fixture.example.com/mcp"}
                                }
                            })
                            .to_string(),
                        ))
                        .expect("request"),
                )
                .await
                .expect("response");
            // Bearer auth middleware rejects unauthenticated requests before the
            // gateway handler, so we accept either 401 (middleware) or 403 (handler).
            let status = response.status();
            assert!(
                status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
                "action `{action}` with no auth should be 401 or 403, got {status}"
            );
        }
    }

    /// T1: /v1/gateway must NOT be mounted when auth is not configured.
    #[tokio::test]
    async fn gateway_routes_not_mounted_when_auth_not_configured() {
        // Build a router with NO bearer token and NO OAuth state → needs_auth=false
        let state =
            AppState::from_registry(build_default_registry()).with_gateway_manager(test_manager());
        let app = build_router_with_bearer(state, None, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/gateway")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"action": "gateway.list", "params": {}}).to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        // Route is not mounted → 404 or 405, never 200.
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "/v1/gateway must not be accessible when auth is not configured"
        );
        assert!(
            response.status() == StatusCode::NOT_FOUND
                || response.status() == StatusCode::METHOD_NOT_ALLOWED,
            "expected 404/405 when gateway not mounted, got {}",
            response.status()
        );
    }

    // ── T5: Catalog-parametric scope test ───────────────────────────────────

    /// T5: every gateway action that has requires_admin=true must return FORBIDDEN
    /// on the API surface when the caller has only lab:read scope.
    #[tokio::test]
    async fn gateway_admin_actions_require_admin_scope_on_api() {
        let admin_actions: Vec<&str> = crate::dispatch::gateway::ACTIONS
            .iter()
            .filter(|spec| spec.requires_admin)
            .map(|spec| spec.name)
            .collect();

        assert!(
            !admin_actions.is_empty(),
            "no gateway admin actions found in catalog — catalog bug"
        );

        let manager = test_manager();
        let app = gateway_routes_with_auth_context(manager, read_only_auth_context());

        for action in admin_actions {
            let response = post_gateway_routes(
                app.clone(),
                json!({
                    "action": action,
                    "params": {
                        "confirm": true,
                        "name": "fixture",
                        "upstream": "fixture",
                        "service": "plex",
                        "url": "https://fixture.example.com/mcp",
                        "spec": {"name":"fixture","url":"https://fixture.example.com/mcp"}
                    }
                }),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "action `{action}` should require lab:admin scope on API"
            );
        }
    }

    /// T5 (MCP surface): every gateway action that has requires_admin=true is
    /// correctly identified by `builtin_action_requires_admin` in mcp/context.rs.
    #[test]
    fn gateway_catalog_requires_admin_matches_mcp_context_gate() {
        use std::future::Future;

        use crate::mcp::context::builtin_action_requires_admin;
        use crate::registry::RegisteredService;

        fn noop_dispatch(
            _: String,
            _: serde_json::Value,
        ) -> std::pin::Pin<
            Box<
                dyn Future<Output = Result<serde_json::Value, crate::dispatch::error::ToolError>>
                    + Send,
            >,
        > {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }

        let entry = RegisteredService {
            name: "gateway",
            description: "Gateway",
            category: "bootstrap",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: crate::dispatch::gateway::ACTIONS,
            dispatch: noop_dispatch,
        };

        for spec in crate::dispatch::gateway::ACTIONS {
            let catalog_says = spec.requires_admin;
            let mcp_says = builtin_action_requires_admin(&entry, spec.name);
            assert_eq!(
                catalog_says, mcp_says,
                "mismatch for `{}`: catalog.requires_admin={catalog_says} but mcp gate={mcp_says}",
                spec.name
            );
        }
    }

    // ── Existing functional tests (updated for authenticated app) ────────────

    #[tokio::test]
    async fn gateway_list_route_exists() {
        let response =
            post_gateway_as_admin(test_manager(), json!({"action":"gateway.list","params":{}}))
                .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gateway_sensitive_actions_require_admin_when_authenticated() {
        let app = gateway_routes_with_auth_context(test_manager(), read_only_auth_context());

        for action in [
            "gateway.list",
            "gateway.status",
            "gateway.service_config.get",
            "gateway.add",
            "gateway.reload",
            "gateway.oauth.probe",
            "gateway.mcp.cleanup",
        ] {
            let response = post_gateway_routes(
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

        let response =
            post_gateway_as_admin(manager, json!({"action":"gateway.list","params":{}})).await;

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
        let app = test_app_with_auth_context(manager.clone(), admin_auth_context());

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
        let response = post_gateway_as_admin(
            test_manager(),
            json!({"action":"gateway.get","params":{"name":"fixture-http"}}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn gateway_test_accepts_proposed_spec() {
        let response = post_gateway_as_admin(
            test_manager(),
            json!({
                "action":"gateway.test",
                "params":{"confirm":true,"spec":{"name":"fixture-stdio","command":"echo","args":["hello"]}}
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gateway_add_update_remove_reload_routes_exist() {
        let manager = test_manager();
        let app = test_app_with_auth_context(manager, admin_auth_context());

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
                priority: 1.0,
            }])
            .await;

        let response = post_gateway_as_admin(
            manager,
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
        let manager = test_manager();
        let app = test_app_with_auth_context(manager, admin_auth_context());
        let response = post_gateway(
            app,
            json!({
                "action":"gateway.add",
                "params":{"spec":{"name":"fixture-http","url":"http://127.0.0.1:9001","bearer_token_env":"FIXTURE_HTTP_TOKEN"}}
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn gateway_actions_endpoint_is_registered() {
        let response = get_gateway_actions(test_app()).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}
