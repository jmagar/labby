//! Top-level axum router — mounts `POST /v1/<service>` for every enabled service
//! and the MCP streamable HTTP transport at `/mcp`.

#[cfg(feature = "gateway")]
#[path = "protected_mcp_route.rs"]
mod protected_mcp_route;

#[cfg(feature = "gateway")]
use protected_mcp_route::protected_mcp_intercept;
#[cfg(all(test, feature = "gateway"))]
use protected_mcp_route::{
    ProtectedRouteExposureDecision, auth_error_response_with_challenge,
    filter_protected_route_list_response, filter_protected_route_sse_event,
    filter_protected_route_sse_stream, find_sse_event_end, prepare_protected_route_request,
    protected_route_exposure_decision, protected_route_json_rpc_error, quoted_challenge_value,
};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "api-docs")]
use axum::response::Html;
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Extension, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header},
    middleware::Next,
    response::IntoResponse,
    routing::{get, post},
};
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::Level;

use labby_auth::AuthLayer;
use labby_auth::error::AuthError as LabAuthError;

use crate::app_manifest::{
    APPS_LAUNCHER_ROUTE, APPS_MANIFEST_API_ROUTE, LABBY_APP_HOST_JS_ROUTE,
    SERVER_LOGS_BROWSER_ROUTE, SERVER_LOGS_DATA_API_PREFIX,
};

use super::router_middleware::lab_auth_deriver;

use super::app_routes::{
    apps_launcher_page, apps_manifest, labby_app_host_js, server_logs_app_page,
};
use super::dev_mockup::{dev_mockup, dev_mockup_named};
use super::{health, services, state::AppState};
use crate::api::error::ApiError;
use crate::dispatch::error::ToolError;

/// Transport backstop for one hosted HTTP request.
///
/// Derived from the configured upstream deadlines on **every** route, never a
/// fixed cap. `/v1` handlers relay upstream calls too (palette execute, gateway
/// dispatch), so a fixed cap would return a bare 504 for a call that had
/// already succeeded — the regression documented on
/// `LabConfig::http_request_timeout` — and would blind the upstream circuit
/// breaker, which declines to count `Cancelled` as a failure precisely because
/// this backstop is derived to exceed the upstream deadline.
///
/// Still a startup snapshot: `AppState::config` is an `Arc<LabConfig>` set once
/// by `with_config` and never replaced on gateway reload, so raising
/// `upstream_request_timeout_ms` / `upstream_relay_timeout_ms` does not move
/// this backstop until the process restarts. Restart after widening a timeout,
/// or make `AppState::config` re-read on reload.
fn http_request_timeout(config: &crate::config::LabConfig) -> Duration {
    config.http_request_timeout()
}

async fn request_timeout(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> axum::response::Response {
    let path = request.uri().path().to_string();
    let timeout = http_request_timeout(&state.config);
    match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(
                surface = "api",
                path,
                timeout_ms = timeout.as_millis(),
                "HTTP request exceeded the transport backstop"
            );
            StatusCode::GATEWAY_TIMEOUT.into_response()
        }
    }
}

fn app_auth_state(state: &AppState) -> Result<labby_auth::state::AuthState, LabAuthError> {
    state
        .oauth_state
        .as_ref()
        .map(|state| (**state).clone())
        .ok_or_else(|| LabAuthError::Config("oauth auth state is not configured".to_string()))
}

async fn app_auth_state_with_protected_routes(
    state: &AppState,
) -> Result<labby_auth::state::AuthState, LabAuthError> {
    let auth_state = app_auth_state(state)?;
    #[cfg(feature = "gateway")]
    if let Some(manager) = state.gateway_manager.as_ref() {
        let routes = manager.protected_route_list().await;
        tracing::debug!(
            route_count = routes.iter().filter(|route| route.enabled).count(),
            "oauth protected resource scope map refreshed from gateway routes"
        );
        auth_state
            .replace_configured_resource_scopes(
                routes
                    .into_iter()
                    .filter(|route| route.enabled)
                    .map(|route| (route.public_resource(), route.scopes)),
            )
            .map_err(|error| {
                LabAuthError::Config(format!("invalid configured protected resource: {error}"))
            })?;
    }
    Ok(auth_state)
}

async fn auth_authorization_server_metadata(
    State(state): State<AppState>,
) -> Result<Json<labby_auth::types::AuthorizationServerMetadata>, LabAuthError> {
    let auth_state = app_auth_state(&state)?;
    Ok(labby_auth::metadata::authorization_server_metadata(State(auth_state)).await)
}

async fn auth_protected_resource_metadata(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Result<axum::response::Response, LabAuthError> {
    #[cfg(feature = "gateway")]
    if let (Some(manager), Some(host)) = (
        state.gateway_manager.as_ref(),
        request_host(&request, state.config.api.trust_forwarded_headers),
    ) && let Some(route) = manager
        .resolve_protected_route_metadata(&host, request.uri().path())
        .await
    {
        tracing::info!(
            host = %host,
            path = %request.uri().path(),
            route = %route.name,
            resource = %route.public_resource(),
            scopes = ?route.scopes,
            "oauth protected resource metadata served"
        );
        let auth_state = app_auth_state_with_protected_routes(&state).await?;
        let public_url = auth_state
            .config
            .public_url
            .as_ref()
            .ok_or_else(|| LabAuthError::Config("LABBY_PUBLIC_URL is required".to_string()))?;
        return Ok(protected_resource_metadata_response(
            labby_auth::types::ProtectedResourceMetadata {
                resource: route.public_resource(),
                authorization_servers: vec![public_url.as_str().trim_end_matches('/').to_string()],
                scopes_supported: route.scopes,
                bearer_methods_supported: vec!["header".to_string()],
            },
        ));
    }
    let auth_state = app_auth_state(&state)?;
    let public_url = auth_state
        .config
        .public_url
        .as_ref()
        .ok_or_else(|| LabAuthError::Config("LABBY_PUBLIC_URL is required".to_string()))?;
    Ok(protected_resource_metadata_response(
        labby_auth::types::ProtectedResourceMetadata {
            resource: labby_auth::metadata::canonical_resource_url(&auth_state),
            authorization_servers: vec![public_url.as_str().trim_end_matches('/').to_string()],
            scopes_supported: auth_state.config.scopes_supported.clone(),
            bearer_methods_supported: vec!["header".to_string()],
        },
    ))
}

#[cfg(feature = "gateway")]
async fn protected_route_resource_metadata(
    State(state): State<AppState>,
    request: Request<Body>,
) -> axum::response::Response {
    let Some(manager) = state.gateway_manager.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(host) = request_host(&request, state.config.api.trust_forwarded_headers) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = request.uri().path();
    let Some(route) = manager.resolve_protected_route_metadata(&host, path).await else {
        tracing::warn!(
            host = %host,
            path = %path,
            "oauth protected resource metadata not found"
        );
        return StatusCode::NOT_FOUND.into_response();
    };
    tracing::info!(
        host = %host,
        path = %path,
        route = %route.name,
        resource = %route.public_resource(),
        scopes = ?route.scopes,
        "oauth protected resource metadata served"
    );
    protected_route_metadata_response(&state, route).await
}

#[cfg(feature = "gateway")]
async fn protected_route_metadata_response(
    state: &AppState,
    route: crate::config::ProtectedMcpRouteConfig,
) -> axum::response::Response {
    let Ok(auth_state) = app_auth_state_with_protected_routes(&state).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Some(public_url) = auth_state.config.public_url.as_ref() else {
        tracing::error!(
            route = %route.name,
            resource = %route.public_resource(),
            "oauth protected resource metadata failed: LABBY_PUBLIC_URL missing"
        );
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    protected_resource_metadata_response(labby_auth::types::ProtectedResourceMetadata {
        resource: route.public_resource(),
        authorization_servers: vec![public_url.as_str().trim_end_matches('/').to_string()],
        scopes_supported: route.scopes,
        bearer_methods_supported: vec!["header".to_string()],
    })
}

fn protected_resource_metadata_response(
    metadata: labby_auth::types::ProtectedResourceMetadata,
) -> axum::response::Response {
    let mut response = Json(metadata).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}

fn request_host(request: &Request<Body>, trust_forwarded_headers: bool) -> Option<String> {
    let forwarded = trust_forwarded_headers
        .then(|| request.headers().get("x-forwarded-host"))
        .flatten();
    forwarded
        .or_else(|| request.headers().get(header::HOST))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(ToOwned::to_owned)
}

async fn auth_jwks(State(state): State<AppState>) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::metadata::jwks(State(app_auth_state(&state)?)).await)
}

async fn auth_register(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Json<labby_auth::types::ClientRegistrationRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::authorize::register_client(
        State(app_auth_state(&state)?),
        ConnectInfo(addr),
        body,
    )
    .await?)
}

async fn auth_authorize(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    query: Query<labby_auth::types::AuthorizeQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::authorize::authorize(
        State(app_auth_state_with_protected_routes(&state).await?),
        ConnectInfo(addr),
        headers,
        query,
    )
    .await?)
}

async fn auth_browser_login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    query: Query<labby_auth::types::BrowserLoginQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::authorize::browser_login(
        State(app_auth_state(&state)?),
        ConnectInfo(addr),
        query,
    )
    .await?)
}

async fn auth_callback(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    query: Query<labby_auth::types::CallbackQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::authorize::callback(
        State(app_auth_state(&state)?),
        headers,
        labby_auth::authorize::RemoteAddr(addr),
        query,
    )
    .await?)
}

async fn auth_token(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    form: axum::extract::Form<labby_auth::types::TokenRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::token::token(
        State(app_auth_state_with_protected_routes(&state).await?),
        Some(Extension(ConnectInfo(addr))),
        headers,
        form,
    )
    .await)
}

async fn auth_revoke(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    form: axum::extract::Form<labby_auth::types::RevocationRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::token::revoke(
        State(app_auth_state(&state)?),
        Some(Extension(ConnectInfo(addr))),
        headers,
        form,
    )
    .await)
}

async fn auth_native_callback(
    query: Query<labby_auth::types::NativeCallbackQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::authorize::native_callback(query).await?)
}

async fn auth_native_poll(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Json<labby_auth::types::NativePollQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(labby_auth::authorize::native_poll(
        State(app_auth_state(&state)?),
        labby_auth::authorize::RemoteAddr(addr),
        body,
    )
    .await?)
}

async fn reject_ambiguous_request_target(
    request: Request<Body>,
    next: Next,
) -> axum::response::Response {
    let path = request.uri().path();
    let encoded = path.to_ascii_lowercase();
    let has_dot_segment = path.split('/').any(|segment| matches!(segment, "." | ".."));
    let has_encoded_separator =
        encoded.contains("%2e") || encoded.contains("%2f") || encoded.contains("%5c");

    if has_dot_segment || has_encoded_separator {
        return StatusCode::BAD_REQUEST.into_response();
    }
    next.run(request).await
}

fn is_public_relay_reserved_path(path: &str) -> bool {
    crate::oauth::public_relay::is_reserved_public_relay_path(path)
}

/// Build the `/v1` sub-router with all feature-gated service routes.
#[cfg_attr(not(feature = "fs"), allow(unused_variables))]
fn build_v1_router(
    state: &AppState,
    api_auth_configured: bool,
    integrated_trusted_host: bool,
) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor, RouteGroup};
    #[cfg(feature = "api-docs")]
    let openapi_spec: Arc<String> = super::openapi::build_openapi_spec(state.registry.services())
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to serialize OpenAPI spec");
            Arc::new(String::from(r#"{"error":"spec generation failed"}"#))
        });
    #[cfg(feature = "api-docs")]
    let spec_for_route = openapi_spec;

    let mut v1 = RouteGroup::empty().route(
        RouteDescriptor::new(
            "GET",
            concat!("/", "{service}", "/actions"),
            "service_actions",
            "services",
            RouteAuth::V1,
        ),
        get(service_actions),
    );
    v1 = v1.nest("/catalog", services::catalog::routes(state.clone()));
    v1 = v1.nest("/depot", services::depot::routes(state.clone()));
    #[cfg(target_os = "linux")]
    if api_auth_configured
        && state.enabled_services.contains("stash")
        && state.registry.dispatch_capability("stash")
            == Some(crate::registry::DispatchCapability::CallerBound)
    {
        v1 = v1.nest("/stash", services::file_stash::routes(state.clone()));
    }
    if api_auth_configured && !integrated_trusted_host {
        v1 = v1.nest("/browser", services::browser::routes(state.clone()));
        v1 = v1.nest(
            "/oauth/relay",
            services::oauth_relay::admin_routes(state.clone()),
        );
        #[cfg(feature = "skills")]
        {
            v1 = v1.nest("/artifacts", services::skills::routes(state.clone()));
            for service in ["bundles", "jobs", "sources", "uploads"] {
                if state.enabled_services.contains(service) {
                    v1 = v1.nest(
                        &format!("/{service}"),
                        services::remote_control::routes(service, state.clone()),
                    );
                }
            }
        }
    } else {
        #[cfg(feature = "skills")]
        tracing::warn!(
            subsystem = "startup",
            phase = "skills.mount.skipped",
            reason = "no_auth_configured",
            "skills service routes not mounted: Agent Skills require API auth"
        );
    }

    #[cfg(feature = "gateway")]
    {
        // upstream oauth must be nested before /gateway so its more-specific prefix wins;
        // only mount when the gateway manager is present (oauth requires it).
        if state.gateway_manager.is_some() && !integrated_trusted_host {
            v1 = v1.nest(
                "/gateway/oauth",
                crate::api::upstream_oauth::gateway_routes(state.clone()),
            );
        }

        // SECURITY (T1): gateway admin actions spawn arbitrary local stdio commands
        // with labby's full process environment. Refuse to mount /v1/gateway when
        // auth is not configured — unauthenticated HTTP access to gateway admin
        // actions is a critical vulnerability. Mirror the /v1/fs refusal pattern.
        if api_auth_configured {
            v1 = v1.nest("/gateway", services::gateway::routes(state.clone()));
            v1 = v1.nest("/snippets", services::snippets::routes(state.clone()));
            if state.gateway_manager.is_some() {
                v1 = v1.nest("/palette", services::palette::routes(state.clone()));
            } else {
                tracing::warn!(
                    subsystem = "startup",
                    phase = "palette.mount.skipped",
                    reason = "gateway_manager_missing",
                    "palette service routes not mounted: gateway manager is not wired"
                );
            }
        } else {
            tracing::warn!(
                subsystem = "startup",
                phase = "gateway.mount.skipped",
                reason = "no_auth_configured",
                "gateway service routes not mounted: HTTP API has no auth configured. \
                 Set LABBY_MCP_HTTP_TOKEN or LABBY_AUTH_MODE=oauth to enable /v1/gateway. \
                 Gateway admin actions can spawn arbitrary processes — never expose them unauthenticated."
            );
            tracing::warn!(
                subsystem = "startup",
                phase = "snippets.mount.skipped",
                reason = "no_auth_configured",
                "snippets service routes not mounted: executable snippets require API auth"
            );
            tracing::warn!(
                subsystem = "startup",
                phase = "palette.mount.skipped",
                reason = "no_auth_configured",
                "palette service routes not mounted: launcher execution requires API auth"
            );
        }
    }

    #[cfg(feature = "api-docs")]
    {
        v1 = v1
            .route(
                RouteDescriptor::new("GET", "/openapi.json", "openapi", "openapi", RouteAuth::V1)
                    .feature("api-docs"),
                get(move || {
                    let spec = spec_for_route.clone();
                    async move {
                        (
                            [
                                (header::CONTENT_TYPE, "application/json"),
                                (header::CACHE_CONTROL, "private, no-store"),
                            ],
                            (*spec).clone(),
                        )
                    }
                }),
            )
            .route(
                RouteDescriptor::new("GET", "/docs", "openapi_docs", "openapi", RouteAuth::V1)
                    .feature("api-docs"),
                get(|| async { Html(include_str!("openapi_docs.html")) }),
            );
    }

    v1 = v1
        .route(
            RouteDescriptor::new(
                "GET",
                APPS_MANIFEST_API_ROUTE.strip_prefix("/v1").unwrap(),
                "apps_manifest",
                "apps",
                RouteAuth::V1,
            ),
            get(apps_manifest),
        )
        .nest("/server_logs", services::server_logs::routes(state.clone()))
        .nest(
            SERVER_LOGS_DATA_API_PREFIX
                .strip_prefix("/v1")
                .expect("server logs data route must be under /v1"),
            services::server_logs::data_routes(state.clone()),
        )
        // Unauthenticated route groups are gated by host_validation_layer —
        // non-loopback Host headers are rejected before reaching the dispatcher
        // (DNS rebinding mitigation for the v1 wizard, lab-bg3e.3.3).
        .nest(
            "/doctor",
            services::doctor::routes(state.clone()).map_router(|router| {
                router.layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                ))
            }),
        )
        .nest(
            "/setup",
            services::setup::routes(state.clone()).map_router(|router| {
                router.layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                ))
            }),
        )
        .nest(
            "/auth/allowed-emails",
            services::auth_admin::routes(state.clone()),
        );

    if state.oauth_state.is_some() {
        v1 = v1.nest(
            "/access/bootstrap-owner",
            services::access_bootstrap::routes(state.clone()),
        );
    }
    v1 = v1
        .merge(services::access_credentials::issue_routes(state.clone()))
        .nest(
            "/access/credentials",
            services::access_credentials::routes(state.clone()),
        );

    #[cfg(feature = "fs")]
    if state
        .registry
        .services()
        .iter()
        .any(|service| service.name == "fs")
    {
        // SECURITY: fs operations read workspace files, so the workspace
        // runtime refuses to mount them on an unauthenticated API surface.
        // Static web UI auth settings do not bypass `/v1` auth when
        // bearer/OAuth auth is configured.
        if crate::workspace::WorkspaceRuntime::should_mount_http_routes(
            state.web_ui_auth_disabled,
            api_auth_configured,
        ) {
            v1 = v1.nest("/fs", services::fs::routes(state.clone()));
        } else {
            tracing::warn!(
                subsystem = "startup",
                phase = "fs.mount.skipped",
                reason = "web_ui_auth_disabled",
                "fs service is configured but LABBY_WEB_UI_AUTH_DISABLED=true would expose workspace files unauthenticated; refusing to mount /v1/fs"
            );
        }
    }

    // Anything under /v1 that no route above matches — a typo, or (the case
    // that motivated this) a service deliberately not mounted in this
    // deployment shape, like /v1/fs without auth or /v1/gateway without
    // api_auth_configured — would otherwise fall through this nest to the
    // outer SPA fallback, which answers non-GET requests with a bare
    // empty-body 404. The frontend has nothing to render from that but a
    // generic "An error occurred" (bead lab-gug4m), exactly the failure mode
    // docs/contracts/agent-error-contract.md forbids.
    //
    // NOTE ON REACHABILITY AND WORDING. This handler answers UNAUTHENTICATED
    // callers: `Router::nest` hoists an inner fallback into the outer
    // `fallback_router`, and `Router::route_layer` passes `fallback_router`
    // through unlayered by design, so the `/v1` auth layer never wraps it.
    // (Registering it as a catch-all route instead — which would land in
    // `path_router` and therefore be covered — is not possible: `/{*rest}`
    // collides with the `/{service}/actions` route above.)
    //
    // Anyone can already distinguish "mounted but unauthorized" (401) from
    // "not mounted" (404) by status code alone, and could before this change
    // too, since unmounted paths previously reached the SPA fallback's 404.
    // So the message deliberately states ONLY the method and path the caller
    // already sent, and must stay that way: naming mount state, or pointing
    // at `*.mount.skipped` logs, would add real deployment-shape detail to an
    // anonymous response on the one surface that refuses to mount
    // stdio-spawning gateway admin without auth. Operator-facing recovery
    // belongs in the startup warnings (which do name the skipped service) and
    // in the error contract's own `recovery` guidance, not here.
    if !integrated_trusted_host
        && (state.bearer_token.is_some() || state.oauth_state.is_some())
        && let Some(installation_id) = state.installation_id.as_deref()
    {
        let mounted_services = state
            .registry
            .services()
            .iter()
            .filter(|service| {
                v1.descriptors
                    .iter()
                    .any(|route| route.mount == service.name)
            })
            .map(|service| service.name.to_string())
            .collect();
        let snapshot = crate::integration_identity::IntegrationIdentity::snapshot(
            installation_id,
            state.bearer_token.is_some(),
            state.oauth_state.as_deref(),
            mounted_services,
        );
        v1 = v1.nest(
            "/integration",
            services::integration_identity::routes(snapshot),
        );
    }
    v1.router = v1.router.fallback(v1_route_not_found);
    v1
}

async fn v1_route_not_found(
    method: Method,
    // `OriginalUri`, not `Uri`: this handler is nested under `/v1`, so a plain
    // `Uri` extractor only sees the path remaining after the nest strips its
    // prefix — `OriginalUri` preserves the full incoming path.
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> axum::response::Response {
    ApiError::new(ToolError::Sdk {
        sdk_kind: "route_not_found".into(),
        message: format!("no route matches {method} {path}", path = uri.path()),
    })
    .into_response()
}

async fn labby_discovery(State(state): State<AppState>) -> axum::response::Response {
    let api_base_url = state
        .auth_config
        .as_ref()
        .and_then(|cfg| cfg.public_url.as_ref())
        .map(|url| url.as_str().trim_end_matches('/').to_string())
        .unwrap_or_else(|| "http://localhost:8765".to_string());
    let mut response = Json(serde_json::json!({
        "apiBaseUrl": api_base_url,
        "paletteCatalogUrl": format!("{api_base_url}/v1/palette/catalog"),
        "paletteSchemaUrl": format!("{api_base_url}/v1/palette/schema"),
        "paletteExecuteUrl": format!("{api_base_url}/v1/palette/execute"),
    }))
    .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response
}

pub fn build_router(
    state: AppState,
    bearer_token: Option<String>,
    auth_state: Option<labby_auth::state::AuthState>,
    mcp_router: Option<Router<AppState>>,
    config_cors_origins: &[String],
) -> Router {
    build_router_with_external_auth(
        state,
        bearer_token,
        auth_state,
        mcp_router,
        config_cors_origins,
        false,
    )
}

/// Build the hosted HTTP router with an optional trusted outer authentication boundary.
///
/// `external_auth_configured` is used by the Unix peer-credential listener. The
/// listener rejects unauthorized streams before HTTP parsing and injects an
/// `AuthContext` into every accepted request. It therefore enables protected
/// route publication without installing the bearer/OAuth middleware a second
/// time. Callers must never set this for a listener that does not enforce and
/// inject authentication itself.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_router_with_external_auth(
    mut state: AppState,
    bearer_token: Option<String>,
    auth_state: Option<labby_auth::state::AuthState>,
    mcp_router: Option<Router<AppState>>,
    config_cors_origins: &[String],
    external_auth_configured: bool,
) -> Router {
    if let Some(ref auth_state) = auth_state {
        state = state.with_oauth_state(auth_state.clone());
    }
    #[cfg(feature = "gateway")]
    if state.access_bootstrap_proof.is_none()
        && let Some(manager) = state.gateway_manager.as_ref()
    {
        let policy = Arc::new(
            crate::dispatch::access_bootstrap::GatewayBootstrapPolicyAuthority::new(
                manager.as_ref().clone(),
                state.access_runtime.as_ref().clone(),
            ),
        );
        let service = Arc::new(
            crate::dispatch::access_bootstrap::DaemonAccessBootstrapProofService::new(
                state.access_runtime.as_ref().clone(),
                policy,
            ),
        );
        state = state.with_access_bootstrap_proof(service);
    }
    if let Some(auth_state) = auth_state.as_ref() {
        if let Err(error) = auth_state.replace_configured_resource_scopes(
            state
                .config
                .protected_mcp_routes
                .iter()
                .filter(|route| route.enabled)
                .map(|route| (route.public_resource(), route.scopes.clone())),
        ) {
            tracing::error!(%error, "invalid configured OAuth protected resource route");
        }
    }
    let static_token = bearer_token.map(Arc::<str>::from);
    state = state.with_bearer_token(static_token.clone());
    let auth_state = auth_state.map(Arc::new);
    let credential_auth_configured = static_token.is_some() || auth_state.is_some();
    let protected_route_auth_configured = credential_auth_configured || external_auth_configured;
    if !protected_route_auth_configured {
        tracing::warn!(
            "HTTP API started without bearer, OAuth, or a trusted outer auth boundary — all published routes are unprotected"
        );
    }

    let integrated_trusted_host = state.trusted_host_verifier.is_some();
    let v1 = build_v1_router(
        &state,
        protected_route_auth_configured,
        integrated_trusted_host,
    );

    let x_request_id = HeaderName::from_static("x-request-id");

    // Build separate protected sub-routers so `/v1/*` can accept browser
    // sessions while `/mcp` remains token-authenticated only.
    let v1_group = crate::api::route_registry::RouteGroup::empty().nest("/v1", v1);
    let resource_url: Option<Arc<str>> = auth_state
        .as_ref()
        .map(|state| labby_auth::metadata::canonical_resource_url(state.as_ref()))
        .or_else(|| {
            state.auth_config.as_ref().and_then(|cfg| {
                cfg.public_url.as_ref().map(|url| {
                    let base = url.as_str().trim_end_matches('/');
                    let path = cfg.resource_path.trim_start_matches('/');
                    if path.is_empty() {
                        base.to_string()
                    } else {
                        format!("{base}/{path}")
                    }
                })
            })
        })
        .map(Arc::from);
    let layer_deriver = state.actor_key_deriver.clone().map(lab_auth_deriver);
    // Build the shared AuthLayer once; per-route variants only differ in
    // whether the session-cookie path is enabled (true for browser-facing
    // /v1 + /dev + /v0.1; false for the bearer-only /mcp transport).
    let make_auth_layer = |allow_session_cookie: bool| -> AuthLayer {
        let mut layer = match auth_state.clone() {
            Some(state) => AuthLayer::from_state(state),
            // Bearer-only path (no OAuth state): grant the same legacy scopes
            // that the old middleware always issued for static-token requests.
            None => AuthLayer::new()
                .with_static_token_scopes(vec!["lab:read".to_string(), "lab:admin".to_string()]),
        };
        layer = layer
            .with_static_token(static_token.clone())
            .with_actor_key_deriver(layer_deriver.clone())
            .with_resource_url(resource_url.clone())
            .with_error_response_mapper(|error| {
                ApiError::new(ToolError::Sdk {
                    sdk_kind: error.kind().to_string(),
                    message: error.to_string(),
                })
                .into_response()
            })
            .with_allow_session_cookie(allow_session_cookie);
        layer = layer.with_project_session_state(state.project_session_state.clone());
        if let Some(adapter) = state.access_credential_adapter.clone() {
            layer = layer
                .with_product_credential_verifier(adapter.clone())
                .with_product_access_grant_resolver(adapter.clone())
                .with_project_session_revalidator(adapter);
        }
        layer
    };
    let v1_protected = if credential_auth_configured {
        v1_group.map_router(|router| router.route_layer(make_auth_layer(true)))
    } else {
        v1_group
    };

    let mcp_protected = mcp_router.map(|mcp| {
        if credential_auth_configured {
            mcp.route_layer(make_auth_layer(false))
        } else {
            mcp
        }
    });

    // Build the outer router: health probes + discovery (no auth) + protected routes (auth).
    // Layers apply bottom-up: last .layer() call = outermost middleware.
    // Desired execution order (outermost → innermost → handler):
    //   SetRequestId → TraceLayer → PropagateRequestId → Timeout → Compression → CORS → handler
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    let public_core = crate::api::route_registry::RouteGroup::empty()
        .route(
            RouteDescriptor::new("GET", "/health", "health", "health", RouteAuth::Public),
            get(health::health),
        )
        .route(
            RouteDescriptor::new("GET", "/ready", "ready", "health", RouteAuth::Public),
            get(health::ready),
        )
        .route(
            RouteDescriptor::new(
                "GET",
                "/.well-known/labby.json",
                "labby_discovery",
                "discovery",
                RouteAuth::Public,
            ),
            get(labby_discovery),
        );
    let mut route_group = public_core.merge(v1_protected);
    if !integrated_trusted_host {
        route_group = route_group
            .merge(services::browser::public_routes().map_router(|router| {
                router.layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                ))
            }))
            .merge(services::oauth_relay::public_routes(state.clone()));
    }
    #[cfg(feature = "gateway")]
    if !integrated_trusted_host {
        let browser_oauth = crate::api::upstream_oauth::browser_routes(state.clone());
        let well_known_oauth = crate::api::upstream_oauth::well_known_routes(state.clone());
        route_group = route_group.merge(browser_oauth).merge(well_known_oauth);
    }
    if let Some(mcp) = mcp_protected.filter(|_| !integrated_trusted_host) {
        route_group = route_group.merge_runtime_router(
            mcp,
            [
                RouteDescriptor::new("GET", "/mcp", "mcp", "mcp", RouteAuth::BearerOnly)
                    .when("mounted only when an MCP HTTP router is configured"),
                RouteDescriptor::new("POST", "/mcp", "mcp", "mcp", RouteAuth::BearerOnly)
                    .when("mounted only when an MCP HTTP router is configured"),
            ],
        );
    }
    // /auth/session and /auth/logout are registered unconditionally — unlike
    // the OAuth-specific routes below, their handlers (browser_session.rs)
    // already have complete fallback logic for web_ui_auth_disabled, a valid
    // static bearer token, and no auth configured at all: /auth/session
    // returns 200 with `authenticated: false` rather than an error, and
    // /auth/logout returns 204 either way. The gateway-admin
    // frontend's loadBrowserSession() unconditionally fetches /auth/session
    // on every page load regardless of which auth mode is configured; if
    // this route only existed behind OAuth being set up, a pure-Bearer (or
    // no-auth-configured) deployment would silently fall through to the
    // Next.js SPA catch-all here, which returns HTML with 200 OK — the
    // frontend's `response.json()` then throws and the UI shows a generic
    // "Unable to reach the authentication service" error instead of a
    // working (or cleanly unauthenticated) session. lab-cfl3v.
    //
    // Consequence of this route now being unconditional: in the default
    // bearer-only, no-OAuth, embedded-web-UI deployment shape,
    // resolve_web_ui_auth_disabled() (cli/serve.rs) resolves
    // web_ui_auth_disabled = true by default, so auth_session() returns a
    // synthetic authenticated-admin session with no credential check at
    // all to any caller who can reach this port. It does not grant real
    // /v1/* access (gated separately by the configured auth boundary), but it does render an
    // "authenticated" admin UI shell for unauthenticated visitors. Tracked
    // in lab-0bl3m — not fixed here.
    if !integrated_trusted_host {
        route_group = route_group
            .route(
                RouteDescriptor::new(
                    "GET",
                    "/auth/session",
                    "auth_session",
                    "oauth",
                    RouteAuth::BrowserSession,
                ),
                get(crate::api::browser_session::auth_session),
            )
            .route(
                RouteDescriptor::new(
                    "POST",
                    "/auth/logout",
                    "auth_logout",
                    "oauth",
                    RouteAuth::BrowserSession,
                ),
                post(crate::api::browser_session::auth_logout),
            )
            .route(
                RouteDescriptor::new(
                    "GET",
                    "/auth/reauth/return",
                    "reauth_return",
                    "oauth",
                    RouteAuth::Public,
                ),
                get(crate::api::browser_session::reauth_return),
            );
        if credential_auth_configured {
            let reauth_routes = crate::api::route_registry::RouteGroup::empty()
                .route(
                    RouteDescriptor::new(
                        "POST",
                        "/auth/reauth",
                        "reauth_start",
                        "oauth",
                        RouteAuth::BrowserSession,
                    ),
                    post(crate::api::browser_session::reauth_start),
                )
                .route(
                    RouteDescriptor::new(
                        "GET",
                        "/auth/reauth/{interaction}",
                        "reauth_poll",
                        "oauth",
                        RouteAuth::BrowserSession,
                    ),
                    get(crate::api::browser_session::reauth_poll),
                )
                .route(
                    RouteDescriptor::new(
                        "DELETE",
                        "/auth/reauth/{interaction}",
                        "reauth_cancel",
                        "oauth",
                        RouteAuth::BrowserSession,
                    ),
                    axum::routing::delete(crate::api::browser_session::reauth_cancel),
                )
                .map_router(|router| router.route_layer(make_auth_layer(true)));
            route_group = route_group.merge(reauth_routes);
        }
        let local_session_routes = services::local_session::routes(state.clone())
            .map_router(|router| router.route_layer(make_auth_layer(true)));
        route_group = route_group.merge(local_session_routes);
        route_group = route_group.nest(
            "/auth/bootstrap",
            services::access_bootstrap_proof::routes(state.clone()),
        );
    }
    if !integrated_trusted_host && let Some(auth_state) = auth_state.as_ref() {
        let routes = crate::api::route_registry::oauth_protocol_routes_for_provider(
            auth_state.inbound_provider.kind(),
        );
        let mut auth_routes = crate::api::route_registry::RouteGroup::empty();
        for (route_id, descriptor) in routes {
            use labby_auth::routes::AuthRouteId;
            let methods = match route_id {
                AuthRouteId::AuthorizationServerMetadata
                | AuthRouteId::AuthorizationServerMetadataPath => {
                    get(auth_authorization_server_metadata)
                }
                AuthRouteId::ProtectedResourceMetadata => get(auth_protected_resource_metadata),
                AuthRouteId::Jwks => get(auth_jwks),
                AuthRouteId::Register if auth_state.config.enable_dynamic_registration => {
                    post(auth_register)
                }
                AuthRouteId::Register => continue,
                AuthRouteId::Authorize => get(auth_authorize),
                AuthRouteId::BrowserLogin => get(auth_browser_login),
                AuthRouteId::ProviderCallback => get(auth_callback),
                AuthRouteId::NativeCallback => get(auth_native_callback),
                AuthRouteId::NativePoll => post(auth_native_poll),
                AuthRouteId::Token => post(auth_token),
                AuthRouteId::Revoke => post(auth_revoke),
            };
            auth_routes = auth_routes.route(descriptor, methods);
        }
        auth_routes = auth_routes.map_router(|router| {
            router.layer(axum::middleware::from_fn(
                labby_auth::routes::auth_dispatch_observability,
            ))
        });
        route_group = route_group.merge(auth_routes);
        #[cfg(feature = "gateway")]
        {
            route_group = route_group.route(
                RouteDescriptor::new(
                    "GET",
                    "/.well-known/oauth-protected-resource/{*route}",
                    "protected_route_resource_metadata",
                    "oauth",
                    RouteAuth::OAuthProtocol,
                )
                .feature("gateway")
                .when("mounted only when OAuth is configured"),
                get(protected_route_resource_metadata),
            );
        }
    }

    // Development mockups are registered before the Next.js static fallback so
    // `/dev/mockup*` resolves from ~/.superpowers/brainstorm/content rather than
    // being swallowed by the SPA. See docs/design/component-development.md.
    if !integrated_trusted_host {
        let dev_routes = crate::api::route_registry::RouteGroup::empty()
            .route(
                RouteDescriptor::new(
                    "GET",
                    "/dev/mockup",
                    "dev_mockup",
                    "dev",
                    RouteAuth::BrowserSession,
                )
                .aliases(&["/dev/mockup/"])
                .when("development/mockup routes"),
                get(dev_mockup),
            )
            .route(
                RouteDescriptor::new(
                    "GET",
                    "/dev/mockup/{name}",
                    "dev_mockup_named",
                    "dev",
                    RouteAuth::BrowserSession,
                )
                .aliases(&["/dev/mockup/{name}/"])
                .when("development/mockup routes"),
                get(dev_mockup_named),
            );
        let dev_routes = if credential_auth_configured {
            dev_routes.map_router(|router| router.route_layer(make_auth_layer(true)))
        } else {
            dev_routes
        };
        route_group = route_group.merge(dev_routes);

        route_group = route_group.route(
            RouteDescriptor::new(
                "GET",
                LABBY_APP_HOST_JS_ROUTE,
                "labby_app_host_js",
                "apps",
                RouteAuth::Public,
            ),
            get(labby_app_host_js),
        );

        let app_routes = crate::api::route_registry::RouteGroup::empty()
            .route(
                RouteDescriptor::new(
                    "GET",
                    APPS_LAUNCHER_ROUTE,
                    "apps_launcher_page",
                    "apps",
                    RouteAuth::BrowserSession,
                )
                .aliases(&[&format!("{APPS_LAUNCHER_ROUTE}/")]),
                get(apps_launcher_page),
            )
            .route(
                RouteDescriptor::new(
                    "GET",
                    SERVER_LOGS_BROWSER_ROUTE,
                    "server_logs_app_page",
                    "apps",
                    RouteAuth::BrowserSession,
                )
                .aliases(&[&format!("{SERVER_LOGS_BROWSER_ROUTE}/")]),
                get(server_logs_app_page),
            );
        let app_routes = if credential_auth_configured {
            app_routes.map_router(|router| router.route_layer(make_auth_layer(true)))
        } else {
            app_routes
        };
        route_group = route_group.merge(app_routes);
    }

    let declared_descriptors = if integrated_trusted_host {
        crate::api::route_registry::build_integrated_trusted_host_route_descriptors()
    } else {
        crate::api::route_registry::build_route_descriptors()
    };
    crate::api::route_registry::validate_mounted_inventory(
        &route_group.descriptors,
        &declared_descriptors,
    )
    .expect("runtime HTTP routes diverged from the declared inventory");

    // Static-file fallback for the Next.js SPA. Protected MCP virtual-host
    // proxying is mounted as an inner middleware below so intercepted responses
    // still pass through the shared request-id/trace/timeout/compression/CORS
    // stack.
    let route_observability = crate::api::route_observability::RouteObservability::new(
        &route_group.descriptors,
        declared_descriptors,
    );
    let trusted_host_verifier = state.trusted_host_verifier.clone();
    let mut router = route_group.router;
    if state.web_assets_enabled() && !integrated_trusted_host {
        router = router.fallback(crate::api::web::serve_web_request);
    }

    #[cfg(feature = "gateway")]
    let protected_proxy_state = state.clone();
    let request_timeout_state = state.clone();
    let router = router.with_state(state);
    // A verifier is installed only by the sealed integrated UDS profile. The
    // listener independently verifies SO_PEERCRED; this inner layer proves the
    // original Core actor on every request and replaces the synthetic peer
    // identity before a handler observes it.
    let router = if let Some(verifier) = trusted_host_verifier {
        router
            .layer(axum::middleware::from_fn(
                labby_auth::trusted_host::require_delegated_actor,
            ))
            .layer(Extension(verifier))
    } else {
        router
    };
    #[cfg(feature = "gateway")]
    let router = router.layer(axum::middleware::from_fn_with_state(
        protected_proxy_state,
        protected_mcp_intercept,
    ));
    // Route evidence is outermost of the protected MCP interceptor so auth
    // denials are correlated just like ordinary Axum route outcomes.
    let router = router.layer(axum::middleware::from_fn_with_state(
        route_observability,
        crate::api::route_observability::record_matched_route,
    ));
    router
        .layer(build_cors_layer(config_cors_origins))
        .layer(CompressionLayer::new())
        .layer(axum::middleware::from_fn_with_state(
            request_timeout_state,
            request_timeout,
        ))
        // This must remain the outermost request layer so the original target
        // is checked before matchit's dot-segment normalization can select a
        // valid route such as `/health`.
        .layer(axum::middleware::from_fn(reject_ambiguous_request_target))
        // PropagateRequestId echoes the id back in the response header.
        .layer(PropagateRequestIdLayer::new(x_request_id.clone()))
        // TraceLayer reads x-request-id set by SetRequestId (outermost).
        .layer(
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                let request_id = req
                    .headers()
                    .get("x-request-id")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("-");
                tracing::span!(
                    Level::INFO,
                    "request",
                    method = %req.method(),
                    path = %req.uri().path(),
                    request_id,
                    status = tracing::field::Empty,
                )
            }),
        )
        // SetRequestId generates a UUID for every request that lacks one.
        .layer(SetRequestIdLayer::new(x_request_id, MakeRequestUuid))
}

#[allow(clippy::too_many_lines)]
#[allow(dead_code)]
pub fn build_router_with_bearer(
    state: AppState,
    bearer_token: Option<String>,
    mcp_router: Option<Router<AppState>>,
) -> Router {
    build_router(state, bearer_token, None, mcp_router, &[])
}

/// Build a `CorsLayer` that allows only explicit trusted origins.
///
/// Sources (env var overrides config.toml):
/// - `LABBY_CORS_ORIGINS` env var (comma-separated `scheme://host[:port]`)
/// - `api.cors_origins` in config.toml (array of strings)
///
/// Always includes `http://localhost`, `http://127.0.0.1`, and `http://[::1]`
/// as safe loopback defaults.
fn build_cors_layer(config_origins: &[String]) -> CorsLayer {
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::{HeaderValue, Method};

    // Env var overrides config.toml when present.
    let raw_origins: Vec<String> = match std::env::var("LABBY_CORS_ORIGINS") {
        Ok(val) if !val.trim().is_empty() => val
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        _ => config_origins.to_vec(),
    };

    let env_origins: Vec<HeaderValue> = raw_origins
        .iter()
        .filter_map(|s| match s.parse::<HeaderValue>() {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    origin = s.as_str(),
                    error = %e,
                    "ignoring unparseable CORS origin"
                );
                None
            }
        })
        .collect();

    // Production loopback origins — always allowed.
    // 8765 is the default labby serve port; both `127.0.0.1` and `localhost`
    // are needed because some browsers resolve only one variant (lab-bg3e.3).
    let mut origins: Vec<HeaderValue> = vec![
        HeaderValue::from_static("http://localhost"),
        HeaderValue::from_static("http://localhost:8765"),
        HeaderValue::from_static("http://127.0.0.1"),
        HeaderValue::from_static("http://127.0.0.1:8765"),
        HeaderValue::from_static("http://[::1]"),
        HeaderValue::from_static("http://[::1]:8765"),
    ];
    // Dev ports (3000/5173/8080) are gated behind LABBY_DEV_MODE=1 to prevent
    // a malicious npm postinstall HTTP server (or rogue browser extension on
    // those origins) from reading Setup API responses on a v1 unauthed lab
    // (lab-bg3e.3 security hardening).
    let dev_mode_enabled = crate::config::resolved_dev_mode();
    if dev_mode_enabled {
        // One-shot WARN at startup so an operator who has LABBY_DEV_MODE=1 in
        // their shell rc can see the wider CORS surface in production logs.
        tracing::warn!(
            subsystem = "api_server",
            phase = "cors.dev_mode_enabled",
            "LABBY_DEV_MODE=1 — additional CORS origins enabled (3000/5173/8080); unset for production"
        );
        origins.extend([
            HeaderValue::from_static("http://localhost:3000"),
            HeaderValue::from_static("http://localhost:5173"),
            HeaderValue::from_static("http://localhost:8080"),
            HeaderValue::from_static("http://127.0.0.1:3000"),
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("http://127.0.0.1:8080"),
        ]);
    }
    origins.extend(env_origins);

    // Explicit allowlist instead of Any — prevents arbitrary headers from
    // allowed origins reaching destructive endpoints (lab-3qn.7).
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("x-request-id"),
            HeaderName::from_static(labby_auth::session::BROWSER_CSRF_HEADER_NAME),
        ])
}

async fn service_actions(
    State(state): State<AppState>,
    axum::extract::Path(service): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entry = state
        .catalog
        .services
        .iter()
        .find(|s| s.name == service)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("unknown service `{service}`"),
        })?;
    let actions = serde_json::to_value(&entry.actions).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("serialize actions: {e}"),
    })?;
    Ok(Json(actions))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use axum::Extension;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn supported_platform_mounts_caller_bound_stash_only_with_api_auth() {
        let state = AppState::new();
        let mounted = build_v1_router(&state, true, false);
        assert!(
            mounted
                .descriptors
                .iter()
                .any(|route| route.mount == "stash")
        );
        let unauthenticated = build_v1_router(&state, false, false);
        assert!(
            unauthenticated
                .descriptors
                .iter()
                .all(|route| route.mount != "stash")
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_platform_does_not_mount_stash() {
        let routes = build_v1_router(&AppState::new(), true, false);
        assert!(
            routes
                .descriptors
                .iter()
                .all(|route| route.mount != "stash")
        );
    }

    /// One representative dispatch route for every registry-backed HTTP service.
    ///
    /// The registry is the denominator: adding a service without classifying its
    /// HTTP exposure makes this test fail. `lab_admin` is the sole reviewed
    /// MCP-only service. Every other entry must resolve to a mounted route and be
    /// rejected by the shared authentication layer before its handler runs.
    fn registry_http_auth_probe(service: &str) -> Option<(Method, String)> {
        let path = match service {
            "lab_admin" => return None,
            "fs" => "/v1/fs/list".to_string(),
            "stash" => "/v1/stash/stats".to_string(),
            name @ ("artifacts" | "browser" | "bundles" | "doctor" | "gateway" | "jobs"
            | "server_logs" | "setup" | "snippets" | "sources" | "uploads") => {
                format!("/v1/{name}")
            }
            unknown => panic!(
                "registered service `{unknown}` has no reviewed HTTP auth probe; add its mounted dispatch route or explicitly classify it as MCP-only"
            ),
        };
        let method = if matches!(service, "fs" | "stash") {
            Method::GET
        } else {
            Method::POST
        };
        Some((method, path))
    }

    #[tokio::test]
    async fn integrated_trusted_host_requires_a_fresh_core_assertion_on_health() {
        let verifier = Arc::new(labby_auth::trusted_host::TrustedHostVerifier::new(1, []));
        let app = build_router_with_bearer(
            AppState::new().with_trusted_host_verifier(verifier),
            None,
            None,
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn every_inventoried_customer_or_write_http_route_authenticates_before_dispatch() {
        let registry = crate::registry::build_default_registry();
        let state = AppState::from_registry(registry.clone());

        for service in registry.services() {
            let Some((method, path)) = registry_http_auth_probe(service.name) else {
                assert_eq!(service.name, "lab_admin", "only lab_admin is MCP-only");
                continue;
            };
            let response = build_router_with_bearer(
                state.clone(),
                Some("registry-denominator-secret".into()),
                None,
            )
            .layer(axum::extract::connect_info::MockConnectInfo(
                SocketAddr::from(([127, 0, 0, 1], 9001)),
            ))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "OAI-CLAUSE-001: registry-backed HTTP service `{}` at `{path}` reached routing without authentication",
                service.name
            );
        }

        let service_names = registry
            .services()
            .iter()
            .map(|service| service.name.to_string())
            .collect::<Vec<_>>();
        let route_inventory = crate::docs::routes::build_route_docs(&service_names);
        const SERVICE_PLACEHOLDER: &str = "{service}";
        const PATH_PLACEHOLDER: &str = "{path}";
        for route in route_inventory
            .iter()
            .filter(|route| route.auth_required && route.handler_group != "mcp")
        {
            let path = route
                .path
                .replace(SERVICE_PLACEHOLDER, "gateway")
                .replace("{email}", "user%40example.com")
                .replace("{machine_id}", "machine-1")
                .replace("{suffix}", "callback")
                .replace("{name}", "default")
                .replace(PATH_PLACEHOLDER, "mcp");
            let method = Method::from_bytes(route.method.as_bytes()).unwrap();
            let response = build_router_with_bearer(
                state.clone(),
                Some("route-denominator-secret".into()),
                None,
            )
            .layer(axum::extract::connect_info::MockConnectInfo(
                SocketAddr::from(([127, 0, 0, 1], 9001)),
            ))
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header(header::HOST, "lab.example.com")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
            let status = response.status();
            // These three browser-session endpoints intentionally self-manage
            // authentication instead of using the bearer middleware: login is
            // absent without OAuth, session returns only an anonymous marker,
            // and unauthenticated logout is an idempotent no-op.
            match route.path.as_str() {
                "/auth/login" => {
                    assert_eq!(
                        status,
                        StatusCode::NOT_FOUND,
                        "login is unavailable without OAuth"
                    );
                    continue;
                }
                "/auth/session" => {
                    assert_eq!(status, StatusCode::OK);
                    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                        .await
                        .unwrap();
                    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(body["authenticated"], false);
                    assert!(body["user"].is_null());
                    continue;
                }
                "/auth/logout" => {
                    assert_eq!(status, StatusCode::NO_CONTENT);
                    continue;
                }
                _ => {}
            }
            let unavailable_without_runtime = route.runtime_condition.is_some();
            assert!(
                status == StatusCode::UNAUTHORIZED
                    || (status == StatusCode::FORBIDDEN && route.bootstrap_proof)
                    || (status == StatusCode::NOT_FOUND && unavailable_without_runtime),
                "OAI-CLAUSE-001: inventoried sensitive route {} {} did not authenticate before dispatch (status={}, group={}, runtime_condition={:?})",
                route.method,
                route.path,
                status,
                route.handler_group,
                route.runtime_condition
            );
        }
    }

    #[test]
    fn forwarded_host_requires_explicit_trusted_proxy_configuration() {
        let request = Request::builder()
            .uri("/mcp")
            .header(header::HOST, "localhost:8765")
            .header("x-forwarded-host", "protected.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            request_host(&request, false).as_deref(),
            Some("localhost:8765")
        );
        assert_eq!(
            request_host(&request, true).as_deref(),
            Some("protected.example")
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn bearer_challenge_escapes_quoted_parameters() {
        assert_eq!(quoted_challenge_value("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(quoted_challenge_value("a,b\n☃"), "a,b%0A%E2%98%83");
        let response = auth_error_response_with_challenge(
            "missing",
            "https://bad.example/meta\n☃",
            &["lab:read,bad\r\nnext".to_string()],
        );
        let challenge = response.headers()[header::WWW_AUTHENTICATE]
            .to_str()
            .expect("serialized challenge is ASCII");
        assert!(challenge.contains("%0A%E2%98%83"));
        assert!(challenge.contains("lab:read,bad%0D%0Anext"));
    }

    #[test]
    fn every_route_backstop_is_derived_from_configured_deadlines() {
        // The backstop must never fire before the deadline it wraps, on ANY
        // route. A fixed cap for non-`/mcp` paths previously returned a bare
        // 504 for calls that had already succeeded and blinded the upstream
        // circuit breaker.
        // Assert against the upstream deadlines themselves, not against
        // `config.http_request_timeout()` — comparing the backstop to the very
        // call it delegates to restates the function body and cannot fail.
        for config in [
            crate::config::LabConfig::default(),
            crate::config::LabConfig {
                upstream_request_timeout_ms: Some(1_000),
                upstream_relay_timeout_ms: Some(1_000),
                ..crate::config::LabConfig::default()
            },
            crate::config::LabConfig {
                upstream_relay_timeout_ms: Some(900_000),
                ..crate::config::LabConfig::default()
            },
        ] {
            let backstop = http_request_timeout(&config);
            let relay = config.upstream_relay_timeout();
            let request = config.upstream_request_timeout();
            assert!(
                backstop > relay && backstop > request,
                "backstop {backstop:?} must exceed the relay ({relay:?}) and request \
                 ({request:?}) deadlines it wraps, so a call that already succeeded \
                 upstream is never reported as a bare 504"
            );
        }
    }

    #[test]
    fn the_backstop_scales_with_a_widened_relay_deadline() {
        // Pins the derivation itself: a fixed cap would leave the backstop
        // unchanged when an operator widens the relay budget, which is exactly
        // the regression this middleware replaced.
        let narrow = crate::config::LabConfig {
            upstream_relay_timeout_ms: Some(60_000),
            ..crate::config::LabConfig::default()
        };
        let wide = crate::config::LabConfig {
            upstream_relay_timeout_ms: Some(600_000),
            ..crate::config::LabConfig::default()
        };
        assert!(
            http_request_timeout(&wide) > http_request_timeout(&narrow),
            "the transport backstop must track a widened relay deadline"
        );
    }

    async fn actor_key_probe(
        auth: Option<Extension<crate::api::oauth::AuthContext>>,
    ) -> Json<serde_json::Value> {
        let actor_key = auth
            .and_then(|Extension(ctx)| ctx.actor_key)
            .map(|key| key.to_string());
        Json(serde_json::json!({ "actor_key": actor_key }))
    }

    #[tokio::test]
    async fn actions_known_service_returns_200() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_array(), "body should be a JSON array of actions");
    }

    #[tokio::test]
    async fn security_oracle_kills_a_composition_with_outer_auth_omitted() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "kill fixture must be open"
        );
        let descriptor = crate::api::route_registry::RouteDescriptor::new(
            "GET",
            "/v1/{service}/actions",
            "service_actions",
            "services",
            crate::api::route_registry::RouteAuth::V1,
        );
        assert!(
            crate::api::route_registry::verify_auth_invariant(&descriptor, response.status())
                .is_err(),
            "independent security oracle did not kill omitted auth middleware"
        );
    }

    #[tokio::test]
    async fn actions_unknown_service_returns_404() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/doesnotexist/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "not_found");
    }

    #[tokio::test]
    async fn v1_unmounted_service_route_returns_structured_json_not_bare_404() {
        // AppState::new() has no bearer/OAuth configured, so /v1/gateway is
        // never nested (see the `gateway.mount.skipped` guard in
        // build_v1_router) and previously fell through the SPA fallback to a
        // bare, empty-body 404 — surfaced by the web UI as a generic "An error
        // occurred" toast (bead lab-gug4m). It must now match the same structured
        // agent-error-contract envelope as every other /v1 failure.
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/gateway")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(content_type.contains("application/json"));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!body.is_empty(), "fallback must not return an empty body");
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "route_not_found");
        assert!(json["message"].as_str().unwrap().contains("/v1/gateway"));
    }

    #[tokio::test]
    async fn v1_unmatched_route_response_discloses_nothing_beyond_the_request() {
        // This fallback is reachable unauthenticated (`nest` hoists it into
        // the outer `fallback_router`, which `route_layer` leaves unlayered),
        // so its body must not name mount state or point at internal logs —
        // it may only echo the method and path the caller already sent.
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/no-such-service")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "route_not_found");

        let message = json["message"].as_str().unwrap();
        assert!(message.contains("/v1/no-such-service"));
        for leak in ["mount", "skipped", "deployment", "log"] {
            assert!(
                !message.to_ascii_lowercase().contains(leak),
                "anonymous 404 body must not mention `{leak}`; got: {message}"
            );
        }
    }

    #[tokio::test]
    async fn v1_fallback_does_not_shadow_real_routes_or_swallow_405() {
        // The fallback must lose to a more specific match, and must not
        // absorb method-not-allowed for a path that does exist.
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);

        let real_route = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            real_route.status(),
            StatusCode::OK,
            "the fallback must not shadow a registered route"
        );

        let wrong_method = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            wrong_method.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "an existing path with a wrong method must stay 405, not become 404"
        );
    }

    #[tokio::test]
    async fn auth_layer_rejects_missing_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        // /v1/setup/actions is behind bearer auth; /health is NOT (lab-3qn.5).
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");
    }

    async fn assert_canonical_auth_failure(response: axum::response::Response) {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("Bearer resource_metadata=\"")),
            "missing RFC 9728 bearer challenge"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");
        assert_eq!(json["contract_version"], 1);
        assert_eq!(json["origin"], "policy");
        assert_eq!(json["recovery"]["action"], "reauthenticate");
        assert_eq!(json["recovery"]["same_arguments"], "never");
        assert_eq!(json["side_effects"], "none_expected");
    }

    #[tokio::test]
    async fn shared_auth_layer_preserves_canonical_contract_for_v1_and_mcp() {
        let auth_state = test_lab_auth_state().await;
        let jwt = issue_test_lab_token(&auth_state);
        let mcp_router = Router::new().route("/mcp", get(|| async { StatusCode::OK }));
        let app = build_router(
            AppState::new(),
            Some("secret-token".to_string()),
            Some(auth_state),
            Some(mcp_router),
            &[],
        );

        let mut challenges = Vec::new();
        for (path, authorization) in [
            ("/v1/setup/actions", None),
            ("/v1/setup/actions", Some("Bearer invalid")),
            ("/mcp", None),
            ("/mcp", Some("Bearer invalid")),
        ] {
            let mut request = Request::builder().method("GET").uri(path);
            if let Some(authorization) = authorization {
                request = request.header(header::AUTHORIZATION, authorization);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            challenges.push(
                response.headers()[header::WWW_AUTHENTICATE]
                    .to_str()
                    .unwrap()
                    .to_string(),
            );
            assert_canonical_auth_failure(response).await;
        }
        assert_eq!(challenges[0], challenges[2]);
        assert_eq!(challenges[1], challenges[3]);

        for (path, token) in [
            ("/v1/setup/actions", "secret-token"),
            ("/mcp", "secret-token"),
            ("/v1/setup/actions", jwt.as_str()),
            ("/mcp", jwt.as_str()),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{path} rejected valid auth"
            );
        }

        let session = seed_browser_session(&Arc::new(test_lab_auth_state().await)).await;
        for path in ["/v1/setup/actions", "/mcp"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(
                            header::COOKIE,
                            format!(
                                "{}={}",
                                labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                                session.session_id
                            ),
                        )
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_canonical_auth_failure(response).await;
        }
    }

    #[tokio::test]
    async fn auth_layer_accepts_valid_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        // Confirm that a valid token reaches the protected /v1 route.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn server_logs_app_route_requires_auth_when_configured() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/apps/server-logs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn server_logs_app_route_serves_browser_html_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/apps/server-logs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("text/html"));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Server logs"));
        assert!(text.contains("/v1/server-logs/query"));
        assert!(text.contains("html.browser"));
        assert!(text.contains("LabbyAppHost"));
        assert!(text.contains("savedViews"));
        assert!(text.contains("drillLinks"));
    }

    #[tokio::test]
    async fn server_logs_query_requires_admin_auth_context_even_without_global_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/server-logs/query")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "forbidden");
    }

    #[tokio::test]
    async fn server_logs_canonical_action_route_dispatches_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/server_logs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["service"], "server_logs");
        assert!(json["actions"].as_array().is_some_and(|actions| {
            actions
                .iter()
                .any(|action| action["name"] == "server_logs.query")
        }));
    }

    #[tokio::test]
    async fn server_logs_help_does_not_require_admin_scope() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/server_logs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["service"], "server_logs");
    }

    #[tokio::test]
    async fn apps_launcher_and_bridge_asset_are_served() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let launcher = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/apps")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(launcher.status(), StatusCode::OK);
        let body = axum::body::to_bytes(launcher.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Labby Apps"));
        assert!(text.contains("/v1/apps/manifest"));
        assert!(text.contains("/apps/assets/labby-app-host.js"));

        let bridge = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/apps/assets/labby-app-host.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bridge.status(), StatusCode::OK);
        let content_type = bridge
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("text/javascript"));
        let body = axum::body::to_bytes(bridge.into_body(), usize::MAX)
            .await
            .unwrap();
        let js = String::from_utf8(body.to_vec()).unwrap();
        assert!(js.contains("LabbyAppHost"));
        assert!(js.contains("callAction"));
        assert!(text.contains("appPath"));
    }

    #[tokio::test]
    async fn apps_manifest_endpoint_derives_action_spec_metadata() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/apps/manifest")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let app = manifest["apps"]
            .as_array()
            .and_then(|apps| apps.iter().find(|app| app["slug"] == "server-logs"))
            .expect("server logs app manifest entry");
        assert_eq!(app["kind"], "browse");
        assert_eq!(app["browser_path"], "/apps/server-logs");
        assert_eq!(app["required_scopes"], serde_json::json!(["lab:admin"]));
        assert_eq!(app["primary_action"]["service"], "server_logs");
        assert_eq!(app["primary_action"]["action"], "server_logs.query");
    }

    #[tokio::test]
    async fn auth_layer_accepts_case_insensitive_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .header(header::AUTHORIZATION, "bearer   secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_ui_auth_disabled_does_not_bypass_v1_auth() {
        let state = AppState::new().with_web_ui_auth_disabled(true);
        let mcp_router: Router<AppState> =
            Router::new().route("/mcp", get(|| async { StatusCode::OK }));
        let app = build_router_with_bearer(state, Some("secret-token".into()), Some(mcp_router));

        let v1_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v1_response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(v1_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");

        let mcp_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mcp_response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(mcp_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");
    }

    #[tokio::test]
    async fn health_endpoint_open_without_auth() {
        // /health must be reachable by monitoring probes without any token (lab-3qn.5).
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_endpoint_open_without_auth() {
        // /ready must be reachable by monitoring probes without any token (lab-3qn.5).
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/ready")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn public_health_ready_and_discovery_stay_outside_bearer_protection() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        for path in ["/health", "/ready", "/.well-known/labby.json"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path} became protected");
        }
        let protected = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(protected.status(), StatusCode::UNAUTHORIZED);
    }

    #[cfg(feature = "api-docs")]
    #[tokio::test]
    async fn openapi_json_requires_bearer_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[cfg(feature = "api-docs")]
    #[tokio::test]
    async fn openapi_json_returns_spec_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get(header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "application/json");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(spec["openapi"], "3.1.0");
        assert!(spec["info"]["title"].as_str().is_some());
        assert!(spec["paths"].as_object().is_some());
    }

    #[cfg(feature = "api-docs")]
    #[tokio::test]
    async fn docs_endpoint_returns_html_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/docs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("scalar"), "HTML should reference Scalar");
        assert!(
            html.contains("openapi.json"),
            "HTML should reference spec URL"
        );
    }

    #[tokio::test]
    async fn bearer_mode_still_accepts_lab_mcp_http_token() {
        let state = AppState::new();
        let app = build_router(state, Some("secret-token".into()), None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oauth_mode_accepts_lab_auth_jwt() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_lab_token(&auth_state);
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oauth_relay_admin_routes_are_enforced_by_full_router() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::oauth::public_relay::PublicRelayRegistryStore::new(
            dir.path().join("relay.json"),
        );
        let manager = Arc::new(
            crate::oauth::public_relay::PublicRelayRegistryManager::load(store)
                .await
                .unwrap(),
        );

        let bearer_app = build_router_with_bearer(
            AppState::new().with_public_relay_manager(Arc::clone(&manager)),
            Some("secret-token".into()),
            None,
        );
        let unauthenticated = bearer_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/oauth/relay/machines")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let static_bearer = bearer_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/oauth/relay/machines")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(static_bearer.status(), StatusCode::OK);

        let auth_state = test_lab_auth_state().await;
        let read_only_token =
            issue_test_token(&auth_state, "https://lab.example.com/mcp", "lab:read");
        let oauth_app = build_router(
            AppState::new().with_public_relay_manager(manager),
            None,
            Some(auth_state),
            None,
            &[],
        );
        let read_only = oauth_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/oauth/relay/machines")
                    .header(header::AUTHORIZATION, format!("Bearer {read_only_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read_only.status(), StatusCode::FORBIDDEN);

        let browser_auth = test_lab_auth_state().await;
        let session = seed_browser_session(&browser_auth).await;
        let browser_app = build_router(AppState::new(), None, Some(browser_auth), None, &[]);
        let missing_csrf = browser_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/oauth/relay/import")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::from(
                        r#"{"devhost":"http://100.99.0.1:38935/callback/devhost"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_csrf.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn doctor_admin_actions_are_enforced_by_api_dispatch_gate() {
        let auth_state = test_lab_auth_state().await;
        let read_only_token =
            issue_test_token(&auth_state, "https://lab.example.com/mcp", "lab:read");
        let app = build_router(AppState::new(), None, Some(auth_state), None, &[]).layer(
            axum::extract::connect_info::MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9001))),
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/doctor")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {read_only_token}"))
                    .body(Body::from(
                        r#"{"action":"oauth.relay.check","params":{"probe_targets":true}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn static_bearer_bind_attaches_actor_key_when_deriver_is_configured() {
        let deriver =
            crate::observability::activity::ActorKeyDeriver::from_secret("test-secret").unwrap();
        let expected = deriver.derive_subject("static-bearer").unwrap();
        let deriver = Arc::new(deriver);
        let layer = AuthLayer::new()
            .with_static_token(Some(Arc::<str>::from("secret-token")))
            .with_actor_key_deriver(Some(lab_auth_deriver(Arc::clone(&deriver))));
        let app = Router::new()
            .route("/probe", get(actor_key_probe))
            .route_layer(layer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["actor_key"], expected.as_str());
    }

    #[tokio::test]
    async fn browser_session_bind_attaches_actor_key_when_deriver_is_configured() {
        let auth_state = Arc::new(test_lab_auth_state().await);
        let session = seed_browser_session(&auth_state).await;
        let deriver =
            crate::observability::activity::ActorKeyDeriver::from_secret("test-secret").unwrap();
        let expected = deriver.derive_subject(&session.subject).unwrap();
        let deriver = Arc::new(deriver);
        let layer = AuthLayer::from_state(Arc::clone(&auth_state))
            .with_actor_key_deriver(Some(lab_auth_deriver(Arc::clone(&deriver))))
            .with_allow_session_cookie(true);
        let app = Router::new()
            .route("/probe", get(actor_key_probe))
            .route_layer(layer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/probe")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["actor_key"], expected.as_str());
    }

    #[tokio::test]
    async fn authenticated_bind_leaves_actor_key_null_without_deriver() {
        let layer = AuthLayer::new().with_static_token(Some(Arc::<str>::from("secret-token")));
        let app = Router::new()
            .route("/probe", get(actor_key_probe))
            .route_layer(layer);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["actor_key"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn auth_session_returns_internal_error_when_lookup_fails() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        auth_state
            .store
            .execute_test_statement("DROP TABLE browser_sessions;")
            .await
            .unwrap();
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn v1_accepts_browser_session_cookie() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mcp_rejects_browser_session_cookie_without_bearer() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let mcp_router = Router::new().route("/mcp", get(|| async { StatusCode::OK }));
        let app = build_router(state, None, Some(auth_state), Some(mcp_router), &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn mcp_rejects_static_bearer_when_oauth_policy_disables_it() {
        let state = AppState::new();
        let mut auth_state = test_lab_auth_state().await;
        Arc::make_mut(&mut auth_state.config).disable_static_token_with_oauth = true;
        let mcp_router = Router::new().route("/mcp", get(|| async { StatusCode::OK }));
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            Some(mcp_router),
            &[],
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header(header::AUTHORIZATION, "Bearer static-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn v1_session_post_requires_csrf_header() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/gateway")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::from(r#"{"action":"gateway.list","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn access_owner_bootstrap_requires_browser_csrf() {
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let config = labby_auth::config::AuthConfig {
            admin_email: "browser@example.com".into(),
            ..Default::default()
        };
        let app = build_router(
            AppState::new().with_auth_config(config),
            None,
            Some(auth_state),
            None,
            &[],
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/access/bootstrap-owner")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::from(
                        r#"{"organization_name":"Local","project_name":"Default"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn access_owner_bootstrap_maps_json_rejection_to_canonical_no_store_error() {
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let config = labby_auth::config::AuthConfig {
            admin_email: "browser@example.com".into(),
            ..Default::default()
        };
        let app = build_router(
            AppState::new().with_auth_config(config),
            None,
            Some(auth_state),
            None,
            &[],
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/access/bootstrap-owner")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(labby_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
                    .body(Body::from(
                        r#"{"organization_name":"Local","project_name":"Default","subject":"forged"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["contract_version"], 1);
        assert_eq!(json["kind"], "validation_failed");
    }

    #[tokio::test]
    async fn access_owner_bootstrap_creates_then_returns_idempotent_success() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let config = labby_auth::config::AuthConfig {
            admin_email: "browser@example.com".into(),
            ..Default::default()
        };
        let access_runtime = Arc::new(
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
        );
        let app = build_router(
            AppState::new()
                .with_auth_config(config)
                .with_access_runtime(access_runtime),
            None,
            Some(auth_state),
            None,
            &[],
        );
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/v1/access/bootstrap-owner")
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::COOKIE,
                    format!(
                        "{}={}",
                        labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                        session.session_id
                    ),
                )
                .header(labby_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
                .body(Body::from(
                    r#"{"organization_name":"Local","project_name":"Default"}"#,
                ))
                .unwrap()
        };

        for (expected_status, expected_outcome) in [
            (StatusCode::CREATED, "created"),
            (StatusCode::OK, "already_applied"),
        ] {
            let response = app.clone().oneshot(request()).await.unwrap();
            assert_eq!(response.status(), expected_status);
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, no-store"
            );
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["status"], expected_outcome);
        }
    }

    #[tokio::test]
    async fn access_owner_bootstrap_rejects_bearer_and_is_absent_without_oauth() {
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_token(&auth_state, "https://lab.example.com/mcp", "lab:admin");
        let config = labby_auth::config::AuthConfig {
            admin_email: "browser@example.com".into(),
            ..Default::default()
        };
        let authenticated = build_router(
            AppState::new().with_auth_config(config),
            None,
            Some(auth_state),
            None,
            &[],
        );
        let body = r#"{"organization_name":"Local","project_name":"Default"}"#;
        let bearer = authenticated
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/access/bootstrap-owner")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bearer.status(), StatusCode::FORBIDDEN);

        let loopback = build_router(AppState::new(), None, None, None, &[])
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/access/bootstrap-owner")
                    .header(header::HOST, "127.0.0.1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(loopback.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn access_owner_bootstrap_is_absent_without_oauth_before_body_validation() {
        let response = build_router(AppState::new(), None, None, None, &[])
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/access/bootstrap-owner")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"organization_name":"Local","project_name":"Default","email":"owner@example.com","subject":"forged"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn auth_session_returns_browser_identity_and_csrf_token() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["user"]["sub"], "browser-user");
        assert_eq!(json["csrf_token"], "csrf-123");
        assert_eq!(json["project_id"], serde_json::Value::Null);
    }

    // lab-cfl3v: /auth/session and /auth/logout must work without OAuth
    // configured — a pure-Bearer (or no-auth-at-all) deployment previously
    // had no backend route registered here at all, so requests silently fell
    // through to the SPA catch-all (HTML, 200 OK) instead of these handlers'
    // own already-correct fallback logic.
    #[tokio::test]
    async fn auth_session_returns_unauthenticated_without_any_auth_configured() {
        let state = AppState::new();
        let app = build_router(state, None, None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], false);
        assert_eq!(json["login_available"], false);
    }

    #[tokio::test]
    async fn auth_session_returns_static_bearer_identity_without_oauth() {
        let state = AppState::new();
        let app = build_router(state, Some("secret-token".to_string()), None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["user"]["sub"], "static-bearer");
        assert_eq!(json["is_admin"], true);
    }

    #[tokio::test]
    async fn auth_logout_returns_no_content_without_any_auth_configured() {
        let state = AppState::new();
        let app = build_router(state, None, None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn auth_session_rejects_wrong_bearer_token_without_oauth() {
        let state = AppState::new();
        let app = build_router(state, Some("secret-token".to_string()), None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(header::AUTHORIZATION, "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], false);
        assert_eq!(json["login_available"], false);
    }

    // lab-0bl3m: resolve_web_ui_auth_disabled() defaults web_ui_auth_disabled
    // to true for the bearer-only + embedded-web-UI shape, and /auth/session
    // is now registered unconditionally (bead lab-cfl3v) — together those mean
    // this dev-bypass branch is reachable by an unauthenticated caller in
    // that default deployment shape, not just in explicit local-dev setups.
    // This test pins the exact observable behavior so a future change to
    // either default is a deliberate, visible diff here.
    #[tokio::test]
    async fn auth_session_returns_dev_identity_when_web_ui_auth_disabled() {
        let state = AppState::new().with_web_ui_auth_disabled(true);
        let app = build_router(state, None, None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["is_admin"], true);
        assert_eq!(json["user"]["sub"], "labby-dev");
    }

    // lab-cfl3v: reproduces the literal symptom the bug report described —
    // with embedded web assets serving the SPA catch-all, /auth/session must
    // still resolve to the JSON handler, not fall through to the HTML shell.
    #[tokio::test]
    async fn auth_session_wins_over_embedded_web_asset_fallback() {
        if !crate::api::web::embedded_web_assets_available() {
            eprintln!(
                "skipping: apps/gateway-admin/out/index.html missing — \
                 run `pnpm --filter gateway-admin build` to populate"
            );
            return;
        }
        let state = AppState::new().with_embedded_web_assets();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("application/json"));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], false);
    }

    #[tokio::test]
    async fn auth_session_uses_configured_browser_cookie_name() {
        let mut auth_state = test_lab_auth_state().await;
        Arc::make_mut(&mut auth_state.config).session_cookie_name = "custom_session".to_string();
        let session = seed_browser_session(&auth_state).await;
        let app = build_router(AppState::new(), None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/session")
                    .header(
                        header::COOKIE,
                        format!("custom_session={}", session.session_id),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["user"]["sub"], "browser-user");
    }

    #[tokio::test]
    async fn auth_layer_accepts_valid_oauth_bearer_token() {
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_lab_token(&auth_state);
        let app = build_router(AppState::new(), None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_logout_revokes_session_and_clears_cookie() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        let store = auth_state.store.clone();
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(labby_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("Max-Age=0"));
        assert!(
            store
                .find_browser_session("sess-123")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn auth_logout_returns_internal_error_when_revocation_fails() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let session = seed_browser_session(&auth_state).await;
        auth_state
            .store
            .execute_test_statement("DROP TABLE browser_sessions;")
            .await
            .unwrap();
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/logout")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(labby_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn oauth_mode_missing_token_returns_www_authenticate_metadata_hint() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let header = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(header.contains("resource_metadata="));
        assert!(header.contains("scope=\"lab:read lab lab:admin\""));
    }

    #[tokio::test]
    async fn authorization_server_metadata_suffix_returns_json_not_spa() {
        let state = AppState::new();
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/.well-known/oauth-authorization-server/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["issuer"], "https://lab.example.com");
        assert_eq!(json["token_endpoint"], "https://lab.example.com/token");
    }

    #[tokio::test]
    async fn disabled_dynamic_registration_is_neither_advertised_nor_mounted() {
        let auth_state = test_lab_auth_state().await;
        assert!(!auth_state.config.enable_dynamic_registration);
        let app = build_router(AppState::new(), None, Some(auth_state), None, &[]);
        let metadata = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(metadata.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(json.get("registration_endpoint").is_none());

        let register = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9001))))
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = register.status();
        let body = axum::body::to_bytes(register.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "unexpected registration response: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[test]
    fn product_token_route_emits_one_canonical_auth_dispatch_error() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let buf = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info,labby_auth=info"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);
        crate::test_support::rebuild_tracing_interest_cache();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let auth_state = test_lab_auth_state().await;
            auth_state
                .store
                .register_client(labby_auth::types::RegisteredClient {
                    client_id: "stale-client".to_string(),
                    redirect_uris: vec!["http://127.0.0.1/callback".to_string()],
                    created_at: 1,
                    token_endpoint_auth_method: "none".to_string(),
                    token_endpoint_auth_methods: Vec::new(),
                    jwks: None,
                    jwks_uri: None,
                })
                .await
                .unwrap();
            let app = build_router(AppState::new(), None, Some(auth_state), None, &[]).layer(
                axum::extract::connect_info::MockConnectInfo(SocketAddr::from((
                    [127, 0, 0, 1],
                    9001,
                ))),
            );
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/token")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .header("x-request-id", "req-stale-refresh")
                        .body(Body::from(
                            "grant_type=refresh_token&client_id=stale-client&refresh_token=dead",
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        });

        drop(_guard);
        let logs = crate::test_support::captured_logs(&buf);
        let dispatch_events = logs
            .lines()
            .filter(|line| {
                line.contains("\"service\":\"auth\"") && line.contains("\"action\":\"oauth.token\"")
            })
            .count();
        assert_eq!(
            dispatch_events, 1,
            "expected one auth dispatch event:\n{logs}"
        );
        for expected in [
            "\"request_id\":\"req-stale-refresh\"",
            "\"kind\":\"invalid_grant\"",
            "\"status\":400",
            "dispatch.error",
        ] {
            assert!(logs.contains(expected), "missing `{expected}` in:\n{logs}");
        }
        for duplicate in [
            "oauth token request received",
            "oauth refresh_token grant received",
            "oauth token rejected: unknown or expired refresh token",
        ] {
            assert!(
                !logs.contains(duplicate),
                "duplicate INFO/WARN event `{duplicate}` remained:\n{logs}"
            );
        }
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_mcp_route_metadata_uses_host_and_path_resource() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_route_config(
            "telemetry",
            "mcp.example.com",
            "/telemetry",
            "http://10.0.0.2:3100",
        );
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let root_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/.well-known/oauth-protected-resource/telemetry")
                    .header(header::HOST, "mcp.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let compatibility_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/telemetry/.well-known/oauth-protected-resource")
                    .header(header::HOST, "mcp.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(root_response.status(), StatusCode::OK);
        assert_eq!(compatibility_response.status(), StatusCode::OK);
        for response in [&root_response, &compatibility_response] {
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/json"
            );
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "public, max-age=3600"
            );
        }
        let root_body = axum::body::to_bytes(root_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let compatibility_body =
            axum::body::to_bytes(compatibility_response.into_body(), usize::MAX)
                .await
                .unwrap();
        assert_eq!(root_body, compatibility_body);
        let json: serde_json::Value = serde_json::from_slice(&root_body).unwrap();
        assert_eq!(json["resource"], "https://mcp.example.com/telemetry");
        assert_eq!(
            json["authorization_servers"],
            serde_json::json!(["https://lab.example.com"])
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_mcp_route_metadata_compatibility_alias_matches_resource() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_route_config(
            "telemetry",
            "mcp.example.com",
            "/telemetry",
            "http://10.0.0.2:3100",
        );
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/telemetry/.well-known/oauth-protected-resource")
                    .header(header::HOST, "mcp.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["resource"], "https://mcp.example.com/telemetry");
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_mcp_route_unauthorized_header_points_to_route_metadata() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_route_config(
            "telemetry",
            "mcp.example.com",
            "/telemetry",
            "http://10.0.0.2:3100",
        );
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/telemetry")
                    .header(header::HOST, "mcp.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource/telemetry\", scope=\"mcp:read mcp:write\""
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_mcp_route_insufficient_scope_returns_rfc_9728_challenge() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_route_config(
            "telemetry",
            "mcp.example.com",
            "/telemetry",
            "http://10.0.0.2:3100",
        );
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_token(&auth_state, "https://mcp.example.com/telemetry", "mcp:read");
        let app = build_router(state, None, Some(auth_state), None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/telemetry")
                    .header(header::HOST, "mcp.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer error=\"insufficient_scope\", scope=\"mcp:read mcp:write\", resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource/telemetry\""
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn public_callback_route_bypasses_matching_protected_route_intercept() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_route_config(
            "callback",
            "callback.tootie.tv",
            "/callback",
            "http://10.0.0.2:3100",
        );
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/callback/devhost?code=abc&state=secret-state")
                    .header(header::HOST, "callback.tootie.tv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(response.headers().get(header::WWW_AUTHENTICATE).is_none());
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_mcp_route_proxies_with_route_audience_token() {
        let backend = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .and(wiremock::matchers::header("mcp-method", "server/discover"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"jsonrpc":"2.0","result":{}}"#),
            )
            .mount(&backend)
            .await;

        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config =
            protected_route_config("telemetry", "mcp.example.com", "/telemetry", &backend.uri());
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://mcp.example.com/telemetry");
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            None,
            &[],
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/telemetry")
                    .header(header::HOST, "mcp.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            r#"{"jsonrpc":"2.0","result":{}}"#
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_mcp_route_can_publish_named_upstream() {
        let backend = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp/extra"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"jsonrpc":"2.0","result":{"upstream":true}}"#),
            )
            .mount(&backend)
            .await;

        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = crate::config::LabConfig {
            upstream: vec![crate::config::UpstreamConfig {
                name: "axon".to_string(),
                enabled: true,
                url: Some(format!("{}/mcp", backend.uri())),
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: "axon".to_string(),
                enabled: true,
                public_host: "mcp.example.com".to_string(),
                public_path: "/axon".to_string(),
                upstream: Some("axon".to_string()),
                backend_url: String::new(),
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
                health_path: None,
                target: None,
            }],
            ..crate::config::LabConfig::default()
        };
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://mcp.example.com/axon");
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            None,
            &[],
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/axon/extra")
                    .header(header::HOST, "mcp.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            r#"{"jsonrpc":"2.0","result":{"upstream":true}}"#
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn named_protected_route_denies_hidden_direct_capabilities() {
        for (method_name, params) in [
            (
                "tools/call",
                serde_json::json!({"name": "danger.delete", "arguments": {}}),
            ),
            (
                "resources/read",
                serde_json::json!({"uri": "secret://credentials"}),
            ),
            (
                "prompts/get",
                serde_json::json!({"name": "admin-reset", "arguments": {}}),
            ),
        ] {
            let backend = MockServer::start().await;
            let tempdir = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                crate::dispatch::gateway::config_store::test_gateway_manager(
                    tempdir.path().join("gateway.toml"),
                    crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
                ),
            );
            let config = protected_named_upstream_config(&backend.uri());
            manager
                .seed_config_unchecked_for_tests(config.to_gateway_config())
                .await;
            let state = AppState::new()
                .with_config(config)
                .with_gateway_manager(manager);
            let auth_state = test_lab_auth_state().await;
            let token = issue_test_route_token(&auth_state, "https://mcp.example.com/safe");
            let app = build_router(
                state,
                Some("static-token".to_string()),
                Some(auth_state),
                None,
                &[],
            );
            let response = app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/safe")
                    .header(header::HOST, "mcp.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::json!({"jsonrpc":"2.0", "method": method_name, "id": 1, "params": params}).to_string()))
                    .unwrap(),
            ).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                body["error"]["data"]["kind"], "route_exposure_denied",
                "{method_name}"
            );
        }
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn named_protected_route_rejects_disabled_or_non_positive_priority_target() {
        for (enabled, priority) in [(false, 1.0), (true, 0.0), (true, f32::NAN)] {
            let backend = MockServer::start().await;
            let tempdir = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                crate::dispatch::gateway::config_store::test_gateway_manager(
                    tempdir.path().join("gateway.toml"),
                    crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
                ),
            );
            let mut config = protected_named_upstream_config(&backend.uri());
            config.upstream[0].enabled = enabled;
            config.upstream[0].priority = priority;
            manager
                .seed_config_unchecked_for_tests(config.to_gateway_config())
                .await;
            let state = AppState::new()
                .with_config(config)
                .with_gateway_manager(manager);
            let auth_state = test_lab_auth_state().await;
            let token = issue_test_route_token(&auth_state, "https://mcp.example.com/safe");
            let app = build_router(
                state,
                Some("static-token".into()),
                Some(auth_state),
                None,
                &[],
            );
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/safe")
                        .header(header::HOST, "mcp.example.com")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            r#"{"jsonrpc":"2.0","method":"tools/list","id":1,"params":{}}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn named_protected_route_returns_denial_when_allowed_notification_gets_204() {
        let backend = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&backend)
            .await;
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_named_upstream_config(&backend.uri());
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://mcp.example.com/safe");
        let app = build_router(
            state,
            Some("static-token".into()),
            Some(auth_state),
            None,
            &[],
        );
        let body = serde_json::json!([
            {"jsonrpc":"2.0","method":"tools/call","id":7,"params":{"name":"danger.delete"}},
            {"jsonrpc":"2.0","method":"notifications/initialized"}
        ]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/safe")
                    .header(header::HOST, "mcp.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["id"], 7);
        assert_eq!(value["error"]["data"]["kind"], "route_exposure_denied");
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn named_protected_route_filters_all_capability_lists() {
        for (method_name, collection, visible, hidden) in [
            ("tools/list", "tools", "safe.search", "danger.delete"),
            (
                "resources/list",
                "resources",
                "public://status",
                "secret://credentials",
            ),
            ("prompts/list", "prompts", "safe-summary", "admin-reset"),
        ] {
            let backend = MockServer::start().await;
            let key = if collection == "resources" {
                "uri"
            } else {
                "name"
            };
            Mock::given(method("POST")).and(path("/mcp")).respond_with(
                ResponseTemplate::new(200).insert_header("content-type", "application/json").set_body_json(
                    serde_json::json!({"jsonrpc":"2.0", "id":1, "result": {(collection): [{(key): visible}, {(key): hidden}]}}),
                ),
            ).mount(&backend).await;
            let tempdir = tempfile::tempdir().unwrap();
            let manager = Arc::new(
                crate::dispatch::gateway::config_store::test_gateway_manager(
                    tempdir.path().join("gateway.toml"),
                    crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
                ),
            );
            let config = protected_named_upstream_config(&backend.uri());
            manager
                .seed_config_unchecked_for_tests(config.to_gateway_config())
                .await;
            let state = AppState::new()
                .with_config(config)
                .with_gateway_manager(manager);
            let auth_state = test_lab_auth_state().await;
            let token = issue_test_route_token(&auth_state, "https://mcp.example.com/safe");
            let app = build_router(
                state,
                Some("static-token".to_string()),
                Some(auth_state),
                None,
                &[],
            );
            let response = app.oneshot(Request::builder().method("POST").uri("/safe")
                .header(header::HOST, "mcp.example.com")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({"jsonrpc":"2.0", "method":method_name, "id":1, "params":{}}).to_string())).unwrap()).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                body["result"][collection].as_array().unwrap().len(),
                1,
                "{method_name}"
            );
            assert_eq!(body["result"][collection][0][key], visible, "{method_name}");
        }
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn named_protected_route_batch_policy_and_list_filter_fail_closed() {
        let config = protected_named_upstream_config("http://127.0.0.1:9");
        let upstream = &config.upstream[0];
        let batch = serde_json::json!([
            {"jsonrpc":"2.0", "method":"tools/list", "id":1, "params":{}},
            {"jsonrpc":"2.0", "method":"tools/call", "id":2, "params":{"name":"danger.delete"}}
        ]);
        let prepared = prepare_protected_route_request(upstream, batch.clone());
        assert_eq!(prepared.errors.len(), 1);
        assert_eq!(prepared.errors[0]["id"], 2);
        assert_eq!(
            prepared
                .forwarded
                .as_ref()
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );

        assert!(
            filter_protected_route_list_response(upstream, &batch[0], b"not-json").is_none(),
            "an unfilterable list response must fail closed"
        );

        let notification_batch = serde_json::json!([
            {"jsonrpc":"2.0", "method":"tools/call", "params":{"name":"danger.delete"}},
            {"jsonrpc":"2.0", "method":"resources/read", "id":3, "params":{}},
            {"jsonrpc":"2.0", "method":"tools/call", "id":4, "params":{"name":"safe.search"}}
        ]);
        let prepared = prepare_protected_route_request(upstream, notification_batch);
        assert_eq!(prepared.errors.len(), 1, "denied notifications stay silent");
        assert_eq!(prepared.errors[0]["id"], 3);
        assert_eq!(prepared.errors[0]["error"]["code"], -32602);
        assert_eq!(prepared.forwarded.unwrap().as_array().unwrap().len(), 1);
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn named_protected_route_filters_sse_list_event_and_preserves_framing() {
        let config = protected_named_upstream_config("http://127.0.0.1:9");
        let request =
            serde_json::json!({"jsonrpc":"2.0", "method":"tools/list", "id":1, "params":{}});
        let event = b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"safe.search\"},{\"name\":\"danger.delete\"}]}}\n\n";
        let filtered =
            filter_protected_route_sse_event(&config.upstream[0], &request, event).unwrap();
        let text = std::str::from_utf8(&filtered).unwrap();
        assert!(text.starts_with("event: message\ndata: "));
        assert!(text.ends_with("\n\n"));
        assert!(text.contains("safe.search"));
        assert!(!text.contains("danger.delete"));
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn named_protected_route_sse_filter_handles_split_stream_chunks() {
        use futures::TryStreamExt;
        let config = protected_named_upstream_config("http://127.0.0.1:9");
        let request =
            serde_json::json!({"jsonrpc":"2.0", "method":"tools/list", "id":1, "params":{}});
        let chunks = futures::stream::iter(vec![
            Ok::<_, reqwest::Error>(bytes::Bytes::from_static(b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,")),
            Ok(bytes::Bytes::from_static(b"\"result\":{\"tools\":[{\"name\":\"safe.search\"},{\"name\":\"danger.delete\"}]}}\n\n")),
        ]);
        let output = filter_protected_route_sse_stream(chunks, config.upstream[0].clone(), request)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let text = String::from_utf8(output.into_iter().flatten().collect()).unwrap();
        assert!(text.contains("safe.search"));
        assert!(!text.contains("danger.delete"));
        assert!(text.ends_with("\n\n"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn named_protected_route_sse_correlates_batch_id_and_multiline_data() {
        let config = protected_named_upstream_config("http://127.0.0.1:9");
        let request = serde_json::json!([
            {"jsonrpc":"2.0", "method":"resources/list", "id":1, "params":{}},
            {"jsonrpc":"2.0", "method":"tools/list", "id":2, "params":{}}
        ]);
        let event = b"data: {\"jsonrpc\":\"2.0\",\"id\":2,\ndata: \"result\":{\"tools\":[{\"name\":\"safe.search\"},{\"name\":\"danger.delete\"}]}}\n\n";
        let filtered =
            filter_protected_route_sse_event(&config.upstream[0], &request, event).unwrap();
        let text = std::str::from_utf8(&filtered).unwrap();
        assert!(text.contains("safe.search"));
        assert!(!text.contains("danger.delete"));
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn named_protected_route_sse_caps_each_event_not_combined_chunk() {
        use futures::TryStreamExt;
        let config = protected_named_upstream_config("http://127.0.0.1:9");
        let request =
            serde_json::json!({"jsonrpc":"2.0", "method":"tools/list", "id":1, "params":{}});
        let padding = " ".repeat(600_000);
        let event = format!(
            "data: {padding}{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"tools\":[]}}}}\n\n"
        );
        let chunk = event.repeat(2);
        assert!(chunk.len() > 1024 * 1024);
        let output = filter_protected_route_sse_stream(
            futures::stream::iter(vec![Ok::<_, reqwest::Error>(bytes::Bytes::from(chunk))]),
            config.upstream[0].clone(),
            request,
        )
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
        assert_eq!(output.len(), 2);
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn named_protected_route_sse_includes_policy_errors() {
        let errors = [protected_route_json_rpc_error(
            serde_json::json!(7),
            -32601,
            "route_exposure_denied",
            "tools",
            "denied".into(),
        )];
        let events = errors
            .iter()
            .filter_map(|error| {
                serde_json::to_string(error)
                    .ok()
                    .map(|json| format!("data: {json}\n\n"))
            })
            .collect::<String>();
        assert!(events.contains("\"id\":7"));
        assert!(events.contains("route_exposure_denied"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn named_protected_route_gates_completion_subscriptions_and_adversarial_lists() {
        let config = protected_named_upstream_config("http://127.0.0.1:9");
        let upstream = &config.upstream[0];
        for request in [
            serde_json::json!({"method":"completion/complete","id":1,"params":{"ref":{"type":"ref/prompt","name":"admin-reset"},"argument":{"name":"x","value":""}}}),
            serde_json::json!({"method":"completion/complete","id":2,"params":{"ref":{"type":"ref/resource","uri":"secret://credentials"},"argument":{"name":"x","value":""}}}),
            serde_json::json!({"method":"resources/subscribe","id":3,"params":{"uri":"secret://credentials"}}),
            serde_json::json!({"method":"resources/unsubscribe","id":4,"params":{"uri":"secret://credentials"}}),
        ] {
            assert!(matches!(
                protected_route_exposure_decision(upstream, &request),
                ProtectedRouteExposureDecision::Denied(_)
            ));
        }
        let batch = serde_json::json!([{"method":"tools/list","id":1,"params":{}}]);
        assert!(
            filter_protected_route_list_response(
                upstream,
                &batch,
                br#"[{"id":99,"result":{"tools":[]}}]"#
            )
            .is_none()
        );
        assert!(
            filter_protected_route_list_response(
                upstream,
                &batch,
                br#"[{"id":1,"result":{"tools":{}}}]"#
            )
            .is_none()
        );
        assert!(
            filter_protected_route_list_response(
                upstream,
                &batch,
                br#"[{"id":1,"result":{"tools":[]}},{"id":1,"result":{"tools":[]}}]"#
            )
            .is_none()
        );
        let oversized = format!("data: {}\n\n", " ".repeat(1024 * 1024));
        assert!(find_sse_event_end(oversized.as_bytes()).unwrap() > 1024 * 1024);
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_domain_mcp_route_intercepts_canonical_mcp_path_by_host() {
        let backend = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"proxied":true}"#),
            )
            .mount(&backend)
            .await;

        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config =
            protected_route_config("telemetry", "telemetry.example.com", "/mcp", &backend.uri());
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://telemetry.example.com/mcp");
        let local_mcp = Router::new().route(
            "/mcp",
            post(|| async { Json(serde_json::json!({"local": true})) }),
        );
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            Some(local_mcp),
            &[],
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(header::HOST, "telemetry.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            r#"{"proxied":true}"#
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_gateway_subset_unauthorized_header_points_to_route_metadata() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_gateway_subset_config();
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ops")
                    .header(header::HOST, "mcp.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource/ops\", scope=\"mcp:ops\""
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_gateway_subset_dispatches_to_scoped_router_after_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = protected_gateway_subset_config();
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let scoped_router = Router::new().route(
            "/ops",
            post(|| async { Json(serde_json::json!({"scoped": true})) }),
        );
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager)
            .with_protected_mcp_routers(std::collections::HashMap::from([(
                "ops".to_string(),
                scoped_router,
            )]));
        let auth_state = test_lab_auth_state().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        let token = auth_state
            .signing_keys
            .issue_access_token(&labby_auth::jwt::AccessClaims {
                iss: "https://lab.example.com".into(),
                sub: "legacy-subject".into(),
                aud: "https://mcp.example.com/ops".into(),
                exp: now + 3600,
                nbf: None,
                iat: now,
                jti: String::new(),
                scope: "mcp:ops".into(),
                azp: "legacy-client".into(),
                identity_issuer: None,
                identity_credential_id: None,
            })
            .unwrap();
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            None,
            &[],
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ops")
                    .header(header::HOST, "mcp.example.com")
                    .header("x-request-id", "protected-subset-test")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-request-id").unwrap(),
            "protected-subset-test"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(body.to_vec()).unwrap(),
            r#"{"scoped":true}"#
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_gateway_subsets_with_same_path_dispatch_by_host() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let mut config = protected_gateway_subset_config();
        let mut second = config.protected_mcp_routes[0].clone();
        second.name = "ops-b".to_string();
        second.public_host = "mcp-b.example.com".to_string();
        config.protected_mcp_routes.push(second);
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;

        let routers = std::collections::HashMap::from([
            (
                "ops".to_string(),
                Router::new().route(
                    "/ops",
                    post(|| async { Json(serde_json::json!({"route": "a"})) }),
                ),
            ),
            (
                "ops-b".to_string(),
                Router::new().route(
                    "/ops",
                    post(|| async { Json(serde_json::json!({"route": "b"})) }),
                ),
            ),
        ]);
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager)
            .with_protected_mcp_routers(routers);
        let auth_state = test_lab_auth_state().await;
        let token_a = issue_test_token(&auth_state, "https://mcp.example.com/ops", "mcp:ops");
        let token_b = issue_test_token(&auth_state, "https://mcp-b.example.com/ops", "mcp:ops");
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            None,
            &[],
        );

        for (host, token, expected) in [
            ("mcp.example.com", token_a, r#"{"route":"a"}"#),
            ("mcp-b.example.com", token_b, r#"{"route":"b"}"#),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/ops")
                        .header(header::HOST, host)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{}}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(String::from_utf8(body.to_vec()).unwrap(), expected);
        }
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_route_invalid_backend_url_returns_structured_error() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = crate::config::LabConfig {
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: "bad".to_string(),
                enabled: true,
                public_host: "mcp.example.com".to_string(),
                public_path: "/bad".to_string(),
                upstream: None,
                backend_url: "://not-a-url".to_string(),
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:read".to_string()],
                health_path: None,
                target: None,
            }],
            ..crate::config::LabConfig::default()
        };
        manager
            .seed_config_unchecked_for_tests(config.to_gateway_config())
            .await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_token(&auth_state, "https://mcp.example.com/bad", "mcp:read");
        let app = build_router(
            state,
            Some("static-token".to_string()),
            Some(auth_state),
            None,
            &[],
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bad")
                    .header(header::HOST, "mcp.example.com")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["kind"], "bad_gateway");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("backend_url is invalid")
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn gateway_oauth_routes_require_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let state = AppState::new().with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/gateway/oauth/status?upstream=test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn browser_oauth_callback_bypasses_bearer_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let state = AppState::new().with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/upstream/callback?upstream=test&state=csrf&code=authcode")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn serves_web_assets_for_browser_routes_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();

        let state = AppState::new().with_web_assets_dir(dir.path().to_path_buf());
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/gateways/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("Labby"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlinked_assets_outside_configured_web_root() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();
        fs::write(outside.path().join("secret.txt"), "top-secret").unwrap();
        unix_fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("secret.txt"),
        )
        .unwrap();

        let state = AppState::new().with_web_assets_dir(dir.path().to_path_buf());
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/secret.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn v1_routes_still_win_over_web_asset_fallback() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();

        let state = AppState::new().with_web_assets_dir(dir.path().to_path_buf());
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("application/json"));
    }

    #[tokio::test]
    async fn serves_embedded_web_assets_without_configured_directory() {
        // The embedded asset bundle is produced by building `apps/gateway-admin`
        // (Next.js static export) into `apps/gateway-admin/out/`. In a fresh
        // workspace clone the dir is empty, which is a valid state for backend
        // work — skip the test rather than fail spuriously.
        if !crate::api::web::embedded_web_assets_available() {
            eprintln!(
                "skipping: apps/gateway-admin/out/index.html missing — \
                 run `pnpm --filter gateway-admin build` to populate"
            );
            return;
        }
        let state = AppState::new().with_embedded_web_assets();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("text/html"));
    }

    #[tokio::test]
    async fn v1_routes_still_win_over_embedded_web_asset_fallback() {
        let state = AppState::new().with_embedded_web_assets();
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(content_type.contains("application/json"));
    }

    #[tokio::test]
    async fn integration_identity_is_authenticated_and_bound_to_mounted_runtime() {
        let default_registry = crate::registry::build_default_registry();
        let doctor = default_registry.service("doctor").unwrap().clone();
        let mut registry = crate::registry::ToolRegistry::new();
        registry.register(doctor.clone());
        let mut unmounted = doctor;
        unmounted.name = "unmounted_fixture";
        registry.register(unmounted);
        let mut state = AppState::from_registry(registry);
        state.installation_id = Some(Arc::from("fixture-installation"));
        for (authenticated, trusted) in [(false, false), (true, true)] {
            let mut candidate = state.clone();
            if authenticated {
                candidate = candidate.with_bearer_token(Some(Arc::from("fixture-token")));
            }
            assert!(
                !build_v1_router(&candidate, authenticated, trusted)
                    .descriptors
                    .iter()
                    .any(|route| route.mount == "integration")
            );
        }
        let mut uninitialized = state.clone();
        uninitialized.installation_id = None;
        uninitialized = uninitialized.with_bearer_token(Some(Arc::from("fixture-token")));
        assert!(
            !build_v1_router(&uninitialized, true, false)
                .descriptors
                .iter()
                .any(|route| route.mount == "integration")
        );
        let app = build_router(state, Some("fixture-token".into()), None, None, &[]);
        let request = |authenticated: bool| {
            let mut request = Request::builder()
                .uri("/v1/integration/identity")
                .header(header::HOST, "localhost");
            if authenticated {
                request = request.header(header::AUTHORIZATION, "Bearer fixture-token");
            }
            request.body(Body::empty()).unwrap()
        };
        assert_eq!(
            app.clone().oneshot(request(false)).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let response = app.oneshot(request(true)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/contracts/integration-identity-v1.schema.json"
        ))
        .unwrap();
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
        assert_eq!(value["auth"]["modes"], serde_json::json!(["static_bearer"]));
        assert_eq!(value["capabilities"], serde_json::json!(["doctor"]));
        assert!(!value.to_string().contains("fixture-token"));
        assert!(!value.to_string().contains("fixture-installation"));
        assert!(
            !crate::api::route_registry::build_integrated_trusted_host_route_descriptors()
                .iter()
                .any(|route| route.mount == "integration")
        );
    }

    #[tokio::test]
    async fn integration_identity_uses_actual_oauth_resource_and_origin() {
        let mut oauth = test_lab_auth_state().await;
        let config = Arc::make_mut(&mut oauth.config);
        config.public_url = Some(url::Url::parse("https://lab.example.com/issuer").unwrap());
        config.resource_path = "/custom/mcp".into();
        let snapshot = crate::integration_identity::IntegrationIdentity::snapshot(
            "fixture",
            true,
            Some(&oauth),
            vec![],
        );
        assert_eq!(
            snapshot.auth.issuer.as_deref(),
            Some("https://lab.example.com/issuer")
        );
        assert_eq!(
            snapshot.auth.audience.as_deref(),
            Some(labby_auth::metadata::canonical_resource_url(&oauth).as_str())
        );
        assert_eq!(
            snapshot.auth.token_endpoint_origin.as_deref(),
            Some("https://lab.example.com")
        );
        assert_eq!(snapshot.auth.modes, ["static_bearer", "oauth2"]);
        assert!(snapshot.auth.credential_generation.is_none());
        assert!(snapshot.auth.principal_cache_scope.is_none());
    }

    async fn test_lab_auth_state() -> labby_auth::state::AuthState {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("bootstrap-secret".to_string()),
            admin_email: "browser@example.com".to_string(),
            google: labby_auth::config::GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_url: None,
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )
                .unwrap(),
            ),
            ..labby_auth::config::AuthConfig::default()
        };
        labby_auth::state::AuthState::new(config).await.unwrap()
    }

    fn issue_test_lab_token(auth_state: &labby_auth::state::AuthState) -> String {
        issue_test_token(auth_state, "https://lab.example.com/mcp", "lab")
    }

    #[cfg(feature = "gateway")]
    fn issue_test_route_token(auth_state: &labby_auth::state::AuthState, audience: &str) -> String {
        issue_test_token(auth_state, audience, "mcp:read mcp:write")
    }

    fn issue_test_token(
        auth_state: &labby_auth::state::AuthState,
        audience: &str,
        scope: &str,
    ) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        auth_state
            .signing_keys
            .issue_access_token(&labby_auth::jwt::AccessClaims {
                iss: "https://lab.example.com".to_string(),
                sub: "google-user".to_string(),
                aud: audience.to_string(),
                exp: now + 3600,
                nbf: None,
                iat: now,
                jti: "test-jti".to_string(),
                scope: scope.to_string(),
                azp: "client".to_string(),
                identity_issuer: Some("https://accounts.google.com".to_string()),
                identity_credential_id: None,
            })
            .unwrap()
    }

    #[cfg(feature = "gateway")]
    fn protected_route_config(
        name: &str,
        host: &str,
        path: &str,
        backend_url: &str,
    ) -> crate::config::LabConfig {
        let backend_url = format!("{}/mcp", backend_url.trim_end_matches('/'));
        crate::config::LabConfig {
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: name.to_string(),
                enabled: true,
                public_host: host.to_string(),
                public_path: path.to_string(),
                upstream: None,
                backend_url,
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
                health_path: None,
                target: None,
            }],
            ..crate::config::LabConfig::default()
        }
    }

    #[cfg(feature = "gateway")]
    fn protected_named_upstream_config(backend_url: &str) -> crate::config::LabConfig {
        crate::config::LabConfig {
            upstream: vec![crate::config::UpstreamConfig {
                name: "restricted".to_string(),
                enabled: true,
                url: Some(format!("{}/mcp", backend_url.trim_end_matches('/'))),
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: Default::default(),
                proxy_resources: true,
                proxy_prompts: true,
                proxy_skills: false,
                expose_tools: Some(vec!["safe.*".to_string()]),
                expose_resources: Some(vec!["public://*".to_string()]),
                expose_prompts: Some(vec!["safe-*".to_string()]),
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: "safe".to_string(),
                enabled: true,
                public_host: "mcp.example.com".to_string(),
                public_path: "/safe".to_string(),
                upstream: Some("restricted".to_string()),
                backend_url: String::new(),
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
                health_path: None,
                target: None,
            }],
            ..crate::config::LabConfig::default()
        }
    }

    #[cfg(feature = "gateway")]
    fn protected_gateway_subset_config() -> crate::config::LabConfig {
        crate::config::LabConfig {
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: "ops".to_string(),
                enabled: true,
                public_host: "mcp.example.com".to_string(),
                public_path: "/ops".to_string(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:ops".to_string()],
                health_path: None,
                target: Some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
                    crate::config::ProtectedGatewaySubsetTarget {
                        project_id: None,
                        upstreams: vec!["gateway-alpha".to_string(), "hidden-upstream".to_string()],
                        services: vec!["gateway".to_string()],
                        expose_code_mode: true,
                        loadout: None,
                    },
                )),
            }],
            ..crate::config::LabConfig::default()
        }
    }

    async fn seed_browser_session(
        auth_state: &labby_auth::state::AuthState,
    ) -> labby_auth::types::BrowserSessionRow {
        let session = labby_auth::types::BrowserSessionRow {
            session_id: "sess-123".to_string(),
            subject: "browser-user".to_string(),
            email: Some("browser@example.com".to_string()),
            csrf_token: "csrf-123".to_string(),
            created_at: 1,
            expires_at: i64::MAX,
            project_binding: None,
        };
        auth_state
            .store
            .upsert_browser_session(session.clone())
            .await
            .unwrap();
        session
    }

    #[tokio::test]
    async fn dev_mockup_routes_require_auth_when_configured() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/dev/mockup/example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "/dev mockup routes must use auth middleware when auth is configured"
        );
    }
}
