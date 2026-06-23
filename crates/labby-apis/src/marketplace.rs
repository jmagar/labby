//! Marketplace: browse and manage plugin, MCP Registry, and ACP Registry entries.
//!
//! `marketplace` is a synthetic service — it has the same module shape as a
//! real API client but its "client" is the local filesystem plus (optionally)
//! the GitHub raw-content endpoint. No authenticated HTTP, no runtime config.
//!
//! The pure types live here. The lab-specific glue (reading
//! `.claude-plugin/marketplace.json`, scanning `~/.claude/plugins/`, and
//! shelling out to `claude plugin install/uninstall`) lives in
//! `crates/lab/src/dispatch/marketplace/`.

pub mod types;

pub use types::{
    Artifact, ArtifactLang, Marketplace, MarketplaceRuntime, Plugin, PluginComponent,
    PluginComponentKind, PluginInstallState, PluginManifestSummary, PluginSource,
};

use crate::core::plugin::{Category, PluginMeta};

/// Compile-time metadata for the marketplace module.
pub const META: PluginMeta = PluginMeta {
    name: "marketplace",
    display_name: "Marketplace",
    description: "Browse Claude Code/Codex marketplaces, MCP Registry servers, ACP agents, and installable components",
    category: Category::Marketplace,
    docs_url: "",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
