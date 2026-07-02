//! `labby update` — install the latest release, then refresh Incus when present.

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::Result;
use clap::Args;

use crate::output::{OutputFormat, print};

const INSTALL_SCRIPT: &str = include_str!("../../../../scripts/install.sh");

#[derive(Debug, Args, Clone)]
pub struct UpdateArgs {
    /// Release tag to install. Defaults to the latest GitHub release with a Labby binary asset.
    #[arg(long, default_value = "latest")]
    pub version: String,
    /// Install directory for the host binary.
    #[arg(long)]
    pub install_dir: Option<PathBuf>,
    /// Do not sync the updated binary into an Incus container.
    #[arg(long)]
    pub no_incus_sync: bool,
    /// Incus container name for the post-update sync.
    #[arg(long)]
    pub container: Option<String>,
    /// Optional public or host-bound URL to verify after the Incus sync.
    #[arg(long)]
    pub check_url: Option<String>,
    /// Fall back to `incus stop --force && incus start` if the service restart path fails.
    #[arg(long)]
    pub force_fallback: bool,
    /// Disable the default Incus force-restart fallback after installing the release.
    #[arg(long, conflicts_with = "force_fallback")]
    pub no_force_fallback: bool,
    /// Print what would happen without installing or syncing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, serde::Serialize)]
struct UpdateOutcome {
    version: String,
    install_dir: PathBuf,
    binary: PathBuf,
    dry_run: bool,
    installed: bool,
    incus_sync: Option<crate::dispatch::setup::incus::IncusSyncOutcome>,
    incus_sync_skipped: Option<String>,
}

pub async fn run(args: UpdateArgs, format: OutputFormat) -> Result<ExitCode> {
    let install_dir = resolve_install_dir(args.install_dir.as_ref())?;
    let binary = install_dir.join("labby");

    if args.dry_run {
        let outcome = UpdateOutcome {
            version: args.version,
            install_dir,
            binary,
            dry_run: true,
            installed: false,
            incus_sync: None,
            incus_sync_skipped: if args.no_incus_sync {
                Some("--no-incus-sync requested".to_string())
            } else {
                None
            },
        };
        render_outcome(outcome, format)?;
        return Ok(ExitCode::SUCCESS);
    }

    run_install_script(&args.version, &install_dir)?;
    let mut incus_sync = None;
    let mut incus_sync_skipped = None;
    if args.no_incus_sync {
        incus_sync_skipped = Some("--no-incus-sync requested".to_string());
    } else {
        match crate::dispatch::setup::incus::sync_incus_binary(
            crate::dispatch::setup::incus::IncusSyncOptions {
                container: args.container,
                binary: Some(binary.clone()),
                check_url: args.check_url,
                force_fallback: args.force_fallback || !args.no_force_fallback,
                dry_run: false,
            },
        ) {
            Ok(outcome) => incus_sync = Some(outcome),
            Err(err) if err.kind() == "incus_sync_container_discovery_failed" => {
                incus_sync_skipped = Some(err.to_string());
            }
            Err(err) => return Err(anyhow::anyhow!(err.to_string())),
        }
    }

    let outcome = UpdateOutcome {
        version: args.version,
        install_dir,
        binary,
        dry_run: false,
        installed: true,
        incus_sync,
        incus_sync_skipped,
    };
    render_outcome(outcome, format)?;
    Ok(ExitCode::SUCCESS)
}

fn run_install_script(version: &str, install_dir: &PathBuf) -> Result<()> {
    let tempdir = tempfile::tempdir()?;
    let script = tempdir.path().join("install.sh");
    std::fs::write(&script, INSTALL_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    }
    let status = Command::new("sh")
        .arg(&script)
        .env("LAB_INSTALL_VERSION", version)
        .env("LAB_INSTALL_DIR", install_dir)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("install.sh failed with status {status}")
    }
}

fn resolve_install_dir(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }
    if let Some(path) = std::env::var_os("LAB_INSTALL_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".local").join("bin"))
}

fn render_outcome(outcome: UpdateOutcome, format: OutputFormat) -> Result<()> {
    if format.is_json() {
        print(&serde_json::to_value(outcome)?, format)?;
        return Ok(());
    }
    if outcome.dry_run {
        println!(
            "dry-run: would install labby {} to {}",
            outcome.version,
            outcome.binary.display()
        );
    } else {
        println!("updated labby: {}", outcome.binary.display());
    }
    if let Some(sync) = &outcome.incus_sync {
        println!("synced Incus container: {}", sync.container);
        if let Some(version) = &sync.remote_version {
            println!("container version: {version}");
        }
    }
    if let Some(reason) = &outcome.incus_sync_skipped {
        println!("Incus sync skipped: {reason}");
    }
    Ok(())
}
