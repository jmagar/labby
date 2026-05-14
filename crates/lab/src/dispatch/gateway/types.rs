use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::config::ImportSource;

/// Surface-neutral notification handle for catalog changes.
///
/// The dispatch layer calls this after gateway reload/add/remove to inform
/// connected transports (e.g. MCP peers) that tools, resources, or prompts
/// have changed.  The concrete implementation lives in the MCP surface
/// (`mcp/server.rs`) so that `rmcp::Peer` never leaks into dispatch.
#[derive(Clone, Debug)]
pub struct CatalogChangeNotifier {
    tx: mpsc::UnboundedSender<GatewayCatalogDiff>,
}

impl CatalogChangeNotifier {
    #[must_use]
    pub fn new(tx: mpsc::UnboundedSender<GatewayCatalogDiff>) -> Self {
        Self { tx }
    }

    pub fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff) {
        let _ = self.tx.send(diff.clone());
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayConfigView {
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub bearer_token_env: Option<String>,
    #[serde(default)]
    pub oauth_enabled: bool,
    #[serde(default)]
    pub proxy_resources: bool,
    #[serde(default)]
    pub proxy_prompts: bool,
    #[serde(default)]
    pub expose_tools: Option<Vec<String>>,
    #[serde(default)]
    pub expose_resources: Option<Vec<String>>,
    #[serde(default)]
    pub expose_prompts: Option<Vec<String>>,
    #[serde(default)]
    pub tool_search_enabled: bool,
    #[serde(default = "default_tool_search_top_k_default")]
    pub tool_search_top_k_default: usize,
    #[serde(default = "default_tool_search_max_tools")]
    pub tool_search_max_tools: usize,
    /// Set when this server was imported from an external config; absent for manual entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<ImportSource>,
}

/// A server discovered from an external MCP config but not yet imported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredServerView {
    pub name: String,
    /// Which client config type it was found in (e.g. "cursor", "vscode", "gemini").
    pub source_client: String,
    /// Absolute path to the config file.
    pub source_path: String,
    /// Transport kind.
    pub transport: McpClientTransportType,
    /// First token of the command (stdio only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_preview: Option<String>,
    /// URL (http only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_preview: Option<String>,
    /// Number of env keys present (values are not shown).
    pub env_key_count: usize,
    /// True if a server with this name is already in the gateway config.
    pub already_configured: bool,
}

/// Why a server was skipped during `gateway.import`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportSkipReason {
    AlreadyConfigured,
    Conflict,
}

/// One skipped entry from a `gateway.import` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSkipView {
    pub name: String,
    pub reason: ImportSkipReason,
}

/// One error entry from a `gateway.import` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportErrorView {
    pub name: String,
    pub message: String,
}

/// Structured result returned by `gateway.import`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportResultView {
    pub imported: Vec<GatewayView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<ImportSkipView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ImportErrorView>,
}

fn default_tool_search_top_k_default() -> usize {
    crate::config::ToolSearchConfig::default().top_k_default
}

fn default_tool_search_max_tools() -> usize {
    crate::config::ToolSearchConfig::default().max_tools
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayRuntimeView {
    pub name: String,
    #[serde(default)]
    pub tool_count: usize,
    #[serde(default)]
    pub resource_count: usize,
    #[serde(default)]
    pub prompt_count: usize,
    #[serde(default)]
    pub exposed_tool_count: usize,
    #[serde(default)]
    pub exposed_resource_count: usize,
    #[serde(default)]
    pub exposed_prompt_count: usize,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayView {
    pub config: GatewayConfigView,
    pub runtime: GatewayRuntimeView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpClientTransportType {
    Http,
    Stdio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientConfigView {
    pub name: String,
    pub r#type: McpClientTransportType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayCatalogDiff {
    #[serde(default)]
    pub tools_changed: bool,
    #[serde(default)]
    pub resources_changed: bool,
    #[serde(default)]
    pub prompts_changed: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceConfigFieldView {
    pub name: String,
    #[serde(default)]
    pub present: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub value_preview: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceConfigView {
    pub service: String,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub fields: Vec<ServiceConfigFieldView>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtualServerMcpPolicyView {
    #[serde(default)]
    pub allowed_actions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceActionView {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub destructive: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayToolExposureRowView {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub exposed: bool,
    #[serde(default)]
    pub matched_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayCleanupMatchView {
    pub pattern: String,
    #[serde(default)]
    pub pids: Vec<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayCleanupView {
    pub upstream: String,
    #[serde(default)]
    pub aggressive: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub gateway_matched: usize,
    #[serde(default)]
    pub local_matched: usize,
    #[serde(default)]
    pub aggressive_matched: usize,
    #[serde(default)]
    pub gateway_killed: usize,
    #[serde(default)]
    pub local_killed: usize,
    #[serde(default)]
    pub aggressive_killed: usize,
    #[serde(default)]
    pub gateway_matches: Vec<GatewayCleanupMatchView>,
    #[serde(default)]
    pub local_matches: Vec<GatewayCleanupMatchView>,
    #[serde(default)]
    pub aggressive_matches: Vec<GatewayCleanupMatchView>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayRuntimeOwnerView {
    pub surface: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub raw: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayMcpRuntimeView {
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub discovered_tool_count: usize,
    #[serde(default)]
    pub exposed_tool_count: usize,
    #[serde(default)]
    pub discovered_resource_count: usize,
    #[serde(default)]
    pub exposed_resource_count: usize,
    #[serde(default)]
    pub discovered_prompt_count: usize,
    #[serde(default)]
    pub exposed_prompt_count: usize,
    #[serde(default)]
    pub likely_stale_count: usize,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub pgid: Option<u32>,
    #[serde(default)]
    pub age_seconds: Option<u64>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub owner: Option<GatewayRuntimeOwnerView>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub runtime_state_path: Option<String>,
    #[serde(default)]
    pub reconciled_at: Option<String>,
}
