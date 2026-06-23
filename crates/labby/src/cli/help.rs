//! `lab help` — print the shared service + action catalog.

use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::{
    catalog::build_catalog,
    config::load_toml,
    output::{OutputFormat, print},
    registry::{
        build_default_registry, filter_built_in_upstream_apis, filter_by_configured_env,
        lab_show_all_enabled,
    },
};

#[derive(Debug, Args)]
pub struct HelpArgs {
    /// Show every compiled-in service, even if required env vars are missing.
    #[arg(long)]
    pub all: bool,
}

/// Run the help subcommand.
pub fn run(args: HelpArgs, format: OutputFormat) -> Result<ExitCode> {
    let config = load_toml(&crate::config::toml_candidates())?;
    let registry = filter_built_in_upstream_apis(
        build_default_registry(),
        config.services.built_in_upstream_apis_enabled,
    );
    let registry = if args.all || lab_show_all_enabled() {
        registry
    } else {
        filter_by_configured_env(&registry)
    };
    let catalog = build_catalog(&registry);
    print(&catalog, format)?;
    Ok(ExitCode::SUCCESS)
}
