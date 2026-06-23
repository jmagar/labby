//! MCP-specific service exception modules.
//!
//! Normal services register directly from `crate::dispatch::<service>` in
//! `crate::registry`. This module only declares adapters that own behavior
//! specific to the MCP surface and cannot be represented by shared dispatch
//! alone.

#[cfg(feature = "deploy")]
pub mod deploy;

pub mod stash;

// Device enrollment actions are MCP-only for now and live outside the shared
// service-dispatch pattern.
pub mod nodes;

#[cfg(feature = "fs")]
pub mod fs;
