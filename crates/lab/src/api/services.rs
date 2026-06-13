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

/// Admin-only allowlist management (`/v1/auth/allowed-emails`).
pub mod auth_admin;

pub mod acp;
/// `GET /v1/catalog` — filtered service+action catalog for the ⌘K palette.
pub mod catalog;
pub mod doctor;
#[cfg(feature = "gateway")]
pub mod gateway;
pub mod logs;
#[cfg(feature = "marketplace")]
pub mod marketplace;
pub mod setup;
#[cfg(feature = "gateway")]
pub mod snippets;
pub mod stash;

#[cfg(feature = "marketplace")]
pub mod registry_v01;

#[cfg(feature = "fs")]
pub mod fs;
