//! `labby setup` — first-run wizard entry point.
//!
//! Thin CLI shim over the `setup` dispatch service. Detects first-run via
//! `setup.state`, then prints either:
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

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::output::theme::CliTheme;
use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Setup UI mode. Standalone setup defaults to full; /setup-core passes plugin.
    #[arg(long, value_enum, default_value_t = SetupModeArg::Full)]
    pub mode: SetupModeArg,

    /// Skip the wizard and exit cleanly. Equivalent to LAB_SKIP_SETUP=1.
    #[arg(long)]
    pub no_setup: bool,

    /// Do not attempt to open the browser (no-op for now; reserved for
    /// the follow-up that adds `webbrowser` integration).
    #[arg(long)]
    pub no_browser: bool,

    /// Smoke-test mode: print the state machine snapshot as JSON and exit.
    /// Used by `just smoke-setup` for CI verification.
    #[arg(long)]
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
    /// Manage the local setup draft.
    Draft(DraftArgs),
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
    /// Copy the labby binary into ~/.local/bin so it is callable in your own terminal.
    Install,
    /// Install the Claude Code plugin for a configured service.
    InstallPlugin(PluginMutationArgs),
    /// Uninstall the Claude Code plugin for a service.
    UninstallPlugin(PluginMutationArgs),
}

#[derive(Debug, Args)]
pub struct DraftArgs {
    #[command(subcommand)]
    pub command: DraftCommand,
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

/// Default URL for the embedded web UI (per Q1: 127.0.0.1:8765).
const DEFAULT_LAB_URL: &str = "http://127.0.0.1:8765";

fn install_self() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let name = exe
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("cannot determine binary name"))?;
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let bin_dir = std::path::PathBuf::from(home).join(".local").join("bin");
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

pub async fn run(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if let Some(command) = args.command {
        return run_command(command, format).await;
    }

    let theme = CliTheme::from_context(format.render_context());

    if std::env::var("LAB_SKIP_SETUP").as_deref() == Ok("1") || args.no_setup {
        eprintln!(
            "{}",
            theme.muted(
                "setup skipped (LAB_SKIP_SETUP=1 or --no-setup); run `labby setup` manually when ready"
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

async fn run_command(command: SetupCommand, format: OutputFormat) -> Result<ExitCode> {
    match command {
        SetupCommand::Draft(args) => {
            run_draft_command(args, format).await?;
        }
        SetupCommand::InstalledPlugins { force } => {
            let value =
                crate::dispatch::setup::dispatch("installed_plugins", json!({ "force": force }))
                    .await?;
            print(&value, format)?;
        }
        SetupCommand::ServicesStatus => {
            let value = crate::dispatch::setup::dispatch("services_status", json!({})).await?;
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
        SetupCommand::Install => {
            let dest = install_self()?;
            println!("installed -> {}", dest.display());
        }
        SetupCommand::InstallPlugin(args) => {
            run_plugin_mutation("install_plugin", args, format).await?;
        }
        SetupCommand::UninstallPlugin(args) => {
            run_plugin_mutation("uninstall_plugin", args, format).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
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
            "http://dookie:8765",
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
        assert_eq!(url, "http://dookie:8765");
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
}
