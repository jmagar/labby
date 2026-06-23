use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SurfaceStateView {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub connected: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SurfaceStatesView {
    #[serde(default)]
    pub cli: SurfaceStateView,
    #[serde(default)]
    pub api: SurfaceStateView,
    #[serde(default)]
    pub mcp: SurfaceStateView,
    #[serde(default)]
    pub webui: SurfaceStateView,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerWarningView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerConfigSummaryView {
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    /// Redacted executable for stdio transport (e.g. `uvx`, `npx`). `None` for HTTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Redacted args for stdio transport (e.g. `["github-chat-mcp"]`). Empty for HTTP.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub configured: bool,
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
    pub surfaces: SurfaceStatesView,
    #[serde(default)]
    pub warnings: Vec<ServerWarningView>,
    #[serde(default)]
    pub config_summary: ServerConfigSummaryView,
    /// OS process id of the spawned stdio child, when connected. `None` for HTTP
    /// transports and for disconnected/disabled stdio servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}
