//! `lab completions <shell>` — emit shell completion scripts.

use std::{io, process::ExitCode};

use anyhow::Result;
use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate};

use crate::cli::Cli;

/// `lab completions` arguments.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub shell: Shell,
}

/// Run the completions subcommand.
///
/// # Errors
/// Returns an error if writing to stdout fails.
#[allow(clippy::unnecessary_wraps)]
pub fn run(args: &CompletionsArgs) -> Result<ExitCode> {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    generate(args.shell, &mut cmd, bin_name, &mut io::stdout());
    Ok(ExitCode::SUCCESS)
}
