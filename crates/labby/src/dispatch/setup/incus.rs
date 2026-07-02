//! Local-only Incus helpers for host-side Labby gateway bootstrap.
//!
//! These helpers are intentionally CLI-only. They are not in the setup action
//! catalog and must not be exposed through MCP, HTTP, or Code Mode.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_yaml::Value;
use sha2::{Digest, Sha256};

use crate::dispatch::error::ToolError;

const INCUS_BOOTSTRAP_SCRIPT: &str = include_str!("../../../../../scripts/incus-bootstrap.sh");
const INSTALL_SCRIPT: &str = include_str!("../../../../../scripts/install.sh");
const GATEWAY_PROFILE_YAML: &str =
    include_str!("../../../../../config/incus/labby-gateway-profile.yaml");
const BACKUP_CONFIG_YAML: &str = include_str!("../../../../../config/incus/labby-backup.yaml");

const SUPPORTED_BACKUP_KEYS: &[&str] = &[
    "snapshots.schedule",
    "snapshots.expiry",
    "snapshots.pattern",
    "snapshots.schedule.stopped",
];

const DEFAULT_CONTAINER_NAME: &str = "labby";
const SERVICE_NAME: &str = "labby.service";
const REMOTE_BINARY_PATH: &str = "/usr/local/bin/labby";
const READY_URL: &str = "http://127.0.0.1:8765/ready";

#[derive(Debug, Deserialize)]
struct IncusConfigDocument {
    config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigApplyOutcome {
    pub container: String,
    pub dry_run: bool,
    pub applied: Vec<BackupConfigEntry>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapOptions {
    pub name: Option<String>,
    pub image: Option<String>,
    pub profile_name: Option<String>,
    pub backup_config: Option<PathBuf>,
    pub no_backup_config: bool,
    pub runtime_profile_name: Option<String>,
    pub storage_driver: Option<String>,
    pub storage_pool: Option<String>,
    pub storage_source: Option<String>,
    pub version: Option<String>,
    pub local_binary: Option<PathBuf>,
    pub skip_install: bool,
    pub dry_run: bool,
    pub tailscale_ssh: bool,
    pub tailscale_hostname: Option<String>,
    pub allow_source_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapArtifacts {
    pub root: PathBuf,
    pub bootstrap_script: PathBuf,
    pub install_script: PathBuf,
    pub profile_file: PathBuf,
    pub backup_config_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct IncusSyncOptions {
    pub container: Option<String>,
    pub binary: Option<PathBuf>,
    pub check_url: Option<String>,
    pub force_fallback: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct IncusSyncOutcome {
    pub container: String,
    pub binary: PathBuf,
    pub dry_run: bool,
    pub fallback_restart_used: bool,
    pub old_pid: Option<u32>,
    pub new_pid: Option<u32>,
    pub local_sha256: Option<String>,
    pub remote_sha256: Option<String>,
    pub local_version: Option<String>,
    pub remote_version: Option<String>,
    pub ready: bool,
    pub check_url: Option<String>,
    pub check_url_ok: Option<bool>,
    pub steps: Vec<String>,
}

pub(crate) fn parse_backup_config(path: &Path) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ToolError::Sdk {
        message: format!("failed to read Incus backup config {}: {e}", path.display()),
        sdk_kind: "incus_backup_config_read_failed".into(),
    })?;
    parse_backup_config_str(&raw)
}

pub(crate) fn parse_backup_config_str(raw: &str) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let doc: IncusConfigDocument = serde_yaml::from_str(raw).map_err(|e| ToolError::Sdk {
        message: format!("invalid Incus backup YAML: {e}"),
        sdk_kind: "incus_backup_config_invalid_yaml".into(),
    })?;

    let mut entries = Vec::new();
    for (key, value) in doc.config {
        validate_backup_key(&key)?;
        entries.push(BackupConfigEntry {
            key,
            value: scalar_to_string(value)?,
        });
    }
    if entries.is_empty() {
        return Err(ToolError::Sdk {
            message: "Incus backup config must contain at least one supported config key".into(),
            sdk_kind: "incus_backup_config_empty".into(),
        });
    }
    Ok(entries)
}

pub(crate) fn apply_backup_config(
    container: &str,
    path: &Path,
    dry_run: bool,
) -> Result<BackupConfigApplyOutcome, ToolError> {
    if container.trim().is_empty() {
        return Err(ToolError::MissingParam {
            message: "missing required parameter `container`".into(),
            param: "container".into(),
        });
    }
    let entries = parse_backup_config(path)?;
    if !dry_run {
        for entry in &entries {
            let status = Command::new("incus")
                .arg("config")
                .arg("set")
                .arg(container)
                .arg(&entry.key)
                .arg(&entry.value)
                .status()
                .map_err(|e| ToolError::Sdk {
                    message: format!("failed to run incus config set: {e}"),
                    sdk_kind: "incus_config_set_failed".into(),
                })?;
            if !status.success() {
                return Err(ToolError::Sdk {
                    message: format!(
                        "incus config set failed for {} on container {}",
                        entry.key, container
                    ),
                    sdk_kind: "incus_config_set_failed".into(),
                });
            }
        }
    }
    Ok(BackupConfigApplyOutcome {
        container: container.to_string(),
        dry_run,
        applied: entries,
    })
}

pub(crate) fn materialize_bootstrap_artifacts(
    root: &Path,
) -> Result<IncusBootstrapArtifacts, ToolError> {
    let scripts_dir = root.join("scripts");
    let config_dir = root.join("config").join("incus");
    std::fs::create_dir_all(&scripts_dir).map_err(|e| ToolError::Sdk {
        message: format!("failed to create {}: {e}", scripts_dir.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    std::fs::create_dir_all(&config_dir).map_err(|e| ToolError::Sdk {
        message: format!("failed to create {}: {e}", config_dir.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;

    let bootstrap_script = scripts_dir.join("incus-bootstrap.sh");
    let install_script = scripts_dir.join("install.sh");
    let profile_file = config_dir.join("labby-gateway-profile.yaml");
    let backup_config_file = config_dir.join("labby-backup.yaml");

    write_materialized_file(&bootstrap_script, INCUS_BOOTSTRAP_SCRIPT, 0o755)?;
    write_materialized_file(&install_script, INSTALL_SCRIPT, 0o755)?;
    write_materialized_file(&profile_file, GATEWAY_PROFILE_YAML, 0o644)?;
    write_materialized_file(&backup_config_file, BACKUP_CONFIG_YAML, 0o644)?;

    Ok(IncusBootstrapArtifacts {
        root: root.to_path_buf(),
        bootstrap_script,
        install_script,
        profile_file,
        backup_config_file,
    })
}

pub(crate) fn bootstrap_command(
    artifacts: &IncusBootstrapArtifacts,
    options: &IncusBootstrapOptions,
) -> Result<IncusBootstrapCommand, ToolError> {
    let mut args = vec![artifacts.bootstrap_script.as_os_str().to_os_string()];
    if options.no_backup_config && options.backup_config.is_some() {
        return Err(ToolError::Sdk {
            message: "--backup-config cannot be combined with --no-backup-config".into(),
            sdk_kind: "incus_bootstrap_invalid_options".into(),
        });
    }
    push_option(&mut args, "--name", options.name.as_deref());
    push_option(&mut args, "--image", options.image.as_deref());
    push_option(&mut args, "--profile-name", options.profile_name.as_deref());
    push_path_option(&mut args, "--profile-file", &artifacts.profile_file);
    if options.no_backup_config {
        push_flag(&mut args, "--no-backup-config", true);
    } else {
        let backup_config = options
            .backup_config
            .clone()
            .or_else(backup_config_from_env)
            .as_ref()
            .map(|path| absolutize_user_path(path))
            .transpose()?
            .unwrap_or_else(|| artifacts.backup_config_file.clone());
        push_path_option(&mut args, "--backup-config", &backup_config);
    }
    push_option(
        &mut args,
        "--runtime-profile-name",
        options.runtime_profile_name.as_deref(),
    );
    push_option(
        &mut args,
        "--storage-driver",
        options.storage_driver.as_deref(),
    );
    push_option(&mut args, "--storage-pool", options.storage_pool.as_deref());
    push_option(
        &mut args,
        "--storage-source",
        options.storage_source.as_deref(),
    );
    push_option(&mut args, "--version", options.version.as_deref());
    if let Some(local_binary) = &options.local_binary {
        push_path_option(
            &mut args,
            "--local-binary",
            &absolutize_user_path(local_binary)?,
        );
    }
    push_flag(&mut args, "--skip-install", options.skip_install);
    push_flag(&mut args, "--dry-run", options.dry_run);
    push_flag(&mut args, "--tailscale-ssh", options.tailscale_ssh);
    if let Some(hostname) = options
        .tailscale_hostname
        .as_deref()
        .or(options.name.as_deref())
    {
        push_option(&mut args, "--tailscale-hostname", Some(hostname));
    }
    push_flag(
        &mut args,
        "--allow-source-fallback",
        options.allow_source_fallback,
    );

    Ok(IncusBootstrapCommand {
        program: OsString::from("sh"),
        args,
        current_dir: artifacts.root.clone(),
    })
}

pub(crate) fn run_incus_bootstrap(options: IncusBootstrapOptions) -> Result<(), ToolError> {
    let tempdir = tempfile::tempdir().map_err(|e| ToolError::Sdk {
        message: format!("failed to create Incus bootstrap tempdir: {e}"),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    let artifacts = materialize_bootstrap_artifacts(tempdir.path())?;
    let command = bootstrap_command(&artifacts, &options)?;
    let status = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.current_dir)
        .status()
        .map_err(|e| ToolError::Sdk {
            message: format!("failed to run Incus bootstrap: {e}"),
            sdk_kind: "incus_bootstrap_failed".into(),
        })?;
    if !status.success() {
        return Err(ToolError::Sdk {
            message: format!("Incus bootstrap failed with status {status}"),
            sdk_kind: "incus_bootstrap_failed".into(),
        });
    }
    Ok(())
}

pub(crate) fn sync_incus_binary(options: IncusSyncOptions) -> Result<IncusSyncOutcome, ToolError> {
    let container = resolve_sync_container(options.container.as_deref())?;
    let binary = resolve_sync_binary(options.binary.as_deref())?;
    let local_sha256 = if options.dry_run {
        None
    } else {
        Some(file_sha256(&binary)?)
    };
    let local_version = command_stdout(
        Command::new(&binary).arg("--version").output(),
        "incus_sync_local_version_failed",
        "failed to read local labby version",
    )
    .ok()
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());
    let mut steps = Vec::new();
    let mut fallback_restart_used = false;

    if options.dry_run {
        steps.push(format!("resolve container `{container}`"));
        steps.push(format!("resolve binary `{}`", binary.display()));
        steps.push(format!("stop {SERVICE_NAME}"));
        steps.push(format!(
            "push binary and atomically install to {REMOTE_BINARY_PATH}"
        ));
        steps.push(format!("start {SERVICE_NAME} and verify {READY_URL}"));
        if let Some(url) = &options.check_url {
            steps.push(format!("check {url}"));
        }
        return Ok(IncusSyncOutcome {
            container,
            binary,
            dry_run: true,
            fallback_restart_used,
            old_pid: None,
            new_pid: None,
            local_sha256,
            remote_sha256: None,
            local_version,
            remote_version: None,
            ready: false,
            check_url: options.check_url,
            check_url_ok: None,
            steps,
        });
    }

    ensure_container_running(&container)?;
    let old_pid = service_main_pid(&container).ok().flatten();
    steps.push(format!(
        "old {SERVICE_NAME} MainPID: {}",
        old_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "none".to_string())
    ));

    if let Err(err) = incus_exec(&container, &["systemctl", "stop", SERVICE_NAME]) {
        steps.push(format!("systemctl stop failed: {}", err));
        if options.force_fallback {
            force_restart_container(&container)?;
            fallback_restart_used = true;
        } else {
            return Err(err);
        }
    } else {
        steps.push(format!("stopped {SERVICE_NAME}"));
    }
    if let Some(pid) = old_pid {
        wait_pid_gone(&container, pid, Duration::from_secs(20))?;
        steps.push(format!("old MainPID {pid} exited"));
    }
    if let Err(err) = reap_lingering_service_processes(&container) {
        steps.push(format!("lingering service reaper failed: {err}"));
        if options.force_fallback {
            force_restart_container(&container)?;
            fallback_restart_used = true;
            ensure_container_running(&container)?;
            drop(incus_exec(&container, &["systemctl", "stop", SERVICE_NAME]));
            reap_lingering_service_processes(&container)?;
            steps.push("used Incus force restart fallback".to_string());
        } else {
            return Err(err);
        }
    }
    steps.push(format!("reaped lingering {SERVICE_NAME} processes"));

    let remote_tmp = format!("/tmp/.labby-sync-{}", std::process::id());
    let target = format!("{container}{remote_tmp}");
    command_ok(
        Command::new("incus")
            .arg("file")
            .arg("push")
            .arg(&binary)
            .arg(&target)
            .output(),
        "incus_sync_push_failed",
        "failed to push labby binary into Incus container",
    )?;
    steps.push(format!("pushed `{}` to `{remote_tmp}`", binary.display()));
    incus_exec(
        &container,
        &[
            "sh",
            "-lc",
            &format!(
                "set -eu; install -m 0755 {remote_tmp} {REMOTE_BINARY_PATH}.new; mv -f {REMOTE_BINARY_PATH}.new {REMOTE_BINARY_PATH}; rm -f {remote_tmp}"
            ),
        ],
    )?;
    steps.push(format!("installed {REMOTE_BINARY_PATH} atomically"));

    drop(incus_exec(
        &container,
        &["systemctl", "reset-failed", SERVICE_NAME],
    ));
    if let Err(err) = incus_exec(&container, &["systemctl", "start", SERVICE_NAME]) {
        steps.push(format!("systemctl start failed: {}", err));
        if options.force_fallback {
            force_restart_container(&container)?;
            fallback_restart_used = true;
        } else {
            return Err(err);
        }
    } else {
        steps.push(format!("started {SERVICE_NAME}"));
    }

    let new_pid = wait_service_pid(&container, old_pid, Duration::from_secs(30))?;
    steps.push(format!("new {SERVICE_NAME} MainPID: {new_pid}"));
    wait_ready(&container, Duration::from_secs(30))?;
    steps.push(format!("verified {READY_URL}"));

    let remote_sha256 = Some(remote_sha256(&container)?);
    if local_sha256 != remote_sha256 {
        return Err(ToolError::Sdk {
            message: "remote labby binary hash does not match local binary after sync".into(),
            sdk_kind: "incus_sync_hash_mismatch".into(),
        });
    }
    steps.push("verified remote binary hash".to_string());

    let remote_version = incus_exec_stdout(&container, &[REMOTE_BINARY_PATH, "--version"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if local_version.is_some() && remote_version.is_some() && local_version != remote_version {
        return Err(ToolError::Sdk {
            message: "remote labby version does not match local binary after sync".into(),
            sdk_kind: "incus_sync_version_mismatch".into(),
        });
    }
    if remote_version.is_some() {
        steps.push("verified remote binary version".to_string());
    }

    let check_url_ok = if let Some(url) = &options.check_url {
        curl_check_url(url)?;
        steps.push(format!("verified {url}"));
        Some(true)
    } else {
        None
    };

    Ok(IncusSyncOutcome {
        container,
        binary,
        dry_run: false,
        fallback_restart_used,
        old_pid,
        new_pid: Some(new_pid),
        local_sha256,
        remote_sha256,
        local_version,
        remote_version,
        ready: true,
        check_url: options.check_url,
        check_url_ok,
        steps,
    })
}

fn write_materialized_file(path: &Path, content: &str, mode: u32) -> Result<(), ToolError> {
    std::fs::write(path, content).map_err(|e| ToolError::Sdk {
        message: format!("failed to write {}: {e}", path.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
            ToolError::Sdk {
                message: format!("failed to chmod {}: {e}", path.display()),
                sdk_kind: "incus_bootstrap_materialize_failed".into(),
            }
        })?;
    }
    let _ = mode;
    Ok(())
}

fn resolve_sync_container(explicit: Option<&str>) -> Result<String, ToolError> {
    if let Some(container) = explicit.filter(|value| !value.trim().is_empty()) {
        return Ok(container.to_string());
    }
    if let Some(container) = std::env::var("LABBY_INCUS_CONTAINER")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(container);
    }

    let raw = command_stdout(
        Command::new("incus")
            .arg("list")
            .arg("--format")
            .arg("csv")
            .arg("-c")
            .arg("ns")
            .output(),
        "incus_sync_list_failed",
        "failed to list Incus containers",
    )?;
    let containers: Vec<(String, String)> = raw
        .lines()
        .filter_map(|line| {
            let (name, state) = line.split_once(',')?;
            Some((name.trim().to_string(), state.trim().to_string()))
        })
        .collect();

    if containers
        .iter()
        .any(|(name, _)| name == DEFAULT_CONTAINER_NAME)
    {
        return Ok(DEFAULT_CONTAINER_NAME.to_string());
    }

    let labby_running: Vec<_> = containers
        .iter()
        .filter(|(name, state)| name.starts_with("labby-") && state.eq_ignore_ascii_case("RUNNING"))
        .map(|(name, _)| name.clone())
        .collect();
    if labby_running.len() == 1 {
        return Ok(labby_running[0].clone());
    }

    let labby_any: Vec<_> = containers
        .iter()
        .filter(|(name, _)| name.starts_with("labby-"))
        .map(|(name, _)| name.clone())
        .collect();
    if labby_any.len() == 1 {
        return Ok(labby_any[0].clone());
    }

    Err(ToolError::Sdk {
        message: if labby_any.is_empty() {
            "could not discover a Labby Incus container; pass --container or set LABBY_INCUS_CONTAINER".into()
        } else {
            format!(
                "multiple Labby-like Incus containers found ({}); pass --container or set LABBY_INCUS_CONTAINER",
                labby_any.join(", ")
            )
        },
        sdk_kind: "incus_sync_container_discovery_failed".into(),
    })
}

fn resolve_sync_binary(explicit: Option<&Path>) -> Result<PathBuf, ToolError> {
    if let Some(path) = explicit {
        return require_binary(path);
    }
    if let Some(path) = std::env::var_os("LABBY_INCUS_BINARY").filter(|value| !value.is_empty()) {
        return require_binary(Path::new(&path));
    }

    let repo_debug = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("target").join("debug").join("labby"))
        .filter(|path| path.is_file());
    if let Some(path) = repo_debug {
        return require_binary(&path);
    }

    let exe = std::env::current_exe().map_err(|e| ToolError::Sdk {
        message: format!("failed to resolve current labby executable: {e}"),
        sdk_kind: "incus_sync_binary_resolve_failed".into(),
    })?;
    require_binary(&exe)
}

fn require_binary(path: &Path) -> Result<PathBuf, ToolError> {
    let path = absolutize_user_path(path)?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(ToolError::Sdk {
            message: format!("labby binary does not exist: {}", path.display()),
            sdk_kind: "incus_sync_binary_missing".into(),
        })
    }
}

fn ensure_container_running(container: &str) -> Result<(), ToolError> {
    let state = command_stdout(
        Command::new("incus")
            .arg("list")
            .arg(container)
            .arg("--format")
            .arg("csv")
            .arg("-c")
            .arg("s")
            .output(),
        "incus_sync_container_state_failed",
        "failed to read Incus container state",
    )?;
    if state.lines().any(|line| line.trim() == "RUNNING") {
        return Ok(());
    }
    command_ok(
        Command::new("incus").arg("start").arg(container).output(),
        "incus_sync_container_start_failed",
        "failed to start Incus container",
    )
}

fn service_main_pid(container: &str) -> Result<Option<u32>, ToolError> {
    let raw = incus_exec_stdout(
        container,
        &[
            "systemctl",
            "show",
            SERVICE_NAME,
            "--property",
            "MainPID",
            "--value",
        ],
    )?;
    let pid = raw.trim().parse::<u32>().ok().filter(|pid| *pid > 0);
    Ok(pid)
}

fn wait_pid_gone(container: &str, pid: u32, timeout: Duration) -> Result<(), ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let alive = incus_exec(container, &["sh", "-lc", &format!("kill -0 {pid}")]).is_ok();
        if !alive {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for old {SERVICE_NAME} MainPID {pid} to exit"),
        sdk_kind: "incus_sync_old_pid_timeout".into(),
    })
}

fn wait_service_pid(
    container: &str,
    old_pid: Option<u32>,
    timeout: Duration,
) -> Result<u32, ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(pid) = service_main_pid(container)? {
            if Some(pid) != old_pid {
                return Ok(pid);
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for {SERVICE_NAME} to start with a new MainPID"),
        sdk_kind: "incus_sync_new_pid_timeout".into(),
    })
}

fn wait_ready(container: &str, timeout: Duration) -> Result<(), ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if incus_exec(container, &["curl", "-fsS", READY_URL]).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for {READY_URL} inside Incus container"),
        sdk_kind: "incus_sync_ready_timeout".into(),
    })
}

fn reap_lingering_service_processes(container: &str) -> Result<(), ToolError> {
    incus_exec(
        container,
        &[
            "sh",
            "-lc",
            &format!(
                "systemctl kill {SERVICE_NAME} --kill-who=all --signal=SIGTERM >/dev/null 2>&1 || true"
            ),
        ],
    )?;
    if wait_no_labby_serve(container, Duration::from_secs(5)).is_ok() {
        return Ok(());
    }
    incus_exec(
        container,
        &[
            "sh",
            "-lc",
            &format!(
                "systemctl kill {SERVICE_NAME} --kill-who=all --signal=SIGKILL >/dev/null 2>&1 || true; pkill -KILL -f '^/usr/local/bin/labby serve' >/dev/null 2>&1 || true"
            ),
        ],
    )?;
    wait_no_labby_serve(container, Duration::from_secs(5))
}

fn wait_no_labby_serve(container: &str, timeout: Duration) -> Result<(), ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let alive = incus_exec(container, &["pgrep", "-f", "^/usr/local/bin/labby serve"]).is_ok();
        if !alive {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for lingering {SERVICE_NAME} processes to exit"),
        sdk_kind: "incus_sync_lingering_process_timeout".into(),
    })
}

fn force_restart_container(container: &str) -> Result<(), ToolError> {
    command_ok(
        Command::new("incus")
            .arg("stop")
            .arg(container)
            .arg("--force")
            .output(),
        "incus_sync_force_stop_failed",
        "failed to force stop Incus container",
    )?;
    command_ok(
        Command::new("incus").arg("start").arg(container).output(),
        "incus_sync_force_start_failed",
        "failed to start Incus container after force stop",
    )
}

fn remote_sha256(container: &str) -> Result<String, ToolError> {
    let raw = incus_exec_stdout(
        container,
        &[
            "sh",
            "-lc",
            &format!("sha256sum {REMOTE_BINARY_PATH} | awk '{{print $1}}'"),
        ],
    )?;
    Ok(raw.trim().to_string())
}

fn file_sha256(path: &Path) -> Result<String, ToolError> {
    let mut file = File::open(path).map_err(|e| ToolError::Sdk {
        message: format!("failed to open {}: {e}", path.display()),
        sdk_kind: "incus_sync_hash_failed".into(),
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    loop {
        let read = file.read(&mut buf).map_err(|e| ToolError::Sdk {
            message: format!("failed to read {}: {e}", path.display()),
            sdk_kind: "incus_sync_hash_failed".into(),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex_bytes(&hasher.finalize()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn curl_check_url(url: &str) -> Result<(), ToolError> {
    command_ok(
        Command::new("curl").arg("-fsS").arg(url).output(),
        "incus_sync_check_url_failed",
        "failed optional sync check URL",
    )
}

fn incus_exec(container: &str, args: &[&str]) -> Result<(), ToolError> {
    let mut command = Command::new("incus");
    command.arg("exec").arg(container).arg("--").args(args);
    command_ok(
        command.output(),
        "incus_sync_exec_failed",
        "failed to run command inside Incus container",
    )
}

fn incus_exec_stdout(container: &str, args: &[&str]) -> Result<String, ToolError> {
    let mut command = Command::new("incus");
    command.arg("exec").arg(container).arg("--").args(args);
    command_stdout(
        command.output(),
        "incus_sync_exec_failed",
        "failed to run command inside Incus container",
    )
}

fn command_ok(
    output: std::io::Result<Output>,
    sdk_kind: &'static str,
    context: &'static str,
) -> Result<(), ToolError> {
    let output = output.map_err(|e| ToolError::Sdk {
        message: format!("{context}: {e}"),
        sdk_kind: sdk_kind.into(),
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(output, sdk_kind, context))
    }
}

fn command_stdout(
    output: std::io::Result<Output>,
    sdk_kind: &'static str,
    context: &'static str,
) -> Result<String, ToolError> {
    let output = output.map_err(|e| ToolError::Sdk {
        message: format!("{context}: {e}"),
        sdk_kind: sdk_kind.into(),
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(command_error(output, sdk_kind, context))
    }
}

fn command_error(output: Output, sdk_kind: &'static str, context: &'static str) -> ToolError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    ToolError::Sdk {
        message: if detail.is_empty() {
            format!("{context}: command exited with {}", output.status)
        } else {
            format!("{context}: {detail}")
        },
        sdk_kind: sdk_kind.into(),
    }
}

fn push_option(args: &mut Vec<OsString>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(OsString::from(flag));
        args.push(OsString::from(value));
    }
}

fn push_path_option(args: &mut Vec<OsString>, flag: &str, value: &Path) {
    args.push(OsString::from(flag));
    args.push(value.as_os_str().to_os_string());
}

fn push_flag(args: &mut Vec<OsString>, flag: &str, enabled: bool) {
    if enabled {
        args.push(OsString::from(flag));
    }
}

fn backup_config_from_env() -> Option<PathBuf> {
    std::env::var_os("LABBY_INCUS_BACKUP_CONFIG")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn absolutize_user_path(path: &Path) -> Result<PathBuf, ToolError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| ToolError::Sdk {
        message: format!("failed to resolve current directory: {e}"),
        sdk_kind: "incus_bootstrap_path_resolve_failed".into(),
    })?;
    Ok(cwd.join(path))
}

fn validate_backup_key(key: &str) -> Result<(), ToolError> {
    if SUPPORTED_BACKUP_KEYS.contains(&key) {
        return Ok(());
    }
    Err(ToolError::Sdk {
        message: format!("unsupported Incus backup config key: {key}"),
        sdk_kind: "incus_backup_config_unsupported_key".into(),
    })
}

fn scalar_to_string(value: Value) -> Result<String, ToolError> {
    match value {
        Value::String(value) => Ok(value),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null | Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => {
            Err(ToolError::Sdk {
                message: "Incus backup config values must be scalar strings, booleans, or numbers"
                    .into(),
                sdk_kind: "incus_backup_config_non_scalar".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn parses_supported_snapshot_keys() {
        let entries = parse_backup_config_str(
            r#"
config:
  snapshots.schedule: "@daily"
  snapshots.expiry: "14d"
  snapshots.pattern: "labby-{{ creation_date|date:'2006-01-02_15-04-05' }}"
  snapshots.schedule.stopped: false
"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 4);
        assert!(
            entries.iter().any(|entry| {
                entry.key == "snapshots.schedule.stopped" && entry.value == "false"
            })
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse_backup_config_str(
            r#"
config:
  security.privileged: true
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_unsupported_key");
    }

    #[test]
    fn rejects_non_scalar_values() {
        let err = parse_backup_config_str(
            r#"
config:
  snapshots.schedule:
    nested: nope
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_non_scalar");
    }

    #[test]
    fn materializes_embedded_bootstrap_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();

        assert!(artifacts.bootstrap_script.exists());
        assert!(artifacts.install_script.exists());
        assert!(artifacts.profile_file.exists());
        assert!(artifacts.backup_config_file.exists());

        let bootstrap = std::fs::read_to_string(&artifacts.bootstrap_script).unwrap();
        assert!(bootstrap.contains("incus-bootstrap.sh"));
        assert!(bootstrap.contains("labby setup --provision --yes"));

        let profile = std::fs::read_to_string(&artifacts.profile_file).unwrap();
        assert!(profile.contains("security.privileged: \"false\""));
    }

    #[test]
    fn builds_bootstrap_command_from_embedded_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            version: Some("v1.2.3".to_string()),
            dry_run: true,
            storage_driver: Some("dir".to_string()),
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();
        let args = command.args;

        assert_eq!(command.program, OsStr::new("sh"));
        assert_eq!(args[0], artifacts.bootstrap_script.as_os_str());
        assert!(has_arg_pair(&args, "--version", OsStr::new("v1.2.3")));
        assert!(has_arg_pair(
            &args,
            "--profile-file",
            artifacts.profile_file.as_os_str()
        ));
        assert!(args.windows(2).any(|pair| pair
            == [
                OsStr::new("--backup-config"),
                artifacts.backup_config_file.as_os_str()
            ]));
        assert!(has_arg_pair(&args, "--storage-driver", OsStr::new("dir")));
        assert!(args.iter().any(|arg| arg == OsStr::new("--dry-run")));
    }

    #[test]
    fn resolves_user_paths_before_switching_to_temp_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            backup_config: Some(PathBuf::from("my-backup.yaml")),
            local_binary: Some(PathBuf::from("target/debug/labby")),
            dry_run: true,
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();
        let args = command.args;
        let cwd = std::env::current_dir().unwrap();

        assert!(has_arg_pair(
            &args,
            "--backup-config",
            cwd.join("my-backup.yaml").as_os_str()
        ));
        assert!(has_arg_pair(
            &args,
            "--local-binary",
            cwd.join("target/debug/labby").as_os_str()
        ));
    }

    #[test]
    fn rejects_conflicting_backup_config_options() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            backup_config: Some(PathBuf::from("my-backup.yaml")),
            no_backup_config: true,
            ..IncusBootstrapOptions::default()
        };

        let err = bootstrap_command(&artifacts, &options).unwrap_err();
        assert_eq!(err.kind(), "incus_bootstrap_invalid_options");
    }

    #[test]
    fn passes_container_name_as_tailscale_hostname() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            name: Some("labby".to_string()),
            dry_run: true,
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();

        assert!(has_arg_pair(
            &command.args,
            "--tailscale-hostname",
            OsStr::new("labby")
        ));
    }

    fn has_arg_pair(args: &[OsString], flag: &str, value: &OsStr) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == OsStr::new(flag) && pair[1] == value)
    }
}
