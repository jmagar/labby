//! `lab stash` — CLI shim for the `stash` service.
//!
//! Thin shim: parse action + key/value params, call the shared dispatcher,
//! and format the result. Mirrors the MCP action surface directly.

use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::cli::helpers::run_confirmable_action_command;
use crate::cli::params::parse_kv_params;
use crate::dispatch::stash::catalog::ACTIONS;
use crate::output::OutputFormat;

/// `lab stash` arguments.
#[derive(Debug, Args)]
pub struct StashArgs {
    /// Action to run, e.g. `help`, `components.list`, `component.get`.
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(ACTIONS.iter().map(|a| a.name)))]
    pub action: String,

    /// Optional `key=value` params for the action.
    #[arg(value_name = "KEY=VALUE", trailing_var_arg = true)]
    pub params: Vec<String>,

    /// Skip confirmation for destructive actions.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,

    /// Print what would be done without executing.
    #[arg(long)]
    pub dry_run: bool,
}

/// Run the `lab stash` subcommand.
///
/// # Errors
/// Returns an error if the stash root cannot be resolved or the action fails.
#[allow(clippy::print_stdout)]
pub async fn run(args: StashArgs, format: OutputFormat) -> Result<ExitCode> {
    let params = parse_kv_params(args.params)?;
    if args.dry_run {
        crate::cli::helpers::print_dry_run("stash", &args.action, &params, format);
        return Ok(ExitCode::SUCCESS);
    }
    run_confirmable_action_command(
        "stash",
        ACTIONS,
        args.action,
        params,
        args.yes,
        format,
        |action, params| async move {
            crate::dispatch::stash::dispatch::dispatch(&action, params).await
        },
    )
    .await
}
