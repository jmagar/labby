//! Dispatch layer for the `deploy` service.
//!
//! All orchestration for the deploy flow lives here. The `mcp/services/deploy.rs`
//! and `cli/deploy.rs` surfaces are thin adapters that go through this module.

pub mod authz;
pub mod build;
pub mod catalog;
pub mod client;
pub mod dispatch;
pub mod host_io;
pub mod lock;
pub mod monitor;
pub mod params;
pub mod runner;
pub mod ssh_session;
pub mod stages;

// Re-exported for surfaces (CLI, MCP, API) that import from this module.
// `unused_imports` is allowed because each surface uses a subset depending on
// which entry point it calls — unused arms appear in narrow feature builds.
#[allow(unused_imports)]
pub use catalog::ACTIONS;
#[allow(unused_imports)]
pub use dispatch::{dispatch, dispatch_mcp, dispatch_with_runner};
