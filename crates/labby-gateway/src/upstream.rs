//! Upstream MCP server proxy — shared types and connection pool.
//!
//! This is the runtime home of `UpstreamPool`. It is re-exported from `lab`'s
//! `crate::dispatch::upstream` as a compatibility shim so every existing surface
//! (CLI, MCP, HTTP API) keeps the same import path. The pool is surface-neutral:
//! both the MCP and API surfaces need access to it, and the layer contract
//! forbids `api -> mcp` dependencies, so it cannot live under either surface.
//
// Many items in pool and types are not yet called from outside the module
// (discovery, resource proxying, circuit breaker probing). They are exercised
// by tests and will be fully wired when `labby serve` gains `[[upstream]]` config
// support. The blanket allow prevents false-positive warnings on partially
// wired public APIs.
#[allow(dead_code)]
pub mod auth;
#[allow(dead_code)]
pub mod http_client;
#[allow(dead_code)]
pub mod pool;
#[allow(dead_code)]
pub mod process_guard;
pub use crate::security::spawn_guard;
#[allow(dead_code)]
pub mod transport;
#[allow(dead_code)]
pub mod types;
