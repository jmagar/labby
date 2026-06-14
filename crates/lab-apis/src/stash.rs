//! Stash — component versioning and deployment service.
//!
//! `stash` tracks Claude Code components (skills, agents, commands, hooks,
//! themes, config files, binaries, and more) as versioned artefacts that can
//! be deployed to local paths or remote gateways.
//!
//! This module is **always-on** (no feature gate), matching the pattern of
//! `device_runtime`. The pure domain types live in [`types`]. Client and
//! provider implementations will be added in later tasks.

pub mod types;

pub use types::{
    MarketplaceOrigin, StashComponent, StashComponentKind, StashDeployTarget, StashExportOptions,
    StashOrigin, StashProviderCapabilities, StashProviderRecord, StashProviderSummary,
    StashRevision, StashWorkspaceShape,
};

pub use types::limits;

use crate::core::plugin::{Category, PluginMeta};

/// Compile-time metadata for the stash module.
pub const META: PluginMeta = PluginMeta {
    name: "stash",
    display_name: "Stash",
    description: "Component versioning and deployment — track, snapshot, and deploy Claude Code artefacts",
    category: Category::Bootstrap,
    docs_url: "",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
