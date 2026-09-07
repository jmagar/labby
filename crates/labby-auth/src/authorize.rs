use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect};
use axum::{Json, response::Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

#[cfg(test)]
static CALLBACK_PROVIDER_LOCK_REACHED: std::sync::LazyLock<tokio::sync::Notify> =
    std::sync::LazyLock::new(tokio::sync::Notify::new);
#[cfg(test)]
static CALLBACK_CAS_PAUSE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static CALLBACK_CAS_OBSERVED: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(0));
#[cfg(test)]
static CALLBACK_CAS_RESUME: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(0));

use crate::error::AuthError;
use crate::google::{AuthorizeUrlRequest, merge_google_scopes};
use crate::session::{append_set_cookie, build_browser_session_cookie};
use crate::state::AuthState;
use crate::types::{
    AuthorizationCodeRow, AuthorizationRequestRow, AuthorizeQuery, CallbackQuery,
    NativeAuthorizationResultRow, NativeAuthorizationStartResponse, NativeCallbackQuery,
    NativePollQuery, NativePollResponse,
};
use crate::util::{expires_at, fingerprint, now_unix, random_token};

/// Peer address used by OAuth callback and native-poll admission control.
pub struct RemoteAddr(pub SocketAddr);

impl<S: Send + Sync> FromRequestParts<S> for RemoteAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let address = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map_or(
                SocketAddr::from(([127, 0, 0, 1], 0)),
                |ConnectInfo(address)| address,
            );
        Ok(Self(address))
    }
}

mod entrypoints;
mod policy;
mod redirect;
mod response;
pub use entrypoints::{browser_login, register_client};
use policy::validate_response_type;
pub(crate) use policy::{check_email_allowlist, validate_resource, validate_scope};
pub(crate) use redirect::is_allowed_redirect_uri;
#[cfg(test)]
use redirect::{host_pattern_matches, wildcard_matches};

const AUTH_REQUEST_TTL_SECS: i64 = 300;
const NATIVE_START_MEDIA_TYPE: &str = "application/vnd.labby.native-oauth-start+json";
const NATIVE_SUCCESS_PAGE: &str = r#"<!doctype html><html><body style="font-family:sans-serif;background:#07131c;color:#e6f4fb;text-align:center;padding-top:4rem"><h2>Signed in to Labby</h2><p>You can close this tab and return to the app.</p></body></html>"#;
const NATIVE_CALLBACK_EXPIRED_PAGE: &str = r#"<!doctype html><html><body style="font-family:sans-serif;background:#07131c;color:#e6f4fb;text-align:center;padding-top:4rem"><h2>Sign-in link expired</h2><p>Return to the app and start sign-in again.</p></body></html>"#;

/// Extract the `IpAddr` from a `SocketAddr`, normalizing IPv4-mapped IPv6
/// addresses (`::ffff:a.b.c.d`) back to plain IPv4 so per-IP rate-limiting
/// keys are consistent regardless of listener address family (lab-77y5.10).
fn remote_ip(addr: SocketAddr) -> IpAddr {
    match addr.ip() {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

pub async fn authorize(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Response, AuthError> {
    state.check_authorize_rate_limit(remote_ip(addr)).await?;
    state.ensure_pending_oauth_state_capacity().await?;
    let client_state_id = fingerprint(&query.state);
    let client = crate::cimd::resolve_client(&state, &query.client_id)
        .await?
        .ok_or_else(|| {
            warn!(
                client_id = %fingerprint(&query.client_id),
                client_state_id = %client_state_id,
                "oauth authorize rejected: unknown client_id"
            );
            AuthError::InvalidGrant("unknown client_id".to_string())
        })?;
    if !client
        .redirect_uris
        .iter()
        .any(|uri| uri == &query.redirect_uri)
    {
        warn!(
            client_id = %fingerprint(&query.client_id),
            redirect_uri_id = %fingerprint(&query.redirect_uri),
            client_state_id = %client_state_id,
            "oauth authorize rejected: redirect URI does not match registered client"
        );
        return Err(AuthError::Validation(
            "redirect_uri does not match the registered client".to_string(),
        ));
    }
    // A CIMD URL is the client ID, not an RFC 7591 registration.  Keep a
    // validated local reference nonetheless: refresh_tokens has a foreign key
    // to registered_clients, and the authorization-code grant must be able to
    // issue a durable refresh token for a CIMD client.  `resolve_client`
    // continues to fetch URL-based clients from their metadata document, so
    // this reference cannot downgrade private_key_jwt authentication.
    if crate::cimd::is_metadata_document_client_id(&query.client_id) {
        state.store.register_client(client.clone()).await?;
    }
    if let Err(error) = validate_response_type(&query.response_type) {
        return response::authorization_error_redirect(
            &state,
            &query,
            "unsupported_response_type",
            error,
        );
    }
    let resource = match validate_resource(&state, query.resource.as_deref()) {
        Ok(resource) => resource,
        Err(error) => {
            return response::authorization_error_redirect(&state, &query, "invalid_target", error);
        }
    };
    let scope = match validate_scope(&state, &resource, &query.scope) {
        Ok(scope) => scope,
        Err(error) => {
            return response::authorization_error_redirect(&state, &query, "invalid_scope", error);
        }
    };
    info!(
        client_id = %fingerprint(&query.client_id),
        redirect_uri_id = %fingerprint(&query.redirect_uri),
        client_state_id = %client_state_id,
        resource_id = %fingerprint(&resource),
        requested_scope_id = %fingerprint(&query.scope),
        normalized_scope_id = %fingerprint(&scope),
        "oauth authorize request received"
    );
    if query.code_challenge_method != "S256" {
        warn!(
            client_id = %fingerprint(&query.client_id),
            client_state_id = %client_state_id,
            code_challenge_method_id = %fingerprint(&query.code_challenge_method),
            "oauth authorize rejected: unsupported PKCE method"
        );
        return response::authorization_error_redirect(
            &state,
            &query,
            "invalid_request",
            AuthError::Validation("code_challenge_method must be S256".to_string()),
        );
    }

    let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
    let is_native = query.redirect_uri == native_callback_endpoint;
    let accepts_native_start = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|part| {
                part.split(';').next().is_some_and(|essence| {
                    essence.trim().eq_ignore_ascii_case(NATIVE_START_MEDIA_TYPE)
                })
            })
        });
    if is_native && !accepts_native_start {
        return Err(AuthError::Validation(format!(
            "native OAuth clients must request `{NATIVE_START_MEDIA_TYPE}` and use the returned poll_token"
        )));
    }
    let native_poll_token = is_native
        .then(|| random_token(32))
        .transpose()?
        .map(|token| {
            let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()));
            (token, hash)
        });

    let provider_code_verifier = random_token(32)?;
    let provider_code_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(provider_code_verifier.as_bytes()));
    let request_state = random_token(24)?;
    let oauth_state_id = fingerprint(&request_state);

    state
        .store
        .insert_bound_authorization_request(
            AuthorizationRequestRow {
                state: request_state.clone(),
                client_id: query.client_id.clone(),
                redirect_uri: query.redirect_uri.clone(),
                client_state: query.state.clone(),
                native_poll_token_hash: native_poll_token.as_ref().map(|(_, hash)| hash.clone()),
                resource: resource.clone(),
                scope: scope.clone(),
                provider_code_verifier,
                code_challenge: query.code_challenge.clone(),
                code_challenge_method: query.code_challenge_method.clone(),
                created_at: now_unix(),
                expires_at: now_unix() + AUTH_REQUEST_TTL_SECS,
            },
            state.inbound_provider_binding(),
        )
        .await?;

    let allowed_emails = state.resolve_allowed_emails().await?;
    // Google's refresh credential belongs to the Google account and Labby's
    // Google OAuth client, not to the downstream DCR/CIMD client. This consent
    // policy is deliberately Google-only; generic OIDC providers control
    // refresh-token consent through their own server-side policy.
    let (provider_credential_present, force_consent) =
        if state.inbound_provider.kind() == crate::config::InboundProviderKind::Google {
            let present = match allowed_emails.as_slice() {
                [email] => state
                    .store
                    .find_google_provider_credential_by_email(email)
                    .await?
                    .is_some_and(|credential| {
                        credential.client_id == state.inbound_provider.client_id()
                    }),
                _ => false,
            };
            (Some(present), allowed_emails.len() != 1 || !present)
        } else {
            (None, false)
        };
    let location = state.inbound_provider.authorize_url(&AuthorizeUrlRequest {
        state: request_state,
        scope: scope.clone(),
        code_challenge: provider_code_challenge,
        code_challenge_method: "S256".to_string(),
        offline_access: true,
        force_consent,
    })?;
    info!(
        client_id = %fingerprint(&query.client_id),
        redirect_uri_id = %fingerprint(&query.redirect_uri),
        client_state_id = %client_state_id,
        oauth_state_id = %oauth_state_id,
        resource_id = %fingerprint(&resource),
        scope_id = %fingerprint(&scope),
        provider = ?state.inbound_provider.kind(),
        allowed_email_count = allowed_emails.len(),
        provider_credential_present,
        force_consent,
        "oauth authorize request redirected to upstream provider"
    );
    debug!(
        client_id = %fingerprint(&query.client_id),
        oauth_state_id = %oauth_state_id,
        provider_authorization_endpoint = %sanitized_authorization_endpoint(&location),
        "oauth authorize redirect URL generated"
    );

    if let Some((poll_token, _)) = native_poll_token {
        let mut response = Json(NativeAuthorizationStartResponse {
            authorization_url: location.to_string(),
            poll_token,
        })
        .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    } else if query.client_id.starts_with("dcr_") {
        let redirect_host = url::Url::parse(&query.redirect_uri)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "local application".to_string());
        let provider_url = escape_html_attribute(location.as_str());
        let html = format!(
            r#"<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><title>Authorize client</title></head><body><main><h1>Authorize client</h1><p>After authorization, Labby will redirect you to <strong>{redirect_host}</strong>.</p><p><a rel="noreferrer" href="{provider_url}">Continue</a></p></main></body></html>"#
        );
        let mut response = (StatusCode::OK, html).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'"),
        );
        Ok(response)
    } else {
        Ok((
            StatusCode::FOUND,
            [(header::LOCATION, location.to_string())],
        )
            .into_response())
    }
}

fn escape_html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn sanitized_authorization_endpoint(location: &url::Url) -> String {
    let mut endpoint = location.clone();
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let _ = endpoint.set_username("");
    let _ = endpoint.set_password(None);
    endpoint.to_string()
}

pub async fn callback(
    State(state): State<AuthState>,
    headers: HeaderMap,
    RemoteAddr(remote_addr): RemoteAddr,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AuthError> {
    state
        .check_callback_rate_limit(remote_ip(remote_addr))
        .await?;
    if query.state.len() > 1024
        || query.code.as_ref().is_some_and(|value| value.len() > 4096)
        || query.error.as_ref().is_some_and(|value| value.len() > 256)
        || query.iss.as_ref().is_some_and(|value| value.len() > 2048)
    {
        return Err(AuthError::Validation(
            "OAuth callback query is too large".into(),
        ));
    }
    let oauth_state_id = fingerprint(&query.state);
    info!(
        oauth_state_id = %oauth_state_id,
        provider = ?state.inbound_provider.kind(),
        "oauth callback received"
    );
    let browser_session = crate::session::read_cookie(&headers, &state.config.session_cookie_name);
    if crate::reauth_browser::callback(
        &state,
        &query.state,
        query.code.as_deref().unwrap_or_default(),
        browser_session.as_deref(),
    )
    .await?
    {
        return Ok(Redirect::to(crate::reauth_browser::RETURN_PATH).into_response());
    }
    if let Some(bound_login) = state
        .store
        .take_bound_browser_login_state(&query.state)
        .await?
    {
        if bound_login.binding != state.inbound_provider_binding() {
            return Err(AuthError::InvalidGrant(
                "browser login provider changed while authorization was in progress".into(),
            ));
        }
        let login = bound_login.value;
        if query.error.is_some() || query.code.is_none() {
            return Err(AuthError::AuthFailed(
                "upstream authorization was denied".into(),
            ));
        }
        if query
            .iss
            .as_deref()
            .is_some_and(|issuer| issuer != state.inbound_provider.issuer())
        {
            return Err(AuthError::AuthFailed(
                "upstream authorization issuer mismatch".into(),
            ));
        }
        let google = state
            .inbound_provider
            .exchange_code(
                query.code.as_deref().unwrap_or_default(),
                &login.provider_code_verifier,
                &query.state,
            )
            .await?;
        let allowed = state.resolve_allowed_emails().await?;
        let authelia_domain = matches!(
            state.inbound_provider.kind(),
            crate::config::InboundProviderKind::Authelia
        )
        .then(|| {
            google
                .email
                .as_deref()?
                .rsplit_once('@')
                .map(|(_, domain)| domain)
        })
        .flatten();
        check_email_allowlist(
            google.email.as_deref(),
            google.email_verified,
            google.hosted_domain.as_deref().or(authelia_domain),
            &allowed,
            &state.config.allowed_email_domains,
        )?;
        let session = crate::session::create_bound_browser_session(
            &state,
            google.subject,
            google.email,
            bound_login.binding,
        )
        .await?;
        let mut response = Redirect::to(&login.return_to).into_response();
        append_set_cookie(
            &mut response,
            &build_browser_session_cookie(&state, &session.session_id),
        );
        info!(
            oauth_state_id = %oauth_state_id,
            subject_id = %fingerprint(&session.subject),
            "browser login callback issued session cookie"
        );
        return Ok(response);
    }

    let bound_request = state
        .store
        .take_bound_authorization_request(&query.state)
        .await
        .map_err(|_| {
            warn!(
                oauth_state_id = %oauth_state_id,
                "oauth callback rejected: authorization state is invalid or expired"
            );
            AuthError::InvalidGrant("authorization state is invalid or expired".to_string())
        })?;
    let request = bound_request.value;
    let provider_binding = bound_request.binding;
    if provider_binding != state.inbound_provider_binding() {
        return response::authorization_callback_error_redirect(
            &state,
            &request.redirect_uri,
            &request.client_state,
            "access_denied",
            &AuthError::InvalidGrant(
                "identity provider changed while authorization was in progress".into(),
            ),
        );
    }
    info!(
        client_id = %fingerprint(&request.client_id),
        redirect_uri_id = %fingerprint(&request.redirect_uri),
        oauth_state_id = %oauth_state_id,
        client_state_id = %fingerprint(&request.client_state),
        resource_id = %fingerprint(&request.resource),
        scope_id = %fingerprint(&request.scope),
        "oauth callback state redeemed"
    );
    macro_rules! callback_try {
        ($expression:expr, $code:literal) => {
            match $expression {
                Ok(value) => value,
                Err(error) => {
                    return response::authorization_callback_error_redirect(
                        &state,
                        &request.redirect_uri,
                        &request.client_state,
                        $code,
                        &error,
                    );
                }
            }
        };
    }
    if query.error.is_some() || query.code.is_none() {
        return response::authorization_callback_error_redirect(
            &state,
            &request.redirect_uri,
            &request.client_state,
            "access_denied",
            &AuthError::AuthFailed("upstream authorization was denied".into()),
        );
    }
    if query
        .iss
        .as_deref()
        .is_some_and(|issuer| issuer != state.inbound_provider.issuer())
    {
        return response::authorization_callback_error_redirect(
            &state,
            &request.redirect_uri,
            &request.client_state,
            "access_denied",
            &AuthError::AuthFailed("upstream authorization issuer mismatch".into()),
        );
    }
    let observed_revocation_epoch = if matches!(
        state.inbound_provider.kind(),
        crate::config::InboundProviderKind::Google
    ) {
        match state.store.google_provider_fence_epoch().await {
            Ok(epoch) => Some(epoch),
            Err(error) => {
                return response::authorization_callback_error_redirect(
                    &state,
                    &request.redirect_uri,
                    &request.client_state,
                    "server_error",
                    &error,
                );
            }
        }
    } else {
        None
    };
    let google = match state
        .inbound_provider
        .exchange_code(
            query.code.as_deref().unwrap_or_default(),
            &request.provider_code_verifier,
            &query.state,
        )
        .await
    {
        Ok(google) => google,
        Err(error) => {
            let code = if matches!(
                error,
                AuthError::OauthNeedsReauth(_) | AuthError::InvalidGrant(_)
            ) {
                "access_denied"
            } else {
                "server_error"
            };
            return response::authorization_callback_error_redirect(
                &state,
                &request.redirect_uri,
                &request.client_state,
                code,
                &error,
            );
        }
    };

    // RFC 6749 §4.1.2.1: errors must redirect to the client's redirect_uri,
    // not surface as a JSON HTTP error. The denial reason is sourced from the
    // AuthError so we only log once (inside check_email_allowlist).
    let allowed = callback_try!(state.resolve_allowed_emails().await, "server_error");
    let authelia_domain = matches!(
        state.inbound_provider.kind(),
        crate::config::InboundProviderKind::Authelia
    )
    .then(|| {
        google
            .email
            .as_deref()?
            .rsplit_once('@')
            .map(|(_, domain)| domain)
    })
    .flatten();
    if let Err(denial) = check_email_allowlist(
        google.email.as_deref(),
        google.email_verified,
        google.hosted_domain.as_deref().or(authelia_domain),
        &allowed,
        &state.config.allowed_email_domains,
    ) {
        let mut redirect_target = callback_try!(
            url::Url::parse(&request.redirect_uri).map_err(|error| {
                // Unreachable in practice: redirect_uri was validated against the
                // client's registered URIs before being stored.
                AuthError::Config(format!("failed to parse registered redirect_uri: {error}"))
            }),
            "server_error"
        );
        redirect_target
            .query_pairs_mut()
            .append_pair("error", "access_denied")
            .append_pair("error_description", &denial.to_string())
            .append_pair("state", &request.client_state);
        response::append_authorization_response_issuer(&state, &mut redirect_target);
        return Ok(Redirect::to(redirect_target.as_str()).into_response());
    }

    let subject_id = fingerprint(&google.subject);
    let verified_email = callback_try!(
        google
            .email
            .as_deref()
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .ok_or_else(|| {
                AuthError::AuthFailed(
                    "the identity provider did not return a verified email address after allowlist validation"
                        .to_string(),
                )
            }),
        "access_denied"
    );
    if matches!(
        state.inbound_provider.kind(),
        crate::config::InboundProviderKind::Authelia
    ) {
        callback_try!(
            state
                .store
                .upsert_bound_verified_inbound_identity(
                    &google.subject,
                    verified_email,
                    now_unix(),
                    provider_binding.clone(),
                )
                .await,
            "server_error"
        );
        return finish_local_authorization(
            &state,
            request,
            google.subject,
            oauth_state_id,
            provider_binding,
        )
        .await;
    }
    // Serialize callback installation with refresh/invalidation for this Google
    // account. SQLite generation CAS below also protects deployments with more
    // than one Labby process sharing the auth database.
    #[cfg(test)]
    CALLBACK_PROVIDER_LOCK_REACHED.notify_waiters();
    let _provider_guard = crate::google_refresh::lock(&google.subject)
        .lock_owned()
        .await;
    let received_provider_refresh_token = google.refresh_token.is_some();
    let existing_credential = callback_try!(
        state
            .store
            .find_google_provider_credential(&google.subject)
            .await,
        "server_error"
    );
    #[cfg(test)]
    if request.client_state == "generation-loss-client-state"
        && CALLBACK_CAS_PAUSE_ENABLED.load(std::sync::atomic::Ordering::Acquire)
    {
        CALLBACK_CAS_OBSERVED.add_permits(1);
        CALLBACK_CAS_RESUME
            .acquire()
            .await
            .expect("test semaphore open")
            .forget();
    }
    let granted_scopes = merge_google_scopes(
        existing_credential
            .as_ref()
            .map(|credential| credential.granted_scopes.as_slice())
            .unwrap_or_default(),
        &google.granted_scopes,
    );
    let scope_upgraded = existing_credential.as_ref().is_none_or(|existing| {
        granted_scopes
            .iter()
            .any(|scope| !existing.granted_scopes.contains(scope))
    });
    let (provider_refresh_token, reused_provider_refresh_token) = if let Some(refresh_token) =
        google.refresh_token.clone()
    {
        (refresh_token, false)
    } else if let Some(existing) = existing_credential
        .as_ref()
        .filter(|credential| credential.client_id == state.inbound_provider.client_id())
    {
        (existing.refresh_token.clone(), true)
    } else {
        warn!(
            client_id = %fingerprint(&request.client_id),
            oauth_state_id = %oauth_state_id,
            subject_id = %subject_id,
            kind = "oauth_needs_reauth",
            "oauth callback rejected: google did not provide a reusable refresh credential"
        );
        let mut redirect_target = callback_try!(
            url::Url::parse(&request.redirect_uri).map_err(|error| {
                AuthError::Config(format!("failed to parse registered redirect_uri: {error}"))
            }),
            "server_error"
        );
        redirect_target
                .query_pairs_mut()
                .append_pair("error", "server_error")
                .append_pair(
                    "error_description",
                    "Google did not issue a reusable offline credential; reconnect and grant access again",
                )
                .append_pair("state", &request.client_state);
        response::append_authorization_response_issuer(&state, &mut redirect_target);
        return Ok(Redirect::to(redirect_target.as_str()).into_response());
    };
    let provider_token_received_at = now_unix();
    let provider_update = crate::types::GoogleProviderCredentialUpdate {
        subject: google.subject.clone(),
        email: Some(verified_email.to_string()),
        client_id: state.inbound_provider.client_id().to_string(),
        granted_scopes: granted_scopes.clone(),
        access_token: google.access_token.clone(),
        refresh_token: provider_refresh_token,
        token_received_at: provider_token_received_at,
        access_token_expires_at: provider_token_received_at
            .saturating_add(i64::try_from(google.expires_in.unwrap_or(3600)).unwrap_or(i64::MAX)),
        issuer: Some("https://accounts.google.com".to_string()),
        refreshed: false,
        scope_upgraded,
    };
    let provider_update_persisted = if let Some(existing) = existing_credential.as_ref() {
        callback_try!(
            state
                .store
                .replace_google_provider_token_bundle_if_generation(
                    provider_update,
                    existing.generation,
                )
                .await,
            "server_error"
        )
    } else {
        callback_try!(
            state
                .store
                .insert_google_provider_token_bundle_if_absent(
                    provider_update,
                    observed_revocation_epoch.expect("Google callback records its fence epoch"),
                )
                .await,
            "server_error"
        )
    };
    if !provider_update_persisted {
        let replacement_present = callback_try!(
            state
                .store
                .has_google_provider_credential_for_subject(&google.subject)
                .await,
            "server_error"
        );
        warn!(
            client_id = %fingerprint(&request.client_id),
            oauth_state_id = %oauth_state_id,
            subject_id = %subject_id,
            observed_provider_generation = ?existing_credential.as_ref().map(|row| row.generation),
            replacement_provider_credential_present = replacement_present,
            kind = "oauth_needs_reauth",
            "oauth callback discarded stale provider exchange after generation changed"
        );
        return response::authorization_callback_error_redirect(
            &state,
            &request.redirect_uri,
            &request.client_state,
            "server_error",
            &AuthError::OauthNeedsReauth(
                "google provider credential changed during authorization; retry authorization"
                    .to_string(),
            ),
        );
    }
    info!(
        client_id = %fingerprint(&request.client_id),
        oauth_state_id = %oauth_state_id,
        subject_id = %subject_id,
        provider_credential_present = true,
        received_provider_refresh_token,
        reused_provider_refresh_token,
        "oauth callback exchanged upstream code successfully"
    );
    finish_local_authorization(
        &state,
        request,
        google.subject,
        oauth_state_id,
        provider_binding,
    )
    .await
}

async fn finish_local_authorization(
    state: &AuthState,
    request: AuthorizationRequestRow,
    subject: String,
    oauth_state_id: String,
    provider_binding: crate::types::ProviderBinding,
) -> Result<Response, AuthError> {
    let auth_code = random_token(24)?;
    let redirect_uri_raw = request.redirect_uri.clone();
    let client_state = request.client_state.clone();
    let client_id = request.client_id.clone();
    let resource = request.resource.clone();
    let scope = request.scope.clone();
    state
        .store
        .insert_bound_auth_code(
            AuthorizationCodeRow {
                code: auth_code.clone(),
                client_id: request.client_id,
                subject,
                redirect_uri: request.redirect_uri.clone(),
                resource: request.resource,
                scope: request.scope,
                code_challenge: request.code_challenge,
                code_challenge_method: request.code_challenge_method,
                provider_refresh_token: None,
                created_at: now_unix(),
                expires_at: expires_at(
                    now_unix(),
                    state.config.auth_code_ttl,
                    &format!("{}_AUTH_CODE_TTL_SECS", state.config.env_prefix),
                )?,
            },
            provider_binding.clone(),
        )
        .await?;
    let auth_code_id = fingerprint(&auth_code);
    info!(
        auth_code_id = %auth_code_id,
        oauth_state_id = %oauth_state_id,
        client_id = %fingerprint(&client_id),
        resource_id = %fingerprint(&resource),
        scope_id = %fingerprint(&scope),
        redirect_uri_id = %fingerprint(&redirect_uri_raw),
        "oauth callback issued local authorization code"
    );
    let native_callback_endpoint = crate::metadata::native_callback_endpoint(state);
    if redirect_uri_raw == native_callback_endpoint {
        let now = now_unix();
        state
            .store
            .insert_bound_native_authorization_result(
                NativeAuthorizationResultRow {
                    poll_token_hash: request.native_poll_token_hash.ok_or_else(|| {
                        AuthError::Storage(
                            "native authorization request is missing its polling credential".into(),
                        )
                    })?,
                    code: auth_code,
                    created_at: now,
                    expires_at: expires_at(
                        now,
                        state.config.auth_code_ttl,
                        &format!("{}_AUTH_CODE_TTL_SECS", state.config.env_prefix),
                    )?,
                },
                provider_binding,
            )
            .await?;
        let mut response = axum::response::Html(NATIVE_SUCCESS_PAGE).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        debug!(
            auth_code_id = %auth_code_id,
            native_callback_endpoint_id = %fingerprint(&native_callback_endpoint),
            "oauth callback stored native authorization code for polling"
        );
        return Ok(response);
    }
    let mut redirect_uri = reqwest::Url::parse(&redirect_uri_raw)
        .map_err(|_| AuthError::Storage("registered redirect URI is invalid".into()))?;
    redirect_uri
        .query_pairs_mut()
        .append_pair("code", &auth_code)
        .append_pair("state", &client_state);
    response::append_authorization_response_issuer(state, &mut redirect_uri);
    let (has_code, has_state, has_issuer) =
        response::authorization_response_query_presence(&redirect_uri);
    info!(
        oauth_state_id = %oauth_state_id,
        auth_code_id = %auth_code_id,
        client_id = %fingerprint(&client_id),
        authorization_response_has_code = has_code,
        authorization_response_has_state = has_state,
        authorization_response_has_issuer = has_issuer,
        redirect_scheme = redirect_uri.scheme(),
        redirect_host = redirect_uri.host_str(),
        redirect_path = redirect_uri.path(),
        "oauth callback authorization response prepared"
    );
    Ok(Redirect::to(redirect_uri.as_str()).into_response())
}

/// Direct-hit fallback for the registered native `redirect_uri`. In the real
/// flow this path is never dereferenced by an actual browser redirect —
/// The provider redirect target is the active provider callback, which detects
/// a native-flow authorization request and short-circuits into stashing the
/// code for `/native/poll` instead of redirecting here. This handler only
/// answers a stray direct visit (e.g. a stale bookmark or a misconfigured
/// client), so `state` is validated for URL-shape consistency but
/// deliberately not looked up — there's nothing to correlate it against.
pub async fn native_callback(
    Query(query): Query<NativeCallbackQuery>,
) -> Result<Response, AuthError> {
    let state_param = query.state.trim();
    if state_param.is_empty() {
        return Err(AuthError::Validation(
            "missing `state` parameter".to_string(),
        ));
    }
    let mut response = (
        StatusCode::GONE,
        axum::response::Html(NATIVE_CALLBACK_EXPIRED_PAGE),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn native_poll(
    State(state): State<AuthState>,
    RemoteAddr(remote_addr): RemoteAddr,
    Json(query): Json<NativePollQuery>,
) -> Result<Response, AuthError> {
    state
        .check_native_poll_rate_limit(remote_ip(remote_addr))
        .await?;
    let poll_token = query.poll_token.trim();
    if poll_token.is_empty() {
        return Err(AuthError::Validation(
            "missing `poll_token` parameter".to_string(),
        ));
    }
    let poll_token_hash = URL_SAFE_NO_PAD.encode(Sha256::digest(poll_token.as_bytes()));
    let mut response = if let Some(row) = state
        .store
        .take_native_authorization_result(&poll_token_hash)
        .await?
    {
        Json(NativePollResponse {
            code: Some(row.code),
        })
        .into_response()
    } else {
        let mut pending = (
            StatusCode::ACCEPTED,
            Json(NativePollResponse { code: None }),
        )
            .into_response();
        pending
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        pending
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn sanitize_return_to(state: &AuthState, requested: Option<&str>) -> String {
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return "/".to_string();
    };
    if requested.starts_with('/') && !requested.starts_with("//") {
        return requested.to_string();
    }
    let Some(public_url) = state.config.public_url.as_ref() else {
        return "/".to_string();
    };
    let Ok(url) = reqwest::Url::parse(requested) else {
        return "/".to_string();
    };
    if url.scheme() != public_url.scheme()
        || url.host_str() != public_url.host_str()
        || url.port_or_known_default() != public_url.port_or_known_default()
    {
        return "/".to_string();
    }
    let mut normalized = url.path().to_string();
    if let Some(query) = url.query() {
        normalized.push('?');
        normalized.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        normalized.push('#');
        normalized.push_str(fragment);
    }
    normalized
}

#[cfg(test)]
pub mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use base64::Engine;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::traits::PublicKeyParts;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tower::util::ServiceExt;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{host_pattern_matches, is_allowed_redirect_uri, wildcard_matches};
    use crate::config::{AuthConfig, AuthMode, GoogleConfig};
    use crate::google::GoogleProvider;
    use crate::state::AuthState;
    use crate::types::{
        AuthorizationRequestRow, GoogleProviderCredentialUpdate, NativeAuthorizationResultRow,
        RegisteredClient,
    };

    use crate::util::now_unix;

    use axum::Router;
    use axum::extract::connect_info::MockConnectInfo;
    use std::net::SocketAddr;

    fn native_poll_token_hash_for(token: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
    }

    fn native_poll_request(poll_token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/native/poll")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "poll_token": poll_token }).to_string()))
            .unwrap()
    }

    // `oneshot` bypasses the live `into_make_service_with_connect_info` layer,
    // so the rate-limit handlers' `ConnectInfo<SocketAddr>` extractor would be
    // missing and every request would 500. Wrap the real router with a mock
    // peer address; handlers that don't extract `ConnectInfo` ignore it.
    fn router(state: AuthState) -> Router {
        crate::routes::router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9001))))
    }

    async fn seed_provider_credential(state: &AuthState, client_id: &str, refresh_token: &str) {
        let now = now_unix();
        state
            .store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "google-user".to_string(),
                email: Some("user@example.com".to_string()),
                client_id: client_id.to_string(),
                granted_scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
                access_token: "provider-access".to_string(),
                refresh_token: refresh_token.to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some("https://accounts.google.com".to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
    }

    fn assert_authorization_error(response: &axum::response::Response, expected_error: &str) {
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Url::parse(value).ok())
            .expect("authorization error redirect location");
        let query = location
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query.get("error").map(String::as_str), Some(expected_error));
        assert_eq!(query.get("state").map(String::as_str), Some("abc"));
        assert_eq!(
            query.get("iss").map(String::as_str),
            Some("https://lab.example.com")
        );
        assert!(
            query
                .get("error_description")
                .is_some_and(|description| !description.is_empty())
        );
    }

    #[tokio::test]
    async fn register_accepts_public_dcr_and_enforces_loopback_redirects() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let app = router(test_auth_state_with_config(config).await);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["http://127.0.0.1:7777/callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn register_accepts_native_callback_endpoint_without_redirect_allowlist() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let state = test_auth_state_with_config(config).await;
        let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "redirect_uris": [native_callback_endpoint] }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn register_rejects_redirect_uris_with_fragments() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        config.allowed_client_redirect_uris = vec!["https://client.example/callback".to_string()];
        let app = router(test_auth_state_with_config(config).await);

        for redirect_uri in [
            "http://127.0.0.1:7777/callback#fragment",
            "com.example.app:/oauth#fragment",
            "https://client.example/callback#fragment",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/register")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            json!({ "redirect_uris": [redirect_uri] }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registration_logs_do_not_include_redirect_uri_query_values() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let app = router(test_auth_state_with_config(config).await);
        let secret = "registration-query-secret";
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": [format!(
                                "http://127.0.0.1:7777/callback?tenant={secret}"
                            )]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let logs = crate::test_support::captured_logs(buf);
        assert!(
            !logs.contains(secret),
            "redirect query leaked into logs: {logs}"
        );
        assert!(
            !logs.contains("redirect_uris"),
            "redirect URI list entered logs: {logs}"
        );
        assert!(logs.contains("\"redirect_uri_count\":1"), "{logs}");
    }

    #[tokio::test]
    async fn register_rejects_native_callback_endpoint_smuggled_with_an_unsafe_redirect_uri() {
        // The native-endpoint bypass in `register_client` is per-redirect_uri —
        // confirm a registration that mixes the native endpoint with an
        // otherwise-disallowed redirect_uri in the same request still fails
        // validation for the whole request, rather than the native match
        // short-circuiting the loop and letting the unsafe URI through.
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let state = test_auth_state_with_config(config).await;
        let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": [
                                native_callback_endpoint,
                                "https://evil.example/callback",
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn dynamically_registered_client_requires_click_consent_showing_redirect_host() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        config.allowed_client_redirect_uris = vec!["https://client.example/callback".to_string()];
        let state = test_auth_state_with_config(config).await;
        let app = router(state);
        let registration = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"redirect_uris": ["https://client.example/callback"]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(registration.into_body(), usize::MAX)
            .await
            .unwrap();
        let client_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_string();
        let uri = format!(
            "/authorize?response_type=code&client_id={client_id}&redirect_uri=https%3A%2F%2Fclient.example%2Fcallback&state=client-state&scope=lab&code_challenge=pkce&code_challenge_method=S256"
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("client.example"));
        assert!(html.contains("Continue"));
        assert!(!html.contains("client-state"));
    }

    #[tokio::test]
    async fn localhost_redirect_consent_warns_with_exact_loopback_host() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let state = test_auth_state_with_config(config).await;
        let app = router(state);
        let registration = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"redirect_uris": ["http://localhost:7777/callback"]}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(registration.status(), StatusCode::OK);
        let body = axum::body::to_bytes(registration.into_body(), usize::MAX)
            .await
            .unwrap();
        let client_id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_string();
        let uri = format!(
            "/authorize?response_type=code&client_id={client_id}&redirect_uri=http%3A%2F%2Flocalhost%3A7777%2Fcallback&state=secret-client-state&scope=lab%3Aread&code_challenge=pkce&code_challenge_method=S256"
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("redirect you to <strong>localhost</strong>"));
        assert!(html.contains("Continue"));
        assert!(!html.contains("secret-client-state"));
    }

    #[tokio::test]
    async fn native_poll_returns_202_with_no_code_for_an_unknown_poll_token() {
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(native_poll_request("never-issued"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("code").is_none());
    }

    #[tokio::test]
    async fn native_authorize_returns_server_generated_polling_credential() {
        let state = test_auth_state().await;
        let native_callback = crate::metadata::native_callback_endpoint(&state);
        state
            .store
            .register_client(RegisteredClient {
                client_id: "native-start-client".to_string(),
                redirect_uris: vec![native_callback.clone()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        let mut uri = Url::parse("https://lab.example.com/authorize").unwrap();
        uri.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", "native-start-client")
            .append_pair("redirect_uri", &native_callback)
            .append_pair("state", "attacker-known-state")
            .append_pair("scope", "lab")
            .append_pair("code_challenge", "pkce")
            .append_pair("code_challenge_method", "S256");
        let uri = format!("{}?{}", uri.path(), uri.query().unwrap());
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(
                        header::ACCEPT,
                        "text/html, Application/Vnd.Labby.Native-Oauth-Start+Json; charset=utf-8; q=1",
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
        let poll_token = json["poll_token"].as_str().unwrap();
        assert_ne!(poll_token, "attacker-known-state");
        assert!(poll_token.len() >= 32);
        assert!(json["authorization_url"].as_str().is_some());
    }

    #[tokio::test]
    async fn native_poll_rejects_missing_poll_token() {
        let app = router(test_auth_state().await);
        let response = app.oneshot(native_poll_request("")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn native_poll_never_accepts_a_polling_secret_in_the_request_uri() {
        let response = router(test_auth_state().await)
            .oneshot(
                Request::builder()
                    .uri("/native/poll?poll_token=must-not-enter-access-logs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn native_poll_is_one_shot_and_returns_the_code_exactly_once() {
        let state = test_auth_state().await;
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: native_poll_token_hash_for("poll-me"),
                code: "the-code".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let app = router(state);

        let first = app
            .clone()
            .oneshot(native_poll_request("poll-me"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "the-code");

        // Second poll for the same token must not still return the code —
        // `take_native_authorization_result` is a one-shot read-and-delete.
        let second = app.oneshot(native_poll_request("poll-me")).await.unwrap();
        assert_eq!(second.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn native_callback_direct_hit_shows_expired_page_and_never_stores_a_code() {
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/native/callback?state=whatever&code=attacker-supplied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn insert_native_authorization_result_overwrites_on_token_hash_collision() {
        // The effectively impossible hash-collision case remains deterministic:
        // last-write-wins, not `DO NOTHING`.
        let state = test_auth_state().await;
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: native_poll_token_hash_for("collide"),
                code: "first-code".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: native_poll_token_hash_for("collide"),
                code: "second-code".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let fetched = state
            .store
            .take_native_authorization_result(&native_poll_token_hash_for("collide"))
            .await
            .unwrap()
            .expect("row should still be present");
        assert_eq!(fetched.code, "second-code");
    }

    #[tokio::test]
    async fn callback_stores_native_flow_code_for_polling_instead_of_redirecting() {
        let native_state = test_auth_state_with_mock_google_native().await;
        let native_callback = crate::metadata::native_callback_endpoint(&native_state);
        let app = router(native_state);
        let mut authorize_uri = Url::parse("https://lab.example.com/authorize").unwrap();
        authorize_uri
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", "native-client")
            .append_pair("redirect_uri", &native_callback)
            .append_pair("state", "native-client-state")
            .append_pair("scope", "lab")
            .append_pair("code_challenge", "challenge")
            .append_pair("code_challenge_method", "S256");
        let authorize_uri = format!(
            "{}?{}",
            authorize_uri.path(),
            authorize_uri.query().unwrap()
        );
        let start = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(authorize_uri)
                    .header(
                        header::ACCEPT,
                        "application/vnd.labby.native-oauth-start+json",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::OK);
        let start_body = axum::body::to_bytes(start.into_body(), usize::MAX)
            .await
            .unwrap();
        let start_json: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
        let poll_token = start_json["poll_token"].as_str().unwrap().to_string();
        let provider_url = Url::parse(start_json["authorization_url"].as_str().unwrap()).unwrap();
        let provider_state = provider_url
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/google/callback?state={provider_state}&code=upstream-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // The native branch never redirects the browser — it shows a static
        // "signed in" page directly.
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("Signed in"));

        let attacker_poll = app
            .clone()
            .oneshot(native_poll_request("native-client-state"))
            .await
            .unwrap();
        assert_eq!(attacker_poll.status(), StatusCode::ACCEPTED);

        let poll = app.oneshot(native_poll_request(&poll_token)).await.unwrap();
        assert_eq!(poll.status(), StatusCode::OK);
        let poll_body = axum::body::to_bytes(poll.into_body(), usize::MAX)
            .await
            .unwrap();
        let poll_json: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
        assert!(poll_json["code"].as_str().is_some());
    }

    #[tokio::test]
    async fn native_poll_rejects_an_attacker_who_only_knows_the_client_state() {
        let native_state = test_auth_state_with_mock_google_native().await;
        let app = router(native_state);
        let callback = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=native-good-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::OK);

        let attacker_poll = app
            .oneshot(native_poll_request("native-client-state"))
            .await
            .unwrap();
        assert_eq!(attacker_poll.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn register_accepts_allowed_non_loopback_redirect_patterns() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        config.allowed_client_redirect_uris =
            vec!["https://callback.example.com/callback/*".to_string()];
        let app = router(test_auth_state_with_config(config).await);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["https://callback.example.com/callback/node-a"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn register_is_rate_limited_after_configured_burst() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        config.register_requests_per_minute = 1;
        let app = router(test_auth_state_with_config(config).await);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["http://127.0.0.1:7777/callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["http://127.0.0.1:8888/callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn wildcard_redirect_patterns_support_leading_and_infix_matches() {
        assert!(wildcard_matches(
            "https://callback.example.com/callback/*",
            "https://callback.example.com/callback/node-a"
        ));
        assert!(wildcard_matches(
            "https://callback.*.com/callback/*",
            "https://callback.example.com/callback/node-a"
        ));
        assert!(!wildcard_matches("/callback", "/callback/extra"));
    }

    #[test]
    fn host_patterns_support_full_label_wildcards_only() {
        assert!(host_pattern_matches(
            "callback.*.com",
            "callback.example.com"
        ));
        assert!(host_pattern_matches(
            "*.example.com",
            "callback.example.com"
        ));
        assert!(!host_pattern_matches(
            "callback.example.com*",
            "callback.example.com"
        ));
        assert!(!host_pattern_matches(
            "*.example.com",
            "callback.nested.example.com"
        ));
    }

    #[test]
    fn wildcard_redirect_patterns_do_not_overmatch_similar_hosts() {
        assert!(!is_allowed_redirect_uri(
            "https://callback.example.com.evil.example/callback/node-a",
            &[String::from("https://callback.example.com/callback/*")]
        ));
        assert!(!is_allowed_redirect_uri(
            "https://callback.example.com.evil.example/callback",
            &[String::from("https://callback.example.com*")]
        ));
    }

    #[test]
    fn native_app_scheme_redirect_uris_are_always_allowed() {
        // Native-app redirects (RFC 8252 §7.1) like `com.raycast:/oauth` or
        // `warp://mcp/oauth2callback` are scoped to whatever app the OS has
        // registered for that private-use scheme, so — like loopback — they
        // don't need a per-client allowlist entry.
        assert!(is_allowed_redirect_uri("com.raycast:/oauth", &[]));
        assert!(is_allowed_redirect_uri("warp://mcp/oauth2callback", &[]));
        assert!(is_allowed_redirect_uri(
            "com.raycast:/oauth",
            &[String::from("https://callback.tootie.tv/callback/*")]
        ));
    }

    #[test]
    fn redirect_uris_with_fragments_are_never_allowed() {
        for redirect_uri in [
            "http://127.0.0.1:7777/callback#fragment",
            "com.raycast:/oauth#fragment",
            "https://callback.tootie.tv/callback/node-a#fragment",
        ] {
            assert!(!is_allowed_redirect_uri(
                redirect_uri,
                &[String::from("https://callback.tootie.tv/callback/*")],
            ));
        }
    }

    #[test]
    fn script_executing_pseudo_schemes_are_never_auto_allowed() {
        assert!(!is_allowed_redirect_uri("javascript:alert(1)", &[]));
        assert!(!is_allowed_redirect_uri("data:text/html,evil", &[]));
        assert!(!is_allowed_redirect_uri("file:///etc/passwd", &[]));
    }

    #[test]
    fn https_redirects_still_require_the_allowlist() {
        assert!(!is_allowed_redirect_uri(
            "https://evil.example/callback",
            &[String::from("https://callback.tootie.tv/callback/*")]
        ));
        assert!(is_allowed_redirect_uri(
            "https://callback.tootie.tv/callback/node-a",
            &[String::from("https://callback.tootie.tv/callback/*")]
        ));
        assert!(is_allowed_redirect_uri(
            "https://chatgpt.com/connector/oauth/test-callback-id",
            &[String::from("https://chatgpt.com/connector/oauth/*")]
        ));
    }

    #[test]
    fn all_https_redirect_pattern_allows_any_https_callback_only() {
        assert!(is_allowed_redirect_uri(
            "https://gemini.google.com/mcp/oauth/callback",
            &[String::from("https://*")]
        ));
        assert!(is_allowed_redirect_uri(
            "https://example.deeply.nested.client.invalid/path/callback?state=ok",
            &[String::from("https://*")]
        ));
        assert!(!is_allowed_redirect_uri(
            "http://example.deeply.nested.client.invalid/path/callback",
            &[String::from("https://*")]
        ));
    }

    #[tokio::test]
    async fn authorize_persists_full_state_and_redirects_to_google() {
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(location.contains("prompt=consent"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authorize_logs_only_sanitized_provider_endpoint_and_state_fingerprint() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=raw-client-state&scope=lab&code_challenge=raw-client-verifier&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);

        let location = Url::parse(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let query: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
        let provider_state = query.get("state").expect("provider state");
        let provider_code_challenge = query
            .get("code_challenge")
            .expect("provider PKCE challenge");
        let logs = crate::test_support::captured_logs(buf);

        for secret in [
            "raw-client-state",
            "raw-client-verifier",
            provider_state,
            provider_code_challenge,
        ] {
            assert!(
                !logs.contains(secret),
                "OAuth authorization secret leaked into logs: {secret}\n{logs}"
            );
            let encoded: String = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
            assert!(
                !logs.contains(&encoded),
                "encoded OAuth authorization secret leaked into logs: {encoded}\n{logs}"
            );
        }
        assert!(
            logs.contains(
                "\"provider_authorization_endpoint\":\"https://accounts.google.com/o/oauth2/v2/auth\""
            ),
            "{logs}"
        );
        assert!(logs.contains("\"oauth_state_id\":"), "{logs}");
        assert!(!logs.contains("\"location\":"), "{logs}");
    }

    #[tokio::test]
    async fn authorize_validates_redirect_against_cimd_document_and_persists_reference() {
        let mut config = test_auth_config();
        config.allowed_client_redirect_uris =
            vec!["https://chatgpt.com/connector/oauth/*".to_string()];
        let state = test_auth_state_with_config(config).await;
        let client_id = "https://chatgpt.com/oauth/test-client/client.json";
        state.cimd_cache.insert(
            client_id.to_string(),
            (
                RegisteredClient {
                    client_id: client_id.to_string(),
                    redirect_uris: vec![
                        "https://chatgpt.com/connector/oauth/test-client".to_string(),
                    ],
                    created_at: now_unix(),
                    token_endpoint_auth_method: "none".to_string(),
                    token_endpoint_auth_methods: Vec::new(),
                    jwks: None,
                    jwks_uri: None,
                },
                now_unix() + 60,
            ),
        );

        let app = router(state.clone());
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=https%3A%2F%2Fchatgpt.com%2Foauth%2Ftest-client%2Fclient.json&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector%2Foauth%2Fother-client&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(rejected.headers().get(header::LOCATION).is_none());

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=https%3A%2F%2Fchatgpt.com%2Foauth%2Ftest-client%2Fclient.json&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector%2Foauth%2Ftest-client&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
        assert!(state.store.find_client(client_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn authorize_omits_forced_consent_once_the_allowed_account_has_a_provider_credential() {
        let state = test_auth_state_with_registered_client().await;
        seed_provider_credential(&state, "client-id", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(!location.contains("prompt="));
    }

    #[tokio::test]
    async fn authorize_forces_consent_when_provider_credential_belongs_to_another_google_client() {
        let state = test_auth_state_with_registered_client().await;
        seed_provider_credential(&state, "old-google-client", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("prompt=consent"));
    }

    #[tokio::test]
    async fn authorize_reuses_the_allowed_account_credential_for_a_new_downstream_client() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .register_client(RegisteredClient {
                client_id: "other-client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:8888/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        seed_provider_credential(&state, "client-id", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=other-client&redirect_uri=http://127.0.0.1:8888/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(
            !location.contains("prompt="),
            "new downstream clients must reuse the sole allowed account's provider credential \
             instead of minting another Google refresh token"
        );
    }

    #[tokio::test]
    async fn authorize_forces_consent_when_multiple_accounts_are_allowed_even_with_a_provider_credential()
     {
        let state = test_auth_state_with_registered_client().await;
        // A second allowed Google account, on top of the default admin_email —
        // resolve_allowed_emails() now returns 2 entries.
        state
            .store
            .add_allowed_user("second-admin@example.com", "admin", now_unix())
            .await
            .unwrap();
        // One allowed account already has a provider credential, but the
        // selected Google subject is unknown until the callback returns.
        seed_provider_credential(&state, "client-id", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(
            location.contains("prompt=consent"),
            "with more than one allowed Google account, a provider credential must not \
             suppress consent because it may belong to a different selected account"
        );
    }

    #[tokio::test]
    async fn authorize_accepts_configured_protected_resource_scopes() {
        let state = test_auth_state_with_registered_client().await;
        state.set_allowed_resource_scopes([(
            "https://mcp.example.com/syslog".to_string(),
            vec!["mcp:read".to_string(), "mcp:write".to_string()],
        )]);
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&resource=https%3A%2F%2Fmcp.example.com%2Fsyslog&scope=mcp%3Aread%20mcp%3Awrite&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
    }

    #[tokio::test]
    async fn authorize_is_rate_limited_after_configured_burst() {
        let mut config = test_auth_config();
        config.authorize_requests_per_minute = 1;
        let state = test_auth_state_with_config(config).await;
        state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        let app = router(state);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::FOUND);

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=def&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn browser_login_starts_upstream_flow_and_persists_return_to_state() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let state = test_auth_state().await;
        let app = router(state.clone());
        let return_to_secret = "return-to-query-secret";
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/login?return_to=%2Fgateways%2F%3Ftab%3Dlab%26token%3D{return_to_secret}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = Url::parse(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert!(
            !location.query_pairs().any(|(key, _)| key == "access_type"),
            "browser login must not request an offline refresh credential"
        );
        assert!(!location.query_pairs().any(|(key, _)| key == "prompt"));
        let upstream_state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();
        let stored = state
            .store
            .take_browser_login_state(&upstream_state)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.return_to,
            format!("/gateways/?tab=lab&token={return_to_secret}")
        );
        let logs = crate::test_support::captured_logs(buf);
        assert!(
            !logs.contains(return_to_secret),
            "browser return_to query leaked into logs: {logs}"
        );
    }

    #[tokio::test]
    async fn browser_login_rejects_when_pending_oauth_state_cap_is_reached() {
        let mut config = test_auth_config();
        config.max_pending_oauth_states = 1;
        let state = test_auth_state_with_config(config).await;
        state
            .store
            .insert_browser_login_state(crate::types::BrowserLoginStateRow {
                state: "existing-login".to_string(),
                return_to: "/".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2Fgateways%2F")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn callback_rejects_expired_or_mismatched_state() {
        let app = router(test_auth_state_with_mock_google().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=bad-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn callback_rejects_shared_db_provider_runtime_mismatch_before_exchange() {
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .activate_inbound_provider(
                "authelia",
                "https://auth.example.test",
                "switched",
                now_unix(),
            )
            .await
            .unwrap();
        state
            .store
            .insert_browser_login_state(crate::types::BrowserLoginStateRow {
                state: "provider-mismatch".into(),
                return_to: "/".into(),
                provider_code_verifier: "verifier".into(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=provider-mismatch&code=must-not-be-exchanged")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn stale_same_issuer_runtime_cannot_create_provider_state_in_new_generation() {
        let state = test_auth_state_with_mock_google().await;
        state
            .store
            .activate_inbound_provider(
                "google",
                crate::google::GOOGLE_ISSUER,
                "new-client-config",
                now_unix(),
            )
            .await
            .unwrap();
        let response = router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))))
            .oneshot(
                Request::builder()
                    .uri("/auth/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authelia_callback_completes_downstream_code_and_token_flow() {
        let upstream_state = "authelia-e2e-state";
        let (provider, _server) = crate::authelia::tests::mock_provider_for_nonce(
            &crate::util::fingerprint(upstream_state),
        )
        .await;
        let base = test_auth_state_with_registered_client().await;
        base.store
            .activate_inbound_provider("authelia", provider.issuer(), "authelia-e2e", now_unix())
            .await
            .unwrap();
        let state = AuthState::for_tests_with_provider(
            (*base.config).clone(),
            base.store.clone(),
            (*base.signing_keys).clone(),
            crate::oauth_provider::InboundProviderRuntime::Authelia(Box::new(provider)),
            base.store
                .inbound_provider_state()
                .await
                .unwrap()
                .generation,
        );
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: upstream_state.into(),
                client_id: "client".into(),
                redirect_uri: "http://127.0.0.1:7777/callback".into(),
                client_state: "downstream-state".into(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".into(),
                scope: "lab".into(),
                provider_code_verifier: "upstream-verifier".into(),
                code_challenge: base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(Sha256::digest(b"downstream-verifier")),
                code_challenge_method: "S256".into(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let app = router(state);
        let callback = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/oidc/callback?state={upstream_state}&code=provider-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        let location = callback
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        let redirect = Url::parse(location).unwrap();
        let code = redirect
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        let token = app.oneshot(Request::builder().method("POST").uri("/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("grant_type=authorization_code&code={code}&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=downstream-verifier")))
            .unwrap()).await.unwrap();
        assert_eq!(token.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authelia_authorization_does_not_inherit_google_consent_policy() {
        let (state, _server) = test_auth_state_with_mock_authelia("unused-nonce-state").await;
        let response = router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1234))))
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=client-state&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        let query: std::collections::HashMap<_, _> = Url::parse(location)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert!(!query.contains_key("prompt"), "{location}");
    }

    #[tokio::test]
    async fn authelia_browser_callback_persists_bound_session() {
        let upstream_state = "authelia-browser-state";
        let (state, _server) = test_auth_state_with_mock_authelia(upstream_state).await;
        state
            .store
            .insert_browser_login_state(crate::types::BrowserLoginStateRow {
                state: upstream_state.into(),
                return_to: "/gateway".into(),
                provider_code_verifier: "upstream-verifier".into(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let response = router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/oidc/callback?state={upstream_state}&code=provider-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/gateway"
        );
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        let session_id = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
        let session = state
            .store
            .find_bound_browser_session(session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            session.binding.identity_issuer,
            state.inbound_provider.issuer()
        );
        assert_eq!(session.value.subject, "authelia-subject-123");
    }

    #[tokio::test]
    async fn authelia_native_callback_publishes_one_bound_poll_result() {
        let upstream_state = "authelia-native-state";
        let poll_token = "native-poll-secret";
        let (state, _server) = test_auth_state_with_mock_authelia(upstream_state).await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: upstream_state.into(),
                client_id: "client".into(),
                redirect_uri: crate::metadata::native_callback_endpoint(&state),
                client_state: "downstream-state".into(),
                native_poll_token_hash: Some(native_poll_token_hash_for(poll_token)),
                resource: "https://lab.example.com/mcp".into(),
                scope: "lab".into(),
                provider_code_verifier: "upstream-verifier".into(),
                code_challenge: base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(Sha256::digest(b"downstream-verifier")),
                code_challenge_method: "S256".into(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let app = router(state);
        let callback = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/oidc/callback?state={upstream_state}&code=provider-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::OK);
        let poll = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/native/poll")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"poll_token":"{poll_token}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oauth_callback_generation_loss_cannot_issue_code_over_fresh_credential() {
        let base_state = test_auth_state_with_registered_client().await;
        let now = now_unix();
        base_state
            .store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "google-subject-123".to_string(),
                email: Some("admin@example.com".to_string()),
                client_id: "client-id".to_string(),
                granted_scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
                access_token: "existing-provider-access".to_string(),
                refresh_token: "existing-provider-refresh".to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some("https://accounts.google.com".to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
        base_state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "generation-loss-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "generation-loss-client-state".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();

        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "late-provider-refresh",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        let state = AuthState::for_tests(
            (*base_state.config).clone(),
            base_state.store.clone(),
            (*base_state.signing_keys).clone(),
            google,
        );

        super::CALLBACK_CAS_PAUSE_ENABLED.store(true, std::sync::atomic::Ordering::Release);
        let request_state = state.clone();
        let response_task = tokio::spawn(async move {
            router(request_state)
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=generation-loss-state&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::CALLBACK_CAS_OBSERVED.acquire(),
        )
        .await
        .expect("callback reached generation CAS")
        .unwrap()
        .forget();
        let generation = state
            .store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .unwrap()
            .generation;
        let peer_store = crate::sqlite::SqliteStore::open_with_key(
            state.config.sqlite_path.clone(),
            state.config.token_encryption_key.clone(),
        )
        .await
        .unwrap();
        let now = now_unix();
        assert!(
            peer_store
                .replace_google_provider_token_bundle_if_generation(
                    GoogleProviderCredentialUpdate {
                        subject: "google-subject-123".to_string(),
                        email: Some("user@example.com".to_string()),
                        client_id: "client-id".to_string(),
                        granted_scopes: vec!["openid".to_string(), "email".to_string()],
                        access_token: "fresh-provider-access".to_string(),
                        refresh_token: "fresh-provider-refresh".to_string(),
                        token_received_at: now,
                        access_token_expires_at: now + 3600,
                        issuer: Some("https://accounts.google.com".to_string()),
                        refreshed: false,
                        scope_upgraded: true,
                    },
                    generation
                )
                .await
                .unwrap()
        );
        super::CALLBACK_CAS_PAUSE_ENABLED.store(false, std::sync::atomic::Ordering::Release);
        super::CALLBACK_CAS_RESUME.add_permits(1);
        let response = response_task.await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        let parameters = Url::parse(location)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            parameters.get("error").map(String::as_str),
            Some("server_error")
        );
        assert!(parameters.contains_key("iss"));
        assert!(!parameters.contains_key("code"));
        let credential = state
            .store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(credential.refresh_token, "fresh-provider-refresh");
    }

    #[tokio::test]
    async fn browser_login_callback_sets_session_cookie_and_redirects_home() {
        let state = test_auth_state_with_mock_google().await;
        let app = router(state.clone());
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2Fgateways%2F")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let location = Url::parse(
            login
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let upstream_state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();

        let callback = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/google/callback?state={upstream_state}&code=upstream-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            callback.headers().get(header::LOCATION).unwrap(),
            "/gateways/"
        );
        let cookie = callback
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .find_map(|value| value.to_str().ok())
            .unwrap();
        assert!(cookie.contains("lab_session="));
    }

    #[tokio::test]
    async fn oauth_client_callback_redirects_with_access_denied_when_email_not_in_allowlist() {
        let mut config = test_auth_config();
        config.admin_email = "allowed@example.com".to_string();
        let base_state = test_auth_state_with_config(config).await;
        base_state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        // Pre-insert an authorization request (OAuth-client flow, not browser-login).
        base_state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "good-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "client-abc".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();

        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(), // email=user@example.com, not in allowlist
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;

        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

        let state = AuthState::for_tests(
            (*base_state.config).clone(),
            base_state.store.clone(),
            (*base_state.signing_keys).clone(),
            google,
        );
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=good-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Must redirect (not 401) with error=access_denied and the original client state.
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        let redirect = Url::parse(location).unwrap();
        let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
        assert_eq!(
            params.get("error").map(|v| v.as_ref()),
            Some("access_denied")
        );
        assert_eq!(params.get("state").map(|v| v.as_ref()), Some("client-abc"));
        assert_eq!(
            params.get("iss").map(|v| v.as_ref()),
            Some("https://lab.example.com")
        );
    }

    #[tokio::test]
    async fn browser_login_callback_rejects_email_not_in_allowlist() {
        let mut config = test_auth_config();
        // "allowed@example.com" is permitted; the mock id_token returns
        // "user@example.com" → callback must be denied with 401.
        config.admin_email = "allowed@example.com".to_string();
        let base_state = test_auth_state_with_config(config).await;
        base_state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();

        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;

        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

        let state = AuthState::for_tests(
            (*base_state.config).clone(),
            base_state.store.clone(),
            (*base_state.signing_keys).clone(),
            google,
        );
        let app = router(state.clone());

        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2F")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let location = Url::parse(
            login
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let upstream_state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();

        let callback = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/google/callback?state={upstream_state}&code=upstream-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_rejects_missing_or_invalid_response_type() {
        let app = router(test_auth_state_with_registered_client().await);
        for uri in [
            "/authorize?client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256",
            "/authorize?response_type=token&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_authorization_error(&response, "unsupported_response_type");
        }
    }

    /// OpenAI auth clause OAI-CLAUSE-014: the production authorization
    /// endpoint supports only the authorization-code flow and requires PKCE
    /// with the S256 transformation. This exercises the HTTP adapter, not just
    /// the policy helper.
    #[tokio::test]
    async fn authorization_endpoint_requires_code_flow_and_pkce_s256() {
        let app = router(test_auth_state_with_registered_client().await);
        let base = "/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab";

        let accepted = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "{base}&code_challenge=pkce&code_challenge_method=S256"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::FOUND);

        for method in ["plain", "s256", "S512"] {
            let rejected = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "{base}&code_challenge=pkce&code_challenge_method={method}"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_authorization_error(&rejected, "invalid_request");
        }

        for missing_pkce_parameter in [
            format!("{base}&code_challenge=pkce"),
            format!("{base}&code_challenge_method=S256"),
        ] {
            let rejected = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(missing_pkce_parameter)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn validate_scope_accepts_supported_scopes_and_rejects_others() {
        let state = test_auth_state().await;
        let canonical = crate::metadata::canonical_resource_url(&state);
        // Empty scope falls back to the least-privilege configured default.
        assert_eq!(
            super::validate_scope(&state, &canonical, "").unwrap(),
            "lab:read"
        );
        assert_eq!(
            super::validate_scope(&state, &canonical, "lab:read").unwrap(),
            "lab:read"
        );
        // Base scope passes.
        assert_eq!(
            super::validate_scope(&state, &canonical, "lab").unwrap(),
            "lab"
        );
        // OAuth scopes are a request ceiling. Requesting an advertised scope
        // does not establish the caller's durable role or membership.
        assert_eq!(
            super::validate_scope(&state, &canonical, "lab:admin").unwrap(),
            "lab:admin"
        );
        // Anything not in scopes_supported is rejected.
        let err = super::validate_scope(&state, &canonical, "lab:write").unwrap_err();
        assert!(err.to_string().contains("lab"), "got: {err}");
    }

    #[tokio::test]
    async fn omitted_initial_scope_defaults_to_least_privilege_read_only_scope() {
        let state = test_auth_state().await;
        let canonical = crate::metadata::canonical_resource_url(&state);
        let selected = super::validate_scope(&state, &canonical, "").unwrap();
        assert_eq!(selected, "lab:read");
        assert!(!selected.split_whitespace().any(|scope| scope == "lab"));
        assert!(
            !selected
                .split_whitespace()
                .any(|scope| scope == "lab:admin")
        );
    }

    #[tokio::test]
    async fn authorize_rejects_nonidentical_registered_redirect_without_redirecting() {
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback/extra&state=abc&scope=lab:read&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(response.headers().get(header::LOCATION).is_none());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["kind"], "validation_failed");
        assert!(
            json["message"]
                .as_str()
                .is_some_and(|message| message.contains("does not match"))
        );
    }

    #[tokio::test]
    async fn authorize_rejects_invalid_scope() {
        let app = router(test_auth_state_with_registered_client().await);
        // `lab:write` is NOT in default scopes_supported; should be rejected.
        // (`lab:admin` IS in scopes_supported as of 2026-05; use a different
        // unsupported scope here.)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab:write&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_authorization_error(&response, "invalid_scope");
    }

    #[tokio::test]
    async fn authorize_rejects_mismatched_resource_parameter() {
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&resource=https://other.example.com/mcp&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_authorization_error(&response, "invalid_target");
    }

    #[tokio::test]
    async fn callback_rejects_expired_state() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "expired-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "client-state".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix() - 300,
                expires_at: now_unix() - 1,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=expired-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    pub async fn test_auth_state() -> AuthState {
        test_auth_state_with_config(test_auth_config()).await
    }

    pub async fn test_auth_state_with_config(config: AuthConfig) -> AuthState {
        AuthState::new(config).await.unwrap()
    }

    pub(crate) fn test_auth_config() -> AuthConfig {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("bootstrap-secret".to_string()),
            enable_dynamic_registration: true,
            allowed_client_redirect_uris: Vec::new(),
            // Matches the mock id_token email returned by signed_test_id_token,
            // so happy-path callback tests pass the allowlist check.
            admin_email: "user@example.com".to_string(),
            google: GoogleConfig {
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
            token_encryption_key: Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "test-google-provider-encryption-key",
            )),
            ..AuthConfig::default()
        }
    }

    pub async fn test_auth_state_with_registered_client() -> AuthState {
        let state = test_auth_state().await;
        state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        state
    }

    pub(crate) async fn test_auth_state_with_mock_google() -> AuthState {
        let state = test_auth_state_with_registered_client().await;
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "good-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "client-state".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        )
    }

    async fn test_auth_state_with_mock_authelia(upstream_state: &str) -> (AuthState, MockServer) {
        let (provider, server) = crate::authelia::tests::mock_provider_for_nonce(
            &crate::util::fingerprint(upstream_state),
        )
        .await;
        let base = test_auth_state_with_registered_client().await;
        base.store
            .activate_inbound_provider("authelia", provider.issuer(), "authelia-e2e", now_unix())
            .await
            .unwrap();
        (
            AuthState::for_tests_with_provider(
                (*base.config).clone(),
                base.store.clone(),
                (*base.signing_keys).clone(),
                crate::oauth_provider::InboundProviderRuntime::Authelia(Box::new(provider)),
                base.store
                    .inbound_provider_state()
                    .await
                    .unwrap()
                    .generation,
            ),
            server,
        )
    }

    /// Same mocked-Google harness as [`test_auth_state_with_mock_google`], but
    /// the pending authorization request's `redirect_uri` is the server's own
    /// `native_callback_endpoint` — exercising the native-flow branch of
    /// `callback()` instead of the normal client-redirect branch.
    async fn test_auth_state_with_mock_google_native() -> AuthState {
        let state = test_auth_state().await;
        let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
        state
            .store
            .register_client(RegisteredClient {
                client_id: "native-client".to_string(),
                redirect_uris: vec![native_callback_endpoint.clone()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "native-good-state".to_string(),
                client_id: "native-client".to_string(),
                redirect_uri: native_callback_endpoint,
                client_state: "native-client-state".to_string(),
                native_poll_token_hash: Some(native_poll_token_hash_for("legitimate-poll-token")),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        )
    }

    pub(crate) fn signed_test_id_token() -> String {
        let claims = json!({
            "iss": "https://accounts.google.com",
            "aud": "client-id",
            "sub": "google-subject-123",
            "email": "user@example.com",
            "email_verified": true,
            "iat": now_unix() as usize,
            "exp": (now_unix() + 3600) as usize,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &test_encoding_key()).unwrap()
    }

    pub(crate) fn test_jwks() -> serde_json::Value {
        let key = test_rsa_key();
        let public_key = key.to_public_key();
        json!({
            "keys": [{
                "kid": "test-kid",
                "alg": "RS256",
                "kty": "RSA",
                "use": "sig",
                "n": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.n_bytes()),
                "e": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.e_bytes()),
            }]
        })
    }

    fn test_rsa_key() -> RsaPrivateKey {
        use std::sync::OnceLock;

        static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
        KEY.get_or_init(|| {
            let mut rng = rand::rng();
            RsaPrivateKey::new(&mut rng, 2048).expect("generate Google RS256 fixture key")
        })
        .clone()
    }

    fn test_encoding_key() -> EncodingKey {
        let pem = test_rsa_key().to_pkcs8_pem(LineEnding::LF).unwrap();
        EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap()
    }

    /// Tests that exercise the merged allowlist path through real callback handlers.
    /// These verify that `resolve_allowed_emails` is correctly wired at both call
    /// sites (browser-login branch and oauth-client branch).
    mod merged_allowlist_callback_tests {
        use axum::body::Body;
        use axum::http::{Request, StatusCode, header};
        use serde_json::json;
        use tower::util::ServiceExt;
        use url::Url;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        use super::{
            signed_test_id_token, test_auth_config, test_auth_state_with_config,
            test_auth_state_with_mock_google, test_jwks,
        };
        use crate::google::GoogleProvider;
        use crate::routes::router;
        use crate::state::AuthState;
        use crate::types::{AuthorizationRequestRow, BrowserLoginStateRow, RegisteredClient};
        use crate::util::now_unix;

        /// Helper that mounts Google mock endpoints on a fresh server and builds
        /// an `AuthState` with that mock, reusing an existing base state's store
        /// and signing keys (so DB writes made to `base_state.store` are visible).
        async fn state_with_mock_google_from(base_state: &AuthState) -> AuthState {
            let server = Box::leak(Box::new(MockServer::start().await));
            Mock::given(method("POST"))
                .and(path("/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "access_token": "google-access-token",
                    "refresh_token": "refresh-token",
                    "expires_in": 3600,
                    "id_token": signed_test_id_token(),
                })))
                .mount(server)
                .await;
            Mock::given(method("GET"))
                .and(path("/certs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
                .mount(server)
                .await;
            let google = GoogleProvider::new(
                "client-id".to_string(),
                "client-secret".to_string(),
                Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
            )
            .unwrap()
            .with_endpoints(
                server.uri().parse::<Url>().unwrap(),
                server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
            )
            .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
            AuthState::for_tests(
                (*base_state.config).clone(),
                base_state.store.clone(),
                (*base_state.signing_keys).clone(),
                google,
            )
        }

        /// The mock id_token always returns `user@example.com`. When admin is set
        /// to a *different* email and that address is added to `allowed_users`, the
        /// browser-login callback must succeed (DB row authorises the login).
        #[tokio::test]
        async fn browser_login_succeeds_for_allowlisted_non_admin_email() {
            let mut config = test_auth_config();
            // Set admin to something other than the id_token email.
            config.admin_email = "admin@example.com".to_string();
            let base_state = test_auth_state_with_config(config).await;

            // Insert id_token email into allowed_users.
            base_state
                .store
                .add_allowed_user("user@example.com", "admin", now_unix())
                .await
                .unwrap();

            let state = state_with_mock_google_from(&base_state).await;

            // Seed the browser-login state row so the callback recognises the flow.
            state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // Successful browser login → redirect with a Set-Cookie header (session).
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
        }

        /// Admin email is always authorised even when the `allowed_users` table is
        /// empty (browser-login branch).
        #[tokio::test]
        async fn browser_login_succeeds_for_admin_when_allowed_users_is_empty() {
            // Default test config sets admin_email = "user@example.com", which
            // matches the id_token returned by signed_test_id_token.
            let base_state = test_auth_state_with_mock_google().await;

            // Confirm no extra rows exist.
            assert!(
                base_state
                    .store
                    .list_allowed_users()
                    .await
                    .unwrap()
                    .is_empty()
            );

            // Seed browser-login state.
            base_state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state-2".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(base_state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state-2&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
        }

        async fn oauth_client_callback_location(codex_issuer_compatibility: bool) -> Url {
            let mut config = test_auth_config();
            config.admin_email = "admin@example.com".to_string();
            config.codex_issuer_compatibility = codex_issuer_compatibility;
            let base_state = test_auth_state_with_config(config).await;

            // Register a client.
            base_state
                .store
                .register_client(RegisteredClient {
                    client_id: "client".to_string(),
                    redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                    created_at: now_unix(),
                    token_endpoint_auth_method: "none".to_string(),
                    token_endpoint_auth_methods: Vec::new(),
                    jwks: None,
                    jwks_uri: None,
                })
                .await
                .unwrap();

            // Add id_token email to allowed_users.
            base_state
                .store
                .add_allowed_user("user@example.com", "admin", now_unix())
                .await
                .unwrap();

            let state = state_with_mock_google_from(&base_state).await;

            // Seed an authorization request row.
            state
                .store
                .insert_authorization_request(AuthorizationRequestRow {
                    state: "oauth-state".to_string(),
                    client_id: "client".to_string(),
                    redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                    client_state: "client-xyz".to_string(),
                    native_poll_token_hash: None,
                    resource: "https://lab.example.com/mcp".to_string(),
                    scope: "lab".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    code_challenge: "challenge".to_string(),
                    code_challenge_method: "S256".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=oauth-state&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let location = response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap();
            Url::parse(location).unwrap()
        }

        /// The oauth-client callback must also succeed for a non-admin email that
        /// exists in `allowed_users`.
        #[tokio::test]
        async fn oauth_client_callback_succeeds_for_allowlisted_non_admin_email() {
            let redirect = oauth_client_callback_location(false).await;
            let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
            assert!(
                params.contains_key("code"),
                "expected code in redirect: {redirect}"
            );
            assert_eq!(
                params.get("state").map(|value| value.as_ref()),
                Some("client-xyz")
            );
            assert_eq!(
                params.get("iss").map(|value| value.as_ref()),
                Some("https://lab.example.com")
            );
            assert!(
                !params.contains_key("error"),
                "unexpected error in redirect: {redirect}"
            );
        }

        #[tokio::test]
        async fn oauth_client_callback_omits_issuer_in_explicit_codex_compatibility_mode() {
            let redirect = oauth_client_callback_location(true).await;
            let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
            assert!(params.contains_key("code"));
            assert_eq!(
                params.get("state").map(|value| value.as_ref()),
                Some("client-xyz")
            );
            assert!(!params.contains_key("iss"));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn oauth_client_callback_logs_redact_redirect_query_values() {
            let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
            let buf = crate::test_support::global_tracing_buffer();
            let redirect = oauth_client_callback_location(false).await;
            let params: std::collections::HashMap<_, _> =
                redirect.query_pairs().into_owned().collect();
            let authorization_code = params.get("code").unwrap();

            let logs = crate::test_support::captured_logs(&buf);
            for secret in [
                authorization_code.as_str(),
                "client-xyz",
                "iss=https%3A%2F%2Flab.example.com",
                redirect.as_str(),
            ] {
                assert!(
                    !logs.contains(secret),
                    "OAuth redirect secret leaked into debug logs: {secret}\n{logs}"
                );
            }
            assert!(logs.contains("\"redirect_path\":\"/callback\""), "{logs}");
        }

        /// Email not in admin or allowed_users must be rejected in the browser-login
        /// branch (401 Unauthorized).
        #[tokio::test]
        async fn browser_login_rejects_email_absent_from_both_admin_and_db() {
            let mut config = test_auth_config();
            // Neither admin nor allowed_users contains "user@example.com" (the id_token email).
            config.admin_email = "admin@example.com".to_string();
            let base_state = test_auth_state_with_config(config).await;

            let state = state_with_mock_google_from(&base_state).await;

            state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state-3".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state-3&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        /// Admin also in the DB table must not appear twice (dedup check via
        /// resolve_allowed_emails, verified indirectly: the callback still succeeds
        /// and there is no panic from duplicate iteration).
        #[tokio::test]
        async fn admin_in_db_table_is_deduped_and_still_authorised() {
            // Default config: admin_email = "user@example.com".
            let base_state = test_auth_state_with_mock_google().await;

            // Also add the admin email to allowed_users — this is the duplicate.
            base_state
                .store
                .add_allowed_user("user@example.com", "self", now_unix())
                .await
                .unwrap();

            // Seed browser-login state.
            base_state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state-4".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(base_state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state-4&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // Must still succeed — dedup should not break the check.
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
        }
    }

    mod allowlist_tests {
        use super::super::check_email_allowlist;

        #[test]
        fn empty_allowlist_permits_any_email() {
            assert!(
                check_email_allowlist(Some("anyone@example.com"), Some(true), None, &[], &[])
                    .is_ok()
            );
        }

        #[test]
        fn empty_allowlist_permits_even_unverified_email() {
            // When no allowlist is configured, email_verified is not enforced.
            assert!(
                check_email_allowlist(Some("anyone@example.com"), Some(false), None, &[], &[])
                    .is_ok()
            );
        }

        #[test]
        fn matching_verified_email_is_permitted() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("alice@example.com"), Some(true), None, &list, &[])
                    .is_ok()
            );
        }

        #[test]
        fn matching_email_is_case_insensitive() {
            // Allowlist is pre-normalized to lowercase at config load.
            // Incoming email from Google may have any case.
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("Alice@Example.com"), Some(true), None, &list, &[])
                    .is_ok()
            );
        }

        #[test]
        fn matching_hosted_domain_is_permitted() {
            // A Workspace account whose `hd` matches an allowed domain gets in
            // without being listed individually.
            assert!(
                check_email_allowlist(
                    Some("newhire@lime-technology.com"),
                    Some(true),
                    Some("lime-technology.com"),
                    &["admin@example.com".to_string()],
                    &["lime-technology.com".to_string()],
                )
                .is_ok()
            );
        }

        #[test]
        fn hosted_domain_match_is_case_insensitive() {
            assert!(
                check_email_allowlist(
                    Some("newhire@lime-technology.com"),
                    Some(true),
                    Some("Lime-Technology.COM"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_ok()
            );
        }

        #[test]
        fn hosted_domain_must_be_verified() {
            // An unverified address is rejected even when `hd` matches.
            assert!(
                check_email_allowlist(
                    Some("newhire@lime-technology.com"),
                    Some(false),
                    Some("lime-technology.com"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn address_suffix_alone_does_not_grant_domain_access() {
            // The whole point of keying on `hd`: a consumer account cannot claim
            // a Workspace domain, so a lookalike address must not be admitted.
            assert!(
                check_email_allowlist(
                    Some("attacker@lime-technology.com"),
                    Some(true),
                    None,
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn lookalike_hosted_domain_is_rejected() {
            assert!(
                check_email_allowlist(
                    Some("attacker@evil-lime-technology.com"),
                    Some(true),
                    Some("evil-lime-technology.com"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn subdomain_of_allowed_domain_is_rejected() {
            assert!(
                check_email_allowlist(
                    Some("attacker@sub.lime-technology.com"),
                    Some(true),
                    Some("sub.lime-technology.com"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn domain_allowlist_alone_still_enforces_the_gate() {
            // With only a domain configured, a non-member is still rejected.
            assert!(
                check_email_allowlist(
                    Some("outsider@example.com"),
                    Some(true),
                    None,
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn non_matching_email_is_rejected() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("eve@example.com"), Some(true), None, &list, &[])
                    .is_err()
            );
        }

        #[test]
        fn unverified_email_is_rejected_even_when_in_allowlist() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("alice@example.com"), Some(false), None, &list, &[])
                    .is_err()
            );
        }

        #[test]
        fn missing_email_verified_claim_is_rejected_when_allowlist_is_set() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("alice@example.com"), None, None, &list, &[]).is_err()
            );
        }

        #[test]
        fn none_email_is_rejected_when_allowlist_is_set() {
            let list = vec!["alice@example.com".to_string()];
            assert!(check_email_allowlist(None, Some(true), None, &list, &[]).is_err());
        }
    }
}
