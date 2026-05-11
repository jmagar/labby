use axum::{
    Json, Router,
    extract::{Extension, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::api::state::AppState;
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::redact::redact_url;

pub fn gateway_routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/upstreams", get(upstreams))
        .route("/probe", post(probe))
        .route("/start", post(start))
        .route("/status", get(status))
        .route("/clear", post(clear))
}

pub fn browser_routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/auth/upstream/callback", get(callback))
        .route("/gateway/oauth/result", get(result_page))
}

/// Route group for `/.well-known/oauth-client` — the CIMD metadata document.
///
/// Mounted unconditionally; returns 404 when upstream OAuth is not configured.
pub fn well_known_routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/.well-known/oauth-client", get(oauth_client_metadata))
}

/// Serve the OAuth Client ID Metadata Document (RFC 9728 / MCP OAuth 2.1).
///
/// Authorization servers that support the Client ID Metadata Document approach
/// (e.g. Cloudflare MCP) fetch this document when they receive an authorization
/// request whose `client_id` is a URL. The lab server uses the URL of this
/// endpoint as its `client_id` for upstreams that do not support RFC 7591
/// dynamic client registration.
async fn oauth_client_metadata(State(state): State<AppState>) -> impl IntoResponse {
    let redirect_uri = state
        .gateway_manager
        .as_ref()
        .and_then(|m| m.oauth_redirect_uri());
    let Some(redirect_uri) = redirect_uri else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let client_id = url::Url::parse(&redirect_uri)
        .ok()
        .map(|mut u| {
            u.set_path("/.well-known/oauth-client");
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        })
        .unwrap_or_default();

    let doc = serde_json::json!({
        "client_id": client_id,
        "client_name": "lab",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "code_challenge_method": "S256"
    });

    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Json(doc),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ProbeRequest {
    url: String,
    #[serde(default)]
    upstream: Option<String>,
    confirm: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct StartRequest {
    upstream: String,
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    upstream: String,
}

#[derive(Debug, Deserialize)]
struct ClearQuery {
    upstream: String,
    confirm: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: String,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResultQuery {
    upstream: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct StartResponse {
    authorization_url: String,
}

#[derive(Debug, Serialize)]
struct UpstreamEntry {
    name: String,
}

async fn upstreams(
    State(state): State<AppState>,
    Extension(auth): Extension<crate::api::oauth::AuthContext>,
) -> Result<Json<Vec<UpstreamEntry>>, ToolError> {
    require_master(&state)?;
    require_admin_scope(&auth, "upstreams")?;
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::internal_message("gateway manager not wired"))?;
    let configs = manager.oauth_upstream_configs().await;
    Ok(Json(
        configs
            .into_iter()
            .map(|c| UpstreamEntry { name: c.name })
            .collect(),
    ))
}

fn require_master(state: &AppState) -> Result<(), ToolError> {
    if state.is_master() {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "upstream oauth routes are master-only".to_string(),
        })
    }
}

fn require_admin_scope(
    auth: &crate::api::oauth::AuthContext,
    action: &str,
) -> Result<(), ToolError> {
    if auth.scopes.iter().any(|scope| scope == "lab:admin") {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "upstream_oauth",
        action,
        subject = %auth.sub,
        kind = "forbidden",
        "upstream oauth action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("upstream oauth `{action}` requires `lab:admin` scope"),
    })
}

async fn callback_subject(
    state: &AppState,
    auth: Option<crate::api::oauth::AuthContext>,
    headers: &HeaderMap,
) -> Result<String, ToolError> {
    if let Some(auth) = auth {
        return Ok(auth.sub);
    }

    if let Some(auth_state) = state.oauth_state.as_ref()
        && let Some(session_id) =
            lab_auth::session::read_cookie(headers, lab_auth::session::BROWSER_SESSION_COOKIE_NAME)
    {
        let session = auth_state
            .store
            .find_browser_session(&session_id)
            .await
            .map_err(|error| {
                ToolError::internal_message(format!("browser session lookup failed: {error}"))
            })?;
        if let Some(session) = session {
            return Ok(session.subject);
        }
    }

    Err(ToolError::Sdk {
        sdk_kind: "oauth_state_invalid".to_string(),
        message: "oauth_state_invalid: authenticated browser session required".to_string(),
    })
}

fn public_url(state: &AppState) -> Result<&url::Url, ToolError> {
    state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
        .ok_or_else(|| ToolError::internal_message("LAB_PUBLIC_URL is required for upstream oauth"))
}

fn append_public_path(base: &url::Url, path: &str) -> Result<url::Url, ToolError> {
    let mut url = base.clone();
    let base_path = url.path().trim_end_matches('/');
    let path = path.trim_start_matches('/');
    let next = if base_path.is_empty() {
        format!("/{path}")
    } else {
        format!("{base_path}/{path}")
    };
    url.set_path(&next);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn probe(
    State(state): State<AppState>,
    Extension(auth): Extension<crate::api::oauth::AuthContext>,
    Json(body): Json<ProbeRequest>,
) -> Result<Json<crate::dispatch::gateway::oauth::ProbeResult>, ToolError> {
    let started = std::time::Instant::now();
    require_master(&state)?;
    require_admin_scope(&auth, "probe")?;
    if body.confirm != Some(true) {
        return Err(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "set confirm=true to probe and prepare upstream oauth".to_string(),
        });
    }
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::internal_message("gateway manager not wired"))?;
    let result = crate::dispatch::gateway::oauth::probe_for_upstream(
        &manager,
        &body.url,
        body.upstream.as_deref(),
    )
    .await
    .inspect_err(|error| {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "probe",
            subject = %auth.sub,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            "upstream oauth probe failed"
        );
    })?;
    info!(
        surface = "api",
        service = "upstream_oauth",
        action = "probe",
        subject = %auth.sub,
        elapsed_ms = started.elapsed().as_millis(),
        url = %redact_url(&body.url),
        oauth_discovered = result.oauth_discovered,
        "upstream oauth probe completed"
    );
    Ok(Json(result))
}

async fn start(
    State(state): State<AppState>,
    Extension(auth): Extension<crate::api::oauth::AuthContext>,
    Json(body): Json<StartRequest>,
) -> Result<Json<StartResponse>, ToolError> {
    let started = std::time::Instant::now();
    require_master(&state)?;
    require_admin_scope(&auth, "start")?;
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::internal_message("gateway manager not wired"))?;
    // Pre-flight: verify SQLite is configured before redirecting the user to
    // the authorization server. Without this check, a misconfigured deployment
    // sends the user off-site only to fail at the callback with "oauth sqlite
    // not configured" — after the user has already left the page.
    if manager.oauth_sqlite().is_none() {
        return Err(ToolError::internal_message(
            "upstream OAuth not configured (missing SQLite store)",
        ));
    }
    let begin = crate::dispatch::gateway::oauth::begin_authorization(
        &manager,
        &body.upstream,
        SHARED_GATEWAY_OAUTH_SUBJECT,
    )
    .await
    .inspect_err(|error| {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "start",
            subject = %auth.sub,
            oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            "upstream oauth start failed"
        );
    })?;
    info!(
        surface = "api",
        service = "upstream_oauth",
        action = "start",
        subject = %auth.sub,
        oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
        elapsed_ms = started.elapsed().as_millis(),
        upstream = %body.upstream,
        "upstream oauth authorization started"
    );
    Ok(Json(StartResponse {
        authorization_url: begin.authorization_url,
    }))
}

async fn status(
    State(state): State<AppState>,
    Query(query): Query<StatusQuery>,
    Extension(auth): Extension<crate::api::oauth::AuthContext>,
) -> Result<Json<crate::dispatch::gateway::oauth::UpstreamOauthStatusView>, ToolError> {
    let started = std::time::Instant::now();
    require_master(&state)?;
    require_admin_scope(&auth, "status")?;
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::internal_message("gateway manager not wired"))?;
    let status = crate::dispatch::gateway::oauth::status(
        &manager,
        &query.upstream,
        SHARED_GATEWAY_OAUTH_SUBJECT,
    )
    .await
    .inspect_err(|error| {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "status",
            subject = %auth.sub,
            oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            "upstream oauth status failed"
        );
    })?;
    info!(
        surface = "api",
        service = "upstream_oauth",
        action = "status",
        subject = %auth.sub,
        oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
        elapsed_ms = started.elapsed().as_millis(),
        upstream = %query.upstream,
        "upstream oauth status retrieved"
    );
    Ok(Json(status))
}

async fn clear(
    State(state): State<AppState>,
    Query(query): Query<ClearQuery>,
    Extension(auth): Extension<crate::api::oauth::AuthContext>,
) -> impl IntoResponse {
    let started = std::time::Instant::now();
    if let Err(error) = require_master(&state) {
        return error.into_response();
    }
    if let Err(error) = require_admin_scope(&auth, "clear") {
        return error.into_response();
    }
    if query.confirm != Some(true) {
        return ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "set ?confirm=true to clear upstream oauth credentials".to_string(),
        }
        .into_response();
    }
    let manager = match state.gateway_manager.clone() {
        Some(manager) => manager,
        None => return ToolError::internal_message("gateway manager not wired").into_response(),
    };
    if let Err(error) = crate::dispatch::gateway::oauth::clear(
        &manager,
        &query.upstream,
        SHARED_GATEWAY_OAUTH_SUBJECT,
    )
    .await
    {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "clear",
            subject = %auth.sub,
            oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
            elapsed_ms = started.elapsed().as_millis(),
            kind = error.kind(),
            "upstream oauth clear failed"
        );
        return error.into_response();
    }
    info!(
        surface = "api",
        service = "upstream_oauth",
        action = "clear",
        subject = %auth.sub,
        oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
        elapsed_ms = started.elapsed().as_millis(),
        upstream = %query.upstream,
        "upstream oauth credentials cleared"
    );
    (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
}

async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
    auth: Option<Extension<crate::api::oauth::AuthContext>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let started = std::time::Instant::now();
    if let Err(error) = require_master(&state) {
        return error.into_response();
    }

    let manager = match state.gateway_manager.clone() {
        Some(manager) => manager,
        None => return ToolError::internal_message("gateway manager not wired").into_response(),
    };
    let base = match public_url(&state) {
        Ok(url) => url.clone(),
        Err(error) => return error.into_response(),
    };
    let mut redirect_url = match append_public_path(&base, "/gateway/oauth/result") {
        Ok(url) => url,
        Err(error) => return error.into_response(),
    };

    // Recover (upstream, subject) from the state token — the AS only sends back
    // `code` and `state`, so we can't require `upstream` as a query param.
    let sqlite = match manager.oauth_sqlite() {
        Some(s) => s,
        None => return ToolError::internal_message("oauth sqlite not configured").into_response(),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (upstream, state_subject) = match sqlite
        .find_upstream_oauth_state_owner(&query.state, now)
        .await
    {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            warn!(
                surface = "api",
                service = "upstream_oauth",
                action = "callback",
                "upstream oauth callback: state token not found or expired"
            );
            return ToolError::Sdk {
                sdk_kind: "auth_failed".to_string(),
                message: "OAuth state token not found or expired".to_string(),
            }
            .into_response();
        }
        Err(e) => {
            return ToolError::internal_message(format!("state lookup failed: {e}"))
                .into_response();
        }
    };

    if state_subject != SHARED_GATEWAY_OAUTH_SUBJECT {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "callback",
            upstream = %upstream,
            state_subject = %state_subject,
            expected_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
            "upstream oauth callback rejected: state subject mismatch"
        );
        return ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "OAuth state subject mismatch".to_string(),
        }
        .into_response();
    }

    // If a browser session is present, verify it matches the subject from state.
    if let Ok(session_subject) = callback_subject(&state, auth.map(|e| e.0), &headers).await {
        if session_subject != SHARED_GATEWAY_OAUTH_SUBJECT {
            warn!(
                surface = "api",
                service = "upstream_oauth",
                action = "callback",
                upstream = %upstream,
                oauth_subject = SHARED_GATEWAY_OAUTH_SUBJECT,
                state_subject = %state_subject,
                "upstream oauth callback: session subject mismatch"
            );
        }
    }

    if let Some(auth_error) = &query.error {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "callback",
            upstream = %upstream,
            error = %auth_error,
            error_description = ?query.error_description,
            "upstream oauth callback received error from authorization server"
        );
        if let Err(revoke_err) = sqlite
            .delete_upstream_oauth_state_by_csrf(&query.state, now)
            .await
        {
            warn!(
                surface = "api",
                service = "upstream_oauth",
                action = "callback",
                "failed to revoke oauth state token after authorization error: {revoke_err}"
            );
        }
        redirect_url
            .query_pairs_mut()
            .append_pair("upstream", &upstream)
            .append_pair("status", "fail")
            .append_pair("error_kind", "authorization_failed");
        return Redirect::to(redirect_url.as_str()).into_response();
    }

    let code = match &query.code {
        Some(code) => code,
        None => {
            if let Err(revoke_err) = sqlite
                .delete_upstream_oauth_state_by_csrf(&query.state, now)
                .await
            {
                warn!(
                    surface = "api",
                    service = "upstream_oauth",
                    action = "callback",
                    "failed to revoke oauth state token after malformed callback: {revoke_err}"
                );
            }
            return ToolError::InvalidParam {
                message: "Callback missing code parameter".to_string(),
                param: "code".to_string(),
            }
            .into_response();
        }
    };

    let result = crate::dispatch::gateway::oauth::complete_authorization_callback(
        &manager,
        &upstream,
        SHARED_GATEWAY_OAUTH_SUBJECT,
        code,
        &query.state,
    )
    .await;

    // Delete the state token on failure to prevent replay; on success it was already consumed atomically.
    if result.is_err() {
        if let Err(revoke_err) = sqlite
            .delete_upstream_oauth_state_by_csrf(&query.state, now)
            .await
        {
            warn!(
                surface = "api",
                service = "upstream_oauth",
                action = "callback",
                "failed to revoke oauth state token after exchange failure: {revoke_err}"
            );
        }
    }

    let status = if let Err(error) = &result {
        warn!(
            surface = "api",
            service = "upstream_oauth",
            action = "callback",
            subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
            state_subject = %state_subject,
            elapsed_ms = started.elapsed().as_millis(),
            upstream = %upstream,
            kind = error.kind(),
            "upstream oauth callback failed"
        );
        "fail"
    } else {
        info!(
            surface = "api",
            service = "upstream_oauth",
            action = "callback",
            subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
            state_subject = %state_subject,
            elapsed_ms = started.elapsed().as_millis(),
            upstream = %upstream,
            "upstream oauth callback completed"
        );
        "ok"
    };
    redirect_url
        .query_pairs_mut()
        .append_pair("upstream", &upstream)
        .append_pair("status", status);

    if let Err(error) = result {
        redirect_url
            .query_pairs_mut()
            .append_pair("error_kind", error.kind());
        return Redirect::to(redirect_url.as_str()).into_response();
    }

    Redirect::to(redirect_url.as_str()).into_response()
}

async fn result_page(Query(query): Query<ResultQuery>) -> Html<String> {
    let upstream = html_escape(&query.upstream);
    let status = if query.status == "ok" {
        "successful"
    } else {
        "failed"
    };
    Html(format!(
        "<html><body><h2>Authorization {status}</h2><p>Upstream <strong>{upstream}</strong> has been processed. You may close this tab.</p></body></html>"
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Extension,
        body::{self, Body},
        http::{HeaderMap, Request, StatusCode, header},
    };
    use lab_auth::{
        config::{AuthConfig, AuthMode, GoogleConfig},
        state::AuthState,
    };
    use tower::ServiceExt;

    use super::{browser_routes, callback_subject, gateway_routes};
    use crate::{
        api::oauth::AuthContext,
        api::state::AppState,
        config::NodeRole,
        dispatch::gateway::{
            SHARED_GATEWAY_OAUTH_SUBJECT,
            manager::{GatewayManager, GatewayRuntimeHandle},
        },
        oauth::upstream::encryption::load_key,
    };

    #[tokio::test]
    async fn callback_rejects_non_master_requests() {
        let state = AppState::new().with_node_role(NodeRole::NonMaster);
        let app = browser_routes(state.clone()).with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/upstream/callback?upstream=test&code=code&state=csrf")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn callback_requires_authenticated_browser_session() {
        let state = AppState::new();
        let headers = HeaderMap::new();
        let error = callback_subject(&state, None, &headers).await.unwrap_err();
        assert_eq!(error.kind(), "oauth_state_invalid");
    }

    #[tokio::test]
    async fn result_page_escapes_upstream_name() {
        let state = AppState::new();
        let app = browser_routes(state.clone()).with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/gateway/oauth/result?upstream=%3Cscript%3Ealert(1)%3C%2Fscript%3E&status=ok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[tokio::test]
    async fn callback_authorization_error_consumes_oauth_state() {
        let (_dir, store, state) = callback_test_state().await;
        let app = browser_routes(state.clone()).with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/upstream/callback?state=csrf-error&error=access_denied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("location");
        assert!(location.contains("status=fail"));
        assert!(location.contains("error_kind=authorization_failed"));
        let owner = store
            .find_upstream_oauth_state_owner("csrf-error", now_seconds())
            .await
            .unwrap();
        assert!(owner.is_none());
    }

    #[tokio::test]
    async fn callback_missing_code_consumes_oauth_state() {
        let (_dir, store, state) = callback_test_state().await;
        let app = browser_routes(state.clone()).with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/upstream/callback?state=csrf-missing-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let owner = store
            .find_upstream_oauth_state_owner("csrf-missing-code", now_seconds())
            .await
            .unwrap();
        assert!(owner.is_none());
    }

    #[tokio::test]
    async fn clear_requires_explicit_confirmation() {
        let state = AppState::new();
        let app = gateway_routes(state.clone())
            .layer(Extension(test_auth_context()))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/clear?upstream=test")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "confirmation_required");
    }

    #[tokio::test]
    async fn probe_requires_explicit_confirmation() {
        let state = AppState::new();
        let app = gateway_routes(state.clone())
            .layer(Extension(test_auth_context()))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/probe")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"url":"https://fixture.example.com/mcp"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "confirmation_required");
    }

    #[tokio::test]
    async fn upstream_oauth_routes_require_admin_scope() {
        let state = AppState::new();
        let app = gateway_routes(state.clone())
            .layer(Extension(read_only_auth_context()))
            .with_state(state);

        let cases = [
            (
                "POST",
                "/probe",
                Some(serde_json::json!({"url":"https://fixture.example.com/mcp","confirm":true})),
            ),
            (
                "POST",
                "/start",
                Some(serde_json::json!({"upstream":"fixture"})),
            ),
            ("GET", "/status?upstream=fixture", None),
            ("POST", "/clear?upstream=fixture&confirm=true", None),
        ];

        for (method, uri, body) in cases {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json");
            let body = body.map(|value| value.to_string()).unwrap_or_default();
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
            let body = body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], "forbidden", "{method} {uri}");
        }
    }

    fn test_auth_context() -> AuthContext {
        AuthContext {
            sub: "browser-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:admin".to_string()],
            issuer: "https://issuer.example".to_string(),
            via_session: true,
            csrf_token: Some("csrf-123".to_string()),
            email: Some("browser@example.com".to_string()),
        }
    }

    fn read_only_auth_context() -> AuthContext {
        AuthContext {
            sub: "read-only-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "https://issuer.example".to_string(),
            via_session: true,
            csrf_token: Some("csrf-123".to_string()),
            email: Some("reader@example.com".to_string()),
        }
    }

    #[allow(dead_code)]
    async fn test_auth_state() -> AuthState {
        let dir = tempfile::tempdir().unwrap();
        let config = AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("bootstrap-secret".to_string()),
            google: GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            ..AuthConfig::default()
        };
        AuthState::new(config).await.unwrap()
    }

    fn now_seconds() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    async fn callback_test_state() -> (tempfile::TempDir, lab_auth::sqlite::SqliteStore, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let store = lab_auth::sqlite::SqliteStore::open(dir.path().join("auth.db"))
            .await
            .unwrap();
        for csrf in ["csrf-error", "csrf-missing-code"] {
            let now = now_seconds();
            store
                .save_upstream_oauth_state(lab_auth::types::UpstreamOauthStateRow {
                    upstream_name: "fixture".to_string(),
                    subject: SHARED_GATEWAY_OAUTH_SUBJECT.to_string(),
                    csrf_token: csrf.to_string(),
                    pkce_verifier: "verifier".to_string(),
                    created_at: now,
                    expires_at: now + 300,
                })
                .await
                .unwrap();
        }
        let key = load_key("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let manager = Arc::new(
            GatewayManager::new(dir.path().join("lab.toml"), GatewayRuntimeHandle::default())
                .with_oauth_resources(
                    store.clone(),
                    key,
                    "https://lab.example.com/auth/upstream/callback".to_string(),
                ),
        );
        let state = AppState::new()
            .with_auth_config(AuthConfig {
                public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
                ..AuthConfig::default()
            })
            .with_gateway_manager(manager);
        (dir, store, state)
    }
}
