//! Top-level axum router — mounts `POST /v1/<service>` for every enabled service
//! and the MCP streamable HTTP transport at `/mcp`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderName, HeaderValue, Method, Request, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use subtle::ConstantTimeEq;
use tower_http::{
    compression::CompressionLayer,
    cors::{AllowOrigin, CorsLayer},
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::Level;

use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::upstream::auth::configured_bearer_token;
use lab_auth::AuthLayer;
use lab_auth::error::AuthError as LabAuthError;

/// Convert lab's strongly-typed [`crate::observability::activity::ActorKeyDeriver`]
/// into the closure-erased [`lab_auth::ActorKeyDeriver`] alias accepted by
/// [`AuthLayer::with_actor_key_deriver`]. Keeps the lab-specific HMAC actor-key
/// derivation while letting lab-auth stay agnostic about consumer-specific
/// observability hooks.
fn lab_auth_deriver(
    deriver: Arc<crate::observability::activity::ActorKeyDeriver>,
) -> Arc<lab_auth::ActorKeyDeriver> {
    Arc::new(move |subject: &str| {
        deriver
            .derive_subject(subject)
            .map(crate::observability::activity::ActorKey::into_arc)
    })
}

const DEV_MARKETPLACE_READ_ACTIONS: &[&str] = &[
    "help",
    "schema",
    "sources.list",
    "plugins.list",
    "plugin.get",
    "plugin.artifacts",
    "plugin.workspace",
    "plugin.components",
    "plugin.deploy.preview",
    "agent.list",
    "agent.get",
    "mcp.config",
    "mcp.list",
    "mcp.get",
    "mcp.versions",
    "mcp.meta.get",
];

/// Constant-time byte comparison using `subtle::ConstantTimeEq` to prevent
/// timing-based token prefix leakage.
pub(crate) fn tokens_equal(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b'?') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit(u32::from(b >> 4), 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit(u32::from(b & 0xf), 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}

pub(crate) fn parse_bearer_token(header_value: &str) -> Option<String> {
    let mut parts = header_value.split_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    if parts.next().is_some() || !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Some(token.to_string())
}

use super::{health, services, state::AppState};
use crate::dispatch::error::ToolError;

fn app_auth_state(state: &AppState) -> Result<lab_auth::state::AuthState, LabAuthError> {
    state
        .oauth_state
        .as_ref()
        .map(|state| (**state).clone())
        .ok_or_else(|| LabAuthError::Config("oauth auth state is not configured".to_string()))
}

async fn app_auth_state_with_protected_routes(
    state: &AppState,
) -> Result<lab_auth::state::AuthState, LabAuthError> {
    let auth_state = app_auth_state(state)?;
    if let Some(manager) = state.gateway_manager.as_ref() {
        let routes = manager.protected_route_list().await;
        tracing::debug!(
            route_count = routes.iter().filter(|route| route.enabled).count(),
            "oauth protected resource scope map refreshed from gateway routes"
        );
        auth_state.set_allowed_resource_scopes(
            routes
                .into_iter()
                .filter(|route| route.enabled)
                .map(|route| (route.public_resource(), route.scopes)),
        );
    }
    Ok(auth_state)
}

async fn auth_authorization_server_metadata(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::metadata::authorization_server_metadata(State(app_auth_state(&state)?)).await)
}

async fn auth_protected_resource_metadata(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Result<impl IntoResponse, LabAuthError> {
    if let (Some(manager), Some(host)) = (state.gateway_manager.as_ref(), request_host(&request))
        && let Some(route) = manager
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
            .ok_or_else(|| LabAuthError::Config("LAB_PUBLIC_URL is required".to_string()))?;
        return Ok(Json(lab_auth::types::ProtectedResourceMetadata {
            resource: route.public_resource(),
            authorization_servers: vec![public_url.as_str().trim_end_matches('/').to_string()],
            scopes_supported: route.scopes,
            bearer_methods_supported: vec!["header".to_string()],
        }));
    }
    Ok(lab_auth::metadata::protected_resource_metadata(State(app_auth_state(&state)?)).await)
}

async fn protected_route_resource_metadata(
    State(state): State<AppState>,
    request: Request<Body>,
) -> axum::response::Response {
    let Some(manager) = state.gateway_manager.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(host) = request_host(&request) else {
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
            "oauth protected resource metadata failed: LAB_PUBLIC_URL missing"
        );
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = Json(lab_auth::types::ProtectedResourceMetadata {
        resource: route.public_resource(),
        authorization_servers: vec![public_url.as_str().trim_end_matches('/').to_string()],
        scopes_supported: route.scopes,
        bearer_methods_supported: vec!["header".to_string()],
    })
    .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}

async fn auth_jwks(State(state): State<AppState>) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::metadata::jwks(State(app_auth_state(&state)?)).await)
}

async fn auth_register(
    State(state): State<AppState>,
    body: Json<lab_auth::types::ClientRegistrationRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::register_client(State(app_auth_state(&state)?), body).await?)
}

async fn auth_authorize(
    State(state): State<AppState>,
    query: Query<lab_auth::types::AuthorizeQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::authorize(
        State(app_auth_state_with_protected_routes(&state).await?),
        query,
    )
    .await?)
}

async fn auth_browser_login(
    State(state): State<AppState>,
    query: Query<lab_auth::types::BrowserLoginQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::browser_login(State(app_auth_state(&state)?), query).await?)
}

async fn auth_callback(
    State(state): State<AppState>,
    query: Query<lab_auth::types::CallbackQuery>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::authorize::callback(State(app_auth_state(&state)?), query).await?)
}

async fn auth_token(
    State(state): State<AppState>,
    form: axum::extract::Form<lab_auth::types::TokenRequest>,
) -> Result<impl IntoResponse, LabAuthError> {
    Ok(lab_auth::token::token(
        State(app_auth_state_with_protected_routes(&state).await?),
        form,
    )
    .await)
}

fn auth_error_response(message: &str, resource_url: Option<&str>) -> axum::response::Response {
    let err = ToolError::Sdk {
        sdk_kind: "auth_failed".into(),
        message: message.into(),
    };
    let mut response = err.into_response();
    if let Some(url) = resource_url {
        let www_auth = crate::api::oauth::www_authenticate_value(url);
        if let Ok(value) = HeaderValue::from_str(&www_auth) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
    }
    response
}

fn auth_error_response_with_challenge(
    message: &str,
    metadata_url: &str,
    scopes: &[String],
) -> axum::response::Response {
    let err = ToolError::Sdk {
        sdk_kind: "auth_failed".into(),
        message: message.into(),
    };
    let mut response = err.into_response();
    let scope = scopes
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let www_auth = format!("Bearer resource_metadata=\"{metadata_url}\", scope=\"{scope}\"");
    if let Ok(value) = HeaderValue::from_str(&www_auth) {
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

fn request_host(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get("x-forwarded-host")
        .or_else(|| request.headers().get(header::HOST))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(ToOwned::to_owned)
}

fn route_resource_metadata_url(route: &crate::config::ProtectedMcpRouteConfig) -> String {
    format!(
        "https://{}/.well-known/oauth-protected-resource{}",
        route.public_host,
        route.public_path.trim_end_matches('/')
    )
}

async fn authenticate_protected_route_request(
    request: &mut Request<Body>,
    route: &crate::config::ProtectedMcpRouteConfig,
    auth_state: Option<&lab_auth::state::AuthState>,
    actor_key_deriver: Option<&crate::observability::activity::ActorKeyDeriver>,
) -> Result<(), axum::response::Response> {
    let resource = route.public_resource();
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_bearer_token);
    let Some(token) = auth_header else {
        tracing::warn!(
            route = %route.name,
            resource = %resource,
            method = %request.method(),
            path = %request.uri().path(),
            "protected MCP route auth failed: missing bearer token"
        );
        return Err(auth_error_response_with_challenge(
            "missing bearer token",
            &route_resource_metadata_url(route),
            &route.scopes,
        ));
    };
    let Some(auth_state) = auth_state else {
        tracing::error!(
            route = %route.name,
            resource = %resource,
            "protected MCP route auth failed: oauth auth state missing"
        );
        return Err(auth_error_response_with_challenge(
            "oauth auth state is not configured",
            &route_resource_metadata_url(route),
            &route.scopes,
        ));
    };
    let Some(expected_issuer) = auth_state
        .config
        .public_url
        .as_ref()
        .map(|url| url.as_str().trim_end_matches('/').to_string())
    else {
        tracing::error!(
            route = %route.name,
            resource = %resource,
            "protected MCP route auth failed: LAB_PUBLIC_URL missing"
        );
        return Err(auth_error_response_with_challenge(
            "server misconfigured: LAB_PUBLIC_URL required for JWT validation",
            &route_resource_metadata_url(route),
            &route.scopes,
        ));
    };
    let claims = auth_state
        .signing_keys
        .validate_access_token_with_issuer(&token, &resource, &expected_issuer)
        .map_err(|error| {
            tracing::warn!(
                error = %error,
                route = %route.name,
                resource = %resource,
                method = %request.method(),
                path = %request.uri().path(),
                "protected MCP route auth failed: JWT validation failed"
            );
            auth_error_response_with_challenge(
                "invalid bearer token",
                &route_resource_metadata_url(route),
                &route.scopes,
            )
        })?;
    let required_scopes = route.scopes.iter().map(String::as_str).collect::<Vec<_>>();
    let granted = claims.scope.split_whitespace().collect::<Vec<_>>();
    if !required_scopes
        .iter()
        .all(|required| granted.iter().any(|scope| scope == required))
    {
        tracing::warn!(
            route = %route.name,
            resource = %resource,
            subject_id = %lab_auth::util::fingerprint(&claims.sub),
            required_scopes = ?required_scopes,
            granted_scopes = ?granted,
            "protected MCP route auth failed: insufficient scope"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "forbidden".into(),
            message: "insufficient OAuth scope for protected MCP route".into(),
        }
        .into_response());
    }
    let subject_id = lab_auth::util::fingerprint(&claims.sub);
    let issuer = claims.iss.clone();
    let granted_scopes = granted.iter().map(|scope| (*scope).to_string()).collect();
    tracing::info!(
        route = %route.name,
        resource = %resource,
        subject_id = %subject_id,
        issuer = %issuer,
        granted_scopes = ?granted,
        "protected MCP route auth accepted"
    );
    request
        .extensions_mut()
        .insert(crate::api::oauth::AuthContext {
            actor_key: derive_actor_key(actor_key_deriver, &claims.sub),
            sub: claims.sub,
            scopes: granted_scopes,
            issuer: claims.iss,
            via_session: false,
            csrf_token: None,
            email: None,
        });
    Ok(())
}

async fn proxy_protected_mcp_route(
    state: &AppState,
    request: Request<Body>,
    route: crate::config::ProtectedMcpRouteConfig,
) -> axum::response::Response {
    let started = Instant::now();
    let suffix = request
        .uri()
        .path()
        .strip_prefix(&route.public_path)
        .unwrap_or("");

    let (mut upstream, upstream_auth_token, upstream_target) =
        match protected_route_upstream_target(state, &route).await {
            Ok(target) => target,
            Err(response) => return response,
        };

    let mut backend_path = upstream.path().trim_end_matches('/').to_string();
    if backend_path.is_empty() {
        backend_path.push('/');
    }
    if !suffix.is_empty() {
        if !backend_path.ends_with('/') {
            backend_path.push('/');
        }
        backend_path.push_str(suffix.trim_start_matches('/'));
    }
    upstream.set_path(&backend_path);
    upstream.set_query(request.uri().query());

    let method = request.method().clone();
    let original_path = request.uri().path().to_string();
    let headers = request.headers().clone();
    let body = match axum::body::to_bytes(request.into_body(), 50 * 1024 * 1024).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                method = %method,
                path = %original_path,
                error = %error,
                "protected MCP route proxy failed: request body read error"
            );
            return ToolError::Sdk {
                sdk_kind: "bad_request".into(),
                message: format!("failed to read MCP request body: {error}"),
            }
            .into_response();
        }
    };
    tracing::info!(
        route = %route.name,
        resource = %route.public_resource(),
        method = %method,
        path = %original_path,
        upstream = %upstream_target,
        upstream_auth = upstream_auth_token.is_some(),
        "protected MCP route proxy start"
    );
    let mut builder = state
        .protected_mcp_http_client
        .request(method.clone(), upstream);
    if let Some(token) = upstream_auth_token {
        builder = builder.bearer_auth(token);
    }
    for header_name in [
        header::ACCEPT,
        header::CONTENT_TYPE,
        HeaderName::from_static("mcp-protocol-version"),
        HeaderName::from_static("mcp-session-id"),
        HeaderName::from_static("last-event-id"),
    ] {
        if let Some(value) = headers.get(&header_name) {
            builder = builder.header(&header_name, value);
        }
    }
    let upstream_response = match builder.body(body).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                method = %method,
                path = %original_path,
                upstream = %upstream_target,
                elapsed_ms = started.elapsed().as_millis(),
                error = %error,
                "protected MCP route proxy failed: backend request failed"
            );
            return ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: format!("protected MCP backend request failed: {error}"),
            }
            .into_response();
        }
    };
    let status = StatusCode::from_u16(upstream_response.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::info!(
        route = %route.name,
        resource = %route.public_resource(),
        method = %method,
        path = %original_path,
        upstream = %upstream_target,
        status = status.as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "protected MCP route proxy finish"
    );
    let mut response = axum::response::Response::builder().status(status);
    for header_name in [
        header::CONTENT_TYPE,
        header::CACHE_CONTROL,
        HeaderName::from_static("mcp-session-id"),
        HeaderName::from_static("mcp-protocol-version"),
    ] {
        if let Some(value) = upstream_response.headers().get(&header_name) {
            response = response.header(&header_name, value);
        }
    }
    response
        .body(Body::from_stream(upstream_response.bytes_stream()))
        .unwrap_or_else(|error| {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                elapsed_ms = started.elapsed().as_millis(),
                error = %error,
                "protected MCP route proxy failed: response build failed"
            );
            ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: format!("failed to build protected MCP response: {error}"),
            }
            .into_response()
        })
}

async fn protected_route_upstream_target(
    state: &AppState,
    route: &crate::config::ProtectedMcpRouteConfig,
) -> Result<(reqwest::Url, Option<String>, String), axum::response::Response> {
    let Some(upstream_name) = route.upstream.as_deref() else {
        let url = reqwest::Url::parse(&route.backend_url).map_err(|error| {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                error = %error,
                "protected MCP route proxy failed: invalid backend_url"
            );
            StatusCode::BAD_GATEWAY.into_response()
        })?;
        return Ok((url, None, "backend_url".to_string()));
    };

    let Some(manager) = state.gateway_manager.as_ref() else {
        tracing::error!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            "protected MCP route proxy failed: gateway manager missing"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "bad_gateway".into(),
            message: "gateway manager is not available for upstream protected route".into(),
        }
        .into_response());
    };
    let Some(upstream_config) = manager.upstream_config(upstream_name).await else {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            "protected MCP route proxy failed: configured upstream not found"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("upstream `{upstream_name}` not found for protected MCP route"),
        }
        .into_response());
    };
    let Some(raw_url) = upstream_config.url.as_deref() else {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            "protected MCP route proxy failed: upstream has no HTTP URL"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "bad_gateway".into(),
            message: format!("upstream `{upstream_name}` does not have an HTTP MCP URL"),
        }
        .into_response());
    };
    let url = reqwest::Url::parse(raw_url).map_err(|error| {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            error = %error,
            "protected MCP route proxy failed: invalid upstream URL"
        );
        StatusCode::BAD_GATEWAY.into_response()
    })?;

    let token = if upstream_config.oauth.is_some() {
        let Some(oauth_manager) = manager.upstream_oauth_manager(upstream_name) else {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                upstream = %upstream_name,
                subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
                "protected MCP route proxy failed: upstream oauth manager missing"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "oauth_needs_reauth".into(),
                message: format!("upstream `{upstream_name}` is not connected with OAuth"),
            }
            .into_response());
        };
        let auth_client = oauth_manager
            .build_auth_client(SHARED_GATEWAY_OAUTH_SUBJECT)
            .await
            .map_err(|error| {
                tracing::warn!(
                    route = %route.name,
                    resource = %route.public_resource(),
                    upstream = %upstream_name,
                    subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
                    kind = error.kind(),
                    error = %error,
                    "protected MCP route proxy failed: upstream oauth auth client unavailable"
                );
                ToolError::Sdk {
                    sdk_kind: error.kind().to_string(),
                    message: format!(
                        "upstream `{upstream_name}` OAuth authorization required: {error}"
                    ),
                }
                .into_response()
            })?;
        Some(auth_client.get_access_token().await.map_err(|error| {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                upstream = %upstream_name,
                subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
                error = %error,
                "protected MCP route proxy failed: upstream oauth token unavailable"
            );
            ToolError::Sdk {
                sdk_kind: "oauth_needs_reauth".into(),
                message: format!("upstream `{upstream_name}` OAuth token unavailable: {error}"),
            }
            .into_response()
        })?)
    } else {
        upstream_config
            .bearer_token_env
            .as_deref()
            .and_then(configured_bearer_token)
    };

    Ok((url, token, format!("upstream:{upstream_name}")))
}

async fn protected_mcp_route_entry(
    state: AppState,
    mut request: Request<Body>,
    route: crate::config::ProtectedMcpRouteConfig,
) -> axum::response::Response {
    let compatibility_metadata_path = format!(
        "{}/.well-known/oauth-protected-resource",
        route.public_path.trim_end_matches('/')
    );
    if *request.method() == Method::GET && request.uri().path() == compatibility_metadata_path {
        tracing::info!(
            route = %route.name,
            resource = %route.public_resource(),
            path = %request.uri().path(),
            "oauth protected resource compatibility metadata served"
        );
        return protected_route_metadata_response(&state, route).await;
    }
    if !matches!(
        *request.method(),
        Method::GET | Method::POST | Method::DELETE
    ) {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            method = %request.method(),
            path = %request.uri().path(),
            "protected MCP route rejected unsupported method"
        );
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if let Err(response) = authenticate_protected_route_request(
        &mut request,
        &route,
        state.oauth_state.as_deref(),
        state.actor_key_deriver.as_deref(),
    )
    .await
    {
        return response;
    }
    proxy_protected_mcp_route(&state, request, route).await
}

async fn protected_mcp_intercept(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<axum::response::Response, std::convert::Infallible> {
    let route = if let (Some(manager), Some(host)) =
        (state.gateway_manager.as_ref(), request_host(&request))
    {
        manager
            .resolve_protected_route(&host, request.uri().path())
            .await
    } else {
        None
    };
    if let Some(route) = route {
        tracing::info!(
            route = %route.name,
            resource = %route.public_resource(),
            method = %request.method(),
            path = %request.uri().path(),
            "protected MCP route matched"
        );
        return Ok(protected_mcp_route_entry(state, request, route).await);
    }
    Ok(next.run(request).await)
}

fn csrf_error_response(message: &str) -> axum::response::Response {
    ToolError::Sdk {
        sdk_kind: "validation_failed".into(),
        message: message.into(),
    }
    .into_response()
}

async fn authenticate_request(
    mut request: Request<Body>,
    next: Next,
    static_token: Option<Arc<str>>,
    auth_state: Option<Arc<lab_auth::state::AuthState>>,
    actor_key_deriver: Option<Arc<crate::observability::activity::ActorKeyDeriver>>,
    resource_url: Option<Arc<str>>,
    allow_session_cookie: bool,
) -> Result<axum::response::Response, std::convert::Infallible> {
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_bearer_token);

    if let Some(token) = auth_header {
        if let Some(ref expected) = static_token
            && tokens_equal(&token, expected.as_ref())
        {
            let sub = "static-bearer".to_string();
            let actor_key = derive_actor_key(actor_key_deriver.as_deref(), &sub);
            request
                .extensions_mut()
                .insert(crate::api::oauth::AuthContext {
                    sub,
                    actor_key,
                    scopes: vec!["lab:read".to_string(), "lab:admin".to_string()],
                    issuer: "local".to_string(),
                    via_session: false,
                    csrf_token: None,
                    email: None,
                });
            return Ok(next.run(request).await);
        }

        if let Some(ref auth_state) = auth_state {
            let Some(expected_issuer) = auth_state
                .config
                .public_url
                .as_ref()
                .map(|url| url.as_str().trim_end_matches('/').to_string())
            else {
                return Ok(auth_error_response(
                    "server misconfigured: LAB_PUBLIC_URL required for JWT validation",
                    resource_url.as_deref(),
                ));
            };
            let expected_aud = lab_auth::metadata::canonical_resource_url(auth_state);
            match auth_state.signing_keys.validate_access_token_with_issuer(
                &token,
                &expected_aud,
                &expected_issuer,
            ) {
                Ok(claims) => {
                    request
                        .extensions_mut()
                        .insert(crate::api::oauth::AuthContext {
                            actor_key: derive_actor_key(actor_key_deriver.as_deref(), &claims.sub),
                            sub: claims.sub,
                            scopes: claims
                                .scope
                                .split_whitespace()
                                .filter(|scope| !scope.is_empty())
                                .map(ToOwned::to_owned)
                                .collect(),
                            issuer: claims.iss,
                            via_session: false,
                            csrf_token: None,
                            email: None,
                        });
                    return Ok(next.run(request).await);
                }
                Err(error) => {
                    tracing::debug!(error = %error, "lab-auth JWT validation failed");
                }
            }
        }

        return Ok(auth_error_response(
            "invalid bearer token",
            resource_url.as_deref(),
        ));
    }

    if allow_session_cookie
        && let Some(auth_state) = auth_state.as_ref()
        && let Some(session_id) = lab_auth::session::read_cookie(
            request.headers(),
            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
        )
    {
        match auth_state.store.find_browser_session(&session_id).await {
            Ok(Some(session)) => {
                if !matches!(
                    *request.method(),
                    Method::GET | Method::HEAD | Method::OPTIONS
                ) {
                    let csrf = request
                        .headers()
                        .get(lab_auth::session::BROWSER_CSRF_HEADER_NAME)
                        .and_then(|value| value.to_str().ok());
                    if csrf != Some(session.csrf_token.as_str()) {
                        return Ok(csrf_error_response("missing or invalid csrf token"));
                    }
                }

                request
                    .extensions_mut()
                    .insert(crate::api::oauth::AuthContext {
                        actor_key: derive_actor_key(actor_key_deriver.as_deref(), &session.subject),
                        sub: session.subject,
                        scopes: vec!["lab:read".to_string(), "lab:admin".to_string()],
                        issuer: "browser-session".to_string(),
                        via_session: true,
                        csrf_token: Some(session.csrf_token),
                        email: session.email,
                    });
                return Ok(next.run(request).await);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(error = %error, "browser session lookup failed");
            }
        }
    }

    // For browser GET requests with no bearer token and no valid session cookie,
    // redirect to /auth/login so the Google OAuth flow can establish a session.
    // Only fires on v1 routes (allow_session_cookie=true); the MCP endpoint uses bearer-only.
    if allow_session_cookie
        && auth_state.is_some()
        && *request.method() == Method::GET
        && request
            .headers()
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html"))
    {
        let return_to = request
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        let encoded = percent_encode_path(return_to);
        let login_url = format!("/auth/login?return_to={encoded}");
        return Ok(axum::response::Redirect::to(&login_url).into_response());
    }

    Ok(auth_error_response(
        if allow_session_cookie {
            "missing bearer token or session cookie"
        } else {
            "missing bearer token"
        },
        resource_url.as_deref(),
    ))
}

fn derive_actor_key(
    deriver: Option<&crate::observability::activity::ActorKeyDeriver>,
    subject: &str,
) -> Option<Arc<str>> {
    deriver
        .and_then(|deriver| deriver.derive_subject(subject))
        .map(crate::observability::activity::ActorKey::into_arc)
}

/// Build the `/v1` sub-router with all feature-gated service routes.
fn build_v1_router(state: &AppState, api_auth_configured: bool) -> Router<AppState> {
    let is_master = state.is_master();
    let openapi_spec: Arc<String> = super::openapi::build_openapi_spec(state.registry.services())
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "failed to serialize OpenAPI spec");
            Arc::new(String::from(r#"{"error":"spec generation failed"}"#))
        });
    let spec_for_route = openapi_spec;

    let mut v1 = Router::new().nest("/nodes", super::nodes::routes(state.clone()));

    if is_master {
        v1 = v1.route("/{service}/actions", get(service_actions));

        // upstream oauth must be nested before /gateway so its more-specific prefix wins;
        // only mount when the gateway manager is present (oauth requires it).
        if state.gateway_manager.is_some() {
            v1 = v1.nest(
                "/gateway/oauth",
                crate::api::upstream_oauth::gateway_routes(state.clone()),
            );
        }

        v1 = v1
            .nest("/acp", services::acp::routes(state.clone()))
            .nest("/gateway", services::gateway::routes(state.clone()))
            .route(
                "/openapi.json",
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
                "/docs",
                get(|| async { Html(include_str!("openapi_docs.html")) }),
            )
            // All v1 unauthenticated route groups (extract, marketplace,
            // doctor, setup) are gated by host_validation_layer — non-loopback
            // Host headers are rejected before reaching the dispatcher (DNS
            // rebinding mitigation for the v1 wizard, lab-bg3e.3.3).
            .nest(
                "/extract",
                services::extract::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest(
                "/marketplace",
                services::marketplace::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest(
                "/doctor",
                services::doctor::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest(
                "/setup",
                services::setup::routes(state.clone()).layer(axum::middleware::from_fn(
                    crate::api::host_validation::host_validation_layer,
                )),
            )
            .nest("/stash", services::stash::routes(state.clone()))
            .nest(
                "/auth/allowed-emails",
                services::auth_admin::routes(state.clone()),
            );

        if state
            .registry
            .services()
            .iter()
            .any(|service| service.name == "logs")
        {
            v1 = v1.nest("/logs", services::logs::routes(state.clone()));
        }

        #[cfg(feature = "fs")]
        if state
            .registry
            .services()
            .iter()
            .any(|service| service.name == "fs")
        {
            // SECURITY: fs operations read workspace files, so refuse to
            // mount them on an unauthenticated API surface. Static web UI
            // auth settings do not bypass `/v1` auth when bearer/OAuth auth
            // is configured.
            if state.web_ui_auth_disabled && !api_auth_configured {
                tracing::warn!(
                    subsystem = "startup",
                    phase = "fs.mount.skipped",
                    reason = "web_ui_auth_disabled",
                    "fs service is configured but LAB_WEB_UI_AUTH_DISABLED=true would expose workspace files unauthenticated; refusing to mount /v1/fs"
                );
            } else {
                v1 = v1.nest("/fs", services::fs::routes(state.clone()));
            }
        }
    }

    macro_rules! mount_if_enabled {
        ($v1:ident, $state:ident, $feat:literal, $name:literal, $mod:ident) => {
            #[cfg(feature = $feat)]
            if $state.registry.services().iter().any(|s| s.name == $name) {
                $v1 = $v1.nest(concat!("/", $name), services::$mod::routes($state.clone()));
            }
        };
        ($v1:ident, $state:ident, $feat:literal, $registry_name:literal, $route_name:literal, $mod:ident) => {
            #[cfg(feature = $feat)]
            if $state
                .registry
                .services()
                .iter()
                .any(|s| s.name == $registry_name)
            {
                $v1 = $v1.nest(
                    concat!("/", $route_name),
                    services::$mod::routes($state.clone()),
                );
            }
        };
    }

    if is_master {
        mount_if_enabled!(v1, state, "radarr", "radarr", radarr);
        mount_if_enabled!(v1, state, "sonarr", "sonarr", sonarr);
        mount_if_enabled!(v1, state, "prowlarr", "prowlarr", prowlarr);
        mount_if_enabled!(v1, state, "plex", "plex", plex);
        mount_if_enabled!(v1, state, "tautulli", "tautulli", tautulli);
        mount_if_enabled!(v1, state, "sabnzbd", "sabnzbd", sabnzbd);
        mount_if_enabled!(v1, state, "qbittorrent", "qbittorrent", qbittorrent);
        mount_if_enabled!(v1, state, "tailscale", "tailscale", tailscale);
        mount_if_enabled!(v1, state, "linkding", "linkding", linkding);
        mount_if_enabled!(v1, state, "memos", "memos", memos);
        mount_if_enabled!(v1, state, "beads", "beads", beads);
        mount_if_enabled!(v1, state, "bytestash", "bytestash", bytestash);
        mount_if_enabled!(v1, state, "paperless", "paperless", paperless);
        mount_if_enabled!(v1, state, "arcane", "arcane", arcane);
        mount_if_enabled!(v1, state, "unraid", "unraid", unraid);
        mount_if_enabled!(v1, state, "unifi", "unifi", unifi);
        mount_if_enabled!(v1, state, "overseerr", "overseerr", overseerr);
        mount_if_enabled!(v1, state, "gotify", "gotify", gotify);
        mount_if_enabled!(v1, state, "openacp", "openacp", openacp);
        mount_if_enabled!(v1, state, "openai", "openai", openai);
        mount_if_enabled!(v1, state, "notebooklm", "notebooklm", notebooklm);
        mount_if_enabled!(v1, state, "qdrant", "qdrant", qdrant);
        mount_if_enabled!(v1, state, "tei", "tei", tei);
        mount_if_enabled!(v1, state, "apprise", "apprise", apprise);
        mount_if_enabled!(v1, state, "dozzle", "dozzle", dozzle);
        mount_if_enabled!(v1, state, "immich", "immich", immich);
        mount_if_enabled!(v1, state, "jellyfin", "jellyfin", jellyfin);
        mount_if_enabled!(v1, state, "navidrome", "navidrome", navidrome);
        mount_if_enabled!(v1, state, "scrutiny", "scrutiny", scrutiny);
        mount_if_enabled!(v1, state, "freshrss", "freshrss", freshrss);
        mount_if_enabled!(v1, state, "loggifly", "loggifly", loggifly);
        mount_if_enabled!(v1, state, "adguard", "adguard", adguard);
        mount_if_enabled!(v1, state, "glances", "glances", glances);
        mount_if_enabled!(
            v1,
            state,
            "uptime_kuma",
            "uptime-kuma",
            "uptime-kuma",
            uptime_kuma
        );
        mount_if_enabled!(v1, state, "pihole", "pihole", pihole);
        mount_if_enabled!(v1, state, "neo4j", "neo4j", neo4j);
        // [lab-scaffold: api-routes]
    }

    v1
}

/// Build the `/v0.1` sub-router with registry REST endpoints.
///
/// Auth middleware is applied via `route_layer()` — same pattern as `/v1`.
#[cfg(feature = "mcpregistry")]
fn build_v0_1_router() -> Router<AppState> {
    Router::new().nest("/v0.1", services::registry_v01::routes())
}

// ── Dev mockup file server ────────────────────────────────────────────────
// Implements the Tier 1 serving model from docs/design/component-development.md §5.
// Serves self-contained HTML from ~/.superpowers/brainstorm/content/ at:
//   GET /dev/mockup          → newest .html file
//   GET /dev/mockup/{name}   → newest .html whose stem contains {name}
//
// Rules (enforced by the doc — do not violate):
//   • These functions MUST live in router.rs alongside dev_marketplace_readonly.
//     The other Claude session strips dev-tooling code from web.rs.
//   • MUST NOT delegate to serve_web_request — that serves the Next.js SPA.
//   • Routes MUST be registered before the static-file fallback.

fn dev_mockup_dir() -> std::path::PathBuf {
    crate::config::home_dir()
        .map(|h| h.join(".superpowers/brainstorm/content"))
        .unwrap_or_else(|| std::path::PathBuf::from(".superpowers/brainstorm/content"))
}

fn dev_mockup_newest(fragment: Option<&str>) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dev_mockup_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("html"))
        .filter(|e| {
            fragment.is_none_or(|n| {
                e.path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.contains(n))
            })
        })
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (e.path(), t))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
}

fn dev_mockup_response(fragment: Option<&str>) -> axum::response::Response {
    use axum::response::Html;
    match dev_mockup_newest(fragment) {
        None => {
            // Escape the fragment before embedding it in HTML to prevent XSS.
            // The name comes from a URL path segment and is user-controlled.
            let escaped = fragment
                .map(|n| {
                    format!(
                        " '{}'",
                        n.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                            .replace('"', "&quot;")
                    )
                })
                .unwrap_or_default();
            Html(format!(
                "<p style='font-family:sans-serif;padding:2rem'>No{escaped} mockup found in \
                 <code>~/.superpowers/brainstorm/content/</code></p>"
            ))
            .into_response()
        }
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(html) => Html(html).into_response(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read dev mockup");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
    }
}

async fn dev_mockup() -> axum::response::Response {
    dev_mockup_response(None)
}

async fn dev_mockup_named(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    dev_mockup_response(Some(&name))
}

// GET /dev/api/nodeinfo — unauthenticated, read-only.
// Returns config.toml values + ~/.lab/.env contents (secrets masked) so the
// setup wizard can pre-populate all fields without requiring a bearer token.
async fn dev_nodeinfo(State(state): State<AppState>) -> axum::response::Response {
    use axum::Json;

    let local_host =
        crate::node::identity::resolve_local_hostname().unwrap_or_else(|_| "local".into());
    let master_url = state
        .config
        .deploy
        .as_ref()
        .and_then(|d| d.defaults.as_ref())
        .and_then(|d| d.master_url.clone())
        .unwrap_or_default();
    let controller = state
        .config
        .node
        .as_ref()
        .and_then(|n| n.controller.clone())
        .unwrap_or_else(|| local_host.clone());

    // dotenvy already loaded ~/.lab/.env at startup, so everything is in std::env.
    // The UI treats MASKED_SECRET as "value already set — leave blank to keep current value".
    const MASKED_SECRET: &str = "***";
    let secret_suffixes = [
        // Deny-list for secret detection. Add new suffixes here when new secret
        // naming conventions are introduced (e.g. LAB_AUTH_SIGNING_KEY).
        // NOTE: `_KEY` intentionally covers `_API_KEY` and future signing-key vars.
        //       `_SECRET` covers `_CLIENT_SECRET` — the more-specific entry is omitted.
        "_KEY", // covers _API_KEY, _SIGNING_KEY, _HMAC_KEY, etc.
        "_TOKEN",
        "_PASSWORD",
        "_SECRET", // covers _CLIENT_SECRET
    ];
    let service_prefixes = [
        "RADARR_",
        "SONARR_",
        "PROWLARR_",
        "PLEX_",
        "TAUTULLI_",
        "OVERSEERR_",
        "SABNZBD_",
        "QBITTORRENT_",
        "UNRAID_",
        "UNIFI_",
        "TAILSCALE_",
        "ARCANE_",
        "LINKDING_",
        "MEMOS_",
        "PAPERLESS_",
        "BYTESTASH_",
        "GOTIFY_",
        "APPRISE_",
        "OPENAI_",
        "QDRANT_",
        "TEI_",
        "LAB_MCP_HTTP_",
        "LAB_LOG",
        "LAB_AUTH_",
        "LAB_PUBLIC_URL",
        "LAB_GOOGLE_",
    ];
    let mut env_values = serde_json::Map::new();
    for (key, val) in std::env::vars() {
        if val.is_empty() {
            continue;
        }
        if !service_prefixes.iter().any(|p| key.starts_with(p)) {
            continue;
        }
        let masked = secret_suffixes.iter().any(|s| key.ends_with(s));
        let display = if masked {
            MASKED_SECRET.to_string()
        } else {
            val
        };
        env_values.insert(key, serde_json::Value::String(display));
    }

    Json(serde_json::json!({
        "local_host": local_host,
        "controller": controller,
        "master_url": master_url,
        "env": env_values,
    }))
    .into_response()
}

async fn dev_marketplace_readonly(
    headers: axum::http::HeaderMap,
    Json(req): Json<crate::api::ActionRequest>,
) -> Result<Json<serde_json::Value>, ToolError> {
    let action = req.action.trim().to_string();
    if !DEV_MARKETPLACE_READ_ACTIONS.contains(&action.as_str()) {
        return Err(ToolError::Sdk {
            sdk_kind: "dev_preview_read_only".to_string(),
            message: format!("dev preview route blocked mutating marketplace action `{action}`"),
        });
    }

    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    services::helpers::handle_action(
        "marketplace",
        "api",
        request_id,
        req,
        crate::dispatch::marketplace::actions(),
        |action, params| async move {
            crate::dispatch::marketplace::dispatch_with_port(
                &action,
                params,
                &services::marketplace::WsNodeRpcPort,
            )
            .await
        },
    )
    .await
}

#[allow(clippy::too_many_lines)]
pub fn build_router(
    mut state: AppState,
    bearer_token: Option<String>,
    auth_state: Option<lab_auth::state::AuthState>,
    mcp_router: Option<Router<AppState>>,
    config_cors_origins: &[String],
) -> Router {
    if let Some(ref auth_state) = auth_state {
        state = state.with_oauth_state(auth_state.clone());
    }
    if let Some(auth_state) = auth_state.as_ref() {
        auth_state.set_allowed_resource_scopes(
            state
                .config
                .protected_mcp_routes
                .iter()
                .filter(|route| route.enabled)
                .map(|route| (route.public_resource(), route.scopes.clone())),
        );
    }
    let static_token = bearer_token.map(Arc::<str>::from);
    state = state.with_bearer_token(static_token.clone());
    let auth_state = auth_state.map(Arc::new);
    let needs_auth = static_token.is_some() || auth_state.is_some();
    if !needs_auth {
        tracing::warn!(
            "HTTP API started without bearer token or OAuth auth state — all protected routes are unprotected"
        );
    }

    let v1 = build_v1_router(&state, needs_auth);

    let x_request_id = HeaderName::from_static("x-request-id");

    // Build separate protected sub-routers so `/v1/*` can accept browser
    // sessions while `/mcp` remains token-authenticated only.
    let v1_router = Router::new().nest("/v1", v1);
    let is_master = state.is_master();
    let resource_url: Option<Arc<str>> = auth_state
        .as_ref()
        .and_then(|state| state.config.public_url.as_ref().map(url::Url::as_str))
        .or_else(|| {
            state
                .auth_config
                .as_ref()
                .and_then(|cfg| cfg.public_url.as_ref().map(url::Url::as_str))
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
            .with_allow_session_cookie(allow_session_cookie);
        layer
    };
    let v1_protected = if needs_auth {
        v1_router.route_layer(make_auth_layer(true))
    } else {
        v1_router
    };

    #[cfg(feature = "mcpregistry")]
    let v0_1_protected = {
        let v0_1_router = build_v0_1_router();
        if needs_auth {
            v0_1_router.route_layer(make_auth_layer(true))
        } else {
            v0_1_router
        }
    };

    let auth_state_for_mcp = auth_state.clone();
    let static_token_for_mcp = static_token.clone();
    let actor_key_deriver_for_mcp = state.actor_key_deriver.clone();
    let resource_url_for_mcp = resource_url.clone();
    let mcp_protected = mcp_router.map(|mcp| {
        let protected_manager_present = state.gateway_manager.is_some() && auth_state.is_some();
        let mcp = if needs_auth {
            mcp.route_layer(axum::middleware::from_fn(
                move |request: Request<Body>, next: Next| {
                    authenticate_request(
                        request,
                        next,
                        static_token_for_mcp.clone(),
                        auth_state_for_mcp.clone(),
                        actor_key_deriver_for_mcp.clone(),
                        resource_url_for_mcp.clone(),
                        false,
                    )
                },
            ))
        } else {
            mcp
        };
        if protected_manager_present {
            mcp.route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                protected_mcp_intercept,
            ))
        } else {
            mcp
        }
    });

    // Build the outer router: health probes + discovery (no auth) + protected routes (auth).
    // Layers apply bottom-up: last .layer() call = outermost middleware.
    // Desired execution order (outermost → innermost → handler):
    //   SetRequestId → TraceLayer → PropagateRequestId → Timeout → Compression → CORS → handler
    let mut router = Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready))
        // POST /v1/nodes/hello is self-registration — exempt from bearer auth.
        .nest("/v1/nodes", super::nodes::public_routes(state.clone()))
        // Backward-compat alias for pre-rename self-registration clients.
        .nest("/v1/fleet", super::nodes::public_routes(state.clone()))
        // GET /v1/nodes/ws is outside bearer-auth middleware by design.
        // The `initialize` JSON-RPC method performs enrollment-token validation; all
        // subsequent node methods require an active session. See docs/runtime/FLEET_METHODS.md.
        .route(
            "/v1/nodes/ws",
            get(crate::api::nodes::fleet::websocket_upgrade),
        )
        // Backward-compat alias for pre-rename websocket clients.
        .route(
            "/v1/fleet/ws",
            get(crate::api::nodes::fleet::websocket_upgrade),
        )
        .merge(v1_protected);
    #[cfg(feature = "mcpregistry")]
    {
        router = router.merge(v0_1_protected);
    }
    if is_master {
        router = router
            .merge(crate::api::upstream_oauth::browser_routes(state.clone()))
            .merge(crate::api::upstream_oauth::well_known_routes(state.clone()));
    }
    if let Some(mcp) = mcp_protected.filter(|_| is_master) {
        router = router.merge(mcp);
    }
    if is_master && let Some(auth_state) = auth_state.as_ref() {
        let _ = auth_state;
        router = router
            .route(
                "/.well-known/oauth-authorization-server",
                get(auth_authorization_server_metadata),
            )
            .route(
                "/.well-known/oauth-authorization-server/{*route}",
                get(auth_authorization_server_metadata),
            )
            .route(
                "/.well-known/oauth-protected-resource/{*route}",
                get(protected_route_resource_metadata),
            )
            .route(
                "/.well-known/oauth-protected-resource",
                get(auth_protected_resource_metadata),
            )
            .route("/jwks", get(auth_jwks))
            .route("/register", post(auth_register))
            .route("/authorize", get(auth_authorize))
            .route("/auth/login", get(auth_browser_login))
            .route(
                "/auth/session",
                get(crate::api::browser_session::auth_session),
            )
            .route(
                "/auth/logout",
                post(crate::api::browser_session::auth_logout),
            )
            .route("/auth/google/callback", get(auth_callback))
            .route("/token", post(auth_token));
    }

    // Dev routes — registered BEFORE the Next.js static fallback so they win
    // over the SPA. See docs/design/component-development.md §5 (two-tier
    // serving model) for the full rationale.
    //
    // /dev/api/*                  → read-only dispatch endpoints (marketplace guard, nodeinfo)
    // /dev/mockup, /dev/mockup/*  → Tier 1 mockup file server: serves HTML from
    //                     ~/.superpowers/brainstorm/content/{name}.html directly.
    //                     Keep this out of `/dev` so real Next.js dev pages can render.
    let dev_routes = Router::new()
        .route("/dev/api/marketplace", post(dev_marketplace_readonly))
        .route("/dev/api/nodeinfo", get(dev_nodeinfo))
        // Mockup page routes — MUST stay before the static fallback (docs/design/component-development.md §5)
        .route("/dev/mockup", get(dev_mockup))
        .route("/dev/mockup/", get(dev_mockup))
        .route("/dev/mockup/{name}", get(dev_mockup_named))
        .route("/dev/mockup/{name}/", get(dev_mockup_named));
    let dev_routes = if needs_auth {
        dev_routes.route_layer(make_auth_layer(true))
    } else {
        dev_routes
    };
    router = router.merge(dev_routes);

    // Static-file fallback for the Next.js SPA. Protected MCP proxying is an
    // outer middleware below so it bypasses the API timeout/compression stack.
    if state.web_assets_enabled() {
        router = router.fallback(crate::api::web::serve_web_request);
    }

    let protected_proxy_state = state.clone();
    router
        .with_state(state)
        .layer(build_cors_layer(config_cors_origins))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(30),
        ))
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
        .layer(axum::middleware::from_fn_with_state(
            protected_proxy_state,
            protected_mcp_intercept,
        ))
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
/// - `LAB_CORS_ORIGINS` env var (comma-separated `scheme://host[:port]`)
/// - `api.cors_origins` in config.toml (array of strings)
///
/// Always includes `http://localhost`, `http://127.0.0.1`, and `http://[::1]`
/// as safe loopback defaults.
fn build_cors_layer(config_origins: &[String]) -> CorsLayer {
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::{HeaderValue, Method};

    // Env var overrides config.toml when present.
    let raw_origins: Vec<String> = match std::env::var("LAB_CORS_ORIGINS") {
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
    // Dev ports (3000/5173/8080) are gated behind LAB_DEV_MODE=1 to prevent
    // a malicious npm postinstall HTTP server (or rogue browser extension on
    // those origins) from reading Setup API responses on a v1 unauthed lab
    // (lab-bg3e.3 security hardening).
    let dev_mode_enabled = std::env::var("LAB_DEV_MODE").as_deref() == Ok("1");
    if dev_mode_enabled {
        // One-shot WARN at startup so an operator who has LAB_DEV_MODE=1 in
        // their shell rc can see the wider CORS surface in production logs.
        tracing::warn!(
            subsystem = "api_server",
            phase = "cors.dev_mode_enabled",
            "LAB_DEV_MODE=1 — additional CORS origins enabled (3000/5173/8080); unset for production"
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
            HeaderName::from_static(lab_auth::session::BROWSER_CSRF_HEADER_NAME),
        ])
}

async fn service_actions(
    State(state): State<AppState>,
    axum::extract::Path(service): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ToolError> {
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

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
                    .uri("/v1/extract/actions")
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
    async fn auth_layer_rejects_missing_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        // /v1/extract/actions is behind bearer auth; /health is NOT (lab-3qn.5).
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
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

    #[tokio::test]
    async fn auth_layer_accepts_valid_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        // Confirm that a valid token reaches the protected /v1 route.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_layer_accepts_case_insensitive_bearer_token() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
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
                    .uri("/v1/extract/actions")
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

    /// When a service is absent from the runtime registry (e.g. filtered out by
    /// `--services`), its `/v1/<service>` routes must NOT be mounted — even if
    /// the feature flag for that service is compiled in.
    ///
    /// This test uses an empty registry to simulate `labby serve --services <other>`
    /// excluding `radarr`, then verifies that `POST /v1/radarr` returns 404 rather
    /// than reaching the handler.
    #[cfg(feature = "radarr")]
    #[tokio::test]
    async fn service_filtered_from_registry_has_no_http_route() {
        use crate::registry::ToolRegistry;

        // An empty registry = no services enabled at runtime.
        let registry = ToolRegistry::new();
        let state = AppState::from_registry(registry);
        let app = build_router_with_bearer(state, None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/radarr")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"help"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "radarr routes must not be mounted when radarr is absent from the runtime registry"
        );
    }

    #[cfg(feature = "uptime_kuma")]
    #[tokio::test]
    async fn uptime_kuma_route_uses_registry_service_name() {
        let mut state = AppState::new();
        state.clients = Arc::new(crate::dispatch::clients::ServiceClients {
            uptime_kuma: Some(Arc::new(
                lab_apis::uptime_kuma::UptimeKumaClient::new(
                    "http://127.0.0.1:3001",
                    lab_apis::core::Auth::None,
                )
                .unwrap(),
            )),
            ..Default::default()
        });
        let app = build_router_with_bearer(state, None, None);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/uptime-kuma")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"contract.status","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/uptime_kuma")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"contract.status","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bearer_mode_still_accepts_lab_mcp_http_token() {
        let state = AppState::new();
        let app = build_router(state, Some("secret-token".into()), None, None, &[]);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
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
                    .uri("/v1/extract/actions")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
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
                    .uri("/v1/extract/actions")
                    .header(
                        header::COOKIE,
                        format!(
                            "{}={}",
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
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
                    .uri("/v1/extract/actions")
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(lab_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
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
                            lab_auth::session::BROWSER_SESSION_COOKIE_NAME,
                            session.session_id
                        ),
                    )
                    .header(lab_auth::session::BROWSER_CSRF_HEADER_NAME, "csrf-123")
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
                    .uri("/v1/extract/actions")
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
    async fn protected_mcp_route_metadata_uses_host_and_path_resource() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let config =
            protected_route_config("syslog", "mcp.tootie.tv", "/syslog", "http://10.0.0.2:3100");
        manager.seed_config(config.clone()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/.well-known/oauth-protected-resource/syslog")
                    .header(header::HOST, "mcp.tootie.tv")
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
        assert_eq!(json["resource"], "https://mcp.tootie.tv/syslog");
    }

    #[tokio::test]
    async fn protected_mcp_route_metadata_compatibility_alias_matches_resource() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let config =
            protected_route_config("syslog", "mcp.tootie.tv", "/syslog", "http://10.0.0.2:3100");
        manager.seed_config(config.clone()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/syslog/.well-known/oauth-protected-resource")
                    .header(header::HOST, "mcp.tootie.tv")
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
        assert_eq!(json["resource"], "https://mcp.tootie.tv/syslog");
    }

    #[tokio::test]
    async fn protected_mcp_route_unauthorized_header_points_to_route_metadata() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let config =
            protected_route_config("syslog", "mcp.tootie.tv", "/syslog", "http://10.0.0.2:3100");
        manager.seed_config(config.clone()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let app = build_router(state, None, Some(auth_state), None, &[]);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/syslog")
                    .header(header::HOST, "mcp.tootie.tv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer resource_metadata=\"https://mcp.tootie.tv/.well-known/oauth-protected-resource/syslog\", scope=\"mcp:read mcp:write\""
        );
    }

    #[tokio::test]
    async fn protected_mcp_route_proxies_with_route_audience_token() {
        let backend = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"jsonrpc":"2.0","result":{}}"#),
            )
            .mount(&backend)
            .await;

        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let config = protected_route_config("syslog", "mcp.tootie.tv", "/syslog", &backend.uri());
        manager.seed_config(config.clone()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://mcp.tootie.tv/syslog");
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
                    .uri("/syslog")
                    .header(header::HOST, "mcp.tootie.tv")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"ping"}"#))
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
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let config = crate::config::LabConfig {
            upstream: vec![crate::config::UpstreamConfig {
                name: "axon".to_string(),
                enabled: true,
                url: Some(format!("{}/mcp", backend.uri())),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            }],
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: "axon".to_string(),
                enabled: true,
                public_host: "mcp.tootie.tv".to_string(),
                public_path: "/axon".to_string(),
                upstream: Some("axon".to_string()),
                backend_url: String::new(),
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
                health_path: None,
            }],
            ..crate::config::LabConfig::default()
        };
        manager.seed_config(config.clone()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://mcp.tootie.tv/axon");
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
                    .header(header::HOST, "mcp.tootie.tv")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"ping"}"#))
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
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
        let config = protected_route_config("syslog", "syslog.tootie.tv", "/mcp", &backend.uri());
        manager.seed_config(config.clone()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let auth_state = test_lab_auth_state().await;
        let token = issue_test_route_token(&auth_state, "https://syslog.tootie.tv/mcp");
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
                    .header(header::HOST, "syslog.tootie.tv")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"jsonrpc":"2.0","method":"ping"}"#))
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

    #[tokio::test]
    async fn gateway_oauth_routes_require_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
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

    #[tokio::test]
    async fn browser_oauth_callback_bypasses_bearer_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            tempdir.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ));
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
                    .uri("/v1/extract/actions")
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
                    .uri("/v1/extract/actions")
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

    async fn test_lab_auth_state() -> lab_auth::state::AuthState {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let config = lab_auth::config::AuthConfig {
            mode: lab_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("bootstrap-secret".to_string()),
            google: lab_auth::config::GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            ..lab_auth::config::AuthConfig::default()
        };
        lab_auth::state::AuthState::new(config).await.unwrap()
    }

    fn issue_test_lab_token(auth_state: &lab_auth::state::AuthState) -> String {
        issue_test_token(auth_state, "https://lab.example.com/mcp", "lab")
    }

    fn issue_test_route_token(auth_state: &lab_auth::state::AuthState, audience: &str) -> String {
        issue_test_token(auth_state, audience, "mcp:read mcp:write")
    }

    fn issue_test_token(
        auth_state: &lab_auth::state::AuthState,
        audience: &str,
        scope: &str,
    ) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        auth_state
            .signing_keys
            .issue_access_token(&lab_auth::jwt::AccessClaims {
                iss: "https://lab.example.com".to_string(),
                sub: "google-user".to_string(),
                aud: audience.to_string(),
                exp: now + 3600,
                iat: now,
                jti: "test-jti".to_string(),
                scope: scope.to_string(),
                azp: "client".to_string(),
            })
            .unwrap()
    }

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
            }],
            ..crate::config::LabConfig::default()
        }
    }

    async fn seed_browser_session(
        auth_state: &lab_auth::state::AuthState,
    ) -> lab_auth::types::BrowserSessionRow {
        let session = lab_auth::types::BrowserSessionRow {
            session_id: "sess-123".to_string(),
            subject: "browser-user".to_string(),
            email: Some("browser@example.com".to_string()),
            csrf_token: "csrf-123".to_string(),
            created_at: 1,
            expires_at: i64::MAX,
        };
        auth_state
            .store
            .upsert_browser_session(session.clone())
            .await
            .unwrap();
        session
    }

    /// `/v0.1/servers` requires bearer auth — unauthenticated requests must get 401,
    /// authenticated requests must reach the handler (200 or 503 if store uninitialized).
    #[cfg(feature = "mcpregistry")]
    #[tokio::test]
    async fn v01_servers_requires_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        // Unauthenticated → 401
        let unauth_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v0.1/servers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            unauth_response.status(),
            StatusCode::UNAUTHORIZED,
            "/v0.1/servers must reject unauthenticated requests"
        );
        let body = axum::body::to_bytes(unauth_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "auth_failed");

        // Authenticated → reaches handler (200 OK or 503 if store not initialized)
        let auth_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v0.1/servers")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = auth_response.status();
        assert!(
            status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
            "/v0.1/servers with valid token must return 200 or 503 (store not initialized), got {status}"
        );
        if status == StatusCode::SERVICE_UNAVAILABLE {
            let body = axum::body::to_bytes(auth_response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], "service_unavailable");
        }
    }

    /// POST /dev/api/marketplace must accept whitelisted read-only actions after auth.
    #[tokio::test]
    async fn dev_marketplace_allows_whitelisted_read_actions() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        for action in DEV_MARKETPLACE_READ_ACTIONS {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/dev/api/marketplace")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, "Bearer secret-token")
                        .body(Body::from(
                            serde_json::to_string(&serde_json::json!({ "action": action }))
                                .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();

            // 200 OK or 4xx from dispatch (action not implemented in test env) — never 403
            assert_ne!(
                response.status(),
                StatusCode::FORBIDDEN,
                "read-only action `{action}` must not be blocked by dev guard"
            );
        }
    }

    /// POST /dev/api/marketplace must block mutating actions after auth.
    #[tokio::test]
    async fn dev_marketplace_blocks_mutating_actions() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        for action in &[
            "plugin.install",
            "plugin.uninstall",
            "sources.add",
            "sources.remove",
            "plugin.workspace.save",
            "plugin.deploy",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/dev/api/marketplace")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, "Bearer secret-token")
                        .body(Body::from(
                            serde_json::to_string(&serde_json::json!({ "action": action }))
                                .unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "mutating action `{action}` must be blocked by dev guard"
            );
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], "dev_preview_read_only");
        }
    }

    /// POST /dev/api/marketplace must require auth when auth is configured.
    #[tokio::test]
    async fn dev_marketplace_requires_auth_when_configured() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dev/api/marketplace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"plugin.install"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "/dev/api/marketplace must use auth middleware when auth is configured"
        );
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

    /// POST /dev/api/marketplace remains open in explicit no-auth local mode.
    #[tokio::test]
    async fn dev_marketplace_allows_no_auth_when_server_has_no_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, None, None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dev/api/marketplace")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"plugin.install"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "no-auth local mode should still reach the read-only dev guard"
        );
    }
}
