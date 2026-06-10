use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::json;

use crate::cli::helpers::{run_action_command, run_confirmable_action_command};
use crate::config::{
    LabConfig, ProtectedMcpRouteConfig, config_toml_path, resolve_auth_for_config,
};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::code_mode::{CodeModeBroker, CodeModeCaller, CodeModeSurface};
use crate::dispatch::gateway::install_gateway_manager;
use crate::dispatch::gateway::manager::{
    GatewayManager, GatewayManagerConfig, GatewayOauthConfig, GatewayRuntimeHandle,
};
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::output::OutputFormat;
use crate::registry::ToolRegistry;

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
    /// Enable the gateway Code Mode search/execute MCP surface.
    Enable,
    /// Disable the gateway Code Mode search/execute MCP surface.
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
    #[arg(long)]
    pub url: Option<String>,
    /// New stdio command for a local MCP server.
    #[arg(long)]
    pub command: Option<String>,
    /// Replace all command arguments with these values (repeat for multiple).
    #[arg(long = "arg")]
    pub args: Vec<String>,
    /// Environment variable name whose value is used as the upstream bearer token.
    #[arg(long)]
    pub bearer_token_env: Option<String>,
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

#[derive(Debug, Deserialize)]
struct GatewayOauthStartView {
    authorization_url: String,
}

async fn build_manager(
    config: &LabConfig,
    discover_upstreams: bool,
) -> Result<Arc<GatewayManager>> {
    let auth_config = resolve_auth_for_config(config)?;
    let upstream_oauth_runtime =
        crate::oauth::upstream::runtime::build_upstream_oauth_runtime(config, &auth_config).await?;
    Ok(build_manager_with_upstream_oauth_runtime(
        config,
        discover_upstreams,
        upstream_oauth_runtime,
    )
    .await)
}

async fn build_manager_with_upstream_oauth_runtime(
    config: &LabConfig,
    discover_upstreams: bool,
    upstream_oauth_runtime: Option<crate::oauth::upstream::runtime::UpstreamOauthRuntime>,
) -> Arc<GatewayManager> {
    let runtime = GatewayRuntimeHandle::default();
    let registry = filtered_builtin_service_registry(config);
    if discover_upstreams {
        // Seed lazily (mirroring `serve`): catalog entries come from config
        // without spawning any upstream processes. Connections are made on
        // demand via the manager's `ensure_*_runtime_ready` paths, so one-shot
        // CLI commands only spawn the upstreams they actually touch.
        let mut pool_builder = UpstreamPool::new()
            .with_request_timeout(config.upstream_request_timeout())
            .with_in_process_connector(crate::mcp::in_process_peer::connector());
        if let Some(rt) = &upstream_oauth_runtime {
            pool_builder = pool_builder.with_oauth_client_cache(rt.cache.clone());
        }
        let pool = Arc::new(pool_builder);
        pool.seed_lazy_upstreams(&config.upstream).await;
        runtime.swap(Some(pool)).await;
    }

    let manager = GatewayManager::from_config(
        GatewayManagerConfig {
            config_path: config_toml_path().unwrap_or_else(|| "config.toml".into()),
            registry,
            service_clients: SharedServiceClients::from_env(),
            in_process_connector: None,
            oauth: upstream_oauth_runtime.map(|rt| GatewayOauthConfig {
                managers: rt.managers,
                cache: rt.cache,
                sqlite: rt.sqlite,
                key: rt.key,
                redirect_uri: rt.redirect_uri,
            }),
        },
        runtime,
    );
    let manager = Arc::new(manager);
    manager.seed_config(config.clone()).await;
    install_gateway_manager(Arc::clone(&manager));
    manager
}

fn filtered_builtin_service_registry(config: &LabConfig) -> ToolRegistry {
    crate::registry::filter_built_in_upstream_apis(
        crate::registry::build_default_registry(),
        config.services.built_in_upstream_apis_enabled,
    )
}

fn protected_route_from_args(args: GatewayProtectedRouteUpsertArgs) -> ProtectedMcpRouteConfig {
    ProtectedMcpRouteConfig {
        name: args.name,
        enabled: args.enabled,
        public_host: args.public_host,
        public_path: args.public_path,
        upstream: args.upstream,
        backend_url: args.backend_url.unwrap_or_default(),
        backend_mcp_path: args.backend_mcp_path.unwrap_or_else(|| "/mcp".to_string()),
        scopes: args.scopes,
        health_path: args.health_path,
    }
}

pub async fn run(args: GatewayArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    let discover_upstreams = !(matches!(
        &args.command,
        GatewayCommand::Mcp(GatewayMcpArgs {
            command: GatewayMcpCommand::List
                | GatewayMcpCommand::Enable(_)
                | GatewayMcpCommand::Disable(_)
                | GatewayMcpCommand::Cleanup(_)
                | GatewayMcpCommand::Auth(GatewayMcpAuthArgs {
                    command: GatewayMcpAuthCommand::Status(_) | GatewayMcpAuthCommand::Clear(_),
                }),
        })
    ) || matches!(&args.command, GatewayCommand::ProtectedRoute(_)));
    let manager = build_manager(config, discover_upstreams).await?;
    // Race the command against SIGINT/SIGTERM so the drain below also runs
    // when the invocation is killed externally (e.g. `timeout 100s labby
    // gateway code exec ...` SIGTERMs at the deadline). Without this the
    // default signal disposition kills the process before the drain and
    // orphans spawned stdio upstream children.
    let result = tokio::select! {
        result = dispatch_command(Arc::clone(&manager), args, format) => result,
        code = shutdown_signal() => Ok(ExitCode::from(code)),
    };
    // INVARIANT: drain the upstream pool before the one-shot CLI exits. The
    // manager is installed into a process-global (`install_gateway_manager`),
    // so `UpstreamConnection` Drop never runs at process exit and spawned
    // stdio upstream process groups (npx/uvx trees) would be orphaned —
    // repeated invocations leak hundreds of child processes.
    if let Some(pool) = manager.current_pool().await {
        pool.drain_for_swap("gateway.cli.exit").await;
    }
    result
}

/// Resolve with the conventional exit code (128 + signum) when SIGINT or
/// SIGTERM arrives.
async fn shutdown_signal() -> u8 {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        let sigterm_recv = async {
            match sigterm.as_mut() {
                Some(sig) => {
                    sig.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => 130,
            () = sigterm_recv => 143,
        }
    }
    #[cfg(not(unix))]
    {
        let _unused = tokio::signal::ctrl_c().await;
        130
    }
}

async fn dispatch_command(
    manager: Arc<GatewayManager>,
    args: GatewayArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    let cli_origin = format!("cli:{}", std::process::id());
    let cli_owner = json!({
        "surface": "cli",
        "client_name": "lab-cli",
        "raw": cli_origin,
    });
    match args.command {
        GatewayCommand::Mcp(args) => match args.command {
            GatewayMcpCommand::Auth(args) => match args.command {
                GatewayMcpAuthCommand::Start(args) => {
                    return run_gateway_oauth_start(manager, args, format).await;
                }
                GatewayMcpAuthCommand::Open(mut args) => {
                    args.open = true;
                    return run_gateway_oauth_start(manager, args, format).await;
                }
                GatewayMcpAuthCommand::Status(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.status".to_string(),
                        json!({ "upstream": args.name, "subject": args.subject }),
                        format,
                        |action, params| async move {
                            crate::dispatch::gateway::dispatch_with_manager(
                                &manager, &action, params,
                            )
                            .await
                        },
                    )
                    .await;
                }
                GatewayMcpAuthCommand::Clear(args) => {
                    return run_action_command(
                        "gateway",
                        "gateway.oauth.clear".to_string(),
                        json!({ "upstream": args.name, "subject": args.subject }),
                        format,
                        |action, params| async move {
                            crate::dispatch::gateway::dispatch_with_manager(
                                &manager, &action, params,
                            )
                            .await
                        },
                    )
                    .await;
                }
            },
            GatewayMcpCommand::List => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.list".to_string(),
                    json!({}),
                    format,
                    |action, params| async move {
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Enable(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.enable".to_string(),
                    json!({
                        "name": args.name,
                        "origin": cli_origin,
                        "owner": cli_owner,
                    }),
                    format,
                    |action, params| async move {
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Disable(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.disable".to_string(),
                    json!({
                        "name": args.name,
                        "cleanup": args.cleanup,
                        "aggressive": args.aggressive,
                        "origin": cli_origin,
                        "owner": cli_owner,
                    }),
                    format,
                    |action, params| async move {
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
                    },
                )
                .await;
            }
            GatewayMcpCommand::Cleanup(args) => {
                return run_action_command(
                    "gateway",
                    "gateway.mcp.cleanup".to_string(),
                    json!({
                        "name": args.name,
                        "aggressive": args.aggressive,
                        "dry_run": args.dry_run,
                    }),
                    format,
                    |action, params| async move {
                        crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params)
                            .await
                    },
                )
                .await;
            }
        },
        GatewayCommand::List => {
            return run_gateway_list(manager, format).await;
        }
        command => {
            if let GatewayCommand::Code(args) = command {
                return run_gateway_code(manager, args, format).await;
            }
            let mut confirmed = true;
            let mut dry_run = false;
            let (action, params) = match command {
                GatewayCommand::List => unreachable!("handled above"),
                GatewayCommand::Get(args) => {
                    ("gateway.get".to_string(), json!({ "name": args.name }))
                }
                GatewayCommand::Test(args) => {
                    ("gateway.test".to_string(), json!({ "name": args.name }))
                }
                GatewayCommand::Add(args) => (
                    "gateway.add".to_string(),
                    json!({
                        "origin": cli_origin,
                        "owner": cli_owner,
                        "spec": {
                            "name": args.name,
                            "url": args.url,
                            "command": args.command,
                            "args": args.args,
                            "bearer_token_env": args.bearer_token_env,
                            "proxy_resources": args.proxy_resources,
                        }
                    }),
                ),
                GatewayCommand::Update(args) => (
                    "gateway.update".to_string(),
                    json!({
                        "name": args.name,
                        "origin": cli_origin,
                        "owner": cli_owner,
                        "patch": {
                            "name": args.new_name,
                            "url": args.url.map(Some),
                            "command": args.command.map(Some),
                            "args": if args.args.is_empty() { None::<Vec<String>> } else { Some(args.args) },
                            "bearer_token_env": args.bearer_token_env.map(Some),
                            "proxy_resources": args.proxy_resources,
                        }
                    }),
                ),
                GatewayCommand::Remove(args) => (
                    "gateway.remove".to_string(),
                    json!({ "name": args.name, "origin": cli_origin, "owner": cli_owner }),
                ),
                GatewayCommand::Quarantine(args) => match args.command {
                    GatewayQuarantineCommand::List => (
                        "gateway.virtual_server.quarantine.list".to_string(),
                        json!({}),
                    ),
                    GatewayQuarantineCommand::Restore(args) => (
                        "gateway.virtual_server.quarantine.restore".to_string(),
                        json!({ "id": args.id }),
                    ),
                },
                GatewayCommand::ProtectedRoute(args) => match args.command {
                    GatewayProtectedRouteCommand::List => {
                        ("gateway.protected_route.list".to_string(), json!({}))
                    }
                    GatewayProtectedRouteCommand::Get(args) => (
                        "gateway.protected_route.get".to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayProtectedRouteCommand::Add(args) => (
                        "gateway.protected_route.add".to_string(),
                        json!({ "route": protected_route_from_args(args) }),
                    ),
                    GatewayProtectedRouteCommand::Update(args) => (
                        "gateway.protected_route.update".to_string(),
                        json!({
                            "name": args.name,
                            "route": ProtectedMcpRouteConfig {
                                name: args.new_name.unwrap_or_else(|| args.name.clone()),
                                enabled: args.enabled.unwrap_or(true),
                                public_host: args.public_host,
                                public_path: args.public_path,
                                upstream: args.upstream,
                                backend_url: args.backend_url.unwrap_or_default(),
                                backend_mcp_path: args.backend_mcp_path.unwrap_or_else(|| "/mcp".to_string()),
                                scopes: args.scopes,
                                health_path: args.health_path,
                            }
                        }),
                    ),
                    GatewayProtectedRouteCommand::Remove(args) => (
                        "gateway.protected_route.remove".to_string(),
                        json!({ "name": args.name }),
                    ),
                    GatewayProtectedRouteCommand::Test(args) => (
                        "gateway.protected_route.test".to_string(),
                        json!({ "route": protected_route_from_args(args) }),
                    ),
                },
                GatewayCommand::Reload => (
                    "gateway.reload".to_string(),
                    json!({ "origin": cli_origin, "owner": cli_owner }),
                ),
                GatewayCommand::Discover(args) => (
                    "gateway.discover".to_string(),
                    json!({
                        "clients": args.clients,
                        "include_existing": args.include_existing,
                    }),
                ),
                GatewayCommand::Import(args) => {
                    confirmed = args.yes;
                    (
                        "gateway.import".to_string(),
                        json!({
                            "all": args.all,
                            "names": args.names,
                            "clients": args.clients,
                        }),
                    )
                }
                GatewayCommand::Pending(args) => match args.command {
                    GatewayPendingCommand::List => {
                        ("gateway.import_pending.list".to_string(), json!({}))
                    }
                    GatewayPendingCommand::Approve(name_args) => {
                        confirmed = name_args.yes;
                        dry_run = name_args.dry_run;
                        (
                            "gateway.import_pending.approve".to_string(),
                            json!({ "name": name_args.name }),
                        )
                    }
                    GatewayPendingCommand::Reject(name_args) => {
                        confirmed = name_args.yes;
                        dry_run = name_args.dry_run;
                        (
                            "gateway.import_pending.reject".to_string(),
                            json!({ "name": name_args.name }),
                        )
                    }
                },
                GatewayCommand::PublicUrls => ("gateway.public_urls.get".to_string(), json!({})),
                GatewayCommand::Mcp(_) => unreachable!("handled above"),
                GatewayCommand::Code(_) => unreachable!("handled above"),
            };

            if dry_run {
                crate::cli::helpers::print_dry_run("gateway", &action, &params, format);
                return Ok(ExitCode::SUCCESS);
            }

            return run_confirmable_action_command(
                "gateway",
                crate::dispatch::gateway::ACTIONS,
                action,
                params,
                confirmed,
                format,
                |action, params| async move {
                    crate::dispatch::gateway::dispatch_with_manager(&manager, &action, params).await
                },
            )
            .await;
        }
    }
}

async fn run_gateway_code(
    manager: Arc<GatewayManager>,
    args: GatewayCodeArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    const CODE_MODE_CLI_MAX_SOURCE_BYTES: u64 = 20 * 1024;

    let registry = manager.builtin_service_registry();
    let broker = CodeModeBroker::new(&registry, Some(manager.as_ref()));
    let caller = CodeModeCaller::TrustedLocal;
    let surface = CodeModeSurface::Cli;

    match args.command {
        GatewayCodeCommand::Status => {
            crate::output::print(&manager.code_mode_config().await, format)?;
        }
        GatewayCodeCommand::Enable => {
            let mut next = manager.code_mode_config().await;
            next.enabled = true;
            let updated = manager.set_code_mode_config(next, None, None).await?;
            crate::output::print(&updated, format)?;
        }
        GatewayCodeCommand::Disable => {
            let mut next = manager.code_mode_config().await;
            next.enabled = false;
            let updated = manager.set_code_mode_config(next, None, None).await?;
            crate::output::print(&updated, format)?;
        }
        GatewayCodeCommand::Exec { code, file } => {
            let code = read_code_mode_source(code, file, CODE_MODE_CLI_MAX_SOURCE_BYTES)?;
            let config = manager.code_mode_config().await;
            let max_tool_calls = config.max_tool_calls;
            let response = broker
                .execute(
                    &code,
                    max_tool_calls,
                    caller,
                    surface,
                    config,
                    crate::dispatch::gateway::code_mode::CodeModeCapabilityFilter::default(),
                )
                .await?;
            crate::output::print(&response, format)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn read_code_mode_source(
    code: Option<String>,
    file: Option<std::path::PathBuf>,
    max_source_bytes: u64,
) -> Result<String> {
    match (code, file) {
        (Some(code), None) => {
            // Check the inline string length BEFORE any further buffering so we
            // never allocate more than max_source_bytes for a too-large arg
            // (Q-L7 fix: moved from post-allocation check).
            if code.len() as u64 > max_source_bytes {
                anyhow::bail!("Code Mode source exceeds {max_source_bytes} bytes");
            }
            Ok(code)
        }
        (None, Some(path)) => {
            // Check metadata BEFORE reading to avoid allocating the full file
            // when it is already known to be too large (already correct; kept).
            let metadata = std::fs::metadata(&path)?;
            if metadata.len() > max_source_bytes {
                anyhow::bail!("Code Mode source file exceeds {max_source_bytes} bytes");
            }
            // Use a capped reader so a file that grows between the stat and
            // the read cannot exceed the budget by more than one byte.
            use std::io::Read as _;
            let mut buf = String::new();
            std::fs::File::open(&path)?
                .take(max_source_bytes + 1)
                .read_to_string(&mut buf)?;
            if buf.len() as u64 > max_source_bytes {
                anyhow::bail!("Code Mode source file exceeds {max_source_bytes} bytes");
            }
            Ok(buf)
        }
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}

async fn run_gateway_oauth_start(
    manager: Arc<GatewayManager>,
    args: GatewayOauthUpstreamArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    let params = json!({ "upstream": args.name, "subject": args.subject });
    let start = std::time::Instant::now();
    let value =
        crate::dispatch::gateway::dispatch_with_manager(&manager, "gateway.oauth.start", params)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "{}",
                    serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
                )
            })?;
    tracing::info!(
        surface = "cli",
        service = "gateway",
        action = "gateway.oauth.start",
        elapsed_ms = start.elapsed().as_millis(),
        "dispatch ok"
    );

    crate::output::print(&value, format)?;

    let start_view: GatewayOauthStartView =
        serde_json::from_value(value.clone()).map_err(|error| {
            anyhow::anyhow!("failed to decode gateway oauth start response: {error}")
        })?;

    let theme = crate::output::theme::CliTheme::from_context(format.render_context());

    if args.open {
        open_in_browser(&start_view.authorization_url)?;
        eprintln!(
            "{}",
            theme.muted("Opened authorization URL in your browser.")
        );
    } else {
        eprintln!(
            "{}\n{}",
            theme.muted("Open this URL to authorize:"),
            theme.accent(&start_view.authorization_url)
        );
    }

    if args.wait {
        // Q-H3: delegate the poll loop to the shared dispatch layer via
        // `gateway.oauth.wait` so all surfaces share the orchestration.
        let subject = args
            .subject
            .as_deref()
            .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
        eprintln!(
            "{}",
            theme.muted(format!(
                "Waiting for OAuth completion for `{}` using shared subject `{}`...",
                args.name, subject
            ))
        );
        let wait_value = crate::dispatch::gateway::dispatch_with_manager(
            &manager,
            "gateway.oauth.wait",
            json!({
                "upstream": args.name,
                "subject": subject,
                "timeout_secs": args.wait_timeout_secs,
            }),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{}",
                serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
            )
        })?;

        let authenticated = wait_value
            .get("authenticated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if authenticated {
            eprintln!(
                "{}",
                theme.success(&format!(
                    "OAuth completed for `{}`. The existing callback route stored credentials for shared subject `{}`.",
                    args.name, subject
                ))
            );
        } else {
            eprintln!(
                "{}",
                theme.warn(&format!(
                    "Timed out waiting for OAuth completion for `{}` after {}s. The browser callback may still succeed later; re-run `labby gateway mcp auth status {}` to check.",
                    args.name, args.wait_timeout_secs, args.name
                ))
            );
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "opening a browser is not supported on this platform"
    ))
}

/// Render `gateway list` with a typed per-server layout instead of the generic
/// Value-shape table (which renders nested objects as `{N keys}` placeholders).
///
/// Format inspired by `claude mcp list` (status icon + one-line per server)
/// and `codex mcp list` (column alignment). JSON mode preserves the full
/// `ServerView` shape for downstream consumers.
async fn run_gateway_list(manager: Arc<GatewayManager>, format: OutputFormat) -> Result<ExitCode> {
    let servers = match manager.list().await {
        Ok(s) => s,
        Err(err) => {
            return Err(anyhow::anyhow!(
                "{}",
                serde_json::to_string(&err).unwrap_or_else(|_| err.to_string())
            ));
        }
    };

    if format.is_json() {
        #[allow(clippy::print_stdout)]
        {
            println!(
                "{}",
                serde_json::to_string_pretty(&servers).unwrap_or_else(|_| "[]".to_string())
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    render_gateway_list_human(&servers, format);
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::print_stdout)]
fn render_gateway_list_human(
    servers: &[crate::dispatch::gateway::view_models::ServerView],
    format: OutputFormat,
) {
    use crate::output::theme::CliTheme;
    let theme = CliTheme::from_context(format.render_context());

    let mut connected = 0usize;
    let mut failed = 0usize;
    let mut disabled = 0usize;
    for s in servers {
        if !s.enabled {
            disabled += 1;
        } else if s.connected {
            connected += 1;
        } else {
            failed += 1;
        }
    }

    let total = servers.len();
    println!(
        "{} {}",
        theme.section(&format!("Lab Gateway · {total} servers")),
        theme.muted(format!(
            "({} connected, {} disconnected, {} disabled)",
            connected, failed, disabled
        )),
    );
    println!();

    if servers.is_empty() {
        println!("  {}", theme.muted("no servers configured"));
        return;
    }

    // Sort: connected first, then failed (enabled but not connected), then disabled.
    // Within each group preserve config order (stable sort).
    let mut servers: Vec<&crate::dispatch::gateway::view_models::ServerView> =
        servers.iter().collect();
    servers.sort_by_key(|s| {
        if !s.enabled {
            2u8
        } else {
            u8::from(!s.connected)
        }
    });
    let servers = servers.as_slice();

    let name_width = servers
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(8);
    let transport_width = servers
        .iter()
        .filter_map(|s| s.config_summary.transport.as_deref())
        .map(|t| t.chars().count())
        .max()
        .unwrap_or(5)
        .max(5);

    for s in servers {
        let icon = if !s.enabled {
            theme.muted("⊘")
        } else if s.connected {
            theme.ok_badge()
        } else {
            theme.warn_badge()
        };

        let name_padded = format!("{:width$}", s.name, width = name_width);
        let name = if s.connected {
            theme.primary(&name_padded)
        } else if !s.enabled {
            theme.muted(&name_padded)
        } else {
            theme.warn(&name_padded)
        };

        let transport_raw = s.config_summary.transport.as_deref().unwrap_or("—");
        let transport_padded = format!("{:width$}", transport_raw, width = transport_width);
        let transport = theme.tertiary(&transport_padded);

        let status_detail = if !s.enabled {
            theme.muted("disabled")
        } else if s.connected {
            let mut parts = Vec::new();
            if s.exposed_tool_count > 0 {
                parts.push(format!("🔧 {}", s.exposed_tool_count));
            }
            if s.exposed_prompt_count > 0 {
                parts.push(format!("💬 {}", s.exposed_prompt_count));
            }
            if s.exposed_resource_count > 0 {
                parts.push(format!("📦 {}", s.exposed_resource_count));
            }
            let joined = if parts.is_empty() {
                "connected".to_string()
            } else {
                parts.join(" · ")
            };
            theme.secondary(&joined)
        } else {
            let msg = s
                .warnings
                .iter()
                .find(|w| !w.message.is_empty())
                .map(|w| {
                    let m = w.message.trim();
                    if m.len() > 80 {
                        format!("{}…", &m[..80])
                    } else {
                        m.to_string()
                    }
                })
                .unwrap_or_else(|| "not connected".to_string());
            theme.warn(&msg)
        };

        // For stdio transports show the full `command arg1 arg2` invocation
        // (e.g. `uvx github-chat-mcp`); for HTTP fall back to the redacted URL.
        let location_raw = match s.config_summary.command.as_deref() {
            Some(command) if !command.is_empty() => {
                let mut line = command.to_string();
                if !s.config_summary.args.is_empty() {
                    line.push(' ');
                    line.push_str(&s.config_summary.args.join(" "));
                }
                Some(line)
            }
            _ => s
                .config_summary
                .target
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        };
        // Append the PID inline — it is only present for connected stdio children.
        let tail = match (location_raw, s.pid) {
            (Some(loc), Some(pid)) => theme.muted(format!("{loc} · pid {pid}")),
            (Some(loc), None) => theme.muted(loc),
            (None, Some(pid)) => theme.muted(format!("pid {pid}")),
            (None, None) => String::new(),
        };

        println!("  {icon} {name}  {transport}  {status_detail}  {tail}");
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::Parser;

    use crate::cli::{Cli, Command};
    use crate::config::{
        LabConfig, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode,
        UpstreamOauthRegistration,
    };
    use crate::oauth::upstream::encryption::load_key;
    use crate::oauth::upstream::runtime::build_upstream_oauth_runtime_from_parts;

    use super::{GatewayCommand, build_manager_with_upstream_oauth_runtime};

    #[test]
    fn gateway_cli_parser_accepts_expected_commands() {
        Cli::command().debug_assert();

        assert!(Cli::try_parse_from(["lab", "gateway", "list"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "get", "fixture-http"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "test", "--name", "fixture-http"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "add",
                "--name",
                "fixture-http",
                "--url",
                "http://127.0.0.1:8791",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "update",
                "fixture-http",
                "--proxy-resources",
                "true",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["lab", "gateway", "remove", "fixture-http"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "import", "--all", "--yes"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "quarantine", "list"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "quarantine", "restore", "plex"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "reload"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "auth",
                "start",
                "fixture-http",
                "--open",
                "--wait",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "auth",
                "open",
                "fixture-http",
                "--wait",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "mcp", "auth", "status", "fixture-http",])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "mcp", "auth", "clear", "fixture-http",])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["lab", "gateway", "mcp", "list",]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "mcp", "enable", "fixture-http",]).is_ok());
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "disable",
                "fixture-http",
                "--cleanup",
                "--aggressive",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "cleanup",
                "fixture-http",
                "--aggressive",
            ])
            .is_ok()
        );
        // Cloudflare-parity: only `gateway code exec` survives. There is no
        // `gateway code search` (that was the dead `code_search` pattern).
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "code",
                "search",
                "--code",
                "async () => tools.slice(0, 3)",
            ])
            .is_err(),
            "`gateway code search` was removed per spec — only `gateway code exec` is supported"
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "code", "schema", "github::search_issues"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "code",
                "exec",
                "--code",
                "await callTool(\"github::search_issues\", {query:\"repo\"})",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "code", "exec", "--file", "snippet.js",])
                .is_ok()
        );
    }

    #[test]
    fn gateway_add_defaults_resource_proxying_on() {
        let cli = Cli::try_parse_from([
            "lab",
            "gateway",
            "add",
            "--name",
            "fixture-http",
            "--url",
            "http://127.0.0.1:8791",
        ])
        .expect("gateway add parses");

        let Command::Gateway(args) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::Add(args) = args.command else {
            panic!("expected gateway add command");
        };

        assert!(args.proxy_resources);
    }

    #[test]
    fn gateway_add_allows_resource_proxying_opt_out() {
        let cli = Cli::try_parse_from([
            "lab",
            "gateway",
            "add",
            "--name",
            "fixture-http",
            "--url",
            "http://127.0.0.1:8791",
            "--proxy-resources",
            "false",
        ])
        .expect("gateway add parses");

        let Command::Gateway(args) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::Add(args) = args.command else {
            panic!("expected gateway add command");
        };

        assert!(!args.proxy_resources);
    }

    #[tokio::test]
    async fn gateway_cli_manager_wires_upstream_oauth_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = LabConfig {
            upstream: vec![UpstreamConfig {
                name: "axon".to_string(),
                enabled: true,
                priority: 1.0,
                url: Some("https://axon.example.com/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: Some(UpstreamOauthConfig {
                    mode: UpstreamOauthMode::AuthorizationCodePkce,
                    registration: UpstreamOauthRegistration::Dynamic,
                    scopes: None,
                }),
                imported_from: None,
            }],
            ..LabConfig::default()
        };
        let sqlite = lab_auth::sqlite::SqliteStore::open(dir.path().join("auth.sqlite"))
            .await
            .expect("sqlite store");
        let key_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [9_u8; 32]);
        let key = load_key(&key_b64).expect("encryption key");
        let oauth_runtime = build_upstream_oauth_runtime_from_parts(
            &config,
            sqlite,
            key,
            "https://lab.example.com/auth/upstream/callback".to_string(),
        );

        let manager =
            build_manager_with_upstream_oauth_runtime(&config, true, Some(oauth_runtime)).await;

        assert!(
            manager.upstream_oauth_manager("axon").is_some(),
            "gateway CLI manager must register OAuth managers for OAuth upstreams"
        );
        assert!(
            manager.oauth_client_cache().is_some(),
            "gateway CLI manager must install an OAuth client cache for Code Mode and upstream calls"
        );
    }
}
