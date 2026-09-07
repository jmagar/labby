//! Shared dispatch layer for the `setup` Bootstrap orchestrator.
//!
//! `setup` is a synthetic Bootstrap service: no external service URL, no
//! feature gate. All fs I/O lives here (per `labby-apis` SDK purity rule).
//! `setup.draft.commit` invokes `doctor.audit.full` inline; that is the
//! single sanctioned cross-service dispatch call (see the orchestrator
//! exception clause in `crates/labby/src/dispatch/CLAUDE.md`).

pub(crate) mod access_bootstrap;
mod bootstrap;
mod catalog;
pub(crate) mod claude_plugins;
mod client;
mod constrained_yaml;
mod dispatch;
mod draft;
pub(crate) mod host_service;
pub(crate) mod incus;
mod params;
mod plugin_hook;
pub(crate) mod provision;
pub(crate) mod proxy;
mod secret_mask;
mod secure_file;
mod settings;
mod state;
mod token;
mod types;

pub use access_bootstrap::{
    cleanup_prepare, complete_prepare, consume_prepare, inspect_prepare, prepare_access_bootstrap,
    recover_prepare, revoke_prepare, status_prepare,
};
pub use bootstrap::{BootstrapOutcome, bootstrap, bootstrap_action, should_bootstrap};
pub use catalog::{ACTIONS, LOCAL_ONLY_ACTIONS, PLUGIN_LIFECYCLE_ACTIONS};
pub use dispatch::dispatch;
pub use types::{
    AccessBootstrapManifest, AccessBootstrapPrepare, AccessBootstrapPrepareOutcome, CommitOutcome,
    DraftEntry, PrepareJournal, PrepareJournalState, SECRET_SENTINEL, SetupClient, SetupSnapshot,
    SetupState,
};

use labby_primitives::plugin::{Category, EnvVar, PluginMeta};

const OPTIONAL_ENV: &[EnvVar] = &[EnvVar {
    name: "LABBY_ACCESS_MIGRATION_EVIDENCE",
    description: "Path to the source/checkpoint-bound approval evidence for an access-store schema migration",
    example: "/run/labby/access-migration-v7.json",
    secret: false,
    ui: None,
}];

/// Compile-time metadata for the setup Bootstrap service.
pub const META: PluginMeta = PluginMeta {
    name: "setup",
    display_name: "Setup",
    description: "First-run + draft-commit configuration flow",
    category: Category::Bootstrap,
    docs_url: "",
    required_env: &[],
    optional_env: OPTIONAL_ENV,
    default_port: None,
    supports_multi_instance: false,
};
