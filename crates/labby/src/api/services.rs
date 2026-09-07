//! Per-service HTTP route handlers.
//!
//! Versioned REST and action-dispatch route modules for the HTTP API.
//!
//! Most service modules expose `pub fn routes(state: AppState) -> Router` that
//! mounts a `POST /` action-dispatch handler matching the MCP `action + params`
//! shape. Modules may also expose versioned REST routers such as
//! `registry_v01`, which serves `/v0.1/servers/*`.

/// Shared dispatch wrapper: confirmation gate, timing, logging.
pub mod helpers;
pub(crate) mod integration_identity;
pub mod local_session;

pub(crate) fn require_session_csrf(
    action: &str,
    headers: &axum::http::HeaderMap,
    auth: Option<&labby_auth::AuthContext>,
) -> Result<(), crate::dispatch::error::ToolError> {
    let valid = auth.is_some_and(|auth| {
        !auth.via_session
            || auth.csrf_token.as_deref().is_some_and(|expected| {
                headers
                    .get(labby_auth::session::BROWSER_CSRF_HEADER_NAME)
                    .and_then(|value| value.to_str().ok())
                    == Some(expected)
            })
    });
    valid
        .then_some(())
        .ok_or_else(|| crate::dispatch::error::ToolError::Forbidden {
            message: format!("{action} requires a valid session CSRF token"),
            required_scopes: vec!["lab:admin".to_owned()],
        })
}

/// Admin-only allowlist management (`/v1/auth/allowed-emails`).
pub mod auth_admin;
pub mod browser;

pub mod access;
/// Browser-session-only owner bootstrap (`/v1/access/bootstrap-owner`).
pub mod access_bootstrap;
pub mod access_bootstrap_proof;
pub mod access_credentials;
pub mod agents;
pub mod projects;
pub mod tasks;

/// `GET /v1/catalog` — filtered service+action catalog for the ⌘K palette.
pub mod catalog;
pub mod depot;
pub mod dev_containers;
pub mod doctor;
pub mod file_stash;
#[cfg(feature = "gateway")]
pub mod gateway;
pub mod oauth_relay;
#[cfg(feature = "gateway")]
pub mod palette;
#[cfg(feature = "skills")]
pub mod remote_control;
pub mod server_logs;
pub mod setup;
#[cfg(feature = "skills")]
pub mod skills;
#[cfg(feature = "gateway")]
pub mod snippets;

#[cfg(feature = "fs")]
pub mod fs;
