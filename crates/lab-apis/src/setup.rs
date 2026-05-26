//! Setup — Bootstrap orchestrator for the first-run flow.
//!
//! `setup` is a synthetic Bootstrap service: no remote API, no
//! `ServiceClient` impl, no required env. The real work (fs I/O, draft
//! merge, audit orchestration) lives in `crates/lab/src/dispatch/setup/`.
//! This module exposes only:
//!
//! - `META` — `PluginMeta` for registry discovery.
//! - Pure types shared across surfaces (`SetupState`, `SetupSnapshot`, …).
//! - `SetupClient` — synthetic marker for the validation helpers used by
//!   the wizard's "type-check before touching disk" guard rail.
//!
//! Always compiled (no feature gate), matching other Bootstrap peers.

pub mod client;
pub mod error;
pub mod types;

pub use client::SetupClient;
pub use error::SetupError;
pub use types::{
    CommitOutcome, DraftEntry, DraftSection, SECRET_SENTINEL, SetupSnapshot, SetupState,
};

use crate::core::plugin::{Category, PluginMeta};

/// Compile-time metadata for the setup module.
pub const META: PluginMeta = PluginMeta {
    name: "setup",
    display_name: "Setup",
    description: "First-run + draft-commit configuration flow",
    category: Category::Bootstrap,
    docs_url: "",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
