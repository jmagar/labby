//! Top-level CLI — clap derive definitions and dispatch router.
//!
//! Every subcommand is a thin shim that parses args, calls into a
//! `lab-apis` client (or a lab-local subsystem), and formats output.
//! See `crates/lab/src/cli/CLAUDE.md` for the rulebook.

pub mod audit;
#[cfg(feature = "controller")]
pub mod completions;
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
pub mod nodes;
pub mod oauth;
pub mod params;
pub mod plugins;
pub mod scaffold;
pub mod serve;
pub mod setup;
pub mod stash;

#[cfg(feature = "adguard")]
pub mod adguard;
#[cfg(feature = "apprise")]
pub mod apprise;
#[cfg(feature = "arcane")]
pub mod arcane;
#[cfg(feature = "beads")]
pub mod beads;
#[cfg(feature = "bytestash")]
pub mod bytestash;
#[cfg(feature = "deploy")]
pub mod deploy;
#[cfg(feature = "dozzle")]
pub mod dozzle;
#[cfg(feature = "freshrss")]
pub mod freshrss;
#[cfg(feature = "glances")]
pub mod glances;
#[cfg(feature = "gotify")]
pub mod gotify;
#[cfg(feature = "immich")]
pub mod immich;
#[cfg(feature = "jellyfin")]
pub mod jellyfin;
#[cfg(feature = "linkding")]
pub mod linkding;
#[cfg(feature = "loggifly")]
pub mod loggifly;
#[cfg(feature = "memos")]
pub mod memos;
#[cfg(feature = "navidrome")]
pub mod navidrome;
#[cfg(feature = "neo4j")]
pub mod neo4j;
#[cfg(feature = "notebooklm")]
pub mod notebooklm;
#[cfg(feature = "openacp")]
pub mod openacp;
#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "overseerr")]
pub mod overseerr;
#[cfg(feature = "paperless")]
pub mod paperless;
#[cfg(feature = "pihole")]
pub mod pihole;
#[cfg(feature = "plex")]
pub mod plex;
#[cfg(feature = "prowlarr")]
pub mod prowlarr;
#[cfg(feature = "qbittorrent")]
pub mod qbittorrent;
#[cfg(feature = "qdrant")]
pub mod qdrant;
#[cfg(feature = "radarr")]
pub mod radarr;
#[cfg(feature = "sabnzbd")]
pub mod sabnzbd;
#[cfg(feature = "scrutiny")]
pub mod scrutiny;
#[cfg(feature = "sonarr")]
pub mod sonarr;
#[cfg(feature = "tailscale")]
pub mod tailscale;
#[cfg(feature = "tautulli")]
pub mod tautulli;
#[cfg(feature = "tei")]
pub mod tei;
#[cfg(feature = "unifi")]
pub mod unifi;
#[cfg(feature = "unraid")]
pub mod unraid;
#[cfg(feature = "uptime_kuma")]
pub mod uptime_kuma;
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
    /// Open the plugin manager TUI.
    Plugins,
    /// Audit service onboarding against the repo contract.
    Audit(audit::AuditArgs),
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
    /// Generate a new service onboarding scaffold.
    Scaffold(scaffold::ScaffoldArgs),
    /// Generate shell completions.
    #[cfg(feature = "controller")]
    Completions(completions::CompletionsArgs),
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
    /// Component versioning and deployment.
    Stash(stash::StashArgs),
    /// Radarr movie collection manager.
    #[cfg(feature = "radarr")]
    Radarr(radarr::RadarrArgs),
    /// Sonarr TV series manager.
    #[cfg(feature = "sonarr")]
    Sonarr(sonarr::SonarrArgs),
    /// Prowlarr indexer manager.
    #[cfg(feature = "prowlarr")]
    Prowlarr(prowlarr::ProwlarrArgs),
    /// Plex media server.
    #[cfg(feature = "plex")]
    Plex(plex::PlexArgs),
    /// Tautulli Plex analytics.
    #[cfg(feature = "tautulli")]
    Tautulli(tautulli::TautulliArgs),
    /// `SABnzbd` download client.
    #[cfg(feature = "sabnzbd")]
    Sabnzbd(sabnzbd::SabnzbdArgs),
    /// qBittorrent download client.
    #[cfg(feature = "qbittorrent")]
    Qbittorrent(qbittorrent::QbittorrentArgs),
    /// Tailscale VPN network.
    #[cfg(feature = "tailscale")]
    Tailscale(tailscale::TailscaleArgs),
    /// Linkding bookmark manager.
    #[cfg(feature = "linkding")]
    Linkding(linkding::LinkdingArgs),
    /// Memos note-taking service.
    #[cfg(feature = "memos")]
    Memos(memos::MemosArgs),
    /// Beads issue tracker.
    #[cfg(feature = "beads")]
    Beads(beads::BeadsArgs),
    /// Bytestash snippet manager.
    #[cfg(feature = "bytestash")]
    Bytestash(bytestash::BytestashArgs),
    /// Paperless-ngx document manager.
    #[cfg(feature = "paperless")]
    Paperless(paperless::PaperlessArgs),
    /// Arcane Docker management UI.
    #[cfg(feature = "arcane")]
    Arcane(arcane::ArcaneArgs),
    /// Unraid server management.
    #[cfg(feature = "unraid")]
    Unraid(unraid::UnraidArgs),
    /// `UniFi` network management.
    #[cfg(feature = "unifi")]
    Unifi(unifi::UnifiArgs),
    /// Overseerr media request manager.
    #[cfg(feature = "overseerr")]
    Overseerr(overseerr::OverseerrArgs),
    /// Gotify push notifications.
    #[cfg(feature = "gotify")]
    Gotify(gotify::GotifyArgs),
    /// `OpenAI` API client.
    #[cfg(feature = "openai")]
    Openai(openai::OpenaiArgs),
    /// Upstream OpenACP daemon.
    #[cfg(feature = "openacp")]
    Openacp(openacp::OpenAcpArgs),
    /// Google NotebookLM client.
    #[cfg(feature = "notebooklm")]
    Notebooklm(notebooklm::NotebooklmArgs),
    /// Qdrant vector database.
    #[cfg(feature = "qdrant")]
    Qdrant(qdrant::QdrantArgs),
    /// HF Text Embeddings Inference.
    #[cfg(feature = "tei")]
    Tei(tei::TeiArgs),
    /// Apprise notification dispatcher.
    #[cfg(feature = "apprise")]
    Apprise(apprise::AppriseArgs),
    /// Deploy the local lab release binary to SSH targets.
    #[cfg(feature = "deploy")]
    Deploy(deploy::DeployArgs),
    #[cfg(feature = "dozzle")]
    Dozzle(dozzle::DozzleArgs),
    #[cfg(feature = "immich")]
    Immich(immich::ImmichArgs),
    /// Jellyfin media server.
    #[cfg(feature = "jellyfin")]
    Jellyfin(jellyfin::JellyfinArgs),
    #[cfg(feature = "navidrome")]
    Navidrome(navidrome::NavidromeArgs),
    #[cfg(feature = "scrutiny")]
    Scrutiny(scrutiny::ScrutinyArgs),
    #[cfg(feature = "freshrss")]
    Freshrss(freshrss::FreshrssArgs),
    #[cfg(feature = "loggifly")]
    Loggifly(loggifly::LoggiflyArgs),
    #[cfg(feature = "adguard")]
    Adguard(adguard::AdguardArgs),
    #[cfg(feature = "glances")]
    Glances(glances::GlancesArgs),
    #[cfg(feature = "uptime_kuma")]
    UptimeKuma(uptime_kuma::UptimeKumaArgs),
    #[cfg(feature = "pihole")]
    Pihole(pihole::PiholeArgs),
    #[cfg(feature = "neo4j")]
    Neo4j(neo4j::Neo4jArgs),
    // [lab-scaffold: cli-variants]
}

/// Dispatch a parsed [`Cli`] to the correct handler.
#[expect(
    clippy::large_stack_frames,
    reason = "all-features command enum is large and dispatch immediately moves one parsed variant"
)]
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
        Command::Plugins => plugins::run(),
        Command::Audit(args) => audit::run(args, format),
        Command::Install(args) => install::run_install(&args).map(|()| ExitCode::SUCCESS),
        Command::Uninstall(args) => install::run_uninstall(&args).map(|()| ExitCode::SUCCESS),
        Command::Init => install::run_init().map(|()| ExitCode::SUCCESS),
        Command::Setup(args) => setup::run(args, format).await,
        Command::Help(args) => help::run(args, format),
        Command::Scaffold(args) => scaffold::run(args, format),
        #[cfg(feature = "controller")]
        Command::Completions(args) => completions::run(&args),
        Command::Extract(cmd) => cmd.run(color).await.map(|()| ExitCode::SUCCESS),
        Command::Gateway(args) => gateway::run(args, format, &config).await,
        Command::Oauth(args) => oauth::run(args, &config).await,
        Command::Logs(args) => logs::run(args, format, &config).await,
        Command::Marketplace(args) => marketplace::run(args, format).await,
        Command::Stash(args) => stash::run(args, format).await,
        #[cfg(feature = "radarr")]
        Command::Radarr(args) => radarr::run(args, format).await,
        #[cfg(feature = "sonarr")]
        Command::Sonarr(args) => sonarr::run(args, format).await,
        #[cfg(feature = "prowlarr")]
        Command::Prowlarr(args) => prowlarr::run(args, format).await,
        #[cfg(feature = "plex")]
        Command::Plex(args) => plex::run(args, format).await,
        #[cfg(feature = "tautulli")]
        Command::Tautulli(args) => tautulli::run(args, format).await,
        #[cfg(feature = "sabnzbd")]
        Command::Sabnzbd(args) => sabnzbd::run(args, format).await,
        #[cfg(feature = "qbittorrent")]
        Command::Qbittorrent(args) => qbittorrent::run(args, format).await,
        #[cfg(feature = "tailscale")]
        Command::Tailscale(args) => tailscale::run(args, format).await,
        #[cfg(feature = "linkding")]
        Command::Linkding(args) => linkding::run(args, format).await,
        #[cfg(feature = "memos")]
        Command::Memos(args) => memos::run(args, format).await,
        #[cfg(feature = "beads")]
        Command::Beads(args) => beads::run(args, format).await,
        #[cfg(feature = "bytestash")]
        Command::Bytestash(args) => bytestash::run(args, format).await,
        #[cfg(feature = "paperless")]
        Command::Paperless(args) => paperless::run(args, format).await,
        #[cfg(feature = "arcane")]
        Command::Arcane(args) => arcane::run(args, format).await,
        #[cfg(feature = "unraid")]
        Command::Unraid(args) => unraid::run(args, format).await,
        #[cfg(feature = "unifi")]
        Command::Unifi(args) => unifi::run(args, format).await,
        #[cfg(feature = "overseerr")]
        Command::Overseerr(args) => overseerr::run(args, format).await,
        #[cfg(feature = "gotify")]
        Command::Gotify(args) => gotify::run(args, format).await,
        #[cfg(feature = "openai")]
        Command::Openai(args) => openai::run(args, format).await,
        #[cfg(feature = "openacp")]
        Command::Openacp(args) => openacp::run(args, format).await,
        #[cfg(feature = "notebooklm")]
        Command::Notebooklm(args) => notebooklm::run(args, format).await,
        #[cfg(feature = "qdrant")]
        Command::Qdrant(args) => qdrant::run(args, format).await,
        #[cfg(feature = "tei")]
        Command::Tei(args) => tei::run(args, format).await,
        #[cfg(feature = "apprise")]
        Command::Apprise(args) => apprise::run(args, format).await,
        #[cfg(feature = "deploy")]
        Command::Deploy(args) => dispatch_deploy(args, format, config.deploy.clone()).await,
        #[cfg(feature = "dozzle")]
        Command::Dozzle(args) => dozzle::run(args, format).await,
        #[cfg(feature = "immich")]
        Command::Immich(args) => immich::run(args, format).await,
        #[cfg(feature = "jellyfin")]
        Command::Jellyfin(args) => jellyfin::run(args, format).await,
        #[cfg(feature = "navidrome")]
        Command::Navidrome(args) => navidrome::run(args, format).await,
        #[cfg(feature = "scrutiny")]
        Command::Scrutiny(args) => scrutiny::run(args, format).await,
        #[cfg(feature = "freshrss")]
        Command::Freshrss(args) => freshrss::run(args, format).await,
        #[cfg(feature = "loggifly")]
        Command::Loggifly(args) => loggifly::run(args, format).await,
        #[cfg(feature = "adguard")]
        Command::Adguard(args) => adguard::run(args, format).await,
        #[cfg(feature = "glances")]
        Command::Glances(args) => glances::run(args, format).await,
        #[cfg(feature = "uptime_kuma")]
        Command::UptimeKuma(args) => uptime_kuma::run(args, format).await,
        #[cfg(feature = "pihole")]
        Command::Pihole(args) => pihole::run(args, format).await,
        #[cfg(feature = "neo4j")]
        Command::Neo4j(args) => neo4j::run(args, format).await,
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
