use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::json;
use tokio::time::sleep;

use crate::cli::helpers::{run_action_command, run_confirmable_action_command};
use crate::config::{LabConfig, ProtectedMcpRouteConfig, config_toml_path};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::dispatch::gateway::install_gateway_manager;
use crate::dispatch::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
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
    List,
    Get(GatewayGetArgs),
    Test(GatewayTestArgs),
    Add(GatewayAddArgs),
    Update(GatewayUpdateArgs),
    Remove(GatewayRemoveArgs),
    Quarantine(GatewayQuarantineArgs),
    ProtectedRoute(GatewayProtectedRouteArgs),
    ToolSearch(GatewayToolSearchArgs),
    Reload,
    Mcp(GatewayMcpArgs),
    /// Scan the machine for MCP server configs from known editors and tools (read-only)
    Discover(GatewayDiscoverArgs),
    /// Import discovered MCP servers into the gateway (disabled by default)
    Import(GatewayImportArgs),
    /// Show resolved public URL configuration (app and MCP gateway)
    PublicUrls,
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
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, default_value_t = false)]
    pub allow_stdio: bool,
}

#[derive(Debug, Args)]
pub struct GatewayAddArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub command: Option<String>,
    #[arg(long = "arg")]
    pub args: Vec<String>,
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    #[arg(long, default_value_t = false)]
    pub proxy_resources: bool,
    #[arg(long, default_value_t = false)]
    pub allow_stdio: bool,
}

#[derive(Debug, Args)]
pub struct GatewayUpdateArgs {
    pub name: String,
    #[arg(long)]
    pub new_name: Option<String>,
    #[arg(long)]
    pub url: Option<String>,
    #[arg(long)]
    pub command: Option<String>,
    #[arg(long = "arg")]
    pub args: Vec<String>,
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    #[arg(long)]
    pub proxy_resources: Option<bool>,
    #[arg(long, default_value_t = false)]
    pub allow_stdio: bool,
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
    List,
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
    List,
    Get(GatewayProtectedRouteNameArgs),
    Add(GatewayProtectedRouteUpsertArgs),
    Update(GatewayProtectedRouteUpdateArgs),
    Remove(GatewayProtectedRouteNameArgs),
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
pub struct GatewayToolSearchArgs {
    #[command(subcommand)]
    pub command: GatewayToolSearchCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayToolSearchCommand {
    Status,
    Enable(GatewayToolSearchSetArgs),
    Disable,
}

#[derive(Debug, Args)]
pub struct GatewayToolSearchSetArgs {
    #[arg(long)]
    pub top_k_default: Option<usize>,
    #[arg(long)]
    pub max_tools: Option<usize>,
}

#[derive(Debug, Args)]
pub struct GatewayMcpArgs {
    #[command(subcommand)]
    pub command: GatewayMcpCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayMcpCommand {
    Auth(GatewayMcpAuthArgs),
    List,
    Enable(GatewayMcpLifecycleArgs),
    Disable(GatewayMcpLifecycleArgs),
    Cleanup(GatewayMcpCleanupArgs),
}

#[derive(Debug, Args)]
pub struct GatewayMcpAuthArgs {
    #[command(subcommand)]
    pub command: GatewayMcpAuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayMcpAuthCommand {
    Start(GatewayOauthUpstreamArgs),
    Open(GatewayOauthUpstreamArgs),
    Status(GatewayOauthUpstreamArgs),
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
    pub allow_stdio: bool,
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

#[derive(Debug, Deserialize)]
struct GatewayOauthStatusView {
    authenticated: bool,
}

async fn build_manager(config: &LabConfig, discover_upstreams: bool) -> Arc<GatewayManager> {
    let runtime = GatewayRuntimeHandle::default();
    let registry = filtered_builtin_service_registry(config);
    if discover_upstreams {
        let pool = Arc::new(UpstreamPool::new());
        pool.discover_all_with_in_process_peers(&config.upstream, &registry)
            .await;
        runtime.swap(Some(pool)).await;
    }

    let manager = Arc::new(
        GatewayManager::new(
            config_toml_path().unwrap_or_else(|| "config.toml".into()),
            runtime,
        )
        .with_builtin_service_registry(registry)
        .with_service_clients(SharedServiceClients::from_env()),
    );
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
    let manager = build_manager(config, discover_upstreams).await;
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
                        "allow_stdio": args.allow_stdio,
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
                        "allow_stdio": args.allow_stdio,
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
        command => {
            let mut confirmed = true;
            let (action, params) = match command {
                GatewayCommand::List => ("gateway.list".to_string(), json!({})),
                GatewayCommand::Get(args) => {
                    ("gateway.get".to_string(), json!({ "name": args.name }))
                }
                GatewayCommand::Test(args) => (
                    "gateway.test".to_string(),
                    json!({ "name": args.name, "allow_stdio": args.allow_stdio }),
                ),
                GatewayCommand::Add(args) => (
                    "gateway.add".to_string(),
                    json!({
                        "origin": cli_origin,
                        "owner": cli_owner,
                        "allow_stdio": args.allow_stdio,
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
                        "allow_stdio": args.allow_stdio,
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
                GatewayCommand::ToolSearch(args) => match args.command {
                    GatewayToolSearchCommand::Status => {
                        ("gateway.tool_search.get".to_string(), json!({}))
                    }
                    GatewayToolSearchCommand::Enable(args) => (
                        "gateway.tool_search.set".to_string(),
                        json!({
                            "enabled": true,
                            "top_k_default": args.top_k_default,
                            "max_tools": args.max_tools,
                        }),
                    ),
                    GatewayToolSearchCommand::Disable => (
                        "gateway.tool_search.set".to_string(),
                        json!({ "enabled": false }),
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
                GatewayCommand::PublicUrls => ("gateway.public_urls.get".to_string(), json!({})),
                GatewayCommand::Mcp(_) => unreachable!("handled above"),
            };

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

    if args.open {
        open_in_browser(&start_view.authorization_url)?;
        eprintln!("Opened authorization URL in your browser.");
    } else {
        eprintln!(
            "Open this URL to authorize:\n{}",
            start_view.authorization_url
        );
    }

    if args.wait {
        let subject = args
            .subject
            .as_deref()
            .unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT);
        eprintln!(
            "Waiting for OAuth completion for `{}` using shared subject `{}`...",
            args.name, subject
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(args.wait_timeout_secs);
        loop {
            let status_value = crate::dispatch::gateway::dispatch_with_manager(
                &manager,
                "gateway.oauth.status",
                json!({ "upstream": args.name, "subject": subject }),
            )
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "{}",
                    serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
                )
            })?;
            let status: GatewayOauthStatusView =
                serde_json::from_value(status_value).map_err(|error| {
                    anyhow::anyhow!("failed to decode gateway oauth status response: {error}")
                })?;
            if status.authenticated {
                eprintln!(
                    "OAuth completed for `{}`. The existing callback route stored credentials for shared subject `{}`.",
                    args.name, subject
                );
                break;
            }
            if std::time::Instant::now() >= deadline {
                eprintln!(
                    "Timed out waiting for OAuth completion for `{}` after {}s. The browser callback may still succeed later; re-run `labby gateway mcp auth status {}` to check.",
                    args.name, args.wait_timeout_secs, args.name
                );
                break;
            }
            sleep(Duration::from_secs(1)).await;
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::Parser;

    use crate::cli::Cli;

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
    }
}
