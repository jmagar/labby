//! Compatibility shim: the upstream MCP proxy pool now lives in the standalone
//! `lab-gateway` crate.
//!
//! Every existing `crate::dispatch::upstream::*` import path keeps working
//! through this re-export so the CLI, MCP, and HTTP API surfaces are unchanged.
//! New runtime work belongs in `lab-gateway`, not here.

pub use lab_gateway::upstream::*;
