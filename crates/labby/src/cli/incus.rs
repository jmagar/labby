//! `labby incus` — low-friction host-side Incus operations.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::output::{OutputFormat, print};

#[derive(Debug, Args)]
pub struct IncusArgs {
    #[command(subcommand)]
    pub command: IncusCommand,
}

#[derive(Debug, Subcommand)]
pub enum IncusCommand {
    /// Bootstrap or converge the supported Incus Labby gateway container.
    Setup(Box<IncusSetupArgs>),
    /// Sync a local labby binary into the Labby Incus container.
    Sync(IncusSyncArgs),
}

#[derive(Debug, Args)]
pub struct IncusSetupArgs {
    /// Container name (default: labby).
    #[arg(long)]
    pub name: Option<String>,
    /// Incus image alias (default: images:ubuntu/24.04).
    #[arg(long)]
    pub image: Option<String>,
    /// Incus profile name (default: labby-gateway).
    #[arg(long)]
    pub profile_name: Option<String>,
    /// Incus snapshot policy YAML path; defaults to the embedded policy.
    #[arg(long)]
    pub backup_config: Option<PathBuf>,
    /// Do not apply an Incus snapshot policy.
    #[arg(long)]
    pub no_backup_config: bool,
    /// Rootless profile for existing containers with a different root pool.
    #[arg(long)]
    pub runtime_profile_name: Option<String>,
    /// Incus storage driver: zfs, btrfs, or dir.
    #[arg(long)]
    pub storage_driver: Option<String>,
    /// Incus storage pool used by the profile root disk.
    #[arg(long)]
    pub storage_pool: Option<String>,
    /// Incus storage source path/dataset for the pool.
    #[arg(long)]
    pub storage_source: Option<String>,
    /// Labby release tag to install. Defaults to latest.
    #[arg(long, default_value = "latest")]
    pub version: Option<String>,
    /// Push a locally built labby binary instead of downloading a release.
    #[arg(long)]
    pub local_binary: Option<PathBuf>,
    /// Use the labby binary already baked into the selected image.
    #[arg(long)]
    pub skip_install: bool,
    /// Print bootstrap commands only.
    #[arg(long)]
    pub dry_run: bool,
    /// Run tailscale up with --ssh when TS_AUTHKEY is set.
    #[arg(long)]
    pub tailscale_ssh: bool,
    /// Hostname to register with Tailscale; defaults to the Incus container name.
    #[arg(long)]
    pub tailscale_hostname: Option<String>,
    /// Allow install.sh cargo fallback if the release asset is unavailable.
    #[arg(long)]
    pub allow_source_fallback: bool,
    /// Confirm bootstrap without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
}

impl Default for IncusSetupArgs {
    fn default() -> Self {
        Self {
            name: None,
            image: None,
            profile_name: None,
            backup_config: None,
            no_backup_config: false,
            runtime_profile_name: None,
            storage_driver: None,
            storage_pool: None,
            storage_source: None,
            version: Some("latest".to_string()),
            local_binary: None,
            skip_install: false,
            dry_run: false,
            tailscale_ssh: false,
            tailscale_hostname: None,
            allow_source_fallback: false,
            yes: false,
        }
    }
}

#[derive(Debug, Args, Clone)]
pub struct IncusSyncArgs {
    /// Incus container name. Defaults to LABBY_INCUS_CONTAINER, then labby, then a single running labby-* container.
    #[arg(long)]
    pub container: Option<String>,
    /// Local labby binary to install. Defaults to LABBY_INCUS_BINARY, target/debug/labby, then the current executable.
    #[arg(long)]
    pub binary: Option<PathBuf>,
    /// Optional public or host-bound URL to verify after the service is ready.
    #[arg(long)]
    pub check_url: Option<String>,
    /// Fall back to `incus stop --force && incus start` if the service restart path fails.
    #[arg(long)]
    pub force_fallback: bool,
    /// Disable the default Incus force-restart fallback.
    #[arg(long, conflicts_with = "force_fallback")]
    pub no_force_fallback: bool,
    /// Print the resolved operation without mutating the container.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(args: IncusArgs, format: OutputFormat) -> Result<ExitCode> {
    match args.command {
        IncusCommand::Setup(args) => run_setup(*args, format).await,
        IncusCommand::Sync(args) => run_sync(args, format).await,
    }
}

pub(crate) async fn run_setup(args: IncusSetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if format.is_json() {
        anyhow::bail!(
            "incus setup does not support --json yet because it streams an imperative Incus bootstrap script"
        );
    }
    confirm_incus_setup(args.dry_run, args.yes)?;
    let options = crate::dispatch::setup::incus::IncusBootstrapOptions {
        name: args.name,
        image: args.image,
        profile_name: args.profile_name,
        backup_config: args.backup_config,
        no_backup_config: args.no_backup_config,
        runtime_profile_name: args.runtime_profile_name,
        storage_driver: args.storage_driver,
        storage_pool: args.storage_pool,
        storage_source: args.storage_source,
        version: args.version,
        local_binary: args.local_binary,
        skip_install: args.skip_install,
        dry_run: args.dry_run,
        tailscale_ssh: args.tailscale_ssh,
        tailscale_hostname: args.tailscale_hostname,
        allow_source_fallback: args.allow_source_fallback,
    };
    crate::dispatch::setup::incus::run_incus_bootstrap(options)?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) async fn run_sync(args: IncusSyncArgs, format: OutputFormat) -> Result<ExitCode> {
    let outcome = crate::dispatch::setup::incus::sync_incus_binary(
        crate::dispatch::setup::incus::IncusSyncOptions {
            container: args.container,
            binary: args.binary,
            check_url: args.check_url,
            force_fallback: args.force_fallback || !args.no_force_fallback,
            dry_run: args.dry_run,
        },
    )?;
    if format.is_json() {
        print(&serde_json::to_value(outcome)?, format)?;
    } else if outcome.dry_run {
        println!(
            "dry-run: would sync {} -> {}:/usr/local/bin/labby",
            outcome.binary.display(),
            outcome.container
        );
        for step in &outcome.steps {
            println!("  - {step}");
        }
    } else {
        println!(
            "synced {} -> {}:/usr/local/bin/labby",
            outcome.binary.display(),
            outcome.container
        );
        if let Some(pid) = outcome.new_pid {
            println!("labby.service MainPID: {pid}");
        }
        if let Some(version) = outcome.remote_version {
            println!("version: {version}");
        }
        if let Some(hash) = outcome.remote_sha256 {
            println!("sha256: {hash}");
        }
        if outcome.fallback_restart_used {
            println!("fallback: incus force restart used");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn confirm_incus_setup(dry_run: bool, mut yes: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    if !yes {
        if !io::stdin().is_terminal() {
            anyhow::bail!("incus setup requires --yes when stdin is not a TTY");
        }
        eprintln!(
            "This will create or update the Labby Incus container, storage/profile config, in-container labby binary, service state, backup policy, and Tailscale join when TS_AUTHKEY is set."
        );
        eprint!("Proceed? [y/N] ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        yes = matches!(answer.trim(), "y" | "Y" | "yes" | "YES");
    }
    if !yes {
        anyhow::bail!("incus setup cancelled");
    }
    Ok(())
}
