//! `labby setup` — primary Incus bootstrap entry point.
//!
//! Bare `labby setup` converges the supported Incus Labby gateway container.
//! The web configuration flow remains available as `labby setup wizard`.
//!
//! The wizard is a thin CLI shim over the `setup` dispatch service. It detects
//! first-run via `setup.state`, then prints either:
//!
//! - first-run: instructions to start `labby serve` and visit `/setup`, or
//! - re-run: instructions to visit `/settings`.
//!
//! Honors `LAB_SKIP_SETUP=1` and `--no-setup` for CI / power users.
//!
//! Browser auto-launch is intentionally deferred to a follow-up so this PR
//! avoids adding the `webbrowser` dependency. The bead's locked decision
//! includes browser launch + headless detection; that wiring can land
//! incrementally without breaking the CLI surface contract.

use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};

use crate::output::theme::CliTheme;
use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Provision this Ubuntu 24.04/Incus box for the Labby gateway.
    #[arg(long)]
    pub provision: bool,

    /// Print the default Incus/provisioning plan and do not mutate anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Confirm provisioning without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,

    /// Skip runtime dependency installation and only converge user/service state.
    #[arg(long)]
    pub skip_deps: bool,

    /// Setup UI mode for `labby setup wizard`.
    #[arg(long, value_enum, default_value_t = SetupModeArg::Full, hide = true)]
    pub mode: SetupModeArg,

    /// Skip the wizard and exit cleanly. Equivalent to LAB_SKIP_SETUP=1.
    #[arg(long, hide = true)]
    pub no_setup: bool,

    /// Do not attempt to open the browser (no-op for now; reserved for
    /// the follow-up that adds `webbrowser` integration).
    #[arg(long, hide = true)]
    pub no_browser: bool,

    /// Smoke-test mode: print the state machine snapshot as JSON and exit.
    /// Used by `just smoke-setup` for CI verification.
    #[arg(long, hide = true)]
    pub smoke: bool,

    #[command(subcommand)]
    pub command: Option<SetupCommand>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SetupModeArg {
    Plugin,
    Full,
}

impl SetupModeArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// Open the web-based first-run wizard or settings flow.
    Wizard(WizardArgs),
    /// Manage the local setup draft.
    Draft(DraftArgs),
    /// Manage the systemd Labby gateway service.
    HostService(HostServiceArgs),
    /// List installed Claude Code lab plugins.
    InstalledPlugins {
        /// Bypass the short in-process cache.
        #[arg(long)]
        force: bool,
    },
    /// Join service configuration, draft, and Claude plugin state.
    ServicesStatus,
    /// Run binary-owned local setup checks for Claude plugin hooks.
    PluginHook {
        /// Check only; do not create missing local setup files.
        #[arg(long)]
        no_repair: bool,
    },
    /// Sync CLAUDE_PLUGIN_OPTION_* env vars into ~/.lab/.env as LAB_* vars.
    PluginSync(PluginSyncArgs),
    /// Read ~/.lab/.env and print current values keyed by userConfig field name.
    PluginExport,
    /// Validate connectivity to the lab MCP server.
    PluginConnectivity {
        /// Server URL to probe; defaults to CLAUDE_PLUGIN_OPTION_SERVER_URL or http://localhost:8765.
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Check local setup prerequisites without mutating the filesystem.
    Check,
    /// Repair missing local setup prerequisites without contacting external services.
    Repair,
    /// Validate or apply local Incus backup policy.
    #[command(alias = "incus-backup")]
    Incusbackup(IncusBackupArgs),
    /// Bootstrap or converge the supported Incus Labby gateway container.
    Incus(IncusBootstrapArgs),
    /// Copy the labby binary into ~/.local/bin so it is callable in your own terminal.
    Install,
    /// Install the Claude Code plugin for a configured service.
    InstallPlugin(PluginMutationArgs),
    /// Uninstall the Claude Code plugin for a service.
    UninstallPlugin(PluginMutationArgs),
}

#[derive(Debug, Args, Clone, Copy)]
pub struct WizardArgs {
    /// Setup UI mode. Standalone setup defaults to full; /setup-core passes plugin.
    #[arg(long, value_enum, default_value_t = SetupModeArg::Full)]
    pub mode: SetupModeArg,
    /// Skip the wizard and exit cleanly. Equivalent to LAB_SKIP_SETUP=1.
    #[arg(long)]
    pub no_setup: bool,
    /// Do not attempt to open the browser.
    #[arg(long)]
    pub no_browser: bool,
    /// Smoke-test mode: print the state machine snapshot as JSON and exit.
    #[arg(long)]
    pub smoke: bool,
}

#[derive(Debug, Args)]
pub struct DraftArgs {
    #[command(subcommand)]
    pub command: DraftCommand,
}

#[derive(Debug, Args)]
pub struct HostServiceArgs {
    #[command(subcommand)]
    pub command: HostServiceCommand,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum HostServiceCommand {
    /// Print the system unit that Labby would install.
    Unit,
    /// Install and start labby.service as a system unit.
    Install {
        /// Copy this labby binary into /usr/local/bin/labby before installing the service.
        #[arg(long)]
        install_self: bool,
        /// Confirm installation and service start.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Read labby.service status.
    Status,
    /// Restart labby.service.
    Restart {
        /// Copy this labby binary into /usr/local/bin/labby before restarting the service.
        #[arg(long)]
        install_self: bool,
        /// Confirm service restart.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Stop, disable, and remove labby.service.
    Uninstall {
        /// Confirm service removal.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DraftCommand {
    /// Delete ~/.lab/.env.draft without modifying ~/.lab/.env.
    Discard(DraftDiscardArgs),
}

#[derive(Debug, Args)]
pub struct DraftDiscardArgs {
    /// Confirm discard without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be dispatched without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PluginSyncArgs {
    /// Skip confirmation for this destructive action.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be dispatched without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PluginMutationArgs {
    /// Service name, for example `plex` or `radarr`.
    pub service: String,
    /// Skip confirmation for destructive actions.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be dispatched without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct IncusBackupArgs {
    #[command(subcommand)]
    pub command: IncusBackupCommand,
}

#[derive(Debug, Subcommand)]
pub enum IncusBackupCommand {
    /// Validate a backup policy YAML without mutating Incus.
    Validate {
        /// Backup policy YAML to validate.
        #[arg(long, default_value = "config/incus/labby-backup.yaml")]
        config: PathBuf,
    },
    /// Apply a backup policy YAML to an Incus instance.
    Apply {
        /// Incus container name.
        #[arg(long)]
        name: String,
        /// Backup policy YAML to apply.
        #[arg(long, default_value = "config/incus/labby-backup.yaml")]
        config: PathBuf,
        /// Print the changes without mutating Incus.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
pub struct IncusBootstrapArgs {
    /// Container name (default: labby).
    #[arg(long)]
    pub name: Option<String>,
    /// Incus image alias (default: images:ubuntu/24.04).
    #[arg(long)]
    pub image: Option<String>,
    /// Incus profile name (default: labby-gateway).
    #[arg(long)]
    pub profile_name: Option<String>,
    /// Incus snapshot policy YAML path; defaults to the embedded policy.
    #[arg(long)]
    pub backup_config: Option<PathBuf>,
    /// Do not apply an Incus snapshot policy.
    #[arg(long)]
    pub no_backup_config: bool,
    /// Rootless profile for existing containers with a different root pool.
    #[arg(long)]
    pub runtime_profile_name: Option<String>,
    /// Incus storage driver: zfs, btrfs, or dir.
    #[arg(long)]
    pub storage_driver: Option<String>,
    /// Incus storage pool used by the profile root disk.
    #[arg(long)]
    pub storage_pool: Option<String>,
    /// Incus storage source path/dataset for the pool.
    #[arg(long)]
    pub storage_source: Option<String>,
    /// Labby release tag to install, e.g. v0.28.0.
    #[arg(long, default_value = "latest")]
    pub version: Option<String>,
    /// Push a locally built labby binary instead of downloading a release.
    #[arg(long)]
    pub local_binary: Option<PathBuf>,
    /// Use the labby binary already baked into the selected image.
    #[arg(long)]
    pub skip_install: bool,
    /// Print bootstrap commands only.
    #[arg(long)]
    pub dry_run: bool,
    /// Run tailscale up with --ssh when TS_AUTHKEY is set.
    #[arg(long)]
    pub tailscale_ssh: bool,
    /// Allow install.sh cargo fallback if the release asset is unavailable.
    #[arg(long)]
    pub allow_source_fallback: bool,
    /// Confirm bootstrap without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

impl Default for IncusBootstrapArgs {
    fn default() -> Self {
        Self {
            name: None,
            image: None,
            profile_name: None,
            backup_config: None,
            no_backup_config: false,
            runtime_profile_name: None,
            storage_driver: None,
            storage_pool: None,
            storage_source: None,
            version: Some("latest".to_string()),
            local_binary: None,
            skip_install: false,
            dry_run: false,
            tailscale_ssh: false,
            allow_source_fallback: false,
            yes: false,
        }
    }
}

/// Default URL for the embedded web UI (per Q1: 127.0.0.1:8765).
const DEFAULT_LAB_URL: &str = "http://127.0.0.1:8765";

fn install_self() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let name = exe
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("cannot determine binary name"))?;
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let bin_dir = PathBuf::from(home).join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let dest = bin_dir.join(name);
    if dest == exe {
        return Ok(dest);
    }
    let tmp = bin_dir.join(format!(".{}.tmp", name.to_string_lossy()));
    std::fs::copy(&exe, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &dest)?;
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == bin_dir))
        .unwrap_or(false);
    if !on_path {
        eprintln!(
            "note: {} is not on your PATH; add:  export PATH=\"$HOME/.local/bin:$PATH\"",
            bin_dir.display()
        );
    }
    Ok(dest)
}

fn install_self_system() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let bin_dir = PathBuf::from("/usr/local/bin");
    std::fs::create_dir_all(&bin_dir)?;
    let dest = bin_dir.join("labby");
    if dest == exe {
        return Ok(dest);
    }
    let tmp = bin_dir.join(".labby.tmp");
    std::fs::copy(&exe, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

pub async fn run(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if args.provision {
        return run_provision(args, format).await;
    }
    if let Some(command) = args.command {
        return run_command(command, format).await;
    }
    if args.smoke || args.no_setup || args.no_browser || !matches!(args.mode, SetupModeArg::Full) {
        return run_wizard(
            WizardArgs {
                mode: args.mode,
                no_setup: args.no_setup,
                no_browser: args.no_browser,
                smoke: args.smoke,
            },
            format,
        )
        .await;
    }
    if args.skip_deps {
        anyhow::bail!("--skip-deps is only valid with --provision");
    }

    let incus_args = IncusBootstrapArgs {
        dry_run: args.dry_run,
        yes: args.yes,
        ..IncusBootstrapArgs::default()
    };
    run_incus_bootstrap_command(incus_args).await?;
    Ok(ExitCode::SUCCESS)
}

async fn run_wizard(args: WizardArgs, format: OutputFormat) -> Result<ExitCode> {
    let theme = CliTheme::from_context(format.render_context());

    if std::env::var("LAB_SKIP_SETUP").as_deref() == Ok("1") || args.no_setup {
        eprintln!(
            "{}",
            theme.muted(
                "setup skipped (LAB_SKIP_SETUP=1 or --no-setup); run `labby setup wizard` manually when ready"
            )
        );
        return Ok(ExitCode::SUCCESS);
    }

    let snapshot = crate::dispatch::setup::dispatch("state", json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("setup.state failed: {e:?}"))?;

    if args.smoke {
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot).unwrap_or_default()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let first_run = snapshot
        .get("first_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let route = if first_run { "/setup" } else { "/settings" };

    let url = format!("{DEFAULT_LAB_URL}{route}?mode={}", args.mode.as_str());
    eprintln!();
    if first_run {
        eprintln!("{}", theme.section("Welcome to lab. First-run detected."));
    } else {
        eprintln!(
            "{}",
            theme.section("lab is already configured. Opening Settings.")
        );
    }
    eprintln!();
    eprintln!(
        "{} Run `labby serve` and visit: {}",
        theme.tertiary("→"),
        theme.accent(&url)
    );
    eprintln!();
    eprintln!(
        "{}",
        theme.muted("Tip: set LAB_SKIP_SETUP=1 to suppress this message in CI.")
    );
    Ok(ExitCode::SUCCESS)
}

async fn run_provision(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if args.command.is_some() {
        anyhow::bail!("--provision cannot be combined with a setup subcommand");
    }
    let mut yes = args.yes;
    let plan = crate::dispatch::setup::provision::provision_plan_text(args.skip_deps);
    if !format.is_json() {
        println!("{plan}");
    }
    if !args.dry_run && !yes {
        if !io::stdin().is_terminal() {
            anyhow::bail!("setup --provision requires --yes when stdin is not a TTY");
        }
        eprint!("Proceed? [y/N] ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        yes = matches!(answer.trim(), "y" | "Y" | "yes" | "YES");
        if !yes {
            anyhow::bail!("setup --provision cancelled");
        }
    }
    let outcome = crate::dispatch::setup::provision::provision(
        crate::dispatch::setup::provision::ProvisionOptions {
            dry_run: args.dry_run,
            yes,
            skip_deps: args.skip_deps,
        },
    )
    .await?;
    if format.is_json() {
        print(&serde_json::to_value(outcome)?, format)?;
    } else if outcome.dry_run {
        println!("dry-run complete; no changes made");
    } else {
        println!(
            "provision complete: executed={}, skipped={}",
            outcome.executed.len(),
            outcome.skipped.len()
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_command(command: SetupCommand, format: OutputFormat) -> Result<ExitCode> {
    match command {
        SetupCommand::Wizard(args) => {
            return run_wizard(args, format).await;
        }
        SetupCommand::Draft(args) => {
            run_draft_command(args, format).await?;
        }
        SetupCommand::HostService(args) => {
            run_host_service_command(args, format).await?;
        }
        SetupCommand::InstalledPlugins { force } => {
            let value =
                crate::dispatch::setup::dispatch("plugins.installed", json!({ "force": force }))
                    .await?;
            print(&value, format)?;
        }
        SetupCommand::ServicesStatus => {
            let value = crate::dispatch::setup::dispatch("services.status", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::PluginHook { no_repair } => {
            // Keep the user's terminal copy in ~/.local/bin fresh each session.
            // Best-effort: a stale or unwritable copy must not block the hook.
            if let Err(err) = install_self() {
                tracing::debug!(?err, "failed to refresh ~/.local/bin copy of labby");
            }
            let value =
                crate::dispatch::setup::dispatch("plugin_hook", json!({ "repair": !no_repair }))
                    .await?;
            print(&value, format)?;
        }
        SetupCommand::PluginSync(args) => {
            let params = json!({ "confirm": true });
            if args.dry_run {
                crate::cli::helpers::print_dry_run("setup", "plugin_sync", &params, format);
                return Ok(ExitCode::SUCCESS);
            }
            // Route through the shared destructive-action helper so TTY users
            // get the interactive confirm prompt and non-TTY callers get a
            // structured refusal — matches the cli/CLAUDE.md contract.
            return crate::cli::helpers::run_confirmable_action_command(
                "setup",
                crate::dispatch::setup::ACTIONS,
                "plugin_sync".to_string(),
                params,
                args.yes,
                format,
                |action, params| async move {
                    crate::dispatch::setup::dispatch(&action, params).await
                },
            )
            .await;
        }
        SetupCommand::PluginExport => {
            let value = crate::dispatch::setup::dispatch("plugin_export", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::PluginConnectivity { server_url } => {
            let params = match server_url {
                Some(url) => json!({ "server_url": url }),
                None => json!({}),
            };
            let value = crate::dispatch::setup::dispatch("plugin_connectivity", params).await?;
            print(&value, format)?;
        }
        SetupCommand::Check => {
            let value = crate::dispatch::setup::dispatch("check", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::Repair => {
            let value = crate::dispatch::setup::dispatch("repair", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::Incusbackup(args) => {
            run_incus_backup_command(args, format).await?;
        }
        SetupCommand::Incus(args) => {
            run_incus_bootstrap_command(args).await?;
        }
        SetupCommand::Install => {
            let dest = install_self()?;
            println!("installed -> {}", dest.display());
        }
        SetupCommand::InstallPlugin(args) => {
            run_plugin_mutation("plugin.install", args, format).await?;
        }
        SetupCommand::UninstallPlugin(args) => {
            run_plugin_mutation("plugin.uninstall", args, format).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_incus_backup_command(args: IncusBackupArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        IncusBackupCommand::Validate { config } => {
            let entries = crate::dispatch::setup::incus::parse_backup_config(&config)?;
            if format.is_json() {
                print(&serde_json::to_value(&entries)?, format)?;
            } else {
                println!("validated {} backup config entries", entries.len());
            }
        }
        IncusBackupCommand::Apply {
            name,
            config,
            dry_run,
        } => {
            let outcome =
                crate::dispatch::setup::incus::apply_backup_config(&name, &config, dry_run)?;
            if format.is_json() {
                print(&serde_json::to_value(&outcome)?, format)?;
            } else if dry_run {
                println!(
                    "dry-run: would apply {} backup config entries to {}",
                    outcome.applied.len(),
                    outcome.container
                );
            } else {
                println!(
                    "applied {} backup config entries to {}",
                    outcome.applied.len(),
                    outcome.container
                );
            }
        }
    }
    Ok(())
}

async fn run_incus_bootstrap_command(args: IncusBootstrapArgs) -> Result<()> {
    confirm_incus_bootstrap(args.dry_run, args.yes)?;
    let options = crate::dispatch::setup::incus::IncusBootstrapOptions {
        name: args.name,
        image: args.image,
        profile_name: args.profile_name,
        backup_config: args.backup_config,
        no_backup_config: args.no_backup_config,
        runtime_profile_name: args.runtime_profile_name,
        storage_driver: args.storage_driver,
        storage_pool: args.storage_pool,
        storage_source: args.storage_source,
        version: args.version,
        local_binary: args.local_binary,
        skip_install: args.skip_install,
        dry_run: args.dry_run,
        tailscale_ssh: args.tailscale_ssh,
        allow_source_fallback: args.allow_source_fallback,
    };
    crate::dispatch::setup::incus::run_incus_bootstrap(options)?;
    Ok(())
}

fn confirm_incus_bootstrap(dry_run: bool, mut yes: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    if !yes {
        if !io::stdin().is_terminal() {
            anyhow::bail!("setup incus requires --yes when stdin is not a TTY");
        }
        eprintln!(
            "This will create or update the Labby Incus container, storage/profile config, in-container labby binary, service state, backup policy, and Tailscale join when TS_AUTHKEY is set."
        );
        eprint!("Proceed? [y/N] ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        yes = matches!(answer.trim(), "y" | "Y" | "yes" | "YES");
    }
    if !yes {
        anyhow::bail!("setup incus cancelled");
    }
    Ok(())
}

async fn run_host_service_command(args: HostServiceArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        HostServiceCommand::Unit => {
            run_host_service_logged(
                "host_service.unit",
                format,
                crate::dispatch::setup::host_service::unit,
            )
            .await?;
        }
        HostServiceCommand::Install {
            install_self: install_self_flag,
            yes,
        } => {
            require_host_service_confirmation("install", yes)?;
            if install_self_flag {
                install_self_system()?;
            }
            run_host_service_logged(
                "host_service.install",
                format,
                crate::dispatch::setup::host_service::install,
            )
            .await?;
        }
        HostServiceCommand::Status => {
            run_host_service_logged(
                "host_service.status",
                format,
                crate::dispatch::setup::host_service::status,
            )
            .await?;
        }
        HostServiceCommand::Restart {
            install_self: install_self_flag,
            yes,
        } => {
            require_host_service_confirmation("restart", yes)?;
            if install_self_flag {
                install_self_system()?;
            }
            run_host_service_logged(
                "host_service.restart",
                format,
                crate::dispatch::setup::host_service::restart,
            )
            .await?;
        }
        HostServiceCommand::Uninstall { yes } => {
            require_host_service_confirmation("uninstall", yes)?;
            run_host_service_logged(
                "host_service.uninstall",
                format,
                crate::dispatch::setup::host_service::uninstall,
            )
            .await?;
        }
    }
    Ok(())
}

fn require_host_service_confirmation(action: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    let error = crate::dispatch::error::ToolError::ConfirmationRequired {
        message: format!("setup host-service {action} is destructive; pass -y/--yes to confirm"),
    };
    Err(anyhow::anyhow!(
        "{}",
        serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
    ))
}

async fn run_host_service_logged<F, Fut, T>(
    action: &'static str,
    format: OutputFormat,
    operation: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, crate::dispatch::error::ToolError>>,
    T: Serialize,
{
    crate::cli::helpers::run_action_command(
        "setup",
        action.to_string(),
        json!({}),
        format,
        |_action, _params| async move {
            let value = operation().await?;
            serde_json::to_value(value).map_err(|err| crate::dispatch::error::ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: err.to_string(),
            })
        },
    )
    .await?;
    Ok(())
}

async fn run_draft_command(args: DraftArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        DraftCommand::Discard(args) => {
            let params = json!({});
            if args.dry_run {
                crate::cli::helpers::print_dry_run("setup", "draft.discard", &params, format);
                return Ok(());
            }
            crate::cli::helpers::run_confirmable_action_command(
                "setup",
                crate::dispatch::setup::ACTIONS,
                "draft.discard".to_string(),
                params,
                args.yes,
                format,
                |action, params| async move {
                    crate::dispatch::setup::dispatch(&action, params).await
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_plugin_mutation(
    action: &'static str,
    args: PluginMutationArgs,
    format: OutputFormat,
) -> Result<()> {
    let params = json!({
        "service": args.service,
        "confirm": true,
    });
    if args.dry_run {
        crate::cli::helpers::print_dry_run("setup", action, &params, format);
        return Ok(());
    }
    if !args.yes {
        anyhow::bail!("setup {action} is destructive; pass -y/--yes to confirm");
    }
    let value = crate::dispatch::setup::dispatch(action, params).await?;
    print(&value, format)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[tokio::test]
    async fn no_setup_flag_exits_cleanly() {
        let code = run(
            SetupArgs {
                provision: false,
                dry_run: false,
                yes: false,
                skip_deps: false,
                mode: SetupModeArg::Full,
                no_setup: true,
                no_browser: true,
                smoke: false,
                command: None,
            },
            OutputFormat::from_json_flag(
                true,
                crate::output::ColorPolicy::Plain,
                crate::output::RenderEnv::stdout(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn bare_setup_parses_as_default_incus_bootstrap() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "-y"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };

        assert!(args.command.is_none());
        assert!(!args.provision);
        assert!(!args.dry_run);
        assert!(args.yes);
    }

    #[test]
    fn parses_setup_wizard_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby", "setup", "wizard", "--mode", "plugin", "--smoke",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Wizard(args)) = args.command else {
            panic!("expected setup wizard subcommand");
        };

        assert!(matches!(args.mode, SetupModeArg::Plugin));
        assert!(args.smoke);
    }

    #[test]
    fn parses_plugin_hook_no_repair_subcommand() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "plugin-hook", "--no-repair"])
            .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::PluginHook { no_repair }) = args.command else {
            panic!("expected plugin-hook subcommand");
        };
        assert!(no_repair);
    }

    #[test]
    fn parses_plugin_sync_export_connectivity_subcommands() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "plugin-sync"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup");
        };
        assert!(matches!(args.command, Some(SetupCommand::PluginSync(_))));

        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "plugin-export"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup");
        };
        assert!(matches!(args.command, Some(SetupCommand::PluginExport)));

        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "plugin-connectivity",
            "--server-url",
            "http://node-a:8765",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup");
        };
        let Some(SetupCommand::PluginConnectivity {
            server_url: Some(url),
        }) = args.command
        else {
            panic!("expected plugin-connectivity with url");
        };
        assert_eq!(url, "http://node-a:8765");
    }

    #[test]
    fn parses_setup_check_and_repair_subcommands() {
        for command in ["check", "repair"] {
            let cli = crate::cli::Cli::try_parse_from(["labby", "setup", command]).unwrap();
            let crate::cli::Command::Setup(args) = cli.command else {
                panic!("expected setup command");
            };
            match (command, args.command) {
                ("check", Some(SetupCommand::Check)) => {}
                ("repair", Some(SetupCommand::Repair)) => {}
                _ => panic!("unexpected setup subcommand for {command}"),
            }
        }
    }

    #[test]
    fn parses_setup_provision_flags() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "--provision",
            "--dry-run",
            "--skip-deps",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };

        assert!(args.provision);
        assert!(args.dry_run);
        assert!(args.skip_deps);
        assert!(args.command.is_none());
    }

    #[test]
    fn parses_incusbackup_apply_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "incusbackup",
            "apply",
            "--name",
            "labby",
            "--config",
            "config/incus/labby-backup.yaml",
            "--dry-run",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Incusbackup(IncusBackupArgs {
            command:
                IncusBackupCommand::Apply {
                    name,
                    config,
                    dry_run,
                },
        })) = args.command
        else {
            panic!("expected setup incusbackup apply subcommand");
        };
        assert_eq!(name, "labby");
        assert_eq!(config, PathBuf::from("config/incus/labby-backup.yaml"));
        assert!(dry_run);
    }

    #[test]
    fn accepts_hidden_hyphenated_incus_backup_alias() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "incus-backup", "validate"])
            .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        assert!(matches!(
            args.command,
            Some(SetupCommand::Incusbackup(IncusBackupArgs {
                command: IncusBackupCommand::Validate { .. }
            }))
        ));
    }

    #[test]
    fn parses_incus_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "incus",
            "--version",
            "v1.2.3",
            "--storage-driver",
            "dir",
            "--name",
            "labby-test",
            "--dry-run",
            "-y",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Incus(args)) = args.command else {
            panic!("expected setup incus subcommand");
        };
        assert_eq!(args.version.as_deref(), Some("v1.2.3"));
        assert_eq!(args.storage_driver.as_deref(), Some("dir"));
        assert_eq!(args.name.as_deref(), Some("labby-test"));
        assert!(args.dry_run);
        assert!(args.yes);
    }

    #[test]
    fn setup_incus_defaults_to_latest_release() {
        let cli =
            crate::cli::Cli::try_parse_from(["labby", "setup", "incus", "--dry-run"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Incus(args)) = args.command else {
            panic!("expected setup incus subcommand");
        };
        assert_eq!(args.version.as_deref(), Some("latest"));
    }

    #[test]
    fn rejects_hyphenated_incus_bootstrap_subcommand() {
        let err =
            crate::cli::Cli::try_parse_from(["labby", "setup", "incus-bootstrap"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn parses_setup_draft_discard_subcommand() {
        let cli =
            crate::cli::Cli::try_parse_from(["labby", "setup", "draft", "discard", "-y"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Draft(DraftArgs {
            command: DraftCommand::Discard(discard),
        })) = args.command
        else {
            panic!("expected setup draft discard subcommand");
        };
        assert!(discard.yes);
    }

    #[test]
    fn parses_host_service_subcommands() {
        for (command, flag, expected) in [
            ("unit", None, HostServiceCommand::Unit),
            (
                "install",
                Some("-y"),
                HostServiceCommand::Install {
                    install_self: false,
                    yes: true,
                },
            ),
            ("status", None, HostServiceCommand::Status),
            (
                "restart",
                Some("-y"),
                HostServiceCommand::Restart {
                    install_self: false,
                    yes: true,
                },
            ),
            (
                "uninstall",
                Some("-y"),
                HostServiceCommand::Uninstall { yes: true },
            ),
            (
                "restart",
                Some("--no-confirm"),
                HostServiceCommand::Restart {
                    install_self: false,
                    yes: true,
                },
            ),
        ] {
            let mut args = vec!["labby", "setup", "host-service", command];
            if let Some(flag) = flag {
                args.push(flag);
            }
            let cli = crate::cli::Cli::try_parse_from(args).unwrap();
            let crate::cli::Command::Setup(args) = cli.command else {
                panic!("expected setup command");
            };
            let Some(SetupCommand::HostService(HostServiceArgs { command: actual })) = args.command
            else {
                panic!("expected setup host-service subcommand");
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn parses_host_service_install_self_flag() {
        for (subcommand, expected) in [
            (
                "install",
                HostServiceCommand::Install {
                    install_self: true,
                    yes: true,
                },
            ),
            (
                "restart",
                HostServiceCommand::Restart {
                    install_self: true,
                    yes: true,
                },
            ),
        ] {
            let cli = crate::cli::Cli::try_parse_from([
                "labby",
                "setup",
                "host-service",
                subcommand,
                "--install-self",
                "-y",
            ])
            .unwrap();
            let crate::cli::Command::Setup(args) = cli.command else {
                panic!("expected setup command");
            };
            let Some(SetupCommand::HostService(HostServiceArgs { command })) = args.command else {
                panic!("expected setup host-service subcommand");
            };
            assert_eq!(command, expected);
        }
    }

    #[tokio::test]
    async fn host_service_destructive_commands_require_confirmation_envelope() {
        for command in [
            HostServiceCommand::Install {
                install_self: false,
                yes: false,
            },
            HostServiceCommand::Restart {
                install_self: false,
                yes: false,
            },
            HostServiceCommand::Uninstall { yes: false },
        ] {
            let err = run_host_service_command(
                HostServiceArgs { command },
                OutputFormat::from_json_flag(
                    true,
                    crate::output::ColorPolicy::Plain,
                    crate::output::RenderEnv::stdout(),
                ),
            )
            .await
            .unwrap_err();
            let envelope: Value = serde_json::from_str(&err.to_string()).unwrap();

            assert_eq!(envelope["kind"], "confirmation_required");
        }
    }
}
