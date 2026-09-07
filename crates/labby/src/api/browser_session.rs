use axum::Extension;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::time::Instant;

use crate::api::ToolError;
use crate::api::auth_helpers::{log_auth_dispatch, log_auth_dispatch_start, request_id};
use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;

use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::reauth::ProofError;
use labby_auth::reauth_browser::PurposeInput;
use labby_auth::session::BROWSER_CSRF_HEADER_NAME;

const DEV_SESSION_EXPIRES_AT: u64 = 253_402_300_799;

fn oauth_state(state: &AppState) -> Option<&labby_auth::state::AuthState> {
    state.oauth_state.as_ref().map(|state| state.as_ref())
}

fn no_store_json(body: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(body),
    )
        .into_response()
}

fn unauthenticated_session_response(login_available: bool) -> Response {
    no_store_json(serde_json::json!({
        "authenticated": false,
        "login_available": login_available,
    }))
}

fn session_cookie(
    headers: &HeaderMap,
    auth_state: &labby_auth::state::AuthState,
) -> Option<String> {
    labby_auth::session::read_cookie(headers, &auth_state.config.session_cookie_name)
}

fn actor_key_for_session(
    state: &AppState,
    session: &labby_auth::types::BrowserSessionRow,
) -> Option<std::sync::Arc<str>> {
    state
        .actor_key_deriver
        .as_deref()
        .and_then(|deriver| deriver.derive_subject(&session.subject))
        .map(crate::observability::activity::ActorKey::into_arc)
}

async fn load_browser_session(
    auth_state: &labby_auth::state::AuthState,
    headers: &HeaderMap,
) -> Result<Option<labby_auth::types::BrowserSessionRow>, labby_auth::error::AuthError> {
    let has_cookie_header = headers.contains_key(header::COOKIE);
    let browser_session_cookie = session_cookie(headers, auth_state);
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
    let mut response = ApiError::new(ToolError::internal_message(message)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}

fn invalid_csrf_response() -> Response {
    let mut response = (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
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

fn proof_error_response(error: ProofError) -> Response {
    let status = match error {
        ProofError::Denied | ProofError::Required => StatusCode::UNAUTHORIZED,
        ProofError::RateLimited | ProofError::Capacity => StatusCode::TOO_MANY_REQUESTS,
        ProofError::InvalidPurpose => StatusCode::UNPROCESSABLE_ENTITY,
        ProofError::Unsupported => StatusCode::NOT_IMPLEMENTED,
        ProofError::Expired | ProofError::Replayed => StatusCode::GONE,
        ProofError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({"kind": error.kind(), "message": error.to_string()})),
    )
        .into_response()
}

fn trusted_origin(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
        .map(|url| url.origin().ascii_serialization())
    else {
        return false;
    };
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(expected.as_str())
}

pub async fn reauth_start(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    headers: HeaderMap,
    Json(input): Json<PurposeInput>,
) -> Response {
    if !trusted_origin(&state, &headers) {
        return invalid_csrf_response();
    }
    let Some(auth) = oauth_state(&state) else {
        return proof_error_response(ProofError::Unsupported);
    };
    match labby_auth::reauth_browser::start(auth, &authority, &input).await {
        Ok(started) => no_store_json(serde_json::to_value(started).unwrap_or_default()),
        Err(error) => proof_error_response(error),
    }
}

pub async fn reauth_poll(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    axum::extract::Path(interaction): axum::extract::Path<String>,
) -> Response {
    let Some(auth) = oauth_state(&state) else {
        return proof_error_response(ProofError::Unsupported);
    };
    match labby_auth::reauth_browser::poll(auth, &authority, &interaction).await {
        Ok(result) => no_store_json(serde_json::to_value(result).unwrap_or_default()),
        Err(error) => proof_error_response(error),
    }
}

pub async fn reauth_cancel(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    headers: HeaderMap,
    axum::extract::Path(interaction): axum::extract::Path<String>,
) -> Response {
    if !trusted_origin(&state, &headers) {
        return invalid_csrf_response();
    }
    let Some(auth) = oauth_state(&state) else {
        return proof_error_response(ProofError::Unsupported);
    };
    match labby_auth::reauth_browser::cancel(auth, &authority, &interaction).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => proof_error_response(error),
    }
}

pub async fn reauth_return() -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, no-store")],
        "<!doctype html><meta charset=utf-8><title>Authentication complete</title><p>Authentication complete. Return to Labby.</p>",
    )
        .into_response()
}

pub async fn auth_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<labby_auth::VerifiedIdentity>>,
) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.get", request_id.as_deref());

    if let Some(Extension(context)) = auth {
        let Some(Extension(identity)) = identity else {
            return internal_error_response("authenticated authority is unavailable");
        };
        let authority = match state.access_runtime.session_authority(identity).await {
            Ok(authority) => authority,
            Err(error) => {
                tracing::warn!(error = %error, "authenticated session authority resolution failed");
                return internal_error_response("authenticated authority is unavailable");
            }
        };
        let mut project_id = None;
        let mut expires_at = DEV_SESSION_EXPIRES_AT;
        if context.via_session
            && let Some(session_state) = state.project_session_state.as_ref()
            && let Some(session_id) =
                labby_auth::session::read_cookie(&headers, &session_state.cookie_name)
        {
            match session_state.store.find_browser_session(&session_id).await {
                Ok(Some(session)) => {
                    project_id = session
                        .project_binding
                        .as_ref()
                        .map(|binding| binding.project_id.clone());
                    expires_at =
                        u64::try_from(session.expires_at).unwrap_or(DEV_SESSION_EXPIRES_AT);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        "failed to load authenticated project session"
                    );
                    log_auth_dispatch(
                        "session.get",
                        request_id.as_deref(),
                        start,
                        Some("internal_error"),
                        context.actor_key.as_deref(),
                    );
                    return internal_error_response("failed to load browser session");
                }
            }
        }
        let response = no_store_json(serde_json::json!({
            "authenticated": true,
            "login_available": false,
            // OAuth scopes are only a transport ceiling. Domain administration
            // is projected from durable access authority, never inferred here.
            "is_admin": authority.capabilities.iter().any(|capability| capability.as_wire() == "platform.manage"),
            "user": {
                "sub": context.sub,
                "email": context.email,
            },
            "project_id": project_id,
            "owner": { "kind": "personal", "id": authority.principal_id },
            "organization_id": authority.organization_id,
            "teams": authority.teams.iter().map(|(id, role, membership_epoch, policy_epoch)| serde_json::json!({"id":id,"role":role.as_wire(),"membership_epoch":membership_epoch,"policy_epoch":policy_epoch})).collect::<Vec<_>>(),
            "projects": authority.projects.iter().map(|(id, role)| serde_json::json!({"id":id,"role":role.as_wire()})).collect::<Vec<_>>(),
            "project": project_id,
            "capabilities": authority.capabilities.iter().map(|capability| capability.as_wire()).collect::<Vec<_>>(),
            "authority_generation": authority.authority_generation,
            "expires_at": expires_at,
            "csrf_token": context.csrf_token.unwrap_or_default(),
        }));
        log_auth_dispatch(
            "session.get",
            request_id.as_deref(),
            start,
            None,
            context.actor_key.as_deref(),
        );
        return response;
    }

    // This route intentionally remains outside the auth middleware so an
    // anonymous browser can discover login availability. Resolve Labby's
    // project-bound cookie explicitly before the development UI fallback;
    // otherwise bearer-only deployments render an authenticated-but-unbound
    // shell even though the browser holds a valid project session.
    if let Some(session_state) = state.project_session_state.as_ref()
        && let Some(session_id) =
            labby_auth::session::read_cookie(&headers, &session_state.cookie_name)
    {
        match session_state.store.find_browser_session(&session_id).await {
            Ok(Some(session)) if session.project_binding.is_some() => {
                let actor_key = actor_key_for_session(&state, &session);
                let binding = session.project_binding.as_ref().expect("guarded above");
                let identity = match labby_auth::VerifiedIdentity::local_credential_with_issuer(
                    labby_auth::Authenticator::BrowserSession,
                    binding.issuer.clone(),
                    binding.source_credential_id.clone(),
                ) {
                    Ok(identity) => identity,
                    Err(_) => {
                        return internal_error_response("authenticated authority is unavailable");
                    }
                };
                let authority = match state.access_runtime.session_authority(identity).await {
                    Ok(authority) => authority,
                    Err(_) => {
                        return internal_error_response("authenticated authority is unavailable");
                    }
                };
                let response = no_store_json(serde_json::json!({
                    "authenticated": true,
                    "login_available": state.oauth_state.is_some(),
                    "is_admin": authority.capabilities.iter().any(|capability| capability.as_wire() == "platform.manage"),
                    "user": {
                        "sub": binding.principal_id,
                        "email": session.email,
                    },
                    "project_id": binding.project_id,
                    "owner": { "kind": "personal", "id": authority.principal_id },
                    "organization_id": authority.organization_id,
                    "teams": authority.teams.iter().map(|(id, role, membership_epoch, policy_epoch)| serde_json::json!({"id":id,"role":role.as_wire(),"membership_epoch":membership_epoch,"policy_epoch":policy_epoch})).collect::<Vec<_>>(),
                    "projects": authority.projects.iter().map(|(id, role)| serde_json::json!({"id":id,"role":role.as_wire()})).collect::<Vec<_>>(),
                    "project": binding.project_id,
                    "capabilities": authority.capabilities.iter().map(|capability| capability.as_wire()).collect::<Vec<_>>(),
                    "authority_generation": authority.authority_generation,
                    "expires_at": session.expires_at,
                    "csrf_token": session.csrf_token,
                }));
                log_auth_dispatch(
                    "session.get",
                    request_id.as_deref(),
                    start,
                    None,
                    actor_key.as_deref(),
                );
                return response;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(error = %error, "failed to load project browser session");
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

    if state.web_ui_auth_disabled {
        // Dev mode bypasses auth entirely — treat the synthetic dev user as admin
        // so admin UI is reachable in local development without real credentials.
        let response = no_store_json(serde_json::json!({
            "authenticated": true,
            "login_available": false,
            "is_admin": true,
            "dev_authority_bypass": true,
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
            .and_then(labby_auth::parse_bearer_token)
        && labby_auth::tokens_equal(&token, expected.as_ref())
    {
        let identity = match labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        ) {
            Ok(identity) => identity,
            Err(_) => return internal_error_response("authenticated authority is unavailable"),
        };
        let authority = match state.access_runtime.session_authority(identity).await {
            Ok(authority) => authority,
            Err(_) => return internal_error_response("authenticated authority is unavailable"),
        };
        let response = no_store_json(serde_json::json!({
            "authenticated": true,
            "login_available": state.oauth_state.is_some(),
            "is_admin": authority.capabilities.iter().any(|capability| capability.as_wire() == "platform.manage"),
            "user": {
                "sub": "static-bearer",
                "email": serde_json::Value::Null,
            },
            "expires_at": DEV_SESSION_EXPIRES_AT,
            "csrf_token": "",
            "owner": { "kind": "personal", "id": authority.principal_id },
            "organization_id": authority.organization_id,
            "teams": authority.teams.iter().map(|(id, role, membership_epoch, policy_epoch)| serde_json::json!({"id":id,"role":role.as_wire(),"membership_epoch":membership_epoch,"policy_epoch":policy_epoch})).collect::<Vec<_>>(),
            "projects": authority.projects.iter().map(|(id, role)| serde_json::json!({"id":id,"role":role.as_wire()})).collect::<Vec<_>>(),
            "project": serde_json::Value::Null,
            "capabilities": authority.capabilities.iter().map(|capability| capability.as_wire()).collect::<Vec<_>>(),
            "authority_generation": authority.authority_generation,
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

    match load_browser_session(&auth_state, &headers).await {
        Ok(Some(session)) => {
            let actor_key = actor_key_for_session(&state, &session);
            let identity = match labby_auth::VerifiedIdentity::external(
                labby_auth::Authenticator::BrowserSession,
                &auth_state.inbound_provider_binding().identity_issuer,
                session.subject.clone(),
            ) {
                Ok(identity) => identity,
                Err(_) => return internal_error_response("authenticated authority is unavailable"),
            };
            let authority = match state.access_runtime.session_authority(identity).await {
                Ok(authority) => authority,
                Err(_) => return internal_error_response("authenticated authority is unavailable"),
            };
            let project_id = session
                .project_binding
                .as_ref()
                .map(|binding| binding.project_id.as_str());
            let body = serde_json::json!({
                "authenticated": true,
                "login_available": login_available,
                "is_admin": authority.capabilities.iter().any(|capability| capability.as_wire() == "platform.manage"),
                "user": {
                    "sub": session.subject,
                    "email": session.email,
                },
                "project_id": project_id,
                "owner": { "kind": "personal", "id": authority.principal_id },
                "organization_id": authority.organization_id,
                "teams": authority.teams.iter().map(|(id, role, membership_epoch, policy_epoch)| serde_json::json!({"id":id,"role":role.as_wire(),"membership_epoch":membership_epoch,"policy_epoch":policy_epoch})).collect::<Vec<_>>(),
                "projects": authority.projects.iter().map(|(id, role)| serde_json::json!({"id":id,"role":role.as_wire()})).collect::<Vec<_>>(),
                "project": project_id,
                "capabilities": authority.capabilities.iter().map(|capability| capability.as_wire()).collect::<Vec<_>>(),
                "authority_generation": authority.authority_generation,
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
            labby_auth::session::append_set_cookie(
                &mut response,
                &labby_auth::session::clear_browser_session_cookie(auth_state),
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
    if let Some(session_id) = session_cookie(&headers, auth_state) {
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
    labby_auth::session::append_set_cookie(
        &mut response,
        &labby_auth::session::clear_browser_session_cookie(&auth_state),
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use labby_auth::types::{BrowserSessionRow, ProjectSessionBinding};

    #[test]
    fn reauthentication_requires_the_exact_configured_origin() {
        let config = labby_auth::config::AuthConfig {
            public_url: Some("https://lab.example.com/base".parse().unwrap()),
            ..Default::default()
        };
        let state = AppState::new().with_auth_config(config);
        let mut headers = HeaderMap::new();
        assert!(!trusted_origin(&state, &headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
        assert!(!trusted_origin(&state, &headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.example"),
        );
        assert!(!trusted_origin(&state, &headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://lab.example.com"),
        );
        assert!(trusted_origin(&state, &headers));
    }

    #[tokio::test]
    async fn recent_auth_errors_keep_stable_public_kinds() {
        for (error, status, kind) in [
            (
                ProofError::Unsupported,
                StatusCode::NOT_IMPLEMENTED,
                "recent_auth_unsupported",
            ),
            (ProofError::Expired, StatusCode::GONE, "recent_auth_expired"),
            (ProofError::Denied, StatusCode::UNAUTHORIZED, "auth_failed"),
        ] {
            let response = proof_error_response(error);
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, no-store"
            );
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], kind);
        }
    }

    #[tokio::test]
    async fn authenticated_project_session_preserves_binding_and_expiry() {
        let directory = tempfile::tempdir().unwrap();
        let session_state = labby_auth::project_session::ProjectSessionState::open(
            directory.path().join("auth.db"),
            "__Host-labby-session",
        )
        .await
        .unwrap();
        let expires_at = 2_000_000_000_i64;
        let session = BrowserSessionRow {
            session_id: "project-session".into(),
            subject: "subject".into(),
            email: None,
            csrf_token: "csrf-token".into(),
            created_at: 1,
            expires_at,
            project_binding: Some(ProjectSessionBinding {
                installation_id: "installation".into(),
                issuer: "issuer".into(),
                subject: "subject".into(),
                principal_id: "principal".into(),
                organization_id: "organization".into(),
                project_id: "project-42".into(),
                loadout_id: "loadout".into(),
                loadout_generation: 1,
                assignment_generation: 1,
                catalog_generation: 1,
                route_id: "route".into(),
                route_generation: 1,
                membership_epoch: 1,
                organization_policy_epoch: 1,
                project_policy_epoch: 1,
                source_credential_id: "credential".into(),
                source_credential_generation: 1,
                scopes: vec!["lab:read".into()],
                resource: "https://lab.example.com".into(),
                audience: "labby".into(),
                source_credential_expires_at: u64::try_from(expires_at).unwrap(),
            }),
        };
        session_state
            .store
            .upsert_browser_session(session)
            .await
            .unwrap();
        let state = AppState::new().with_project_session_state(session_state);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-labby-session=project-session"),
        );
        let auth = AuthContext {
            sub: "principal".into(),
            actor_key: None,
            scopes: vec!["lab:read".into()],
            issuer: "issuer".into(),
            via_session: true,
            csrf_token: Some("csrf-token".into()),
            email: None,
        };

        let response = auth_session(State(state.clone()), headers.clone(), None, None)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let response = auth_session(State(state), headers, Some(Extension(auth)), None)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
