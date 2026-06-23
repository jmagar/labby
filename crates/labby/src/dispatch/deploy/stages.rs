//! Deploy pipeline stage functions.
//!
//! Each stage takes an [`Arc<I>`] where `I: HostIo` plus owned arguments —
//! this avoids HRTB `Send` errors (Rust issue #100013) by ensuring no
//! borrowed references cross `await` points.
//!
//! ## `host='?'` sentinel
//!
//! Stage functions do not know the caller's host alias. All [`DeployError`]
//! variants that carry a `host` field are populated with the literal `"?"`
//! sentinel. The caller is responsible for routing errors through
//! `runner::host_err` to replace the sentinel with the real alias before
//! emitting results or logs.

use std::path::Path;
use std::sync::Arc;

use super::host_io::HostIo;
use super::params;
use super::ssh_session::shell_quote;
use labby_apis::deploy::DeployError;

use crate::config::ServiceScope;

// ── Outcome types ──────────────────────────────────────────────────────────

/// Outcome of the preflight stage.
#[derive(Debug, Clone, Copy)]
pub struct PreflightOutcome {
    /// True when the remote artifact already matches `local_sha256`; the
    /// transfer stage is skipped entirely.
    pub skip_transfer: bool,
}

/// Outcome of the transfer + install stage.
#[derive(Debug, Clone)]
pub struct TransferOutcome {
    pub bytes: u64,
    /// Path of the `.bak.<ts>` file the previous binary (if any) was moved to.
    #[allow(dead_code)]
    pub backup_path: Option<String>,
}

/// Outcome of the restart stage.
#[derive(Debug, Clone, Copy)]
pub struct RestartOutcome {
    /// True when `unit` was `None` — no systemd action was taken.
    #[allow(dead_code)]
    pub skipped: bool,
}

// ── Stage functions ────────────────────────────────────────────────────────

/// Preflight: architecture match + canary write + sha256 skip probe.
///
/// The canary-write check uses `sh -c` deliberately. `remote_path` is
/// allowlist-validated via `params::validate_remote_path` at the top of this
/// function and the canary filename is a fixed pattern — the path is also
/// shell-single-quote-escaped before interpolation.
///
/// Takes `Arc<I>` and owned strings so the resulting future holds only
/// `Send` values across await points (no HRTB via Rust issue #100013).
pub async fn preflight<I: HostIo + 'static>(
    io: Arc<I>,
    remote_path: String,
    local_triple: String,
    local_sha256: String,
) -> Result<PreflightOutcome, DeployError> {
    // 0) Validate remote_path against the allowlist before any shell use.
    params::validate_remote_path(&remote_path)?;

    // 1) Architecture match.
    let (code, stdout, stderr) = io.run_argv(&["uname", "-m"]).await?;
    if code != 0 {
        return Err(DeployError::PreflightFailed {
            host: "?".into(),
            reason: format!("uname -m exit {code}: {}", stderr.trim()),
        });
    }
    let remote_arch = stdout.trim().to_string();
    let local_arch = triple_to_arch(&local_triple);
    if !arch_matches(&local_arch, &remote_arch) {
        return Err(DeployError::ArchMismatch {
            host: "?".into(),
            local: local_arch,
            remote: remote_arch,
        });
    }

    // 2) Writable install dir (canary touch + rm).
    let parent = Path::new(&remote_path)
        .parent()
        .ok_or_else(|| DeployError::PreflightFailed {
            host: "?".into(),
            reason: format!("remote_path `{remote_path}` has no parent directory"),
        })?
        .to_string_lossy()
        .into_owned();
    // Canary lives under a fixed filename.  The parent dir is derived from
    // the allowlist-validated remote_path but we still single-quote-escape
    // the interpolated paths for defense-in-depth.
    let canary = format!("{parent}/.lab.deploy.canary.$$");
    let sq_canary = shell_quote(&canary);
    let probe = format!("touch {sq_canary} && rm -f -- {sq_canary}");
    let (code, _stdout, stderr) = io.run_argv(&["sh", "-c", &probe]).await?;
    if code != 0 {
        return Err(DeployError::PreflightFailed {
            host: "?".into(),
            reason: format!("install dir `{parent}` not writable: {}", stderr.trim()),
        });
    }

    // 3) Remote sha256 probe — when it matches, transfer is skipped.
    let remote_sha = io.sha256_remote(&remote_path).await?;
    Ok(PreflightOutcome {
        skip_transfer: remote_sha.as_deref() == Some(local_sha256.as_str()),
    })
}

/// Transfer + atomic install. The sequence is:
///
/// 1. `upload_stream` the new artifact to `<remote_path>.new.partial`
/// 2. `mv -- <.partial> <.new>`
/// 3. `sha256sum <.new>` must equal `local_sha256` — else remove and abort
/// 4. if `<remote_path>` exists, `mv -- <current> <remote_path>.bak.<ts>`
/// 5. `mv -- <.new> <remote_path>` (atomic on same filesystem)
///
/// Takes `Arc<I>` and owned strings so the resulting future holds only
/// `Send` values across await points (no HRTB via Rust issue #100013).
pub async fn transfer_and_install<I: HostIo + 'static, R>(
    io: Arc<I>,
    remote_path: String,
    local_sha256: String,
    reader: R,
) -> Result<TransferOutcome, DeployError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let partial = format!("{remote_path}.new.partial");
    let staged = format!("{remote_path}.new");

    // 1) stream to .partial
    let bytes = io.upload_stream(&partial, reader).await?;

    // 2) rename .partial -> .new
    let (code, _stdout, stderr) = io.run_argv(&["mv", "--", &partial, &staged]).await?;
    if code != 0 {
        return Err(DeployError::TransferFailed {
            host: "?".into(),
            reason: format!("rename partial -> staged: {}", stderr.trim()),
        });
    }

    // 3) integrity verify
    let remote_sha =
        io.sha256_remote(&staged)
            .await?
            .ok_or_else(|| DeployError::TransferFailed {
                host: "?".into(),
                reason: "post-upload sha256 probe returned no hash".into(),
            })?;
    if remote_sha != local_sha256 {
        // best-effort cleanup; drop the result explicitly (lint: let-underscore-drop)
        drop(io.run_argv(&["rm", "-f", "--", &staged]).await);
        return Err(DeployError::IntegrityMismatch { host: "?".into() });
    }

    // 4) backup existing (only if present)
    let backup = format!("{remote_path}.bak.{}", backup_timestamp());
    let maybe_existing = io.sha256_remote(&remote_path).await?;
    let backup_path = if maybe_existing.is_some() {
        let (code, _stdout, stderr) = io.run_argv(&["mv", "--", &remote_path, &backup]).await?;
        if code != 0 {
            return Err(DeployError::InstallFailed {
                host: "?".into(),
                reason: format!("backup rename: {}", stderr.trim()),
            });
        }
        Some(backup)
    } else {
        None
    };

    // 5) atomic swap .new -> remote_path
    let (code, _stdout, stderr) = io.run_argv(&["mv", "--", &staged, &remote_path]).await?;
    if code != 0 {
        return Err(DeployError::InstallFailed {
            host: "?".into(),
            reason: format!("final rename: {}", stderr.trim()),
        });
    }

    // 6) ensure the binary is executable (upload preserves no mode bits)
    let (code, _stdout, stderr) = io.run_argv(&["chmod", "755", "--", &remote_path]).await?;
    if code != 0 {
        return Err(DeployError::InstallFailed {
            host: "?".into(),
            reason: format!("chmod 755: {}", stderr.trim()),
        });
    }

    Ok(TransferOutcome { bytes, backup_path })
}

/// Restart a systemd unit and wait for it to be active.
///
/// When `unit` is `None`, returns `skipped = true` without running any
/// remote command. Unit names are validated against
/// `params::validate_service_name` before any `systemctl` call.
///
/// Takes `Arc<I>` and an owned `unit` string so the resulting future holds
/// only `Send` values across await points (no HRTB via Rust issue #100013).
pub async fn restart<I: HostIo + 'static>(
    io: Arc<I>,
    unit: Option<String>,
    scope: Option<ServiceScope>,
) -> Result<RestartOutcome, DeployError> {
    let Some(unit) = unit else {
        return Ok(RestartOutcome { skipped: true });
    };
    params::validate_service_name(&unit)?;

    let user_scope = matches!(scope, Some(ServiceScope::User));

    fn systemctl_argv(user_scope: bool, subcommand: &str, unit: &str) -> Vec<String> {
        let mut v: Vec<String> = vec!["systemctl".into()];
        if user_scope {
            v.push("--user".into());
        }
        v.push(subcommand.to_string());
        v.push(unit.to_string());
        v
    }

    let restart_args = systemctl_argv(user_scope, "restart", &unit);
    let restart_argv: Vec<&str> = restart_args.iter().map(String::as_str).collect();
    let (code, _stdout, stderr) = io.run_argv(&restart_argv).await?;
    if code != 0 {
        return Err(DeployError::RestartFailed {
            host: "?".into(),
            reason: format!("restart exit {code}: {}", stderr.trim()),
        });
    }

    let mut wait_args = systemctl_argv(user_scope, "is-active", &unit);
    // Insert --wait before the unit name (last element) per systemctl convention.
    let unit_pos = wait_args.len() - 1;
    wait_args.insert(unit_pos, "--wait".into());
    let wait_argv: Vec<&str> = wait_args.iter().map(String::as_str).collect();
    let (code, _stdout, stderr) = io.run_argv(&wait_argv).await?;
    if code != 0 {
        return Err(DeployError::RestartFailed {
            host: "?".into(),
            reason: format!("is-active --wait exit {code}: {}", stderr.trim()),
        });
    }
    Ok(RestartOutcome { skipped: false })
}

/// Verify the newly installed binary by running `<remote_path> --version`.
///
/// Takes `Arc<I>` and an owned `remote_path` string so the resulting future
/// holds only `Send` values across await points (no HRTB via Rust issue #100013).
pub async fn verify<I: HostIo + 'static>(
    io: Arc<I>,
    remote_path: String,
) -> Result<(), DeployError> {
    let (code, _stdout, stderr) = io.run_argv(&[remote_path.as_str(), "--version"]).await?;
    if code != 0 {
        return Err(DeployError::VerifyFailed {
            host: "?".into(),
            reason: format!("--version exit {code}: {}", stderr.trim()),
        });
    }
    Ok(())
}

// ── Internal helpers ────────────────────────────────────────────────────────

pub(super) fn triple_to_arch(triple: &str) -> String {
    triple.split('-').next().unwrap_or(triple).to_string()
}

pub(super) fn normalize_arch(arch: &str) -> &str {
    match arch {
        "amd64" | "x64" => "x86_64",
        "arm64" => "aarch64",
        // uname -m on 32-bit ARM Linux returns `armv7l` (little-endian suffix).
        // Rust triples use `armv7` without the `l`. armhf is a Debian alias.
        "armv7l" | "armhf" => "armv7",
        other => other,
    }
}

pub(super) fn arch_matches(local: &str, remote: &str) -> bool {
    normalize_arch(local) == normalize_arch(remote)
}

pub(super) fn backup_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
        .to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_preflight {
    use super::*;
    use crate::dispatch::deploy::runner::test_support::*;

    #[tokio::test]
    async fn rejects_arch_mismatch() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("aarch64\n"));
        let err = preflight(
            io,
            "/usr/local/bin/labby".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "abc123".to_string(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "arch_mismatch");
    }

    #[tokio::test]
    async fn reports_skip_when_remote_sha_matches() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("x86_64\n"));
        io.push_run(RunResp::ok("")); // canary
        io.push_sha(Some("abc123".to_string()));
        let res = preflight(
            io,
            "/usr/local/bin/labby".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "abc123".to_string(),
        )
        .await
        .unwrap();
        assert!(res.skip_transfer);
    }

    #[tokio::test]
    async fn does_not_skip_when_remote_sha_differs() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("x86_64\n"));
        io.push_run(RunResp::ok(""));
        io.push_sha(Some("deadbeef".to_string()));
        let res = preflight(
            io,
            "/usr/local/bin/labby".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "abc123".to_string(),
        )
        .await
        .unwrap();
        assert!(!res.skip_transfer);
    }

    #[tokio::test]
    async fn rejects_non_writable_install_dir() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("x86_64\n"));
        io.push_run(RunResp::fail(1, "permission denied"));
        let err = preflight(
            io,
            "/usr/local/bin/labby".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "abc123".to_string(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "preflight_failed");
    }

    #[tokio::test]
    async fn canary_write_goes_to_parent_not_binary_path() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("x86_64\n"));
        io.push_run(RunResp::ok(""));
        io.push_sha(None);
        let _ = preflight(
            io.clone(),
            "/usr/local/bin/labby".to_string(),
            "x86_64-unknown-linux-gnu".to_string(),
            "abc123".to_string(),
        )
        .await
        .unwrap();
        let ops = io.ops();
        // Second op is the canary sh -c; assert the path targets the parent dir.
        let probe = ops
            .iter()
            .find(|o| o.starts_with("run:sh,-c,"))
            .expect("canary run recorded");
        assert!(probe.contains("/usr/local/bin/.lab.deploy.canary"));
        assert!(!probe.contains("'/usr/local/bin/labby'"));
    }
}

#[cfg(test)]
mod tests_transfer_install {
    use super::*;
    use crate::dispatch::deploy::runner::test_support::*;

    #[tokio::test]
    async fn transfer_streams_to_partial_then_renames_and_swaps() {
        let io = Arc::new(RecordingIo::new());
        // mv partial -> staged
        io.push_run(RunResp::ok(""));
        // sha256 of staged (matches)
        io.push_sha(Some("abc123".to_string()));
        // sha256 probe for existing binary (Some => exists)
        io.push_sha(Some("oldsha".to_string()));
        // mv existing -> .bak.<ts>
        io.push_run(RunResp::ok(""));
        // mv staged -> remote_path
        io.push_run(RunResp::ok(""));
        // chmod 755 remote_path
        io.push_run(RunResp::ok(""));

        let outcome = transfer_and_install(
            io.clone(),
            "/usr/local/bin/labby".to_string(),
            "abc123".to_string(),
            tokio::io::empty(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.bytes, 0);
        assert!(outcome.backup_path.is_some());

        let ops = io.ops();
        assert!(
            ops.iter()
                .any(|o| o == "upload:/usr/local/bin/labby.new.partial"),
            "ops: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|o| o == "run:mv,--,/usr/local/bin/labby.new.partial,/usr/local/bin/labby.new"),
            "ops: {ops:?}"
        );
        assert!(
            ops.iter().any(|o| o == "sha256:/usr/local/bin/labby.new"),
            "ops: {ops:?}"
        );
        // backup rename targets the existing binary
        assert!(
            ops.iter()
                .any(|o| o.starts_with("run:mv,--,/usr/local/bin/labby,/usr/local/bin/labby.bak.")),
            "ops: {ops:?}"
        );
        // atomic swap
        assert!(
            ops.iter()
                .any(|o| o == "run:mv,--,/usr/local/bin/labby.new,/usr/local/bin/labby"),
            "ops: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|o| o == "run:chmod,755,--,/usr/local/bin/labby"),
            "ops: {ops:?}"
        );
    }

    #[tokio::test]
    async fn integrity_mismatch_aborts_before_swap() {
        let io = Arc::new(RecordingIo::new());
        // mv partial -> staged ok
        io.push_run(RunResp::ok(""));
        // sha256 of staged differs from local
        io.push_sha(Some("deadbeef".to_string()));
        // cleanup rm -f (best effort)
        io.push_run(RunResp::ok(""));

        let err = transfer_and_install(
            io.clone(),
            "/usr/local/bin/labby".to_string(),
            "abc123".to_string(),
            tokio::io::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "integrity_mismatch");

        let ops = io.ops();
        // must NOT have performed the final swap or the backup rename
        assert!(
            !ops.iter()
                .any(|o| o == "run:mv,--,/usr/local/bin/labby.new,/usr/local/bin/labby"),
            "ops: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|o| o.starts_with("run:mv,--,/usr/local/bin/labby,/usr/local/bin/labby.bak.")),
            "ops: {ops:?}"
        );
    }

    #[tokio::test]
    async fn no_backup_when_target_absent() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("")); // mv partial -> staged
        io.push_sha(Some("abc123".into())); // staged sha matches
        io.push_sha(None); // existing binary absent
        io.push_run(RunResp::ok("")); // final swap
        io.push_run(RunResp::ok("")); // chmod 755

        let outcome = transfer_and_install(
            io.clone(),
            "/usr/local/bin/labby".to_string(),
            "abc123".to_string(),
            tokio::io::empty(),
        )
        .await
        .unwrap();
        assert!(outcome.backup_path.is_none());
        assert_eq!(outcome.bytes, 0);

        let ops = io.ops();
        assert!(
            ops.iter()
                .any(|o| o == "run:mv,--,/usr/local/bin/labby.new,/usr/local/bin/labby"),
            "ops: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|o| o == "run:chmod,755,--,/usr/local/bin/labby"),
            "ops: {ops:?}"
        );
    }
}

#[cfg(test)]
mod tests_restart_verify {
    use super::*;
    use crate::config::ServiceScope;
    use crate::dispatch::deploy::runner::test_support::*;

    #[tokio::test]
    async fn skips_restart_when_unit_is_none() {
        let io = Arc::new(RecordingIo::new());
        let r = restart(io.clone(), None, None).await.unwrap();
        assert!(r.skipped);
        assert!(io.ops().is_empty());
    }

    #[tokio::test]
    async fn restart_system_scope_uses_systemctl() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("")); // restart
        io.push_run(RunResp::ok("active\n")); // is-active --wait

        restart(
            io.clone(),
            Some("labby".to_string()),
            Some(ServiceScope::System),
        )
        .await
        .unwrap();
        let ops = io.ops();
        assert!(
            ops.iter().any(|o| o == "run:systemctl,restart,labby"),
            "ops: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|o| o == "run:systemctl,is-active,--wait,labby"),
            "ops: {ops:?}"
        );
    }

    #[tokio::test]
    async fn restart_user_scope_uses_systemctl_user() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok(""));
        io.push_run(RunResp::ok(""));

        restart(
            io.clone(),
            Some("lab-worker".to_string()),
            Some(ServiceScope::User),
        )
        .await
        .unwrap();
        let ops = io.ops();
        assert!(
            ops.iter()
                .any(|o| o == "run:systemctl,--user,restart,lab-worker"),
            "ops: {ops:?}"
        );
        assert!(
            ops.iter()
                .any(|o| o == "run:systemctl,--user,is-active,--wait,lab-worker"),
            "ops: {ops:?}"
        );
    }

    #[tokio::test]
    async fn restart_rejects_invalid_unit_names() {
        let io = Arc::new(RecordingIo::new());
        let err = restart(io.clone(), Some("bad;unit".to_string()), None)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "validation_failed");
        assert!(io.ops().is_empty(), "must fail before emitting any command");
    }

    #[tokio::test]
    async fn verify_runs_version_and_rejects_nonzero_exit() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::fail(1, "unknown flag"));
        let err = verify(io.clone(), "/usr/local/bin/labby".to_string())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), "verify_failed");
        let ops = io.ops();
        assert!(
            ops.iter()
                .any(|o| o == "run:/usr/local/bin/labby,--version"),
            "ops: {ops:?}"
        );
    }

    #[tokio::test]
    async fn verify_accepts_zero_exit() {
        let io = Arc::new(RecordingIo::new());
        io.push_run(RunResp::ok("labby 0.3.4\n"));
        verify(io, "/usr/local/bin/labby".to_string())
            .await
            .unwrap();
    }
}

#[cfg(test)]
mod tests_arch {
    use super::*;

    #[test]
    fn arch_aliases_normalize_correctly() {
        // canonical names are unchanged
        assert_eq!(normalize_arch("x86_64"), "x86_64");
        assert_eq!(normalize_arch("aarch64"), "aarch64");
        assert_eq!(normalize_arch("i686"), "i686");
        // Docker/OCI aliases
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("x64"), "x86_64");
        assert_eq!(normalize_arch("arm64"), "aarch64");
    }

    #[test]
    fn arch_matches_handles_aliases() {
        assert!(arch_matches("x86_64", "amd64"));
        assert!(arch_matches("amd64", "x86_64"));
        assert!(arch_matches("aarch64", "arm64"));
        assert!(arch_matches("arm64", "aarch64"));
        assert!(!arch_matches("x86_64", "aarch64"));
        assert!(!arch_matches("amd64", "arm64"));
    }
}
