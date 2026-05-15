//! `labby registry` — CLI shim for MCP Registry operations.
//!
//! Provides `install` to look up a server from the MCP registry and add it
//! to the local gateway in one step.  The dispatch path is:
//!
//! 1. `marketplace::dispatch("mcp.install", {name, version, gateway_ids, bearer_token_env})`
//!    which internally:
//!    a. Fetches the server record from the registry
//!    b. Extracts `server.remotes[0].url`
//!    c. Validates the URL against SSRF rules
//!    d. Calls `gateway.add` to register it
//!
//! No new dispatch actions are needed — this is a thin CLI shim over `mcp.install`.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::cli::helpers::run_confirmable_action_command;
use crate::output::OutputFormat;

/// `labby registry` arguments.
#[derive(Debug, Args)]
pub struct RegistryArgs {
    #[command(subcommand)]
    pub command: RegistryCommand,
}

/// Subcommands for `labby registry`.
#[derive(Debug, Subcommand)]
pub enum RegistryCommand {
    /// Fetch a server from the MCP registry and add it to the local gateway.
    Install(RegistryInstallArgs),
}

/// Arguments for `labby registry install`.
#[derive(Debug, Args)]
pub struct RegistryInstallArgs {
    /// Qualified registry server name (e.g. `io.modelcontextprotocol/everything`).
    pub name: String,
    /// Pinned version to install (default: latest).
    #[arg(long)]
    pub version: Option<String>,
    /// Environment variable name that holds the bearer token for this gateway entry.
    ///
    /// Must be a valid env var name (e.g. `MY_SERVICE_TOKEN`), not the raw token
    /// value.  If omitted, no bearer auth is configured on the new gateway entry.
    #[arg(long, value_name = "ENV_VAR")]
    pub bearer_env: Option<String>,
    /// Override the gateway entry name.  Defaults to the registry server name.
    #[arg(long)]
    pub gateway_name: Option<String>,
    /// Skip the destructive-action confirmation prompt (required for non-interactive use).
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Run `labby registry`.
pub async fn run(args: RegistryArgs, format: OutputFormat) -> Result<ExitCode> {
    match args.command {
        RegistryCommand::Install(install_args) => run_install(install_args, format).await,
    }
}

async fn run_install(args: RegistryInstallArgs, format: OutputFormat) -> Result<ExitCode> {
    // Derive the gateway entry name from the registry server name when not overridden.
    // The last path segment of a qualified name like `io.example/my-server` is used
    // so the gateway name is short and human-readable.
    let gateway_name = args
        .gateway_name
        .clone()
        .unwrap_or_else(|| derive_gateway_name(&args.name));

    let params = json!({
        "name": args.name,
        "version": args.version.as_deref().unwrap_or("latest"),
        "gateway_ids": [gateway_name],
        "bearer_token_env": args.bearer_env,
    });

    run_confirmable_action_command(
        "marketplace",
        crate::dispatch::marketplace::actions(),
        "mcp.install".to_string(),
        params,
        args.yes,
        format,
        |action, params| async move {
            crate::dispatch::marketplace::dispatch(&action, params).await
        },
    )
    .await
}

/// Derive a short gateway name from a qualified registry server name.
///
/// `io.example/my-server` → `my-server`
/// `my-server` (no slash) → `my-server`
fn derive_gateway_name(registry_name: &str) -> String {
    registry_name
        .rsplit('/')
        .next()
        .unwrap_or(registry_name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::Parser;

    use crate::cli::Cli;

    use super::*;

    #[test]
    fn registry_cli_parser_accepts_install_command() {
        Cli::command().debug_assert();

        assert!(
            Cli::try_parse_from(["lab", "registry", "install", "io.example/my-server"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "registry",
                "install",
                "io.example/my-server",
                "--version",
                "1.2.3",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "registry",
                "install",
                "io.example/my-server",
                "--bearer-env",
                "MY_SERVICE_TOKEN",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "registry",
                "install",
                "io.example/my-server",
                "--gateway-name",
                "my-service",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "registry", "install", "io.example/my-server", "-y"])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "registry",
                "install",
                "io.example/my-server",
                "--yes"
            ])
            .is_ok()
        );
    }

    #[test]
    fn registry_install_parser_rejects_missing_name() {
        assert!(Cli::try_parse_from(["lab", "registry", "install"]).is_err());
    }

    #[test]
    fn derive_gateway_name_extracts_last_segment() {
        assert_eq!(derive_gateway_name("io.example/my-server"), "my-server");
        assert_eq!(derive_gateway_name("my-server"), "my-server");
        assert_eq!(
            derive_gateway_name("io.modelcontextprotocol/everything"),
            "everything"
        );
    }
}
