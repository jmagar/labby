//! `labby docs` — generated documentation artifacts.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::docs;
use crate::output::OutputFormat;
use crate::output::theme::CliTheme;

#[derive(Debug, Args)]
pub struct DocsArgs {
    #[command(subcommand)]
    pub command: DocsCommand,
}

#[derive(Debug, Subcommand)]
pub enum DocsCommand {
    /// Regenerate every tracked generated-docs artifact.
    Generate,
    /// Verify generated-docs artifacts are fresh.
    Check,
}

#[allow(clippy::print_stdout)]
pub fn run(args: DocsArgs, format: OutputFormat) -> Result<ExitCode> {
    let theme = CliTheme::from_context(format.render_context());
    match args.command {
        DocsCommand::Generate => {
            let outcome = docs::generate()?;
            println!(
                "{}",
                theme.muted(format!("generated {} docs artifacts", outcome.checked))
            );
            Ok(ExitCode::SUCCESS)
        }
        DocsCommand::Check => {
            let outcome = docs::check()?;
            if outcome.stale.is_empty() {
                println!(
                    "{} {}",
                    theme.muted(format!("checked {} docs artifacts:", outcome.checked)),
                    theme.success("fresh")
                );
                Ok(ExitCode::SUCCESS)
            } else {
                for path in &outcome.stale {
                    println!("{} {}", theme.warn("stale:"), theme.primary(path));
                }
                Ok(ExitCode::FAILURE)
            }
        }
    }
}
