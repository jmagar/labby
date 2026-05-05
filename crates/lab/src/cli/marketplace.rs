//! `labby marketplace` — CLI shim for marketplace plugin management.
//!
//! Most actions follow the shared dispatch layer. `generate` is intentionally
//! CLI-local because it writes a release marketplace tree to disk.
//! Always-on (synthetic service). See `apprise.rs` for the reference pattern.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::helpers::{print_dry_run, run_confirmable_action_command};
use crate::output::OutputFormat;

mod generator;

/// `labby marketplace` arguments.
#[derive(Debug, Args)]
pub struct MarketplaceArgs {
    #[command(subcommand)]
    pub command: Option<MarketplaceCommand>,
    /// Action to run (e.g. sources.list, plugins.list, plugin.install).
    #[arg(default_value = "help")]
    pub action: String,
    /// Action-specific parameters as JSON.
    #[arg(long)]
    pub params: Option<String>,
    /// Skip confirmation for destructive actions.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be done without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum MarketplaceCommand {
    /// Generate a Claude Code marketplace tree from compiled-in PluginMeta.
    Generate(generator::GenerateArgs),
}

/// Run the `labby marketplace` subcommand.
///
/// # Errors
/// Returns an error if dispatch fails.
pub async fn run(args: MarketplaceArgs, format: OutputFormat) -> Result<ExitCode> {
    if let Some(command) = args.command {
        return match command {
            MarketplaceCommand::Generate(generate_args) => generator::run_generate(generate_args),
        };
    }
    let params = args
        .params
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if args.dry_run {
        print_dry_run("marketplace", &args.action, &params);
        return Ok(ExitCode::SUCCESS);
    }
    run_confirmable_action_command(
        "marketplace",
        crate::dispatch::marketplace::actions(),
        args.action,
        params,
        args.yes,
        format,
        |action, params| async move {
            crate::dispatch::marketplace::dispatch(&action, params).await
        },
    )
    .await
}
