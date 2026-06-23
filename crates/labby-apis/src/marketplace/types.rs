//! Public marketplace types. Serde shapes match the gateway-admin TS types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Marketplace runtime / ecosystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarketplaceRuntime {
    Claude,
    Codex,
    Gemini,
}

/// Marketplace source kind. Matches `MarketplaceSource` on the frontend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginSource {
    Github,
    Git,
    #[default]
    Local,
}

/// A configured marketplace (local JSON file or remote repo).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Marketplace {
    pub id: String,
    pub name: String,
    pub owner: String,
    #[serde(rename = "ghUser")]
    pub gh_user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub source: PluginSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub desc: String,
    #[serde(rename = "autoUpdate")]
    pub auto_update: bool,
    #[serde(rename = "totalPlugins")]
    pub total_plugins: u32,
    #[serde(rename = "lastUpdated")]
    pub last_updated: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<MarketplaceRuntime>,
}

/// Normalized manifest summary attached to a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifestSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<Value>,
}

/// High-level plugin component kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginComponentKind {
    Skills,
    Apps,
    McpServers,
    LspServers,
    Commands,
    Agents,
    Assets,
    Hooks,
    Monitors,
    Bin,
    Settings,
    OutputStyles,
    Themes,
    Channels,
    Files,
}

/// A semantically identified plugin component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginComponent {
    pub kind: PluginComponentKind,
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

/// Normalized install state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInstallState {
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(rename = "installedAt", skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// A plugin entry within a marketplace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    /// Marketplace id this plugin belongs to.
    pub mkt: String,
    pub ver: String,
    pub desc: String,
    pub tags: Vec<String>,
    pub installed: bool,
    #[serde(rename = "hasUpdate", skip_serializing_if = "Option::is_none")]
    pub has_update: Option<bool>,
    #[serde(rename = "installedAt", skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<MarketplaceRuntime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(rename = "marketplaceId", skip_serializing_if = "Option::is_none")]
    pub marketplace_id: Option<String>,
    #[serde(rename = "version", skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(rename = "description", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifestSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<PluginComponent>>,
    #[serde(rename = "installState", skip_serializing_if = "Option::is_none")]
    pub install_state: Option<PluginInstallState>,
    #[serde(rename = "sourcePath", skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(rename = "cachePath", skip_serializing_if = "Option::is_none")]
    pub cache_path: Option<String>,
}

/// Syntax-highlight hint for plugin artifact files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactLang {
    Json,
    Yaml,
    Markdown,
    Bash,
    Toml,
    Text,
}

/// A single file shipped with an installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub lang: ArtifactLang,
    pub content: String,
}
