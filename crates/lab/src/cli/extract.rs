//! `labby extract` — CLI surface for the extract module.
#![allow(clippy::print_stderr)]
//!
//! Thin shim. All logic lives in `lab_apis::extract::ExtractClient`. This
//! file owns:
//!   1. The clap subcommand definition.
//!   2. The destructive-confirmation prompt for `--apply`.
//!   3. The lab-specific `.env` writer (with timestamped backup).
//!   4. The table/JSON output formatter.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use owo_colors::{OwoColorize, XtermColors};

use crate::config::{env_merge, write_service_creds};
use crate::output::{ColorPolicy, OutputFormat, RenderEnv, print};
use lab_apis::extract::{ExtractClient, ExtractReport, ScanTarget, Uri};

/// `labby extract [uri] [--apply | --diff] [-y] [--json]`
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExtractCmd {
    /// Appdata path to scan. Local (`/path` or `~/path`) or SSH (`host:/path`).
    pub uri: Option<String>,

    /// Write the extracted creds into `~/.lab/.env` (destructive — prompts).
    #[arg(long)]
    pub apply: bool,

    /// Show what would change vs the current `~/.lab/.env`, no writes.
    #[arg(long, conflicts_with = "apply")]
    pub diff: bool,

    /// Skip the destructive-action confirmation prompt.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Don't actually write — just show what would happen with `--apply`.
    #[arg(long)]
    pub dry_run: bool,

    /// Overwrite conflicting keys instead of skipping them.
    #[arg(long)]
    pub force: bool,

    /// Render as JSON instead of a table.
    #[arg(long)]
    pub json: bool,

    /// Override the env-file path (defaults to `~/.lab/.env`).
    #[arg(long, value_name = "PATH")]
    pub env_path: Option<PathBuf>,
}

impl ExtractCmd {
    /// Run the command.
    ///
    /// # Errors
    /// Propagates any error from `ExtractClient::scan`, the confirmation
    /// prompt, or the `.env` writer.
    pub async fn run(self, color_policy: ColorPolicy) -> Result<()> {
        let client = ExtractClient::new();
        let report = client
            .scan(self.scan_target()?)
            .await
            .with_context(|| "scan failed")?;

        if self.apply {
            self.apply_report(&report, color_policy)?;
        } else if self.diff {
            self.diff_report(&report)?;
        } else {
            self.print_report(&report, color_policy)?;
        }

        Ok(())
    }

    fn apply_report(&self, report: &ExtractReport, color_policy: ColorPolicy) -> Result<()> {
        let target = self.resolve_env_path()?;

        let merge_request = merge_request_from_creds(&report.creds, self.force);
        let preview = env_merge::preview(&target, &merge_request)
            .with_context(|| format!("preview {}", target.display()))?;

        // Rule 8: idempotence check — skip backup and write if nothing would change.
        if preview.written == 0 && preview.skipped.is_empty() {
            eprintln!("{}", "Already up to date — nothing to write.".dimmed());
            return Ok(());
        }

        self.print_report(report, color_policy)?;
        eprintln!(
            "\n{} {} {} fields to {}",
            "→".color(XtermColors::LightAzureRadiance),
            "would write".color(XtermColors::LightAzureRadiance).bold(),
            report.creds.len().color(XtermColors::FlushOrange),
            target.display()
        );

        if !self.yes && !self.dry_run && !confirm_destructive("extract.apply")? {
            anyhow::bail!("aborted by user");
        }
        if self.dry_run {
            eprintln!("{}", "(dry-run — no changes written)".dimmed());
            return Ok(());
        }

        // Canonical merge owns backup, atomic write, permissions, and retention.
        let outcome = write_service_creds(&target, &report.creds, self.force)
            .with_context(|| format!("write {}", target.display()))?;
        if let Some(backup) = &outcome.backup_path {
            eprintln!(
                "  {} {}",
                "backup →".dimmed(),
                backup.display().to_string().dimmed()
            );
        }

        for w in &outcome.skipped {
            eprintln!("  {} {}", "⚠".color(XtermColors::FlushOrange), w);
        }
        eprintln!(
            "  {} {}",
            "✓".color(XtermColors::BrightGreen),
            target.display()
        );
        Ok(())
    }

    fn diff_report(&self, report: &ExtractReport) -> Result<()> {
        let target = self.resolve_env_path()?;
        let preview = env_merge::preview(
            &target,
            &merge_request_from_creds(&report.creds, self.force),
        )
        .with_context(|| format!("preview {}", target.display()))?;

        if preview.changes.is_empty() {
            eprintln!("{}", "No credentials found — nothing to diff.".dimmed());
        }
        for change in preview.changes {
            match change.status {
                env_merge::PreviewStatus::Add => eprintln!(
                    "  {} {}",
                    "+".color(XtermColors::BrightGreen).bold(),
                    change.key
                ),
                env_merge::PreviewStatus::Update => eprintln!(
                    "  {} {}",
                    "~".color(XtermColors::FlushOrange).bold(),
                    change.key
                ),
                env_merge::PreviewStatus::Unchanged => {
                    eprintln!("  {} {}", "=".dimmed(), change.key)
                }
                env_merge::PreviewStatus::Conflict => eprintln!(
                    "  {} {}",
                    "!".color(XtermColors::FlushOrange).bold(),
                    change.key
                ),
            };
        }
        for warning in preview.skipped {
            eprintln!("  {} {}", "⚠".color(XtermColors::FlushOrange), warning);
        }
        Ok(())
    }

    fn print_report(&self, report: &ExtractReport, color_policy: ColorPolicy) -> Result<()> {
        let format = OutputFormat::from_json_flag(self.json, color_policy, RenderEnv::stdout());
        print(report, format)
    }

    fn scan_target(&self) -> Result<ScanTarget> {
        match self.uri.as_deref() {
            Some(uri) => {
                let uri: Uri = uri.parse().with_context(|| format!("invalid uri: {uri}"))?;
                Ok(ScanTarget::Targeted(uri))
            }
            None if self.apply || self.diff => {
                anyhow::bail!("extract.apply and extract.diff require a targeted uri")
            }
            None => Ok(ScanTarget::Fleet),
        }
    }

    fn resolve_env_path(&self) -> Result<PathBuf> {
        if let Some(p) = &self.env_path {
            return Ok(p.clone());
        }
        let home = std::env::var("HOME").context("$HOME not set")?;
        Ok(PathBuf::from(format!("{home}/.lab/.env")))
    }
}

fn merge_request_from_creds(
    creds: &[lab_apis::extract::ServiceCreds],
    force: bool,
) -> env_merge::MergeRequest {
    let mut entries = Vec::new();
    for cred in creds {
        let svc_upper = cred.service.to_uppercase();
        if let Some(url) = &cred.url {
            entries.push(env_merge::EnvEntry::new(
                format!("{svc_upper}_URL"),
                url.clone(),
            ));
        }
        if let Some(secret) = &cred.secret {
            entries.push(env_merge::EnvEntry::new(
                cred.env_field.clone(),
                secret.clone(),
            ));
        }
    }
    env_merge::MergeRequest {
        entries,
        force,
        expected_mtime: None,
    }
}

/// Interactive confirmation prompt for destructive actions.
/// Returns `true` if the user confirmed, `false` otherwise.
fn confirm_destructive(action: &str) -> Result<bool> {
    use is_terminal::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprintln!(
        "{} {} is destructive. Continue? [y/N]",
        "⚠".color(XtermColors::FlushOrange),
        action.color(XtermColors::LightAzureRadiance).bold(),
    );
    let mut buf = String::new();
    std::io::stdin()
        .read_line(&mut buf)
        .context("read confirmation")?;
    Ok(buf.trim().eq_ignore_ascii_case("y"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_apis::extract::ScanTarget;

    #[test]
    fn bare_extract_maps_to_fleet_scan() {
        let cmd = ExtractCmd {
            uri: None,
            apply: false,
            diff: false,
            yes: false,
            dry_run: false,
            force: false,
            json: false,
            env_path: None,
        };

        assert!(matches!(
            cmd.scan_target().expect("scan target"),
            ScanTarget::Fleet
        ));
    }

    #[test]
    fn targeted_extract_maps_to_targeted_scan() {
        let cmd = ExtractCmd {
            uri: Some("/tmp/appdata".to_owned()),
            apply: false,
            diff: false,
            yes: false,
            dry_run: false,
            force: false,
            json: false,
            env_path: None,
        };

        assert!(matches!(
            cmd.scan_target().expect("scan target"),
            ScanTarget::Targeted(_)
        ));
    }

    #[test]
    fn apply_and_diff_still_require_uri() {
        let apply = ExtractCmd {
            uri: None,
            apply: true,
            diff: false,
            yes: false,
            dry_run: false,
            force: false,
            json: false,
            env_path: None,
        };
        let diff = ExtractCmd {
            uri: None,
            apply: false,
            diff: true,
            yes: false,
            dry_run: false,
            force: false,
            json: false,
            env_path: None,
        };

        assert!(apply.scan_target().is_err());
        assert!(diff.scan_target().is_err());
    }
}
