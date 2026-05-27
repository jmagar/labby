//! Workspace filesystem browser service.
//!
//! # Purpose
//!
//! Exposes a jailed read-only view of the user-configured workspace
//! directory so the admin chat UI can attach files by path (not by blob
//! upload) and the server-side ACP agent can read them directly.
//!
//! # Phase 1 scope (lab-f1t2.1)
//!
//! - Workspace-root resolution from `config.toml` `[workspace].root`.
//! - Structured `workspace_not_configured` error for callers.
//!
//! Phases 2 and 3 add `fs.list` (MCP + HTTP) and `fs.preview` (HTTP only)
//! on top of this scaffold.
//!
//! # Not in lab-apis
//!
//! This service is intentionally local-only with no `lab-apis` counterpart,
//! matching the `lab_admin` precedent. See `crates/lab/Cargo.toml` for the
//! feature flag.

pub mod catalog;
pub mod client;

#[cfg(feature = "fs")]
pub mod dispatch;
#[cfg(feature = "fs")]
pub mod params;

#[cfg(feature = "fs")]
pub(crate) use client::not_configured_error;

#[cfg(feature = "fs")]
pub use dispatch::{dispatch, dispatch_with_root, open_for_preview};
