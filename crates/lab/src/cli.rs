//! Top-level CLI — clap derive definitions and dispatch router.
//!
//! Every subcommand is a thin shim that parses args, calls into a
//! `lab-apis` client (or a lab-local subsystem), and formats output.
//! See `crates/lab/src/cli/CLAUDE.md` for the rulebook.

pub mod docs;
pub mod doctor;
pub mod extract;
pub mod gateway;
pub mod health;
pub mod help;
pub mod helpers;
pub mod install;
pub mod logs;
pub mod marketplace;
#[cfg(feature = "mcpregistry")]
pub mod mcpregistry;
pub mod nodes;
pub mod oauth;
pub mod params;
pub mod serve;
pub mod setup;
pub mod stash;

#[cfg(feature = "deploy")]
pub mod deploy;
// [lab-scaffold: cli-modules]

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::LabConfig;
use crate::output::{ColorPolicy, OutputFormat, RenderEnv};

/// `lab` — pluggable homelab CLI + MCP server SDK.
#[derive(Debug, Parser)]
#[command(name = "labby", version, about, long_about = None, disable_help_subcommand = true)]
pub struct Cli {
    /// Emit JSON instead of human-readable tables.
    #[arg(long, global = true)]
    pub json: bool,

    /// Control human-readable CLI styling.
    #[arg(long, global = true, value_enum, default_value_t = ColorPolicy::Auto)]
    pub color: ColorPolicy,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Resolved output format based on the `--json` flag.
    #[must_use]
    pub fn format(&self) -> OutputFormat {
        OutputFormat::from_json_flag(self.json, self.color, RenderEnv::stdout())
    }
}

/// Every top-level subcommand. Service subcommands are added in later
/// plans as each service comes online.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the MCP server (stdio or HTTP transport).
    Serve(serve::ServeArgs),
    /// Start the MCP server over stdio.
    Mcp(serve::McpServeArgs),
    /// Audit configured services and report problems.
    Doctor(doctor::DoctorArgs),
    /// Generate and verify code-owned documentation artifacts.
    Docs(docs::DocsArgs),
    /// Query nodes from the configured controller.
    Nodes(nodes::NodesArgs),
    /// Quick reachability check for configured services.
    Health,
    /// Install one or more services into `.mcp.json`.
    Install(install::InstallArgs),
    /// Uninstall services from `.mcp.json`.
    Uninstall(install::UninstallArgs),
    /// First-time setup wizard.
    Init,
    /// Open the web-based first-run wizard (or settings) — lab-bg3e.3.
    Setup(setup::SetupArgs),
    /// Print the service + action catalog.
    Help(help::HelpArgs),
    /// Generate shell completions.
    /// Scan a local or SSH appdata path and extract service credentials.
    Extract(extract::ExtractCmd),
    /// Manage proxied upstream MCP gateways.
    Gateway(gateway::GatewayArgs),
    /// Run local OAuth callback relay helpers.
    Oauth(oauth::OauthArgs),
    /// Search fleet logs on the configured master.
    Logs(logs::LogsArgs),
    /// Claude plugin marketplace manager.
    Marketplace(marketplace::MarketplaceArgs),
    /// MCP Registry — look up and install servers from registry.modelcontextprotocol.io.
    #[cfg(feature = "mcpregistry")]
    Registry(mcpregistry::RegistryArgs),
    /// Component versioning and deployment.
    Stash(stash::StashArgs),
    /// Radarr movie collection manager.
    /// Sonarr TV series manager.
    /// Prowlarr indexer manager.
    /// Plex media server.
    /// Tautulli Plex analytics.
    /// `SABnzbd` download client.
    /// qBittorrent download client.
    /// Tailscale VPN network.
    /// Linkding bookmark manager.
    /// Memos note-taking service.
    /// Beads issue tracker.
    /// Bytestash snippet manager.
    /// Arcane Docker management UI.
    /// Unraid server management.
    /// `UniFi` network management.
    /// Overseerr media request manager.
    /// Gotify push notifications.
    /// `OpenAI` API client.
    /// Upstream OpenACP daemon.
    /// Google NotebookLM client.
    /// Qdrant vector database.
    /// HF Text Embeddings Inference.
    /// Apprise notification dispatcher.
    /// Deploy the local lab release binary to SSH targets.
    #[cfg(feature = "deploy")]
    Deploy(deploy::DeployArgs),
    // [lab-scaffold: cli-variants]
}

/// Dispatch a parsed [`Cli`] to the correct handler.
pub async fn dispatch(cli: Cli, config: LabConfig) -> Result<ExitCode> {
    let format = cli.format();
    let color = cli.color;
    match cli.command {
        Command::Serve(args) => serve::run(args, &config).await,
        Command::Mcp(args) => serve::run_mcp(args, &config).await,
        Command::Doctor(args) => doctor::run(args, format).await,
        Command::Docs(args) => docs::run(args),
        Command::Nodes(args) => nodes::run(args, format, &config).await,
        Command::Health => health::run(format).await,
        Command::Install(args) => install::run_install(&args).map(|()| ExitCode::SUCCESS),
        Command::Uninstall(args) => install::run_uninstall(&args).map(|()| ExitCode::SUCCESS),
        Command::Init => install::run_init().map(|()| ExitCode::SUCCESS),
        Command::Setup(args) => setup::run(args, format).await,
        Command::Help(args) => help::run(args, format),
        Command::Extract(cmd) => cmd.run(color).await.map(|()| ExitCode::SUCCESS),
        Command::Gateway(args) => gateway::run(args, format, &config).await,
        Command::Oauth(args) => oauth::run(args, &config).await,
        Command::Logs(args) => logs::run(args, format, &config).await,
        Command::Marketplace(args) => marketplace::run(args, format).await,
        #[cfg(feature = "mcpregistry")]
        Command::Registry(args) => mcpregistry::run(args, format).await,
        Command::Stash(args) => stash::run(args, format).await,
        #[cfg(feature = "deploy")]
        Command::Deploy(args) => dispatch_deploy(args, format, config.deploy.clone()).await,
        // [lab-scaffold: cli-dispatch]
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn cli_parses_global_color_flag() {
        let cli = Cli::parse_from(["lab", "--color", "plain", "doctor"]);
        assert_eq!(cli.color, ColorPolicy::Plain);
        assert!(matches!(cli.command, Command::Doctor(_)));
    }

    #[test]
    fn cli_defaults_color_policy_to_auto() {
        let cli = Cli::parse_from(["lab", "doctor"]);
        assert_eq!(cli.color, ColorPolicy::Auto);
        assert!(matches!(cli.command, Command::Doctor(_)));
    }

    #[test]
    fn cli_doctor_accepts_auth_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "auth"]);
        assert!(matches!(
            cli.command,
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::Auth)
            })
        ));
    }

    #[test]
    fn cli_doctor_accepts_system_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "system"]);
        assert!(matches!(
            cli.command,
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::System)
            })
        ));
    }

    #[test]
    fn cli_doctor_accepts_services_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "services"]);
        assert!(matches!(
            cli.command,
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::Services)
            })
        ));
    }
}

/// Deploy dispatch extracted to a helper so the match arm stays a single expression,
/// which prevents rustfmt from merging the `// [lab-scaffold: cli-dispatch]` anchor
/// with a trailing `}`.
#[cfg(feature = "deploy")]
async fn dispatch_deploy(
    args: deploy::DeployArgs,
    format: OutputFormat,
    deploy_cfg: Option<crate::config::DeployPreferences>,
) -> Result<ExitCode> {
    let runner = crate::dispatch::deploy::client::build_runner(deploy_cfg.unwrap_or_default());
    deploy::run(args, format, &runner)
        .await
        .map(|()| ExitCode::SUCCESS)
}
