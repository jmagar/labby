use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{ArgGroup, Args, Subcommand};

use crate::config::LabConfig;
use crate::oauth::local_relay::{LocalRelayConfig, run_local_relay};
use crate::oauth::target::{resolve_explicit_target, resolve_machine_target};

#[derive(Debug, Args)]
pub struct OauthArgs {
    #[command(subcommand)]
    pub command: OauthCommand,
}

#[derive(Debug, Subcommand)]
pub enum OauthCommand {
    /// Run a local OAuth callback relay that forwards to a machine or explicit target.
    RelayLocal(RelayLocalArgs),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("target")
        .required(true)
        .multiple(false)
        .args(["machine", "forward_base"])
))]
pub struct RelayLocalArgs {
    #[arg(long)]
    pub machine: Option<String>,
    #[arg(long)]
    pub forward_base: Option<String>,
    #[arg(long)]
    pub port: u16,
}

pub async fn run(args: OauthArgs, config: &LabConfig) -> Result<ExitCode> {
    match args.command {
        OauthCommand::RelayLocal(args) => run_relay_local(args, config).await,
    }
}

async fn run_relay_local(args: RelayLocalArgs, config: &LabConfig) -> Result<ExitCode> {
    let resolved_target = match (&args.machine, &args.forward_base) {
        (Some(machine_id), None) => resolve_machine_target(&config.oauth.machines, machine_id)
            .with_context(|| format!("resolve oauth relay machine `{machine_id}`"))?,
        (None, Some(forward_base)) => resolve_explicit_target(forward_base, Some(args.port))
            .context("resolve explicit oauth relay target")?,
        _ => anyhow::bail!("exactly one of --machine or --forward-base is required"),
    };

    run_local_relay(LocalRelayConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.port),
        resolved_target,
        request_timeout: Duration::from_secs(10),
    })
    .await?;

    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    use crate::cli::Cli;

    #[test]
    fn oauth_relay_local_cli_parses_machine_target() {
        Cli::command().debug_assert();

        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-local",
            "--machine",
            "dookie",
            "--port",
            "38935",
        ])
        .expect("machine target should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayLocal(RelayLocalArgs {
                        machine,
                        forward_base,
                        port,
                    }),
            }) => {
                assert_eq!(machine.as_deref(), Some("dookie"));
                assert!(forward_base.is_none());
                assert_eq!(port, 38935);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_local_cli_parses_explicit_target() {
        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-local",
            "--forward-base",
            "http://100.88.16.79:38935/callback/dookie",
            "--port",
            "38935",
        ])
        .expect("explicit target should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayLocal(RelayLocalArgs {
                        machine,
                        forward_base,
                        port,
                    }),
            }) => {
                assert!(machine.is_none());
                assert_eq!(
                    forward_base.as_deref(),
                    Some("http://100.88.16.79:38935/callback/dookie")
                );
                assert_eq!(port, 38935);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_local_cli_rejects_both_target_flags() {
        let result = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-local",
            "--machine",
            "dookie",
            "--forward-base",
            "http://100.88.16.79:38935/callback/dookie",
            "--port",
            "38935",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn oauth_relay_local_cli_resolves_explicit_target() {
        let resolved =
            resolve_explicit_target("http://100.88.16.79:38935/callback/dookie", Some(38935))
                .expect("explicit target should resolve");

        assert_eq!(resolved.machine_id, None);
        assert_eq!(
            resolved.target_url.as_str(),
            "http://100.88.16.79:38935/callback/dookie"
        );
        assert_eq!(resolved.default_port, Some(38935));
    }
}
