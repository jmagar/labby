//! Upstream MCP server proxy — shared types and connection pool.
//!
//! Lives in `dispatch/` (not `mcp/`) because both the MCP and API surfaces
//! need access to `UpstreamPool`. The layer contract forbids `api -> mcp`
//! dependencies, so shared types must live in the dispatch layer.
//
// Many items in pool and types are not yet called from outside the module
// (discovery, resource proxying, circuit breaker probing). They are exercised
// by tests and will be fully wired when `labby serve` gains `[[upstream]]` config
// support. The blanket allow prevents false-positive warnings on partially
// wired public APIs.
#[allow(dead_code)]
pub(crate) mod auth;
#[allow(dead_code)]
pub mod pool;
#[allow(dead_code)]
pub mod transport;
#[allow(dead_code)]
pub mod types;
