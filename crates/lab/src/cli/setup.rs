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
    /// List installed Claude Code lab plugins.
    InstalledPlugins {
        /// Bypass the short in-process cache.
        #[arg(long)]
        force: bool,
    },
    /// Join service configuration, draft, and Claude plugin state.
    ServicesStatus,
    /// Install the Claude Code plugin for a configured service.
    InstallPlugin(PluginMutationArgs),
    /// Uninstall the Claude Code plugin for a service.
    UninstallPlugin(PluginMutationArgs),
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

pub async fn run(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if let Some(command) = args.command {
        return run_command(command, format).await;
    }

    if std::env::var("LAB_SKIP_SETUP").as_deref() == Ok("1") || args.no_setup {
        eprintln!(
            "setup skipped (LAB_SKIP_SETUP=1 or --no-setup); run `labby setup` manually when ready"
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
        eprintln!("Welcome to lab. First-run detected.");
    } else {
        eprintln!("lab is already configured. Opening Settings.");
    }
    eprintln!();
    eprintln!("→ Run `labby serve` and visit: {url}");
    eprintln!();
    eprintln!("Tip: set LAB_SKIP_SETUP=1 to suppress this message in CI.");
    Ok(ExitCode::SUCCESS)
}

async fn run_command(command: SetupCommand, format: OutputFormat) -> Result<ExitCode> {
    match command {
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
        SetupCommand::InstallPlugin(args) => {
            run_plugin_mutation("install_plugin", args, format).await?;
        }
        SetupCommand::UninstallPlugin(args) => {
            run_plugin_mutation("uninstall_plugin", args, format).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
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
        crate::cli::helpers::print_dry_run("setup", action, &params);
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
mod tests {
    use super::*;

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
}
