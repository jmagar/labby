use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct GatewayArgs {
    #[command(subcommand)]
    pub command: GatewayCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayCommand {
    /// List configured gateways and their runtime status.
    List,
    /// Get one configured gateway.
    Get(GatewayGetArgs),
    /// Test a configured or proposed gateway without saving it.
    Test(GatewayTestArgs),
    /// Add a gateway and reconcile runtime state.
    Add(GatewayAddArgs),
    /// Update a gateway and reconcile runtime state.
    Update(GatewayUpdateArgs),
    /// Remove a gateway and reconcile runtime state.
    Remove(GatewayRemoveArgs),
    /// Manage Lab-backed virtual servers quarantined during config migration.
    Quarantine(GatewayQuarantineArgs),
    /// Manage public MCP routes protected by Lab OAuth.
    ProtectedRoute(GatewayProtectedRouteArgs),
    /// Reload gateways from config and reconcile runtime state.
    Reload,
    /// Manage upstream MCP server lifecycle and OAuth.
    Mcp(GatewayMcpArgs),
    /// Scan the machine for MCP server configs from known editors and tools (read-only)
    Discover(GatewayDiscoverArgs),
    /// Import discovered MCP servers into the gateway (disabled by default)
    Import(GatewayImportArgs),
    /// Manage pending discovered servers waiting for approval
    Pending(GatewayPendingArgs),
    /// Show resolved public URL configuration (app and MCP gateway)
    PublicUrls,
    /// Search, inspect, and execute Code Mode snippets through dispatch
    Code(GatewayCodeArgs),
    /// Generate and approve Code Mode upstream hint proposals.
    Enrich(GatewayEnrichArgs),
}

#[derive(Debug, Args)]
pub struct GatewayEnrichArgs {
    #[command(subcommand)]
    pub command: Option<GatewayEnrichCommand>,
    #[arg(long = "upstream")]
    pub upstreams: Vec<String>,
    #[arg(long)]
    pub all: bool,
    #[arg(long, default_value = "deterministic", value_parser = ["deterministic", "claude", "codex"])]
    pub provider: String,
    #[arg(long)]
    pub max_upstreams: Option<usize>,
    #[arg(long)]
    pub timeout_ms: Option<u64>,
    /// Skip confirmation for provider-backed preview runs.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum GatewayEnrichCommand {
    Apply(GatewayEnrichApplyArgs),
}

#[derive(Debug, Args)]
pub struct GatewayEnrichApplyArgs {
    #[arg(long)]
    pub upstream: String,
    #[arg(long)]
    pub hint: String,
    #[arg(long, alias = "suggestion-hash")]
    pub metadata_hash: String,
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct GatewayCodeArgs {
    #[command(subcommand)]
    pub command: GatewayCodeCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayCodeCommand {
    /// Read gateway-wide Code Mode settings.
    Status,
    /// Enable the gateway codemode MCP surface.
    Enable,
    /// Disable the gateway codemode MCP surface.
    Disable,
    /// Execute a sandboxed JavaScript snippet that calls the typed
    /// `codemode.<upstream>.<tool>` helpers (or `callTool` directly).
    Exec {
        #[arg(long, conflicts_with = "file")]
        code: Option<String>,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct GatewayPendingArgs {
    #[command(subcommand)]
    pub command: GatewayPendingCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayPendingCommand {
    /// List discovered servers waiting for approval
    List,
    /// Approve a pending server and add it to the gateway (disabled by default)
    Approve(GatewayPendingNameArgs),
    /// Reject a pending server and tombstone it so it never re-appears
    Reject(GatewayPendingNameArgs),
}

#[derive(Debug, Args)]
pub struct GatewayPendingNameArgs {
    pub name: String,
    /// Skip the destructive-action confirmation prompt.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be done without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct GatewayDiscoverArgs {
    /// Limit scan to specific client kinds (comma-separated: cursor,claude-code,vscode,...)
    #[arg(long, value_delimiter = ',')]
    pub clients: Vec<String>,
    /// Also show servers already present in the gateway config
    #[arg(long, default_value_t = false)]
    pub include_existing: bool,
}

#[derive(Debug, Args)]
pub struct GatewayImportArgs {
    /// Import every discovered server not already in the gateway config
    #[arg(long, default_value_t = false)]
    pub all: bool,
    /// Specific server names to import (space-separated)
    #[arg(long = "name")]
    pub names: Vec<String>,
    /// Limit discovery to specific client kinds (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub clients: Vec<String>,
    /// Skip confirmation for the destructive config import.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct GatewayGetArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayTestArgs {
    /// Name of a configured gateway to test (omit to test with inline --url/--command).
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct GatewayAddArgs {
    /// Unique name for the gateway upstream.
    #[arg(long)]
    pub name: String,
    /// HTTP(S) URL for a remote MCP server (mutually exclusive with --command).
    #[arg(long)]
    pub url: Option<String>,
    /// Stdio command to launch for a local MCP server (mutually exclusive with --url).
    #[arg(long)]
    pub command: Option<String>,
    /// Additional arguments passed to the stdio command (repeat for multiple).
    #[arg(long = "arg")]
    pub args: Vec<String>,
    /// Environment variable name whose value is used as the upstream bearer token.
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub proxy_resources: bool,
}

#[derive(Debug, Args)]
pub struct GatewayUpdateArgs {
    /// Name of the gateway upstream to update.
    pub name: String,
    /// Rename the gateway upstream to this new name.
    #[arg(long)]
    pub new_name: Option<String>,
    /// New HTTP(S) URL for a remote MCP server.
    #[arg(long, conflicts_with = "command")]
    pub url: Option<String>,
    /// Clear the HTTP(S) URL from this gateway.
    #[arg(long, conflicts_with = "url")]
    pub clear_url: bool,
    /// New stdio command for a local MCP server.
    #[arg(long, conflicts_with = "url")]
    pub command: Option<String>,
    /// Clear the stdio command from this gateway.
    #[arg(long, conflicts_with = "command")]
    pub clear_command: bool,
    /// Replace all command arguments with these values (repeat for multiple).
    #[arg(long = "arg")]
    pub args: Vec<String>,
    /// Environment variable name whose value is used as the upstream bearer token.
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    /// Clear the upstream bearer token environment variable name.
    #[arg(long, conflicts_with = "bearer_token_env")]
    pub clear_bearer_token_env: bool,
    #[arg(long)]
    pub proxy_resources: Option<bool>,
}

#[derive(Debug, Args)]
pub struct GatewayRemoveArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayQuarantineArgs {
    #[command(subcommand)]
    pub command: GatewayQuarantineCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayQuarantineCommand {
    /// List Lab-backed virtual servers quarantined during config migration.
    List,
    /// Restore a quarantined Lab-backed virtual server into the active gateway list.
    Restore(GatewayQuarantineRestoreArgs),
}

#[derive(Debug, Args)]
pub struct GatewayQuarantineRestoreArgs {
    pub id: String,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteArgs {
    #[command(subcommand)]
    pub command: GatewayProtectedRouteCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayProtectedRouteCommand {
    /// List Gateway-managed public MCP routes protected by Lab OAuth.
    List,
    /// Get one Gateway-managed protected MCP route.
    Get(GatewayProtectedRouteNameArgs),
    /// Add a Gateway-managed protected MCP route.
    Add(GatewayProtectedRouteUpsertArgs),
    /// Replace a Gateway-managed protected MCP route.
    Update(GatewayProtectedRouteUpdateArgs),
    /// Remove a Gateway-managed protected MCP route.
    Remove(GatewayProtectedRouteNameArgs),
    /// Validate a proposed protected MCP route without saving it.
    Test(GatewayProtectedRouteUpsertArgs),
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteNameArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteUpdateArgs {
    pub name: String,
    #[arg(long)]
    pub new_name: Option<String>,
    #[arg(long)]
    pub enabled: Option<bool>,
    #[arg(long)]
    pub public_host: String,
    #[arg(long)]
    pub public_path: String,
    #[arg(long)]
    pub upstream: Option<String>,
    #[arg(long)]
    pub backend_url: Option<String>,
    #[arg(long, hide = true)]
    pub backend_mcp_path: Option<String>,
    #[arg(long = "scope")]
    pub scopes: Vec<String>,
    #[arg(long)]
    pub health_path: Option<String>,
    /// Expose a scoped Lab gateway MCP surface instead of proxying one backend.
    #[arg(long)]
    pub gateway_subset: bool,
    /// Upstream names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_upstream: Vec<String>,
    /// Built-in Lab service names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_service: Vec<String>,
    /// Expose codemode on this gateway subset.
    #[arg(long)]
    pub expose_code_mode: bool,
}

#[derive(Debug, Args)]
pub struct GatewayProtectedRouteUpsertArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long, default_value_t = true)]
    pub enabled: bool,
    #[arg(long)]
    pub public_host: String,
    #[arg(long)]
    pub public_path: String,
    #[arg(long)]
    pub upstream: Option<String>,
    #[arg(long)]
    pub backend_url: Option<String>,
    #[arg(long, hide = true)]
    pub backend_mcp_path: Option<String>,
    #[arg(long = "scope")]
    pub scopes: Vec<String>,
    #[arg(long)]
    pub health_path: Option<String>,
    /// Expose a scoped Lab gateway MCP surface instead of proxying one backend.
    #[arg(long)]
    pub gateway_subset: bool,
    /// Upstream names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_upstream: Vec<String>,
    /// Built-in Lab service names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_service: Vec<String>,
    /// Expose codemode on this gateway subset.
    #[arg(long)]
    pub expose_code_mode: bool,
}

#[derive(Debug, Args)]
pub struct GatewayMcpArgs {
    #[command(subcommand)]
    pub command: GatewayMcpCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayMcpCommand {
    /// Manage upstream MCP server OAuth credentials.
    Auth(GatewayMcpAuthArgs),
    /// List upstream MCP runtime state, discovery counts, and likely stale process counts.
    List,
    /// Enable an upstream MCP server so new sessions discover and proxy it again.
    Enable(GatewayMcpLifecycleArgs),
    /// Disable an upstream MCP server and optionally clean up running processes.
    Disable(GatewayMcpLifecycleArgs),
    /// Kill or preview running processes associated with one upstream MCP server.
    Cleanup(GatewayMcpCleanupArgs),
}

#[derive(Debug, Args)]
pub struct GatewayMcpAuthArgs {
    #[command(subcommand)]
    pub command: GatewayMcpAuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayMcpAuthCommand {
    /// Start the upstream OAuth flow and print the browser authorization URL.
    Start(GatewayOauthUpstreamArgs),
    /// Start the upstream OAuth flow and open the authorization URL in a browser.
    Open(GatewayOauthUpstreamArgs),
    /// Read upstream OAuth status for the shared gateway credential.
    Status(GatewayOauthUpstreamArgs),
    /// Clear stored upstream OAuth credentials for the shared gateway credential.
    Clear(GatewayOauthUpstreamArgs),
}

#[derive(Debug, Args)]
pub struct GatewayOauthUpstreamArgs {
    pub name: String,
    #[arg(long)]
    pub subject: Option<String>,
    #[arg(long, default_value_t = false)]
    pub open: bool,
    #[arg(long, default_value_t = false)]
    pub wait: bool,
    #[arg(long, default_value_t = 120)]
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub struct GatewayMcpLifecycleArgs {
    pub name: String,
    #[arg(long, default_value_t = false)]
    pub cleanup: bool,
    #[arg(long, default_value_t = false)]
    pub aggressive: bool,
}

#[derive(Debug, Args)]
pub struct GatewayMcpCleanupArgs {
    pub name: String,
    #[arg(long, default_value_t = false)]
    pub aggressive: bool,
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}
