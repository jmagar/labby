//! Outbound OAuth support for upstream MCP servers.
//!
//! The outbound upstream OAuth runtime now lives in the reusable `lab-auth`
//! crate (`labby_auth::upstream`). This module is a compatibility shim that
//! re-exports it so existing `crate::oauth::upstream::*` import paths keep
//! working. New code should import from `labby_auth::upstream` directly.

pub use labby_auth::upstream::*;
