//! `lab help` — print CLI usage for the binary or a specific subcommand.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, CommandFactory};

use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct HelpArgs {
    /// Show help for a specific subcommand (e.g. `lab help gateway`).
    pub service: Option<String>,
}

/// Run the help subcommand.
pub fn run(args: HelpArgs, _format: OutputFormat) -> Result<ExitCode> {
    let mut cmd = crate::cli::Cli::command();
    match args.service {
        None => {
            cmd.print_long_help()?;
            println!();
        }
        Some(ref name) => match cmd.find_subcommand_mut(name) {
            Some(sub) => {
                sub.print_long_help()?;
                println!();
            }
            None => {
                eprintln!("No subcommand '{name}'. Run `lab help` to see available commands.");
                return Ok(ExitCode::FAILURE);
            }
        },
    }
    Ok(ExitCode::SUCCESS)
}
