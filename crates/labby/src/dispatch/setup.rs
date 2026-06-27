//! Shared dispatch layer for the `setup` Bootstrap orchestrator.
//!
//! `setup` is a synthetic Bootstrap service: no external service URL, no
//! feature gate. All fs I/O lives here (per `lab-apis` SDK purity rule).
//! `setup.draft.commit` invokes `doctor.audit.full` inline; that is the
//! single sanctioned cross-service dispatch call (see the orchestrator
//! exception clause in `crates/lab/src/dispatch/CLAUDE.md`).

mod bootstrap;
mod catalog;
mod claude_plugins;
mod client;
mod dispatch;
mod draft;
pub(crate) mod host_service;
mod params;
mod plugin_hook;
pub(crate) mod provision;
mod secret_mask;
mod settings;
mod state;
mod token;

pub use bootstrap::{BootstrapOutcome, bootstrap, bootstrap_action, should_bootstrap};
pub use catalog::{ACTIONS, PLUGIN_LIFECYCLE_ACTIONS};
pub use dispatch::dispatch;
