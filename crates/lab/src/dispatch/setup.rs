//! Shared dispatch layer for the `setup` Bootstrap orchestrator.
//!
//! `setup` is a synthetic Bootstrap service: no external service URL, no
//! feature gate. All fs I/O lives here (per `lab-apis` SDK purity rule).
//! `setup.draft.commit` invokes `doctor.audit.full` inline; that is the
//! single sanctioned cross-service dispatch call (see the orchestrator
//! exception clause in `crates/lab/src/dispatch/CLAUDE.md`).

mod catalog;
mod claude_plugins;
mod client;
mod dispatch;
mod draft;
mod params;
mod plugin_hook;
mod secret_mask;
mod state;

pub use catalog::ACTIONS;
pub use dispatch::dispatch;
