//! Temporary compatibility shim: the gateway runtime moved into the
//! `labby-gateway` crate.
//!
//! Business logic now lives in `labby_gateway::gateway`. This module re-exports
//! it so existing `crate::dispatch::gateway::*` callers keep working during the
//! extraction. New runtime work should import `labby_gateway::gateway` directly.
//! The host-owned config-store implementation (which keeps `LabConfig` and the
//! `config.toml` render path in `lab`) lives in `config_store`.

pub use labby_gateway::gateway::*;

pub mod config_store;
