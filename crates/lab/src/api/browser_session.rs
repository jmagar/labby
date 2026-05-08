use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::time::Instant;

use crate::api::ToolError;
use crate::api::auth_helpers::{log_auth_dispatch, log_auth_dispatch_start, request_id};
use crate::api::state::AppState;

use lab_auth::session::{BROWSER_CSRF_HEADER_NAME, BROWSER_SESSION_COOKIE_NAME};

const DEV_SESSION_EXPIRES_AT: u64 = 253_402_300_799;

fn oauth_state(state: &AppState) -> Option<&lab_auth::state::AuthState> {
    state.oauth_state.as_ref().map(|state| state.as_ref())
}

fn no_store_json(body: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, no-store")],
        axum::Json(body),
    )
        .into_response()
}

fn unauthenticated_session_response(login_available: bool) -> Response {
    no_store_json(serde_json::json!({
        "authenticated": false,
        "login_available": login_available,
    }))
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    lab_auth::session::read_cookie(headers, BROWSER_SESSION_COOKIE_NAME)
}

fn actor_key_for_session(
    state: &AppState,
    session: &lab_auth::types::BrowserSessionRow,
) -> Option<std::sync::Arc<str>> {
    state
        .actor_key_deriver
        .as_deref()
        .and_then(|deriver| deriver.derive_subject(&session.subject))
        .map(crate::observability::activity::ActorKey::into_arc)
}

async fn load_browser_session(
    auth_state: &lab_auth::state::AuthState,
    headers: &HeaderMap,
) -> Result<Option<lab_auth::types::BrowserSessionRow>, lab_auth::error::AuthError> {
    let has_cookie_header = headers.contains_key(header::COOKIE);
    let browser_session_cookie = session_cookie(headers);
    let has_browser_session_cookie = browser_session_cookie.is_some();
    tracing::info!(
        has_cookie_header,
        has_browser_session_cookie,
        "auth session request received"
    );

    let Some(session_id) = browser_session_cookie else {
        return Ok(None);
    };

    match auth_state.store.find_browser_session(&session_id).await {
        Ok(session) => {
            tracing::info!(
                has_cookie_header,
                has_browser_session_cookie,
                session_found = session.is_some(),
                "auth session lookup completed"
            );
            Ok(session)
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                has_cookie_header,
                has_browser_session_cookie,
                "auth session lookup failed"
            );
            Err(error)
        }
    }
}

fn internal_error_response(message: &'static str) -> Response {
    let mut response = ToolError::internal_message(message).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}

fn invalid_csrf_response() -> Response {
    let mut response = (
        StatusCode::UNPROCESSABLE_ENTITY,
        axum::Json(serde_json::json!({
            "kind": "validation_failed",
            "message": "missing or invalid csrf token",
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}

pub async fn auth_session(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.get", request_id.as_deref());

    if state.web_ui_auth_disabled {
        // Dev mode bypasses auth entirely — treat the synthetic dev user as admin
        // so admin UI is reachable in local development without real credentials.
        let response = no_store_json(serde_json::json!({
            "authenticated": true,
            "login_available": false,
            "is_admin": true,
            "user": {
                "sub": "labby-dev",
                "email": serde_json::Value::Null,
            },
            "expires_at": DEV_SESSION_EXPIRES_AT,
            "csrf_token": "",
        }));
        log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
        return response;
    }

    // If a valid static bearer token is presented, treat the caller as a
    // first-class authenticated session for browser-state purposes. This lets
    // automation tools (e.g. agent-browser with --headers) drive the UI while
    // OAuth remains enabled for normal browser users.
    if let Some(expected) = state.bearer_token.as_ref()
        && let Some(token) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(lab_auth::parse_bearer_token)
        && lab_auth::tokens_equal(&token, expected.as_ref())
    {
        let response = no_store_json(serde_json::json!({
            "authenticated": true,
            "login_available": state.oauth_state.is_some(),
            "is_admin": true,
            "user": {
                "sub": "static-bearer",
                "email": serde_json::Value::Null,
            },
            "expires_at": DEV_SESSION_EXPIRES_AT,
            "csrf_token": "",
        }));
        log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
        return response;
    }

    let login_available = state.oauth_state.is_some();
    let Some(auth_state) = oauth_state(&state) else {
        let response = unauthenticated_session_response(false);
        log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
        return response;
    };

    let admin_email = state
        .auth_config
        .as_ref()
        .map(|cfg| cfg.admin_email.as_str())
        .unwrap_or("");

    match load_browser_session(&auth_state, &headers).await {
        Ok(Some(session)) => {
            let actor_key = actor_key_for_session(&state, &session);
            let is_admin = session
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(admin_email) && !admin_email.is_empty());
            let body = serde_json::json!({
                "authenticated": true,
                "login_available": login_available,
                "is_admin": is_admin,
                "user": {
                    "sub": session.subject,
                    "email": session.email,
                },
                "expires_at": session.expires_at,
                "csrf_token": session.csrf_token,
            });
            log_auth_dispatch(
                "session.get",
                request_id.as_deref(),
                start,
                None,
                actor_key.as_deref(),
            );
            return no_store_json(body);
        }
        Ok(None) => {
            let response = unauthenticated_session_response(login_available);
            log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
            return response;
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to load browser session for auth session");
            log_auth_dispatch(
                "session.get",
                request_id.as_deref(),
                start,
                Some("internal_error"),
                None,
            );
            return internal_error_response("failed to load browser session");
        }
    }
}

pub async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.logout", request_id.as_deref());

    if state.web_ui_auth_disabled {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(auth_state) = oauth_state(&state) {
            lab_auth::session::append_set_cookie(
                &mut response,
                &lab_auth::session::clear_browser_session_cookie(auth_state),
            );
        }
        log_auth_dispatch("session.logout", request_id.as_deref(), start, None, None);
        return response;
    }

    let Some(auth_state) = oauth_state(&state) else {
        log_auth_dispatch("session.logout", request_id.as_deref(), start, None, None);
        return StatusCode::NO_CONTENT.into_response();
    };

    let mut response = StatusCode::NO_CONTENT.into_response();
    let mut actor_key = None;
    if let Some(session_id) = session_cookie(&headers) {
        let csrf = headers
            .get(BROWSER_CSRF_HEADER_NAME)
            .and_then(|value| value.to_str().ok());
        match auth_state.store.find_browser_session(&session_id).await {
            Ok(Some(session)) => {
                actor_key = actor_key_for_session(&state, &session);
                if csrf != Some(session.csrf_token.as_str()) {
                    tracing::warn!(
                        has_csrf_header = csrf.is_some(),
                        "auth logout rejected: missing or invalid csrf token"
                    );
                    log_auth_dispatch(
                        "session.logout",
                        request_id.as_deref(),
                        start,
                        Some("validation_failed"),
                        actor_key.as_deref(),
                    );
                    return invalid_csrf_response();
                }
                if let Err(error) = auth_state.store.revoke_browser_session(&session_id).await {
                    tracing::error!(error = %error, "failed to revoke browser session");
                    log_auth_dispatch(
                        "session.logout",
                        request_id.as_deref(),
                        start,
                        Some("internal_error"),
                        actor_key.as_deref(),
                    );
                    return internal_error_response("failed to revoke browser session");
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(error = %error, "failed to load browser session for logout");
                log_auth_dispatch(
                    "session.logout",
                    request_id.as_deref(),
                    start,
                    Some("internal_error"),
                    None,
                );
                return internal_error_response("failed to load browser session");
            }
        }
    }
    lab_auth::session::append_set_cookie(
        &mut response,
        &lab_auth::session::clear_browser_session_cookie(&auth_state),
    );
    log_auth_dispatch(
        "session.logout",
        request_id.as_deref(),
        start,
        None,
        actor_key.as_deref(),
    );
    response
}
