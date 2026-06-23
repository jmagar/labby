use std::process::ExitCode;

use anyhow::Result;
use clap::Subcommand;

use crate::dispatch::gateway::code_mode;

#[derive(Debug, clap::Args)]
pub struct InternalArgs {
    #[command(subcommand)]
    pub command: InternalCommand,
}

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    /// Run the sandboxed Code Mode JavaScript helper process.
    #[command(hide = true)]
    CodeModeRunner,
}

pub fn run(args: InternalArgs) -> Result<ExitCode> {
    match args.command {
        InternalCommand::CodeModeRunner => Ok(code_mode::run_code_mode_runner_stdio()),
    }
}
