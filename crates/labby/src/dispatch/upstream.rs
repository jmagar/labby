//! Temporary compatibility shim: the upstream MCP proxy pool now lives in the
//! `labby-gateway` crate.
//!
//! Every existing `crate::dispatch::upstream::*` import path keeps working
//! through this re-export during the extraction. New runtime work should import
//! `labby_gateway::upstream` directly.

pub use labby_gateway::upstream::*;
