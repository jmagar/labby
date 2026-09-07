use std::{net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
    routing::post,
};
use labby_auth::VerifiedIdentity;
use serde::Deserialize;
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(descriptors.next().unwrap(), post(handle))
        .route(descriptors.next().unwrap(), post(search_tools))
        .route(descriptors.next().unwrap(), post(describe_tool))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "gateway", RouteAuth::V1),
        RouteDescriptor::new(
            "POST",
            "/codemode/tools/search",
            "search_tools",
            "gateway",
            RouteAuth::V1,
        ),
        RouteDescriptor::new(
            "POST",
            "/codemode/tools/describe",
            "describe_tool",
            "gateway",
            RouteAuth::V1,
        ),
    ]
    .into_iter()
    .map(|route| {
        route
            .feature("gateway")
            .when("mounted only when API authentication is configured")
    })
    .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolSearchRequest {
    query: String,
    #[serde(default = "default_tool_search_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDescribeRequest {
    target: String,
}

const fn default_tool_search_limit() -> usize {
    50
}

async fn search_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(request): Json<ToolSearchRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let started = std::time::Instant::now();
    private_tool_browser_admin(&auth)
        .map_err(|error| private_tool_error(error, "tools.search", started, &headers))?;
    if request.query.len() > labby_codemode::QUERY_MAX_BYTES {
        return Err(private_tool_error(
            ToolError::InvalidParam {
                message: format!(
                    "query exceeds {} UTF-8 bytes",
                    labby_codemode::QUERY_MAX_BYTES
                ),
                param: "query".into(),
            },
            "tools.search",
            started,
            &headers,
        ));
    }
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(manager_not_wired)
        .map_err(|error| private_tool_error(error, "tools.search", started, &headers))?;
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let response = manager
        .search_admin_tools(subject, &request.query, request.limit)
        .await
        .map_err(|error| private_tool_error(error, "tools.search", started, &headers))?;
    tracing::info!(
        surface = "api",
        service = "gateway",
        action = "tools.search",
        elapsed_ms = started.elapsed().as_millis(),
        result_count = response.results.len(),
        response_bytes = serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()),
        request_id = headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        "dispatch completed"
    );
    Ok(no_referrer(Json(response)))
}

async fn describe_tool(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(request): Json<ToolDescribeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let started = std::time::Instant::now();
    private_tool_browser_admin(&auth)
        .map_err(|error| private_tool_error(error, "tools.describe", started, &headers))?;
    if request.target.len() > labby_codemode::TARGET_MAX_BYTES {
        return Err(private_tool_error(
            ToolError::InvalidParam {
                message: format!(
                    "target exceeds {} UTF-8 bytes",
                    labby_codemode::TARGET_MAX_BYTES
                ),
                param: "target".into(),
            },
            "tools.describe",
            started,
            &headers,
        ));
    }
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(manager_not_wired)
        .map_err(|error| private_tool_error(error, "tools.describe", started, &headers))?;
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let response = manager
        .describe_admin_tool(subject, &request.target)
        .await
        .map_err(|error| private_tool_error(error, "tools.describe", started, &headers))?;
    tracing::info!(
        surface = "api",
        service = "gateway",
        action = "tools.describe",
        elapsed_ms = started.elapsed().as_millis(),
        response_bytes = serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()),
        request_id = headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        "dispatch completed"
    );
    Ok(no_referrer(Json(response)))
}

fn private_tool_browser_admin(auth: &Option<Extension<AuthContext>>) -> Result<(), ToolError> {
    if has_admin_scope(auth.as_ref()) {
        return Ok(());
    }
    Err(ToolError::Forbidden {
        message: "tool browser requires `lab:admin` scope".into(),
        required_scopes: vec!["lab:admin".into()],
    })
}

fn manager_not_wired() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "gateway manager not wired".into(),
    }
}

fn private_tool_error(
    error: ToolError,
    action: &'static str,
    started: std::time::Instant,
    headers: &HeaderMap,
) -> ApiError {
    let elapsed_ms = started.elapsed().as_millis();
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    if error.kind() == "internal_error" {
        let diagnostic =
            labby_runtime::agent_error::redact_secret_like_segments(&error.to_string());
        tracing::error!(
            surface = "api",
            service = "gateway",
            action,
            elapsed_ms,
            kind = error.kind(),
            error = %diagnostic,
            request_id,
            "dispatch failed"
        );
    } else {
        tracing::warn!(
            surface = "api",
            service = "gateway",
            action,
            elapsed_ms,
            kind = error.kind(),
            request_id,
            "dispatch failed"
        );
    }
    ApiError::new(error).with_service_action("gateway", action)
}

fn no_referrer<T: serde::Serialize>(body: Json<T>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    (headers, body)
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
    if !crate::access::gateway_transport_requires_admin(action) {
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

fn http_oauth_subject(auth: Option<&AuthContext>, request_subject: Option<&str>) -> Option<String> {
    auth.and_then(|auth| {
        crate::dispatch::oauth_subject::oauth_upstream_subject_for_request(
            Some(auth),
            request_subject,
        )
        .map(|subject| subject.into_owned())
    })
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
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_gateway_admin(&req.action, request_id, auth.as_ref())?;
    let auth_context = auth.as_ref().ok_or_else(|| {
        ApiError::from(ToolError::Forbidden {
            message: "Gateway operation is not authorized".into(),
            required_scopes: Vec::new(),
        })
    })?;
    let identity = identity
        .ok_or_else(|| {
            ApiError::from(ToolError::Forbidden {
                message: "Gateway operation is not authorized".into(),
                required_scopes: Vec::new(),
            })
        })?
        .0;
    let installation_id = state.installation_id.as_deref().unwrap_or("installation");
    let team_id = headers
        .get("x-labby-team-id")
        .and_then(|value| value.to_str().ok());
    crate::access::authorize_gateway_action(
        &state.access_runtime,
        identity,
        &auth_context.0,
        installation_id,
        team_id,
        &req.action,
    )
    .await
    .map_err(ApiError::from)?;
    let team_id = team_id.map(str::to_owned);
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let auth_for_dispatch = auth.clone();
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "gateway manager not wired".to_string(),
        })?;

    handle_action_with_meta(
        "gateway",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::gateway::ACTIONS,
        move |action, params| {
            let manager = Arc::clone(&manager);
            let subject = subject.clone();
            let auth = auth_for_dispatch.clone();
            async move {
                let params = crate::access::qualify_team_gateway_params(
                    &action,
                    team_id.as_deref(),
                    params,
                )?;
                let params = inject_gateway_owner(&action, params, subject.as_deref(), request_id);
                // Unlike trusted stdio MCP, an unauthenticated HTTP request
                // must never inherit the shared gateway OAuth credential.
                let oauth_subject =
                    http_oauth_subject(auth.as_ref().map(|value| &value.0), subject.as_deref());
                let oauth_subject = crate::access::gateway_runtime_subject(
                    &action,
                    team_id.as_deref(),
                    oauth_subject.as_deref(),
                );
                let mut response = crate::dispatch::gateway::dispatch_with_manager_scoped(
                    &manager,
                    &action,
                    params,
                    crate::dispatch::gateway::GatewayEnrichmentScope {
                        route_visible_upstreams: None,
                        oauth_subject,
                    },
                )
                .await?;
                crate::access::filter_team_gateway_projection(team_id.as_deref(), &mut response);
                Ok(response)
            }
        },
    )
    .await
}

fn inject_gateway_owner(
    action: &str,
    params: Value,
    subject: Option<&str>,
    request_id: Option<&str>,
) -> Value {
    if !crate::dispatch::gateway::shared::action_accepts_runtime_owner(action) {
        return params;
    }
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    let owner = crate::dispatch::gateway::shared::make_api_runtime_owner(subject, request_id);
    let origin = owner.raw.clone();
    // Serialize the owner struct into its JSON shape for the params object.
    // The fields match the GatewayRuntimeOwnerParams shape consumed by dispatch.
    object.insert(
        "owner".to_string(),
        serde_json::json!({
            "surface": owner.surface,
            "subject": owner.subject,
            "request_id": owner.request_id,
            "raw": owner.raw,
        }),
    );
    if let Some(origin) = origin {
        object.insert("origin".to_string(), Value::String(origin));
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

    use super::{gateway_action_requires_admin, http_oauth_subject, inject_gateway_owner};

    use crate::api::oauth::AuthContext;
    use crate::api::{
        router::{build_router_with_bearer, build_router_with_external_auth},
        state::AppState,
    };
    use crate::config::{
        LabConfig, UpstreamConfig, VirtualServerConfig, VirtualServerSurfacesConfig,
    };
    use crate::dispatch::gateway::config_store::{
        load_gateway_config, test_gateway_manager, write_gateway_config,
    };
    use crate::dispatch::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use crate::registry::build_default_registry;

    // ── Test fixtures ────────────────────────────────────────────────────────

    fn test_manager_with_path() -> (Arc<GatewayManager>, std::path::PathBuf) {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let path = std::env::temp_dir().join(format!(
            "labby-gateway-api-test-{}-{}.toml",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        (
            Arc::new(test_gateway_manager(
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
    async fn authorized_test_state(manager: Arc<GatewayManager>) -> AppState {
        let directory = tempfile::Builder::new()
            .prefix("labby-gateway-access-test-")
            .tempdir_in(std::env::current_dir().expect("test working directory"))
            .expect("access tempdir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("secure access tempdir");
        }
        let directory = directory.keep();
        let runtime =
            Arc::new(crate::access::AccessRuntime::initialize(directory.join("access.db")).await);
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .expect("static bearer identity");
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity, "Local", "Default")
                    .expect("bootstrap input"),
            )
            .await
            .expect("bootstrap access authority");
        AppState::from_registry(build_default_registry())
            .with_gateway_manager(manager)
            .with_access_runtime(runtime)
    }

    async fn test_app_with_manager(manager: Arc<GatewayManager>) -> Router {
        let state = authorized_test_state(manager).await;
        // Use a static bearer token so needs_auth=true and /v1/gateway is mounted.
        build_router_with_bearer(state, Some("test-token".into()), None)
    }

    async fn test_app() -> Router {
        test_app_with_manager(test_manager()).await
    }

    /// App with bearer auth + an injected AuthContext (for scope-gated tests).
    async fn test_app_with_auth_context(manager: Arc<GatewayManager>, auth: AuthContext) -> Router {
        test_app_with_manager(manager).await.layer(Extension(auth))
    }

    /// Mount ONLY the gateway route group with a layered `AuthContext` and no
    /// bearer-auth middleware — exercises the per-action scope gate in isolation.
    ///
    /// The full-router static bearer path always injects `lab:admin`, so it
    /// cannot model a non-admin caller. Mounting `services::gateway::routes`
    /// directly (mirroring `upstream_oauth_routes_require_admin_scope`) lets the
    /// layered read-only context survive to the handler's scope gate.
    async fn gateway_routes_with_auth_context(
        manager: Arc<GatewayManager>,
        auth: AuthContext,
    ) -> Router {
        let state = authorized_test_state(manager).await;
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .expect("static bearer identity");
        super::routes(state.clone())
            .router
            .layer(Extension(auth))
            .layer(Extension(identity))
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

    #[test]
    fn gateway_owner_injection_skips_strict_read_only_actions() {
        let params = json!({"upstream": "fixture"});
        let enriched = inject_gateway_owner(
            "gateway.skills.list",
            params.clone(),
            Some("admin-user"),
            Some("request-1"),
        );
        assert_eq!(enriched, params);
    }

    #[test]
    fn gateway_owner_injection_preserves_mutation_provenance() {
        let enriched = inject_gateway_owner(
            "gateway.add",
            json!({"spec": {"name": "fixture"}}),
            Some("admin-user"),
            Some("request-1"),
        );
        assert_eq!(enriched["owner"]["surface"], "api");
        assert_eq!(enriched["owner"]["subject"], "admin-user");
        assert_eq!(enriched["owner"]["request_id"], "request-1");
        assert_eq!(enriched["origin"], "api:admin-user:request-1");
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn gateway_skills_list_api_keeps_strict_action_params_clean() {
        let response = post_gateway_as_admin(
            test_manager(),
            json!({"action": "gateway.skills.list", "params": {}}),
        )
        .await;

        assert_ne!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        if !response.status().is_success() {
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_ne!(payload["kind"], "invalid_param");
            assert!(!payload.to_string().contains("unknown field `owner`"));
        }
    }

    #[test]
    fn http_oauth_subject_fails_closed_without_verified_auth_context() {
        assert!(http_oauth_subject(None, None).is_none());
        assert!(http_oauth_subject(None, Some("forged-subject")).is_none());

        let admin = admin_auth_context();
        assert_eq!(
            http_oauth_subject(Some(&admin), Some("admin-user")).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );

        let reader = read_only_auth_context();
        assert_eq!(
            http_oauth_subject(Some(&reader), Some("read-only-user")).as_deref(),
            Some("read-only-user")
        );
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
        let app = test_app_with_auth_context(manager, admin_auth_context()).await;
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
        let app = test_app().await;

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

    #[tokio::test]
    async fn trusted_outer_auth_mounts_gateway_without_bearer_middleware() {
        let state = authorized_test_state(test_manager()).await;
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .expect("static bearer identity");
        let app = build_router_with_external_auth(state, None, None, None, &[], true)
            .layer(Extension(admin_auth_context()))
            .layer(Extension(identity));

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

        assert_eq!(response.status(), StatusCode::OK);
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
        let app = gateway_routes_with_auth_context(manager, read_only_auth_context()).await;

        for action in admin_actions {
            let response = post_gateway_routes(
                app.clone(),
                json!({
                    "action": action,
                    "params": {
                        "confirm": true,
                        "name": "fixture",
                        "upstream": "fixture",
                        "service": "gateway-alpha",
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

    #[tokio::test]
    async fn resource_lease_api_returns_typed_document_for_admin() {
        let registry = labby_auth::resource_registry::ResourceRegistry::new();
        let manager = Arc::new(
            test_gateway_manager(
                std::env::temp_dir().join("labby-resource-lease-api.toml"),
                GatewayRuntimeHandle::default(),
            )
            .with_resource_registry(registry),
        );
        let app = gateway_routes_with_auth_context(manager, admin_auth_context()).await;
        let response = post_gateway_routes(
            app,
            json!({
                "action": "gateway.oauth.resource_lease.create",
                "params": {
                    "resource": "https://proxy.example:53147/mcp",
                    "scopes": ["mcp:read"],
                    "ttl_secs": 120,
                    "owner": "api-test"
                }
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let lease: labby_auth::resource_registry::ResourceLease =
            serde_json::from_slice(&body).unwrap();
        assert_eq!(lease.resource, "https://proxy.example:53147/mcp");
        assert_eq!(lease.scopes, vec!["mcp:read"]);
        assert!(!lease.id.is_empty());
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
    async fn gateway_code_mode_mcp_ui_update_persists_via_api() {
        let _guard = crate::config::process_code_mode_test_guard();
        let (manager, path) = test_manager_with_path();
        manager
            .seed_config_unchecked_for_tests(
                LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        mcp_ui_enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    ..LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;
        assert!(manager.code_mode_app_state().is_enabled());

        let response = post_gateway_as_admin(
            Arc::clone(&manager),
            json!({
                "action": "gateway.code_mode.set",
                "params": {"mcp_ui_enabled": false}
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload["mcp_ui_enabled"], false);
        assert!(!manager.code_mode_app_state().is_enabled());
        assert!(!manager.code_mode_config().await.mcp_ui_enabled);

        let persisted = load_gateway_config(&path).expect("load persisted gateway config");
        assert!(!persisted.code_mode.mcp_ui_enabled);

        let restarted = Arc::new(test_gateway_manager(path, GatewayRuntimeHandle::default()));
        restarted
            .seed_config_unchecked_for_tests(persisted.to_gateway_config())
            .await;
        assert!(!restarted.code_mode_app_state().is_enabled());
        assert!(!restarted.code_mode_config().await.mcp_ui_enabled);
    }

    #[tokio::test]
    async fn gateway_sensitive_actions_require_admin_when_authenticated() {
        let app = gateway_routes_with_auth_context(test_manager(), read_only_auth_context()).await;

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
                        "service": "gateway-alpha",
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
            .seed_config_unchecked_for_tests(
                LabConfig {
                    virtual_servers: vec![VirtualServerConfig {
                        id: "stale-service".to_string(),
                        service: "missing-service".to_string(),
                        enabled: true,
                        surfaces: VirtualServerSurfacesConfig {
                            mcp: true,
                            ..VirtualServerSurfacesConfig::default()
                        },
                        mcp_policy: None,
                    }],
                    ..LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;

        let response =
            post_gateway_as_admin(manager, json!({"action":"gateway.list","params":{}})).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload[0]["id"], "stale-service");
        assert_eq!(payload[0]["warnings"][0]["code"], "unknown_service");
    }

    #[tokio::test]
    async fn gateway_reload_quarantines_stale_virtual_server_before_list() {
        let (manager, path) = test_manager_with_path();
        write_gateway_config(
            &path,
            &LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "stale-service".to_string(),
                    service: "missing-service".to_string(),
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
        let app = test_app_with_auth_context(manager.clone(), admin_auth_context()).await;

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
        assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-service");
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
        let app = test_app_with_auth_context(manager, admin_auth_context()).await;

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
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
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
    async fn gateway_routes_do_not_require_destructive_confirm_under_data_loss_definition() {
        let manager = test_manager();
        let app = test_app_with_auth_context(manager, admin_auth_context()).await;
        let response = post_gateway(
            app,
            json!({
                "action":"gateway.add",
                "params":{"spec":{"name":"fixture-http","url":"http://127.0.0.1:9001","bearer_token_env":"FIXTURE_HTTP_TOKEN"}}
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gateway_actions_endpoint_is_registered() {
        let response = get_gateway_actions(test_app().await).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn post_tool_browser(
        app: Router,
        path: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    #[tokio::test]
    async fn api_admin_tool_browser_requires_exact_admin_scope() {
        let read = gateway_routes_with_auth_context(test_manager(), read_only_auth_context()).await;
        let response =
            post_tool_browser(read, "/codemode/tools/search", json!({"query":"issues"})).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let admin = gateway_routes_with_auth_context(test_manager(), admin_auth_context()).await;
        let response =
            post_tool_browser(admin, "/codemode/tools/search", json!({"query":"issues"})).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::REFERRER_POLICY).unwrap(),
            "no-referrer"
        );
    }

    #[tokio::test]
    async fn api_admin_tool_browser_rejects_authority_injection() {
        let admin = gateway_routes_with_auth_context(test_manager(), admin_auth_context()).await;
        let response = post_tool_browser(
            admin,
            "/codemode/tools/search",
            json!({"query":"issues", "scope": {}}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_admin_tool_describe_enforces_auth_validation_and_neutral_not_found() {
        let read = gateway_routes_with_auth_context(test_manager(), read_only_auth_context()).await;
        let forbidden = post_tool_browser(
            read,
            "/codemode/tools/describe",
            json!({"target":"alpha::ping"}),
        )
        .await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        let admin = gateway_routes_with_auth_context(test_manager(), admin_auth_context()).await;
        let oversized = post_tool_browser(
            admin.clone(),
            "/codemode/tools/describe",
            json!({"target":"x".repeat(labby_codemode::TARGET_MAX_BYTES + 1)}),
        )
        .await;
        assert_eq!(oversized.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let unknown = post_tool_browser(
            admin,
            "/codemode/tools/describe",
            json!({"target":"alpha::missing"}),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(unknown.into_body(), usize::MAX)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(payload["kind"], "unknown_tool");
    }

    #[test]
    fn unknown_tool_maps_to_not_found_for_private_describe() {
        use axum::response::IntoResponse;
        let response = crate::api::error::ApiError::new(crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "unknown_tool".into(),
            message: "hidden".into(),
        })
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn team_policy_actions_reach_domain_authorization_without_admin_scope() {
        assert!(!gateway_action_requires_admin("gateway.loadout.list"));
        assert!(!gateway_action_requires_admin("gateway.loadout.add"));
        assert!(!gateway_action_requires_admin(
            "gateway.protected_route.remove"
        ));
        assert!(gateway_action_requires_admin("gateway.add"));
        assert!(gateway_action_requires_admin("gateway.oauth.clear"));
        assert!(gateway_action_requires_admin("gateway.unknown"));
    }
}
