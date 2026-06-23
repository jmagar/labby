//! CLI surface for the `deploy` service.
//!
//! Thin shim over `dispatch::deploy`. Destructive actions (`run`,
//! `rollback`) require `-y` / `--yes` / `--no-confirm` on a non-TTY, or
//! will prompt interactively when stdin is a TTY.

use std::io::IsTerminal as _;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::dispatch::deploy;
use crate::dispatch::deploy::authz::McpContext;
use crate::output::theme::CliTheme;
use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct DeployArgs {
    #[command(subcommand)]
    pub cmd: DeployCmd,
}

#[derive(Debug, Subcommand)]
pub enum DeployCmd {
    /// Show resolved deploy hosts and defaults.
    ConfigList,
    /// Dry-run: resolve targets, hash local artifact, show what would happen.
    Plan {
        /// SSH aliases to include in the plan.
        #[arg(required = true)]
        targets: Vec<String>,
    },
    /// Destructive: build, transfer, install, restart, verify.
    Run {
        /// SSH aliases to deploy to.
        #[arg(required = true)]
        targets: Vec<String>,
        /// Confirm the destructive operation (required non-interactively).
        #[arg(short = 'y', long = "yes", visible_alias = "no-confirm")]
        yes: bool,
        /// Dry-run: plan only, do not transfer or install anything.
        #[arg(long)]
        dry_run: bool,
        /// Maximum number of hosts to work on concurrently.
        #[arg(long)]
        max_parallel: Option<u32>,
        /// Abort remaining hosts on the first failure.
        #[arg(long)]
        fail_fast: bool,
    },
    /// Destructive: restore the most recent backup on each target.
    Rollback {
        /// SSH aliases to roll back.
        #[arg(required = true)]
        targets: Vec<String>,
        /// Confirm the destructive operation.
        #[arg(short = 'y', long = "yes", visible_alias = "no-confirm")]
        yes: bool,
        /// Dry-run: plan only, do not roll back.
        #[arg(long)]
        dry_run: bool,
    },
    /// Watch SSH hosts and emit JSON events when they go online or offline.
    ///
    /// Emits one newline-delimited JSON line per state change to stdout.
    /// Suitable for use with the Claude Code Monitor tool.
    Monitor {
        /// SSH aliases to watch (must exist in deploy config).
        #[arg(required = true)]
        targets: Vec<String>,
        /// Poll interval in seconds.
        #[arg(long, default_value = "30")]
        interval: u64,
        /// TCP probe timeout in seconds.
        #[arg(long, default_value = "3")]
        timeout: u64,
    },
}

impl DeployArgs {
    /// Targets extracted from the subcommand (empty for `config-list`).
    #[allow(dead_code)]
    #[must_use]
    pub fn cmd_targets(&self) -> Vec<String> {
        match &self.cmd {
            DeployCmd::Plan { targets }
            | DeployCmd::Run { targets, .. }
            | DeployCmd::Rollback { targets, .. }
            | DeployCmd::Monitor { targets, .. } => targets.clone(),
            DeployCmd::ConfigList => vec![],
        }
    }

    /// Whether the operator passed `-y`.
    #[allow(dead_code)]
    #[must_use]
    pub fn cmd_yes(&self) -> bool {
        matches!(
            &self.cmd,
            DeployCmd::Run { yes: true, .. } | DeployCmd::Rollback { yes: true, .. }
        )
    }
}

/// Require confirmation for a destructive operation.
///
/// - On a TTY: prompts interactively; proceeds on `y`/`Y`.
/// - Not on a TTY without `-y`: returns an error.
fn confirm_destructive(yes: bool, label: &str, theme: CliTheme) -> Result<()> {
    if yes {
        return Ok(());
    }
    if std::io::stdin().is_terminal() {
        eprint!(
            "{} {} ",
            theme.warn(&format!("{label} is destructive. Proceed?")),
            theme.muted("[y/N]")
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            bail!("aborted");
        }
        Ok(())
    } else {
        bail!("{label} is destructive; pass -y / --yes to confirm non-interactively");
    }
}

/// Execute a deploy CLI invocation against the concrete `DefaultRunner`.
pub async fn run(
    args: DeployArgs,
    format: OutputFormat,
    runner: &deploy::runner::DefaultRunner,
) -> Result<()> {
    if let DeployCmd::Monitor {
        targets,
        interval,
        timeout,
    } = args.cmd
    {
        deploy::monitor::watch_hosts(
            runner,
            targets,
            std::time::Duration::from_secs(interval),
            std::time::Duration::from_secs(timeout),
        )
        .await?;
        return Ok(());
    }

    let theme = CliTheme::from_context(format.render_context());
    let (action, params) = match args.cmd {
        DeployCmd::ConfigList => ("config.list", json!({})),
        DeployCmd::Plan { targets } => ("plan", json!({ "targets": targets })),
        DeployCmd::Run {
            targets,
            yes,
            dry_run,
            max_parallel,
            fail_fast,
        } => {
            if dry_run {
                ("plan", json!({ "targets": targets }))
            } else {
                confirm_destructive(yes, "deploy run", theme)?;
                (
                    "run",
                    json!({
                        "targets": targets,
                        "confirm": true,
                        "max_parallel": max_parallel,
                        "fail_fast": fail_fast,
                    }),
                )
            }
        }
        DeployCmd::Rollback {
            targets,
            yes,
            dry_run,
        } => {
            if dry_run {
                ("plan", json!({ "targets": targets }))
            } else {
                confirm_destructive(yes, "deploy rollback", theme)?;
                ("rollback", json!({ "targets": targets, "confirm": true }))
            }
        }
        DeployCmd::Monitor { .. } => unreachable!(),
    };

    // Scope the MCP context to CLI so authz treats this as a local operator
    // action rather than a headless MCP call.
    let value: Value = deploy::authz::MCP_CONTEXT
        .scope(
            McpContext::Cli,
            deploy::dispatch_with_runner(action, params, runner),
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    print(&value, format)?;
    Ok(())
}
