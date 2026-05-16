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
pub mod extract;
pub mod gateway;
pub mod logs;
pub mod marketplace;
pub mod setup;
pub mod stash;

#[cfg(feature = "radarr")]
pub mod radarr;

#[cfg(feature = "sonarr")]
pub mod sonarr;

#[cfg(feature = "prowlarr")]
pub mod prowlarr;

#[cfg(feature = "plex")]
pub mod plex;

#[cfg(feature = "tautulli")]
pub mod tautulli;

#[cfg(feature = "sabnzbd")]
pub mod sabnzbd;

#[cfg(feature = "qbittorrent")]
pub mod qbittorrent;

#[cfg(feature = "tailscale")]
pub mod tailscale;

#[cfg(feature = "linkding")]
pub mod linkding;

#[cfg(feature = "beads")]
pub mod beads;
#[cfg(feature = "memos")]
pub mod memos;
#[cfg(feature = "mcpregistry")]
pub mod registry_v01;

#[cfg(feature = "bytestash")]
pub mod bytestash;

#[cfg(feature = "paperless")]
pub mod paperless;

#[cfg(feature = "arcane")]
pub mod arcane;

#[cfg(feature = "unraid")]
pub mod unraid;

#[cfg(feature = "unifi")]
pub mod unifi;

#[cfg(feature = "overseerr")]
pub mod overseerr;

#[cfg(feature = "gotify")]
pub mod gotify;

#[cfg(feature = "openai")]
pub mod openai;

#[cfg(feature = "openacp")]
pub mod openacp;

#[cfg(feature = "notebooklm")]
pub mod notebooklm;

#[cfg(feature = "qdrant")]
pub mod qdrant;

#[cfg(feature = "tei")]
pub mod tei;

#[cfg(feature = "apprise")]
pub mod apprise;

#[cfg(feature = "fs")]
pub mod fs;

#[cfg(feature = "dozzle")]
pub mod dozzle;

#[cfg(feature = "immich")]
pub mod immich;

#[cfg(feature = "jellyfin")]
pub mod jellyfin;

#[cfg(feature = "navidrome")]
pub mod navidrome;

#[cfg(feature = "scrutiny")]
pub mod scrutiny;

#[cfg(feature = "freshrss")]
pub mod freshrss;

#[cfg(feature = "loggifly")]
pub mod loggifly;

#[cfg(feature = "adguard")]
pub mod adguard;

#[cfg(feature = "glances")]
pub mod glances;

#[cfg(feature = "uptime_kuma")]
pub mod uptime_kuma;

#[cfg(feature = "pihole")]
pub mod pihole;

#[cfg(feature = "neo4j")]
pub mod neo4j;
