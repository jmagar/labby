//! CLI-only management helpers for the system `labby.service`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::dispatch::error::ToolError;

mod microsandbox_image;

const SERVICE_NAME: &str = "labby.service";
const WATCHDOG_SERVICE_NAME: &str = "labby-watchdog.service";
const WATCHDOG_TIMER_NAME: &str = "labby-watchdog.timer";
const WATCHDOG_ESCALATION_NAME: &str = "labby-watchdog-escalation.service";
const SERVICE_INSTALLATION_ROOT: &str = "/home/labby/.labby";
const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(300);
const CAPTURE_BYTES: usize = 16 * 1024;
const PREVIOUS_HOST_RELEASE_DIR: &str = "/var/lib/labby/host-service-previous";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct HostServiceStatus {
    pub installed: bool,
    pub load_state: Option<String>,
    pub active_state: Option<String>,
    pub sub_state: Option<String>,
    pub main_pid: Option<u32>,
    pub exec_main_status: Option<i32>,
    pub unit_path: PathBuf,
    pub process_exe: Option<PathBuf>,
    pub local_ready: Option<bool>,
    pub local_ready_error: Option<String>,
    pub ready_owned_by_service: Option<bool>,
    pub docker_labby_master_running: Option<bool>,
    pub docker_labby_master_error: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct HostServiceOutcome {
    pub ok: bool,
    pub changed: bool,
    pub message: String,
    pub unit_path: PathBuf,
    pub stdout: String,
    pub stderr: String,
}

struct CommandCapture {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapturedActiveState {
    Active,
    Inactive,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapturedUnitFileState {
    Enabled,
    EnabledRuntime,
    Disabled,
}

impl CapturedActiveState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, ToolError> {
        match value {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            "failed" => Ok(Self::Failed),
            _ => Err(ToolError::Sdk {
                sdk_kind: "host_service_previous_manifest_invalid".into(),
                message: format!("invalid retained ActiveState `{value}`"),
            }),
        }
    }
}

impl CapturedUnitFileState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::EnabledRuntime => "enabled-runtime",
            Self::Disabled => "disabled",
        }
    }

    fn parse(value: &str) -> Result<Self, ToolError> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "enabled-runtime" => Ok(Self::EnabledRuntime),
            "disabled" => Ok(Self::Disabled),
            _ => Err(ToolError::Sdk {
                sdk_kind: "host_service_previous_manifest_invalid".into(),
                message: format!("invalid retained UnitFileState `{value}`"),
            }),
        }
    }
}

pub(crate) async fn unit() -> Result<String, ToolError> {
    Ok(unit_text().to_string())
}

pub(crate) async fn install() -> Result<HostServiceOutcome, ToolError> {
    let port = preflight_port_available("install").await?;
    let path = unit_path();
    let text = unit_text();
    std::fs::create_dir_all(unit_dir()).map_err(io_error)?;
    let snapshot = HostServiceSnapshot::capture(&path).await?;
    let changed = std::fs::read_to_string(&path).ok().as_deref() != Some(text);
    match install_commit(port, path.clone(), text, changed).await {
        Ok(outcome) => Ok(outcome),
        Err(primary) => match snapshot.rollback(&path).await {
            Ok(()) => Err(host_transaction_failure(&primary, None)),
            Err(rollback) => Err(host_transaction_failure(&primary, Some(&rollback))),
        },
    }
}

fn persist_previous_host_release(
    binary: Option<&[u8]>,
    state: Option<(CapturedActiveState, CapturedUnitFileState)>,
) -> Result<(), ToolError> {
    persist_previous_host_release_at(Path::new(PREVIOUS_HOST_RELEASE_DIR), binary, state)
}

fn persist_previous_host_release_at(
    root: &Path,
    binary: Option<&[u8]>,
    state: Option<(CapturedActiveState, CapturedUnitFileState)>,
) -> Result<(), ToolError> {
    persist_previous_host_release_with_checkpoint(root, binary, state, || Ok(()))
}

fn persist_previous_host_release_with_checkpoint(
    root: &Path,
    binary: Option<&[u8]>,
    state: Option<(CapturedActiveState, CapturedUnitFileState)>,
    after_binary_write: impl FnOnce() -> Result<(), ToolError>,
) -> Result<(), ToolError> {
    let (Some(binary), Some((active, enabled))) = (binary, state) else {
        return Ok(());
    };
    let binary_path = root.join("labby");
    let manifest_path = root.join("manifest");
    let previous_binary = read_optional(&binary_path)?;
    let previous_manifest = read_optional(&manifest_path)?;
    std::fs::create_dir_all(root).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    let publication = (|| {
        restore_executable(&binary_path, Some(binary))?;
        after_binary_write()?;
        atomic_write(
            &manifest_path,
            format!("active={}\nenabled={}\n", active.as_str(), enabled.as_str()).as_bytes(),
        )
    })();
    if let Err(primary) = publication {
        // Retention is a pair: preserving active service state is insufficient
        // if a failed upgrade leaves an older manifest beside a newer binary.
        let mut failures = Vec::new();
        collect_restore(
            &mut failures,
            "retained binary",
            restore_executable(&binary_path, previous_binary.as_deref()),
        );
        collect_restore(
            &mut failures,
            "retained manifest",
            restore_optional(&manifest_path, previous_manifest.as_deref()),
        );
        if failures.is_empty() {
            return Err(primary);
        }
        return Err(ToolError::Sdk {
            sdk_kind: "host_service_previous_release_restore_failed".into(),
            message: format!(
                "previous release retention failed: {primary}; restoring retained release failed: {}",
                failures.join("; ")
            ),
        });
    }
    Ok(())
}

pub(crate) async fn rollback_previous_release() -> Result<HostServiceOutcome, ToolError> {
    let root = Path::new(PREVIOUS_HOST_RELEASE_DIR);
    let prior = std::fs::read(root.join("labby")).map_err(|error| ToolError::Sdk {
        sdk_kind: "host_service_no_previous_release".into(),
        message: format!("no retained previous host release is available: {error}"),
    })?;
    let (desired_active, desired_enabled) = parse_previous_host_manifest(
        &std::fs::read_to_string(root.join("manifest")).map_err(io_error)?,
    )?;
    let destination = Path::new("/usr/local/bin/labby");
    let current = read_optional(destination)?;
    let current_state = capture_systemd_state(SERVICE_NAME).await?;
    restore_executable(destination, Some(&prior))?;
    let activation = async {
        restore_captured_unit_file_state(SERVICE_NAME, desired_enabled).await?;
        restore_captured_active_state(SERVICE_NAME, desired_active).await
    }
    .await;
    if let Err(primary) = activation {
        let binary_restore = restore_executable(destination, current.as_deref());
        let enabled_restore = restore_captured_unit_file_state(SERVICE_NAME, current_state.1).await;
        let active_restore = restore_captured_active_state(SERVICE_NAME, current_state.0).await;
        return Err(ToolError::Sdk {
            sdk_kind: "host_service_release_rollback_failed".into(),
            message: format!(
                "previous host release activation failed: {primary}; candidate restoration: binary={binary_restore:?}, enabled={enabled_restore:?}, active={active_restore:?}"
            ),
        });
    }
    std::fs::remove_dir_all(root).map_err(io_error)?;
    Ok(HostServiceOutcome {
        ok: true,
        changed: true,
        message: "restored the retained previous host release".into(),
        unit_path: unit_path(),
        stdout: String::new(),
        stderr: String::new(),
    })
}

fn parse_previous_host_manifest(
    text: &str,
) -> Result<(CapturedActiveState, CapturedUnitFileState), ToolError> {
    let props = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect::<BTreeMap<_, _>>();
    Ok((
        CapturedActiveState::parse(props.get("active").copied().unwrap_or(""))?,
        CapturedUnitFileState::parse(props.get("enabled").copied().unwrap_or(""))?,
    ))
}

pub(crate) async fn install_self_transaction(
    source: &Path,
) -> Result<HostServiceOutcome, ToolError> {
    let port = preflight_port_available("install").await?;
    let path = unit_path();
    let text = unit_text();
    std::fs::create_dir_all(unit_dir()).map_err(io_error)?;
    let snapshot = HostServiceSnapshot::capture(&path).await?;
    let destination = Path::new("/usr/local/bin/labby");
    let prior_binary = read_optional(destination)?;
    let prior_state = snapshot
        .unit
        .as_ref()
        .map(|_| (snapshot.active, snapshot.enabled));
    let changed = snapshot.unit.as_deref() != Some(text.as_bytes());
    run_self_install_transaction(
        destination,
        prior_binary.as_deref(),
        async {
            install_executable(source, destination)?;
            let outcome = install_commit(port, path.clone(), text, changed).await?;
            persist_previous_host_release(prior_binary.as_deref(), prior_state)?;
            Ok(outcome)
        },
        || snapshot.rollback(&path),
    )
    .await
}

/// Binary replacement, activation, and recovery retention share one rollback
/// boundary, including failures after an atomic file replacement committed.
async fn run_self_install_transaction<F, R, RF>(
    destination: &Path,
    prior_binary: Option<&[u8]>,
    operation: F,
    restore_service: R,
) -> Result<HostServiceOutcome, ToolError>
where
    F: Future<Output = Result<HostServiceOutcome, ToolError>>,
    R: FnOnce() -> RF,
    RF: Future<Output = Result<(), ToolError>>,
{
    match operation.await {
        Ok(outcome) => Ok(outcome),
        Err(primary) => {
            let mut failures = Vec::new();
            collect_restore(
                &mut failures,
                "prior binary",
                restore_executable(destination, prior_binary),
            );
            collect_restore(
                &mut failures,
                "prior service snapshot",
                restore_service().await,
            );
            if failures.is_empty() {
                Err(ToolError::Sdk {
                    sdk_kind: "host_service_upgrade_rolled_back".into(),
                    message: format!(
                        "host-service upgrade failed and the prior binary/service state was restored: {primary}"
                    ),
                })
            } else {
                Err(ToolError::Sdk {
                    sdk_kind: "host_service_upgrade_rollback_failed".into(),
                    message: format!(
                        "host-service upgrade failed: {primary}; rollback residuals: {}",
                        failures.join("; ")
                    ),
                })
            }
        }
    }
}

fn install_executable(source: &Path, destination: &Path) -> Result<(), ToolError> {
    install_executable_with_permissions(source, destination, set_executable_permissions)
}

fn install_executable_with_permissions(
    source: &Path,
    destination: &Path,
    permissions: impl FnOnce(&Path) -> Result<(), ToolError>,
) -> Result<(), ToolError> {
    let bytes = std::fs::read(source).map_err(io_error)?;
    atomic_write(destination, &bytes)?;
    permissions(destination)
}

fn set_executable_permissions(destination: &Path) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o755))
            .map_err(io_error)?;
    }
    #[cfg(not(unix))]
    let _ = destination;
    Ok(())
}

fn restore_executable(destination: &Path, prior: Option<&[u8]>) -> Result<(), ToolError> {
    match prior {
        Some(bytes) => {
            atomic_write(destination, bytes)?;
            set_executable_permissions(destination)
        }
        None => match std::fs::remove_file(destination) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        },
    }
}

async fn install_commit(
    port: u16,
    path: PathBuf,
    text: &str,
    changed: bool,
) -> Result<HostServiceOutcome, ToolError> {
    if changed {
        atomic_write(&path, text.as_bytes())?;
        host_checkpoint("unit-write")?;
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    append_optional_verify(&path, &mut stdout, &mut stderr).await?;
    host_checkpoint("unit-verify")?;
    let daemon = run_systemctl(&["daemon-reload"]).await?;
    stdout.push_str(&daemon.stdout);
    stderr.push_str(&daemon.stderr);
    host_checkpoint("daemon-reload")?;
    let enable = run_systemctl(&["enable", SERVICE_NAME]).await?;
    stdout.push_str(&enable.stdout);
    stderr.push_str(&enable.stderr);
    host_checkpoint("enable")?;
    if std::env::var_os("LABBY_HOST_WATCHDOG").as_deref() == Some(std::ffi::OsStr::new("1")) {
        atomic_write(
            &unit_dir().join(WATCHDOG_SERVICE_NAME),
            watchdog_service_text().as_bytes(),
        )?;
        atomic_write(
            &unit_dir().join(WATCHDOG_TIMER_NAME),
            watchdog_timer_text().as_bytes(),
        )?;
        atomic_write(
            &unit_dir().join(WATCHDOG_ESCALATION_NAME),
            watchdog_escalation_text().as_bytes(),
        )?;
        host_checkpoint("watchdog-units")?;
        let reload = run_systemctl(&["daemon-reload"]).await?;
        stdout.push_str(&reload.stdout);
        stderr.push_str(&reload.stderr);
        host_checkpoint("watchdog-reload")?;
        let watchdog = run_systemctl(&["enable", "--now", WATCHDOG_TIMER_NAME]).await?;
        stdout.push_str(&watchdog.stdout);
        stderr.push_str(&watchdog.stderr);
        host_checkpoint("watchdog-enable")?;
    }
    provision_oauth_encryption_key_before_restart().await?;
    host_checkpoint("credential-provision")?;
    microsandbox_image::prepare_before_restart().await?;
    host_checkpoint("sandbox-prepare")?;
    let restart = run_systemctl(&["restart", SERVICE_NAME]).await?;
    stdout.push_str(&restart.stdout);
    stderr.push_str(&restart.stderr);
    host_checkpoint("restart")?;
    if let Err(err) = poll_ready(port).await {
        stderr.push_str(&format!("\nreadiness failed: {err}"));
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "installed {SERVICE_NAME}, but local readiness did not pass: {err}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            ),
        });
    }
    host_checkpoint("readiness")?;
    Ok(HostServiceOutcome {
        ok: true,
        changed,
        message: format!("{SERVICE_NAME} installed and running"),
        unit_path: path,
        stdout,
        stderr,
    })
}

fn host_checkpoint(label: &str) -> Result<(), ToolError> {
    host_checkpoint_from(
        std::env::var("LABBY_HOST_SERVICE_FAIL_AFTER")
            .ok()
            .as_deref(),
        label,
    )
}

fn host_checkpoint_from(configured: Option<&str>, label: &str) -> Result<(), ToolError> {
    if configured == Some(label) {
        return Err(ToolError::Sdk {
            sdk_kind: "host_service_injected_failure".into(),
            message: format!("injected host-service activation failure after {label}"),
        });
    }
    Ok(())
}

struct HostServiceSnapshot {
    unit: Option<Vec<u8>>,
    watchdog_service: Option<Vec<u8>>,
    watchdog_timer: Option<Vec<u8>>,
    watchdog_escalation: Option<Vec<u8>>,
    active: CapturedActiveState,
    enabled: CapturedUnitFileState,
    watchdog_active: CapturedActiveState,
    watchdog_enabled: CapturedUnitFileState,
    service_env: Option<Vec<u8>>,
    env_backups: BTreeSet<PathBuf>,
    dropins: DirectorySnapshot,
}

struct DirectorySnapshot {
    existed: bool,
    files: Vec<(std::ffi::OsString, Vec<u8>)>,
}

impl HostServiceSnapshot {
    async fn capture(path: &Path) -> Result<Self, ToolError> {
        let unit = read_optional(path)?;
        let watchdog_service = read_optional(&unit_dir().join(WATCHDOG_SERVICE_NAME))?;
        let watchdog_timer = read_optional(&unit_dir().join(WATCHDOG_TIMER_NAME))?;
        let watchdog_escalation = read_optional(&unit_dir().join(WATCHDOG_ESCALATION_NAME))?;
        let service_env_path = Path::new(SERVICE_INSTALLATION_ROOT).join(".env");
        let service_env = read_optional(&service_env_path)?;
        let env_backups = matching_sibling_files(&service_env_path, ".env.bak.")?;
        let dropins = DirectorySnapshot::capture(Path::new("/etc/systemd/system/labby.service.d"))?;
        let (active, enabled) = if unit.is_some() {
            capture_systemd_state(SERVICE_NAME).await?
        } else {
            (
                CapturedActiveState::Inactive,
                CapturedUnitFileState::Disabled,
            )
        };
        let (watchdog_active, watchdog_enabled) = if watchdog_timer.is_some() {
            capture_systemd_state(WATCHDOG_TIMER_NAME).await?
        } else {
            (
                CapturedActiveState::Inactive,
                CapturedUnitFileState::Disabled,
            )
        };
        Ok(Self {
            unit,
            watchdog_service,
            watchdog_timer,
            watchdog_escalation,
            active,
            enabled,
            watchdog_active,
            watchdog_enabled,
            service_env,
            env_backups,
            dropins,
        })
    }

    async fn rollback(&self, path: &Path) -> Result<(), ToolError> {
        let mut failures = Vec::new();
        collect_restore(
            &mut failures,
            "main unit",
            restore_optional(path, self.unit.as_deref()),
        );
        let service_env_path = Path::new(SERVICE_INSTALLATION_ROOT).join(".env");
        collect_restore(
            &mut failures,
            "service environment",
            restore_optional(&service_env_path, self.service_env.as_deref()),
        );
        if self.service_env.is_some() {
            collect_async(
                &mut failures,
                "service environment ownership",
                run_command("chown", &["labby:labby", "/home/labby/.labby/.env"]).await,
            );
        }
        collect_restore(
            &mut failures,
            "service environment backups",
            remove_new_matching_siblings(&service_env_path, ".env.bak.", &self.env_backups),
        );
        collect_restore(
            &mut failures,
            "service drop-ins",
            self.dropins
                .restore(Path::new("/etc/systemd/system/labby.service.d")),
        );
        collect_restore(
            &mut failures,
            "watchdog service",
            restore_optional(
                &unit_dir().join(WATCHDOG_SERVICE_NAME),
                self.watchdog_service.as_deref(),
            ),
        );
        collect_restore(
            &mut failures,
            "watchdog escalation",
            restore_optional(
                &unit_dir().join(WATCHDOG_ESCALATION_NAME),
                self.watchdog_escalation.as_deref(),
            ),
        );
        collect_restore(
            &mut failures,
            "watchdog timer",
            restore_optional(
                &unit_dir().join(WATCHDOG_TIMER_NAME),
                self.watchdog_timer.as_deref(),
            ),
        );
        collect_async(
            &mut failures,
            "daemon reload",
            run_systemctl(&["daemon-reload"]).await,
        );
        collect_restore(
            &mut failures,
            "main enablement",
            restore_captured_unit_file_state(SERVICE_NAME, self.enabled).await,
        );
        collect_restore(
            &mut failures,
            "main activity",
            restore_captured_active_state(SERVICE_NAME, self.active).await,
        );
        collect_restore(
            &mut failures,
            "watchdog enablement",
            restore_captured_unit_file_state(WATCHDOG_TIMER_NAME, self.watchdog_enabled).await,
        );
        collect_restore(
            &mut failures,
            "watchdog activity",
            restore_captured_active_state(WATCHDOG_TIMER_NAME, self.watchdog_active).await,
        );
        rollback_result(failures)
    }
}

async fn restore_captured_active_state(
    unit: &str,
    state: CapturedActiveState,
) -> Result<(), ToolError> {
    match state {
        CapturedActiveState::Active => {
            run_systemctl(&["restart", unit]).await?;
        }
        CapturedActiveState::Inactive => {
            run_systemctl(&["stop", unit]).await?;
            run_systemctl(&["reset-failed", unit]).await?;
        }
        CapturedActiveState::Failed => {
            // Starting the restored unit is the only supported way to recreate a
            // systemd failed state. A successful start means exact restoration is
            // impossible, so fail closed and retain the compound rollback error.
            drop(run_systemctl(&["start", unit]).await);
            let (actual, _) = capture_systemd_state(unit).await?;
            if actual != CapturedActiveState::Failed {
                return Err(ToolError::Sdk {
                    sdk_kind: "host_service_state_restore_failed".into(),
                    message: format!(
                        "restored {unit}, but could not reproduce prior failed state (found {actual:?})"
                    ),
                });
            }
        }
    }
    Ok(())
}

async fn restore_captured_unit_file_state(
    unit: &str,
    state: CapturedUnitFileState,
) -> Result<(), ToolError> {
    match state {
        CapturedUnitFileState::Enabled => drop(run_systemctl(&["enable", unit]).await?),
        CapturedUnitFileState::EnabledRuntime => {
            drop(run_systemctl(&["disable", unit]).await?);
            drop(run_systemctl(&["enable", "--runtime", unit]).await?);
        }
        CapturedUnitFileState::Disabled => drop(run_systemctl(&["disable", unit]).await?),
    }
    Ok(())
}

impl DirectorySnapshot {
    fn capture(path: &Path) -> Result<Self, ToolError> {
        if !path.exists() {
            return Ok(Self {
                existed: false,
                files: Vec::new(),
            });
        }
        let mut files = Vec::new();
        for entry in std::fs::read_dir(path).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if !entry.file_type().map_err(io_error)?.is_file() {
                return Err(ToolError::Sdk {
                    sdk_kind: "host_service_state_capture_failed".into(),
                    message: format!(
                        "cannot transactionally snapshot non-file service drop-in `{}`",
                        entry.path().display()
                    ),
                });
            }
            files.push((
                entry.file_name(),
                std::fs::read(entry.path()).map_err(io_error)?,
            ));
        }
        Ok(Self {
            existed: true,
            files,
        })
    }

    fn restore(&self, path: &Path) -> Result<(), ToolError> {
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        if self.existed {
            std::fs::create_dir_all(path).map_err(io_error)?;
            for (name, bytes) in &self.files {
                atomic_write(&path.join(name), bytes)?;
            }
        }
        Ok(())
    }
}

fn matching_sibling_files(path: &Path, prefix: &str) -> Result<BTreeSet<PathBuf>, ToolError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut matches = BTreeSet::new();
    match std::fs::read_dir(parent) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(io_error)?;
                if entry.file_name().to_string_lossy().starts_with(prefix) {
                    matches.insert(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    Ok(matches)
}

fn remove_new_matching_siblings(
    path: &Path,
    prefix: &str,
    prior: &BTreeSet<PathBuf>,
) -> Result<(), ToolError> {
    for candidate in matching_sibling_files(path, prefix)? {
        if !prior.contains(&candidate) {
            std::fs::remove_file(candidate).map_err(io_error)?;
        }
    }
    Ok(())
}

fn collect_restore(failures: &mut Vec<String>, action: &str, result: Result<(), ToolError>) {
    if let Err(error) = result {
        failures.push(format!("{action}: {error}"));
    }
}

fn collect_async(
    failures: &mut Vec<String>,
    action: &str,
    result: Result<CommandCapture, ToolError>,
) {
    if let Err(error) = result {
        failures.push(format!("{action}: {error}"));
    }
}

fn rollback_result(failures: Vec<String>) -> Result<(), ToolError> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "host_service_rollback_failed".into(),
            message: format!(
                "host-service rollback left residual failures: {}",
                failures.join("; ")
            ),
        })
    }
}

async fn capture_systemd_state(
    unit: &str,
) -> Result<(CapturedActiveState, CapturedUnitFileState), ToolError> {
    let output = run_systemctl(&[
        "show",
        unit,
        "--property=ActiveState,UnitFileState",
        "--no-pager",
    ])
    .await?;
    parse_captured_systemd_state(&output.stdout, unit)
}

fn parse_captured_systemd_state(
    stdout: &str,
    unit: &str,
) -> Result<(CapturedActiveState, CapturedUnitFileState), ToolError> {
    let props = parse_systemctl_show(stdout);
    let active = props.get("ActiveState").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "host_service_state_capture_failed".into(),
        message: format!("systemctl did not report ActiveState for {unit}"),
    })?;
    let enabled = props.get("UnitFileState").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "host_service_state_capture_failed".into(),
        message: format!("systemctl did not report UnitFileState for {unit}"),
    })?;
    let active = match active.as_str() {
        "active" => CapturedActiveState::Active,
        "inactive" => CapturedActiveState::Inactive,
        "failed" => CapturedActiveState::Failed,
        other => {
            return Err(ToolError::Sdk {
                sdk_kind: "host_service_state_capture_failed".into(),
                message: format!("unsupported ActiveState `{other}` for {unit}"),
            });
        }
    };
    let enabled = match enabled.as_str() {
        "enabled" => CapturedUnitFileState::Enabled,
        "enabled-runtime" => CapturedUnitFileState::EnabledRuntime,
        "disabled" => CapturedUnitFileState::Disabled,
        other => {
            return Err(ToolError::Sdk {
                sdk_kind: "host_service_state_capture_failed".into(),
                message: format!("unsupported UnitFileState `{other}` for {unit}"),
            });
        }
    };
    Ok((active, enabled))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ToolError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn restore_optional(path: &Path, bytes: Option<&[u8]>) -> Result<(), ToolError> {
    match bytes {
        Some(bytes) => atomic_write(path, bytes),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        },
    }
}

fn host_transaction_failure(primary: &ToolError, rollback: Option<&ToolError>) -> ToolError {
    let (sdk_kind, message) = match rollback {
        None => (
            "host_service_install_rolled_back",
            format!(
                "host-service activation failed and the prior unit/service state was restored: {primary}"
            ),
        ),
        Some(rollback) => (
            "host_service_install_rollback_failed",
            format!(
                "host-service activation failed: {primary}; rollback also failed: {rollback}; inspect the system unit and service state before retrying"
            ),
        ),
    };
    ToolError::Sdk {
        sdk_kind: sdk_kind.into(),
        message,
    }
}

pub(crate) async fn status() -> Result<HostServiceStatus, ToolError> {
    let path = unit_path();
    let port = configured_local_port()?;
    let installed = path.is_file();
    let (docker_labby_master_running, docker_labby_master_error) =
        match docker_labby_master_running().await {
            Ok(value) => (value, None),
            Err(err) => (None, Some(err.user_message().to_string())),
        };
    let (ready_response, mut local_ready_error) = match check_ready(port).await {
        Ok(value) => (Some(value), None),
        Err(err) => (None, Some(err)),
    };
    let mut load_state = None;
    let mut active_state = None;
    let mut sub_state = None;
    let mut main_pid = None;

    let exec_main_status = if installed {
        let output = run_systemctl(&[
            "show",
            SERVICE_NAME,
            "--property=LoadState,ActiveState,SubState,MainPID,ExecMainStatus",
            "--no-pager",
        ])
        .await?;
        let props = parse_systemctl_show(&output.stdout);
        load_state = non_empty_prop(&props, "LoadState");
        active_state = non_empty_prop(&props, "ActiveState");
        sub_state = non_empty_prop(&props, "SubState");
        main_pid = parse_main_pid(&props);
        non_empty_prop(&props, "ExecMainStatus").and_then(|value| value.parse().ok())
    } else {
        None
    };

    let process_exe = main_pid.and_then(process_exe);
    let ready_owned_by_service = match ready_response {
        Some(true) => match readiness_owner_matches(main_pid, port).await {
            Ok(value) => Some(value),
            Err(err) => {
                local_ready_error = Some(err.user_message().to_string());
                None
            }
        },
        Some(false) => Some(false),
        None => None,
    };
    let local_ready = match (ready_response, ready_owned_by_service) {
        (Some(true), Some(true)) => Some(true),
        (Some(true), Some(false)) => {
            local_ready_error.get_or_insert_with(|| {
                "ready endpoint responded, but the listener is not labby.service".to_string()
            });
            Some(false)
        }
        (Some(false), _) => Some(false),
        (None, _) | (Some(true), None) => None,
    };

    Ok(HostServiceStatus {
        installed,
        load_state,
        active_state,
        sub_state,
        main_pid,
        exec_main_status,
        unit_path: path,
        process_exe,
        local_ready,
        local_ready_error,
        ready_owned_by_service,
        docker_labby_master_running,
        docker_labby_master_error,
    })
}

pub(crate) async fn installed_and_ready() -> Result<bool, ToolError> {
    let path = unit_path();
    let port = configured_local_port()?;
    if !unit_file_is_current(&path) || !Path::new("/usr/local/bin/labby").is_file() {
        return Ok(false);
    }

    let output = match run_systemctl(&[
        "show",
        SERVICE_NAME,
        "--property=LoadState,ActiveState,MainPID",
        "--no-pager",
    ])
    .await
    {
        Ok(output) => output,
        Err(err) if command_not_found(&err) => return Ok(false),
        Err(err) => return Err(err),
    };
    let props = parse_systemctl_show(&output.stdout);
    if non_empty_prop(&props, "LoadState").as_deref() != Some("loaded")
        || non_empty_prop(&props, "ActiveState").as_deref() != Some("active")
    {
        return Ok(false);
    }
    let Some(main_pid) = parse_main_pid(&props) else {
        return Ok(false);
    };
    if check_ready(port).await.unwrap_or(false) {
        readiness_owner_matches(Some(main_pid), port).await
    } else {
        Ok(false)
    }
}

pub(crate) async fn restart() -> Result<HostServiceOutcome, ToolError> {
    let port = preflight_port_available("restart").await?;
    let path = unit_path();
    provision_oauth_encryption_key_before_restart().await?;
    microsandbox_image::prepare_before_restart().await?;
    let restart = run_systemctl(&["restart", SERVICE_NAME]).await?;
    let mut stderr = restart.stderr;
    if let Err(err) = poll_ready(port).await {
        stderr.push_str(&format!("\nreadiness failed: {err}"));
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "restarted {SERVICE_NAME}, but local readiness did not pass: {err}\nstdout:\n{}\nstderr:\n{stderr}",
                restart.stdout
            ),
        });
    }
    Ok(HostServiceOutcome {
        ok: true,
        changed: true,
        message: format!("{SERVICE_NAME} restarted"),
        unit_path: path,
        stdout: restart.stdout,
        stderr,
    })
}

async fn provision_oauth_encryption_key_before_restart() -> Result<(), ToolError> {
    let env = Path::new("/home/labby/.labby/.env");
    let outcome = super::bootstrap::ensure_oauth_encryption_key_at(env)?;
    if outcome.changed {
        // env_merge's atomic replacement is created by the provisioning user;
        // restore the service account ownership before systemd reads it.
        run_command("chown", &["labby:labby", path_to_str(env)?]).await?;
        if let Some(backup) = outcome.backup_path.as_deref() {
            run_command("chown", &["labby:labby", path_to_str(backup)?]).await?;
        }
        tracing::info!(
            service = "setup",
            action = "oauth_encryption_key.provision",
            changed = true,
            backup_created = outcome.backup_path.is_some(),
            "provisioned OAuth credential encryption key before service restart"
        );
    }
    Ok(())
}

pub(crate) async fn uninstall() -> Result<HostServiceOutcome, ToolError> {
    let path = unit_path();
    let mut stdout = String::new();
    let mut stderr = String::new();
    if path.exists() {
        let disable = run_systemctl(&["disable", "--now", SERVICE_NAME]).await?;
        stdout.push_str(&disable.stdout);
        stderr.push_str(&disable.stderr);
        std::fs::remove_file(&path).map_err(io_error)?;
        let daemon = run_systemctl(&["daemon-reload"]).await?;
        stdout.push_str(&daemon.stdout);
        stderr.push_str(&daemon.stderr);
        Ok(HostServiceOutcome {
            ok: true,
            changed: true,
            message: format!("{SERVICE_NAME} disabled and removed"),
            unit_path: path,
            stdout,
            stderr,
        })
    } else {
        Ok(HostServiceOutcome {
            ok: true,
            changed: false,
            message: format!("{SERVICE_NAME} is not installed"),
            unit_path: path,
            stdout,
            stderr,
        })
    }
}

fn unit_text() -> &'static str {
    r"[Unit]
Description=Labby host gateway
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
User=labby
Group=labby
ExecStart=/usr/local/bin/labby serve
WorkingDirectory=/home/labby
Environment=HOME=/home/labby
Environment=XDG_CACHE_HOME=/home/labby/.cache
Environment=XDG_CONFIG_HOME=/home/labby/.config
Environment=XDG_DATA_HOME=/home/labby/.local/share
Environment=PATH=/home/labby/.local/bin:/usr/local/bin:/usr/bin:/bin
Environment=LABBY_STDIO_SANDBOX_REQUIRED=1
EnvironmentFile=-/home/labby/.labby/.env
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/home/labby/.labby
InaccessiblePaths=-/home/labby/.codex -/home/labby/.claude -/home/labby/.gemini
ProtectHome=read-only
PrivateTmp=true
RestrictNamespaces=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictSUIDSGID=true
LockPersonality=true
ProtectKernelTunables=true
ProtectKernelModules=true
CapabilityBoundingSet=
SystemCallFilter=@system-service
TasksMax=1000
MemoryMax=4G
Restart=on-failure
RestartSec=3
KillSignal=SIGINT

[Install]
WantedBy=multi-user.target
"
}

fn watchdog_service_text() -> &'static str {
    // `${...}` below is systemd/shell syntax, not a missing Rust formatting argument.
    #[allow(clippy::literal_string_with_formatting_args)]
    r#"[Unit]
Description=Labby bounded readiness watchdog
After=labby.service
ConditionPathExists=!/run/labby-maintenance
OnFailure=labby-watchdog-escalation.service

[Service]
Type=oneshot
TimeoutStartSec=60
EnvironmentFile=-/home/labby/.labby/.env
ExecStart=/bin/sh -c 'url="http://127.0.0.1:$${LABBY_MCP_HTTP_PORT:-8765}/ready"; /usr/bin/curl -fsS --max-time 3 "$url" && exit 0; sleep 5; systemctl restart labby.service || exit 1; for delay in 1 2 4 8; do sleep "$delay"; /usr/bin/curl -fsS --max-time 3 "$url" && exit 0; done; systemctl show labby.service --property=Result,NRestarts,ActiveState --no-pager >&2; exit 1'
"#
}

fn watchdog_escalation_text() -> &'static str {
    r#"[Unit]
Description=Persist and escalate repeated Labby watchdog recovery failure

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'install -d -m 0700 /home/labby/.labby/watchdog; { date -u; systemctl show labby.service --property=Result,NRestarts,ActiveState --no-pager; journalctl -u labby.service -n 100 --no-pager; } >>/home/labby/.labby/watchdog/recovery-failures.log; systemd-cat -t labby-watchdog-escalation echo "Labby recovery exhausted; notification owner must alert on this tag"'
"#
}

fn watchdog_timer_text() -> &'static str {
    r"[Unit]
Description=Periodically probe Labby readiness

[Timer]
OnBootSec=2min
OnUnitActiveSec=30s
RandomizedDelaySec=5s
Persistent=false

[Install]
WantedBy=timers.target
"
}

#[cfg(test)]
fn exercise_watchdog<P, R>(
    maintenance: bool,
    delays: &[u64],
    mut probe: P,
    mut restart: R,
) -> (bool, Vec<u64>)
where
    P: FnMut() -> bool,
    R: FnMut() -> bool,
{
    if maintenance || probe() {
        return (true, Vec::new());
    }
    if !restart() {
        return (false, Vec::new());
    }
    let mut used = Vec::new();
    for delay in delays {
        used.push(*delay);
        if probe() {
            return (true, used);
        }
    }
    (false, used)
}

fn unit_file_is_current(path: &Path) -> bool {
    std::fs::read_to_string(path).ok().as_deref() == Some(unit_text())
}

fn unit_dir() -> PathBuf {
    PathBuf::from(SYSTEM_UNIT_DIR)
}

fn unit_path() -> PathBuf {
    unit_dir().join(SERVICE_NAME)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    atomic_write_with_parent_sync(path, bytes, sync_parent_directory)
}

fn sync_parent_directory(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        // Match configuration and durable-state publication: the file is
        // synced before atomic replacement, but we do not claim Unix directory
        // fsync durability on platforms without that operation.
        let _ = dir;
        Ok(())
    }
}

fn atomic_write_with_parent_sync(
    path: &Path,
    bytes: &[u8],
    sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), ToolError> {
    let dir = path.parent().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("cannot determine parent directory for `{}`", path.display()),
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(io_error)?;
    std::io::Write::write_all(&mut temp, bytes).map_err(io_error)?;
    temp.as_file_mut().sync_all().map_err(io_error)?;
    temp.persist(path).map_err(|err| io_error(err.error))?;
    sync_parent(dir).map_err(|error| ToolError::Sdk {
        sdk_kind: "host_service_parent_sync_failed".into(),
        message: format!(
            "persisted `{}`, but failed to sync parent directory `{}`: {error}; verify the file before retrying",
            path.display(),
            dir.display()
        ),
    })?;
    Ok(())
}

async fn append_optional_verify(
    path: &Path,
    stdout: &mut String,
    stderr: &mut String,
) -> Result<(), ToolError> {
    match run_command("systemd-analyze", &["verify", path_to_str(path)?]).await {
        Ok(output) => {
            stdout.push_str(&output.stdout);
            stderr.push_str(&output.stderr);
            Ok(())
        }
        Err(err) if command_not_found(&err) => {
            stderr.push_str("systemd-analyze not found; skipped unit verification\n");
            Ok(())
        }
        Err(err) => Err(err),
    }
}

async fn preflight_port_available(operation: &str) -> Result<u16, ToolError> {
    let docker_running = docker_labby_master_running().await?;
    let port = configured_local_port()?;
    let holder = port_holder(port).await?;
    let (active_state, main_pid) = if holder.is_some() {
        systemctl_service_identity().await?
    } else {
        (None, None)
    };
    if docker_running != Some(true)
        && holder.is_some()
        && active_state.as_deref() == Some("active")
        && main_pid.is_some_and(|pid| process_listens_on_port(pid, port))
    {
        return Ok(port);
    }
    preflight_decision(
        operation,
        port,
        docker_running,
        holder.as_deref(),
        active_state.as_deref(),
        main_pid,
    )?;
    Ok(port)
}

fn preflight_decision(
    operation: &str,
    port: u16,
    docker_running: Option<bool>,
    holder: Option<&str>,
    active_state: Option<&str>,
    main_pid: Option<u32>,
) -> Result<(), ToolError> {
    if docker_running == Some(true) {
        return Err(ToolError::Conflict {
            message: format!(
                "cannot {operation} {SERVICE_NAME}: Docker container `labby-master` is running; stop it before starting the host gateway"
            ),
            existing_id: "labby-master".to_string(),
        });
    }

    if let Some(holder) = holder
        && !holder_can_be_host_service_from(holder, active_state, main_pid)
    {
        return Err(ToolError::Conflict {
            message: format!(
                "cannot {operation} {SERVICE_NAME}: local port {port} is already in use:\n{holder}"
            ),
            existing_id: format!("127.0.0.1:{port}"),
        });
    }
    Ok(())
}

fn configured_local_port() -> Result<u16, ToolError> {
    configured_local_port_at(Path::new(SERVICE_INSTALLATION_ROOT))
}

fn configured_local_port_at(service_root: &Path) -> Result<u16, ToolError> {
    let env_file_port = env_file_value_at(&service_root.join(".env"), "LABBY_MCP_HTTP_PORT")?;
    let config_port = crate::config::load_toml_from_fixed_root(&[service_root.join("config.toml")])
        .map_err(|error| ToolError::InvalidParam {
            message: format!("invalid host-service config: {error:#}"),
            param: "config.toml".into(),
        })?
        .mcp
        .port;
    configured_local_port_from(env_file_port.as_deref(), config_port).map_err(|message| {
        ToolError::InvalidParam {
            message,
            param: "LABBY_MCP_HTTP_PORT".into(),
        }
    })
}

fn configured_local_port_from(
    service_env: Option<&str>,
    config_port: Option<u16>,
) -> Result<u16, String> {
    fn parse_port(value: &str, source: &str) -> Result<u16, String> {
        value
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| {
                format!("{source} LABBY_MCP_HTTP_PORT must be an integer from 1 through 65535")
            })
    }

    if let Some(value) = service_env {
        return parse_port(value, "service environment");
    }
    if let Some(port) = config_port {
        return (port != 0).then_some(port).ok_or_else(|| {
            "config.toml mcp.port must be an integer from 1 through 65535".to_string()
        });
    }
    Ok(8765)
}

fn env_file_value_at(path: &Path, key: &str) -> Result<Option<String>, ToolError> {
    let entries = match dotenvy::from_path_iter(path) {
        Ok(entries) => entries,
        Err(error) if error.not_found() => return Ok(None),
        Err(error) => {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "authoritative service environment `{}` is unreadable: {error}",
                    path.display()
                ),
                param: "LABBY_MCP_HTTP_PORT".into(),
            });
        }
    };
    for entry in entries {
        let (name, value) = entry.map_err(|error| ToolError::InvalidParam {
            message: format!(
                "authoritative service environment `{}` is malformed: {error}",
                path.display()
            ),
            param: "LABBY_MCP_HTTP_PORT".into(),
        })?;
        if name == key {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

async fn port_holder(port: u16) -> Result<Option<String>, ToolError> {
    match run_command("ss", &["-ltnp", &format!("sport = :{port}")]).await {
        Ok(output) => {
            let lines = output
                .stdout
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.starts_with("State"))
                .collect::<Vec<_>>();
            if lines.is_empty() {
                Ok(None)
            } else {
                Ok(Some(lines.join("\n")))
            }
        }
        Err(err) if command_not_found(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

async fn readiness_owner_matches(main_pid: Option<u32>, port: u16) -> Result<bool, ToolError> {
    let Some(pid) = main_pid else {
        return Ok(false);
    };
    let Some(holder) = port_holder(port).await? else {
        return Ok(false);
    };
    Ok(holder_contains_pid(&holder, pid)
        || process_listens_on_port(pid, port)
        || (!holder_contains_pid_data(&holder) && process_cmdline_is_labby(pid)))
}

fn holder_can_be_host_service_from(
    holder: &str,
    active_state: Option<&str>,
    main_pid: Option<u32>,
) -> bool {
    active_state == Some("active")
        && main_pid.is_some_and(|pid| {
            holder_contains_pid(holder, pid)
                || (!holder_contains_pid_data(holder) && process_cmdline_is_labby(pid))
        })
}

#[cfg(test)]
fn readiness_owner_matches_from(main_pid: Option<u32>, holder: Option<&str>) -> bool {
    let Some(pid) = main_pid else {
        return false;
    };
    holder.is_some_and(|holder| holder_contains_pid(holder, pid))
}

fn holder_contains_pid(holder: &str, pid: u32) -> bool {
    let needle = format!("pid={pid}");
    holder.match_indices(&needle).any(|(start, _)| {
        holder[start + needle.len()..]
            .chars()
            .next()
            .is_none_or(|next| !next.is_ascii_digit())
    })
}

fn holder_contains_pid_data(holder: &str) -> bool {
    holder.contains("pid=")
}

fn process_cmdline_is_labby(pid: u32) -> bool {
    let path = format!("/proc/{pid}/cmdline");
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let mut parts = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    let Some(program) = parts.next() else {
        return false;
    };
    program.ends_with(b"/labby") || program == b"labby"
}

fn process_listens_on_port(pid: u32, port: u16) -> bool {
    let inodes = listener_socket_inodes(port);
    !inodes.is_empty() && process_has_socket_inode(pid, &inodes)
}

fn listener_socket_inodes(port: u16) -> BTreeSet<String> {
    let mut inodes = BTreeSet::new();
    for inode in listener_socket_entries(port) {
        inodes.insert(inode);
    }
    inodes
}

fn listener_socket_entries(port: u16) -> Vec<String> {
    let mut entries = Vec::new();
    collect_listener_socket_entries("/proc/net/tcp", port, &mut entries);
    collect_listener_socket_entries("/proc/net/tcp6", port, &mut entries);
    entries
}

fn collect_listener_socket_entries(path: &str, port: u16, entries: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() <= 9 || fields[3] != "0A" {
            continue;
        }
        let Some((_, port_hex)) = fields[1].rsplit_once(':') else {
            continue;
        };
        if u16::from_str_radix(port_hex, 16).ok() == Some(port) {
            entries.push(fields[9].to_string());
        }
    }
}

fn process_has_socket_inode(pid: u32, inodes: &BTreeSet<String>) -> bool {
    let fd_dir = format!("/proc/{pid}/fd");
    let Ok(entries) = std::fs::read_dir(fd_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Ok(target) = std::fs::read_link(entry.path()) else {
            return false;
        };
        let Some(target) = target.to_str() else {
            return false;
        };
        let Some(inode) = target
            .strip_prefix("socket:[")
            .and_then(|rest| rest.strip_suffix(']'))
        else {
            return false;
        };
        inodes.contains(inode)
    })
}

async fn docker_labby_master_running() -> Result<Option<bool>, ToolError> {
    match run_command(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", "labby-master"],
    )
    .await
    {
        Ok(output) => Ok(Some(output.stdout.trim() == "true")),
        Err(err) if command_not_found(&err) => Ok(None),
        Err(err) if docker_container_missing(&err) => Ok(Some(false)),
        Err(err) => Err(err),
    }
}

async fn poll_ready(port: u16) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut last_err = String::new();
    while tokio::time::Instant::now() < deadline {
        match check_ready(port).await {
            Ok(true) => match systemctl_service_identity().await {
                Ok((active_state, main_pid)) if active_state.as_deref() == Some("active") => {
                    match readiness_owner_matches(main_pid, port).await {
                        Ok(true) => return Ok(()),
                        Ok(false) => {
                            last_err =
                                "ready endpoint responded, but the listener is not labby.service"
                                    .to_string();
                        }
                        Err(err) => last_err = err.to_string(),
                    }
                }
                Ok((active_state, _)) => {
                    last_err = format!(
                        "ready endpoint responded, but {SERVICE_NAME} is not active ({})",
                        active_state.unwrap_or_else(|| "unknown".to_string())
                    );
                }
                Err(err) => last_err = err.to_string(),
            },
            Ok(false) => last_err = "ready endpoint returned non-success".to_string(),
            Err(err) => last_err = err,
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
    Err(last_err)
}

async fn systemctl_service_identity() -> Result<(Option<String>, Option<u32>), ToolError> {
    let output = run_systemctl(&[
        "show",
        SERVICE_NAME,
        "--property=ActiveState,MainPID",
        "--no-pager",
    ])
    .await?;
    let props = parse_systemctl_show(&output.stdout);
    Ok((
        non_empty_prop(&props, "ActiveState"),
        parse_main_pid(&props),
    ))
}

async fn check_ready(port: u16) -> Result<bool, String> {
    // See api/state.rs::build_protected_mcp_http_client for why this call is
    // needed under "rustls-no-provider" -- idempotent, safe to ignore Err.
    drop(rustls::crypto::ring::default_provider().install_default());
    let url = format!("http://127.0.0.1:{port}/ready");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    Ok(response.status().is_success())
}

fn process_exe(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

fn parse_systemctl_show(stdout: &str) -> BTreeMap<String, String> {
    stdout
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn non_empty_prop(props: &BTreeMap<String, String>, key: &str) -> Option<String> {
    props
        .get(key)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_main_pid(props: &BTreeMap<String, String>) -> Option<u32> {
    non_empty_prop(props, "MainPID").and_then(|value| {
        let pid = value.parse::<u32>().ok()?;
        (pid != 0).then_some(pid)
    })
}

async fn run_systemctl(args: &[&str]) -> Result<CommandCapture, ToolError> {
    run_command("systemctl", args).await
}

async fn run_command(program: &str, args: &[&str]) -> Result<CommandCapture, ToolError> {
    let command_display = command_display(program, args);
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("failed to run `{command_display}`: {err}"),
    })?;
    let stdout = tokio::spawn(read_capped(child.stdout.take()));
    let stderr = tokio::spawn(read_capped(child.stderr.take()));
    let status = tokio::time::timeout(COMMAND_TIMEOUT, child.wait())
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("command timed out after {COMMAND_TIMEOUT:?}: {command_display}"),
        })?
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to wait for `{command_display}`: {err}"),
        })?;
    let stdout = stdout
        .await
        .unwrap_or_else(|err| format!("failed to join command output reader: {err}"));
    let stderr = stderr
        .await
        .unwrap_or_else(|err| format!("failed to join command output reader: {err}"));
    let captured = CommandCapture {
        status,
        stdout,
        stderr,
    };
    if captured.status.success() {
        Ok(captured)
    } else {
        let stdout = redact_command_output(&captured.stdout);
        let stderr = redact_command_output(&captured.stderr);
        Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "command failed: {command_display}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
                captured.status, stdout, stderr
            ),
        })
    }
}

async fn read_capped<R>(reader: Option<R>) -> String
where
    R: AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return String::new();
    };
    let mut captured = Vec::new();
    let mut truncated = false;
    let mut chunk = [0; 1024];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                captured.extend_from_slice(&chunk[..n]);
                if captured.len() > CAPTURE_BYTES {
                    let excess = captured.len() - CAPTURE_BYTES;
                    captured.drain(..excess);
                    truncated = true;
                }
            }
            Err(err) => return format!("failed to read command output: {err}"),
        }
    }
    let mut text = String::from_utf8_lossy(&captured).to_string();
    if truncated {
        text.insert_str(0, "...[truncated]\n");
    }
    text
}

fn command_display(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_command_output(output: &str) -> String {
    const MAX_LINES: usize = 40;
    const MAX_BYTES: usize = 4096;
    let joined = output
        .lines()
        .rev()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let capped = if joined.len() > MAX_BYTES {
        let mut cut = MAX_BYTES;
        while cut > 0 && !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}...[truncated]", &joined[..cut])
    } else {
        joined
    };
    labby_runtime::redact::redact_stdio_value(&capped)
        .lines()
        .map(|line| {
            if let Some((prefix, _)) = line.split_once("Authorization: Bearer ") {
                format!("{prefix}Authorization: Bearer [redacted]")
            } else if line.contains("TS_AUTHKEY=") {
                "TS_AUTHKEY=[redacted]".to_string()
            } else {
                // Canonical helper; the retired local copy's `tskey-` prefix
                // was folded into it, so Tailscale keys stay covered (the
                // shared marker is `[REDACTED]` rather than `[redacted]`).
                labby_runtime::redact::redact_secret_like_segments(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn path_to_str(path: &Path) -> Result<&str, ToolError> {
    path.to_str().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("path is not valid UTF-8: `{}`", path.display()),
    })
}

fn command_not_found(err: &ToolError) -> bool {
    let message = err.to_string();
    message.contains("failed to run `")
        && (message.contains("No such file or directory") || message.contains("os error 2"))
}

fn docker_container_missing(err: &ToolError) -> bool {
    let message = err.to_string();
    let message = message.to_lowercase();
    let has_container_id = message.contains("labby-master");
    let has_not_found = message.contains("no such object")
        || message.contains("no such container")
        || message.contains("not found");
    has_container_id && has_not_found
}

fn io_error(err: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_retention_publication_preserves_previous_recovery_pair() {
        for existing in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("previous");
            let old_manifest = b"active=inactive\nenabled=disabled\n";
            if existing {
                std::fs::create_dir(&root).unwrap();
                std::fs::write(root.join("labby"), b"older retained binary").unwrap();
                std::fs::write(root.join("manifest"), old_manifest).unwrap();
            }
            let error = persist_previous_host_release_with_checkpoint(
                &root,
                Some(b"newly retained binary"),
                Some((CapturedActiveState::Active, CapturedUnitFileState::Enabled)),
                || {
                    assert_eq!(
                        std::fs::read(root.join("labby")).unwrap(),
                        b"newly retained binary"
                    );
                    Err(io_error(std::io::Error::other(
                        "injected manifest publication failure",
                    )))
                },
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("injected manifest publication failure")
            );
            if existing {
                assert_eq!(
                    std::fs::read(root.join("labby")).unwrap(),
                    b"older retained binary"
                );
                assert_eq!(std::fs::read(root.join("manifest")).unwrap(), old_manifest);
            } else {
                assert!(!root.join("labby").exists());
                assert!(!root.join("manifest").exists());
            }
        }
    }

    #[tokio::test]
    async fn self_install_rolls_back_activation_when_previous_release_retention_fails() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("candidate");
        let destination = dir.path().join("labby");
        let unit = dir.path().join("labby.service");
        let retained = dir.path().join("previous");
        std::fs::write(&source, b"candidate binary").unwrap();
        std::fs::write(&destination, b"prior binary").unwrap();
        std::fs::write(&unit, b"prior service").unwrap();
        // A real filesystem failure after activation, without global hooks or
        // touching the machine's service manager.
        std::fs::write(&retained, b"not a directory").unwrap();
        let error = run_self_install_transaction(
            &destination,
            Some(b"prior binary"),
            async {
                install_executable(&source, &destination)?;
                std::fs::write(&unit, b"candidate service").map_err(io_error)?;
                persist_previous_host_release_at(
                    &retained,
                    Some(b"prior binary"),
                    Some((CapturedActiveState::Active, CapturedUnitFileState::Enabled)),
                )?;
                panic!("retention must fail");
            },
            || async {
                assert_eq!(std::fs::read(&destination).unwrap(), b"prior binary");
                assert_eq!(std::fs::read(&unit).unwrap(), b"candidate service");
                std::fs::write(&unit, b"prior service").map_err(io_error)
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "host_service_upgrade_rolled_back");
        assert_eq!(std::fs::read(&destination).unwrap(), b"prior binary");
        assert_eq!(std::fs::read(&unit).unwrap(), b"prior service");
    }

    #[tokio::test]
    async fn self_install_rolls_back_binary_when_permissions_fail_after_replacement() {
        for prior in [Some(b"prior binary".as_slice()), None] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("candidate");
            let destination = dir.path().join("labby");
            std::fs::write(&source, b"candidate binary").unwrap();
            if let Some(prior) = prior {
                std::fs::write(&destination, prior).unwrap();
            }
            let restored = std::cell::Cell::new(false);
            let error = run_self_install_transaction(
                &destination,
                prior,
                async {
                    install_executable_with_permissions(&source, &destination, |path| {
                        assert_eq!(std::fs::read(path).unwrap(), b"candidate binary");
                        Err(io_error(std::io::Error::other("injected chmod failure")))
                    })?;
                    panic!("activation must not follow failed executable installation");
                },
                || async {
                    assert_eq!(read_optional(&destination).unwrap().as_deref(), prior);
                    restored.set(true);
                    Ok(())
                },
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind(), "host_service_upgrade_rolled_back");
            assert!(error.to_string().contains("injected chmod failure"));
            assert!(restored.get());
            assert_eq!(read_optional(&destination).unwrap().as_deref(), prior);
        }
    }

    #[tokio::test]
    async fn self_install_reports_service_rollback_failure_after_restoring_binary() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("labby");
        let error = run_self_install_transaction(
            &destination,
            Some(b"prior binary"),
            async {
                Err(io_error(std::io::Error::other(
                    "injected activation failure",
                )))
            },
            || async {
                assert_eq!(std::fs::read(&destination).unwrap(), b"prior binary");
                Err(io_error(std::io::Error::other(
                    "injected service rollback failure",
                )))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "host_service_upgrade_rollback_failed");
        assert!(error.to_string().contains("injected activation failure"));
        assert!(
            error
                .to_string()
                .contains("injected service rollback failure")
        );
    }

    #[test]
    fn atomic_write_surfaces_parent_directory_sync_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("labby.service");
        let error = atomic_write_with_parent_sync(&path, b"new unit", |_| {
            Err(std::io::Error::other("injected directory fsync failure"))
        })
        .unwrap_err();

        assert_eq!(error.kind(), "host_service_parent_sync_failed");
        assert!(
            error
                .to_string()
                .contains("injected directory fsync failure")
        );
        assert!(
            error
                .to_string()
                .contains("verify the file before retrying")
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new unit");
    }

    #[test]
    fn transaction_failure_distinguishes_successful_and_failed_rollback() {
        let primary = ToolError::Sdk {
            sdk_kind: "probe_failed".into(),
            message: "readiness timed out".into(),
        };
        assert_eq!(
            host_transaction_failure(&primary, None).kind(),
            "host_service_install_rolled_back"
        );
        let rollback = ToolError::Sdk {
            sdk_kind: "restore_failed".into(),
            message: "unit restore denied".into(),
        };
        let compound = host_transaction_failure(&primary, Some(&rollback));
        assert_eq!(compound.kind(), "host_service_install_rollback_failed");
        assert!(compound.to_string().contains("readiness timed out"));
        assert!(compound.to_string().contains("unit restore denied"));
    }

    #[test]
    fn opt_in_watchdog_is_bounded_and_maintenance_aware() {
        let service = watchdog_service_text();
        assert!(service.contains("ConditionPathExists=!/run/labby-maintenance"));
        assert!(service.contains("TimeoutStartSec=60"));
        assert!(service.contains("--max-time 3"));
        assert!(service.contains("restart labby.service"));
        assert!(service.contains("Result,NRestarts,ActiveState"));
        assert!(service.contains("OnFailure=labby-watchdog-escalation.service"));
        assert!(watchdog_escalation_text().contains("recovery-failures.log"));
        assert!(watchdog_escalation_text().contains("systemd-cat -t labby-watchdog-escalation"));
        let timer = watchdog_timer_text();
        assert!(timer.contains("OnUnitActiveSec=30s"));
        assert!(unit_text().contains("StartLimitBurst=5"));
    }

    #[test]
    fn watchdog_behavior_covers_success_hang_exhaustion_backoff_and_maintenance() {
        let (ok, delays) = exercise_watchdog(false, &[1, 2, 4, 8], || true, || panic!());
        assert!(ok);
        assert!(delays.is_empty());

        let mut probes = 0;
        let (ok, delays) = exercise_watchdog(
            false,
            &[1, 2, 4, 8],
            || {
                probes += 1;
                probes == 4
            },
            || true,
        );
        assert!(ok);
        assert_eq!(delays, vec![1, 2, 4]);

        let (ok, delays) = exercise_watchdog(false, &[1, 2, 4, 8], || false, || true);
        assert!(!ok);
        assert_eq!(delays, vec![1, 2, 4, 8]);

        let (ok, _) = exercise_watchdog(false, &[1, 2, 4, 8], || false, || false);
        assert!(
            !ok,
            "restart/start-limit exhaustion must fail synchronously"
        );
        let (ok, delays) = exercise_watchdog(true, &[1], || panic!(), || panic!());
        assert!(ok);
        assert!(delays.is_empty());

        for _ in 0..5 {
            assert!(!exercise_watchdog(false, &[1], || false, || false).0);
        }
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::literal_string_with_formatting_args)]
    fn generated_watchdog_shell_executes_restart_backoff_and_exhaustion() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = root.path().join("calls");
        let count = root.path().join("count");
        let hung_pid = root.path().join("hung.pid");
        let write_exe = |name: &str, body: &str| {
            let path = bin.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let curl = write_exe(
            "curl",
            "#!/bin/sh\ncase \" $* \" in *' --max-time 3 '*) ;; *) exit 64;; esac\nif [ \"${HANG:-0}\" = 1 ]; then /bin/sleep 30 & child=$!; echo $child >\"$HUNG_PID\"; (/bin/sleep 0.05; kill $child 2>/dev/null || :) & timer=$!; wait $child 2>/dev/null || :; wait $timer; echo curl:bounded-timeout >>\"$CALLS\"; exit 28; fi\nn=$(cat \"$COUNT\" 2>/dev/null || echo 0); n=$((n+1)); echo $n >\"$COUNT\"; echo curl >>\"$CALLS\"; test $n -ge ${SUCCEED_AT:-999}\n",
        );
        let systemctl = write_exe(
            "systemctl",
            "#!/bin/sh\necho systemctl:$* >>\"$CALLS\"; test \"${RESTART_OK:-1}\" = 1\n",
        );
        let sleep = write_exe("sleep", "#!/bin/sh\necho sleep:$1 >>\"$CALLS\"\n");
        let line = watchdog_service_text()
            .lines()
            .find_map(|line| {
                line.strip_prefix("ExecStart=/bin/sh -c '")
                    .and_then(|v| v.strip_suffix('\''))
            })
            .unwrap();
        let script = line
            .replace("/usr/bin/curl", curl.to_str().unwrap())
            .replace("systemctl ", &format!("{} ", systemctl.display()))
            .replace("sleep ", &format!("{} ", sleep.display()))
            .replace("$$", "$");
        let run = |succeed_at: &str, hang: bool| {
            drop(std::fs::remove_file(&log));
            drop(std::fs::remove_file(&count));
            std::process::Command::new("sh")
                .args(["-c", &script])
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("CALLS", &log)
                .env("COUNT", &count)
                .env("SUCCEED_AT", succeed_at)
                .env("HANG", if hang { "1" } else { "0" })
                .env("HUNG_PID", &hung_pid)
                .status()
                .unwrap()
        };
        assert!(run("1", false).success());
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(!calls.contains("systemctl:restart"));
        assert!(run("3", false).success());
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("systemctl:restart labby.service"));
        assert!(calls.contains("sleep:1"));
        let started = std::time::Instant::now();
        assert!(!run("999", true).success());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "emulated watchdog timeout was not bounded"
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("curl:bounded-timeout"));
        let pid = std::fs::read_to_string(&hung_pid).unwrap();
        let status = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap();
        assert!(!status.success(), "hung curl child survived its timeout");
        for delay in ["sleep:1", "sleep:2", "sleep:4", "sleep:8"] {
            assert!(calls.contains(delay), "missing {delay}: {calls}");
        }
    }

    #[test]
    fn optional_unit_restore_is_exact_and_removes_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watchdog.timer");
        restore_optional(&path, Some(b"prior\n")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"prior\n");
        restore_optional(&path, None).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn directory_snapshot_restores_exact_files_and_removes_transaction_additions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dropins");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("prior.conf"), b"prior\n").unwrap();
        let snapshot = DirectorySnapshot::capture(&path).unwrap();
        std::fs::write(path.join("prior.conf"), b"candidate\n").unwrap();
        std::fs::write(path.join("new.conf"), b"new\n").unwrap();
        snapshot.restore(&path).unwrap();
        assert_eq!(std::fs::read(path.join("prior.conf")).unwrap(), b"prior\n");
        assert!(!path.join("new.conf").exists());
    }

    #[test]
    fn rollback_backup_cleanup_preserves_prior_and_removes_new_backups() {
        let root = tempfile::tempdir().unwrap();
        let env = root.path().join(".env");
        let prior = root.path().join(".env.bak.prior");
        let created = root.path().join(".env.bak.created");
        std::fs::write(&prior, b"prior").unwrap();
        let before = matching_sibling_files(&env, ".env.bak.").unwrap();
        std::fs::write(&created, b"created").unwrap();
        remove_new_matching_siblings(&env, ".env.bak.", &before).unwrap();
        assert!(prior.exists());
        assert!(!created.exists());
    }

    #[test]
    fn every_host_activation_boundary_can_be_fault_injected() {
        for label in [
            "unit-write",
            "unit-verify",
            "daemon-reload",
            "enable",
            "watchdog-units",
            "watchdog-reload",
            "watchdog-enable",
            "credential-provision",
            "sandbox-prepare",
            "restart",
            "readiness",
        ] {
            assert_eq!(
                host_checkpoint_from(Some(label), label).unwrap_err().kind(),
                "host_service_injected_failure"
            );
        }
        assert!(host_checkpoint_from(None, "restart").is_ok());
    }

    #[test]
    fn capture_requires_both_systemd_authorities() {
        assert_eq!(
            parse_captured_systemd_state("ActiveState=active\nUnitFileState=enabled\n", "x")
                .unwrap(),
            (CapturedActiveState::Active, CapturedUnitFileState::Enabled)
        );
        assert_eq!(
            parse_captured_systemd_state("ActiveState=inactive\nUnitFileState=disabled\n", "x")
                .unwrap(),
            (
                CapturedActiveState::Inactive,
                CapturedUnitFileState::Disabled
            )
        );
        assert_eq!(
            parse_captured_systemd_state(
                "ActiveState=failed\nUnitFileState=enabled-runtime\n",
                "x"
            )
            .unwrap(),
            (
                CapturedActiveState::Failed,
                CapturedUnitFileState::EnabledRuntime
            )
        );
        assert_eq!(
            parse_captured_systemd_state("ActiveState=active\n", "x")
                .unwrap_err()
                .kind(),
            "host_service_state_capture_failed"
        );
    }

    #[test]
    fn retained_host_release_manifest_preserves_exact_systemd_states() {
        for (active, enabled) in [
            (CapturedActiveState::Active, CapturedUnitFileState::Enabled),
            (
                CapturedActiveState::Inactive,
                CapturedUnitFileState::EnabledRuntime,
            ),
            (CapturedActiveState::Failed, CapturedUnitFileState::Disabled),
        ] {
            let text = format!("active={}\nenabled={}\n", active.as_str(), enabled.as_str());
            assert_eq!(
                parse_previous_host_manifest(&text).unwrap(),
                (active, enabled)
            );
        }
    }

    #[test]
    fn executable_restore_reinstates_prior_bytes_or_absence() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("labby");
        restore_executable(&destination, Some(b"previous")).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous");
        restore_executable(&destination, None).unwrap();
        assert!(!destination.exists());
    }

    #[test]
    fn rollback_aggregates_every_independent_failure() {
        let error = rollback_result(vec![
            "main unit: denied".into(),
            "daemon reload: timeout".into(),
            "watchdog activity: exhausted".into(),
        ])
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("main unit: denied"));
        assert!(message.contains("daemon reload: timeout"));
        assert!(message.contains("watchdog activity: exhausted"));
    }

    #[test]
    fn unit_uses_hardened_system_binary_and_lab_env() {
        let unit = unit_text();

        assert!(unit.contains("Description=Labby host gateway"));
        assert!(unit.contains("User=labby"));
        assert!(unit.contains("Group=labby"));
        assert!(unit.contains("ExecStart=/usr/local/bin/labby serve"));
        assert!(unit.contains("WorkingDirectory=/home/labby"));
        assert!(unit.contains("Environment=HOME=/home/labby"));
        assert!(
            unit.contains("Environment=PATH=/home/labby/.local/bin:/usr/local/bin:/usr/bin:/bin")
        );
        assert!(unit.contains("EnvironmentFile=-/home/labby/.labby/.env"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert!(!unit.contains("%h"));
    }

    #[test]
    fn unit_does_not_hard_code_public_bind_or_port() {
        let unit = unit_text();

        assert!(!unit.contains("--host 0.0.0.0"));
        assert!(!unit.contains("--port 8765"));
    }

    #[test]
    fn authoritative_service_dotenv_uses_dotenv_parser_and_reports_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "LABBY_MCP_HTTP_PORT='9123'\n").unwrap();
        assert_eq!(
            env_file_value_at(&path, "LABBY_MCP_HTTP_PORT")
                .unwrap()
                .as_deref(),
            Some("9123")
        );
        std::fs::write(&path, "MALFORMED LINE\n").unwrap();
        assert!(env_file_value_at(&path, "LABBY_MCP_HTTP_PORT").is_err());
    }

    #[test]
    fn daemon_port_uses_one_fixed_service_root_for_dotenv_and_toml() {
        let service = tempfile::tempdir().unwrap();
        std::fs::write(service.path().join("config.toml"), "[mcp]\nport = 9001\n").unwrap();
        assert_eq!(configured_local_port_at(service.path()).unwrap(), 9001);

        std::fs::write(service.path().join(".env"), "LABBY_MCP_HTTP_PORT=9002\n").unwrap();
        assert_eq!(configured_local_port_at(service.path()).unwrap(), 9002);
    }

    #[test]
    fn unit_contains_restart_limit_settings() {
        let unit = unit_text();

        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("RestartSec=3"));
        assert!(unit.contains("StartLimitIntervalSec=60"));
        assert!(unit.contains("StartLimitBurst=5"));
        assert!(unit.contains("KillSignal=SIGINT"));
    }

    #[test]
    fn unit_contains_hardening_baseline() {
        let unit = unit_text();

        for directive in [
            "NoNewPrivileges=true",
            "ProtectSystem=strict",
            "ProtectHome=read-only",
            "PrivateTmp=true",
            "RestrictNamespaces=true",
            "RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX",
            "RestrictSUIDSGID=true",
            "LockPersonality=true",
            "ProtectKernelTunables=true",
            "ProtectKernelModules=true",
            "CapabilityBoundingSet=",
            "SystemCallFilter=@system-service",
            "TasksMax=1000",
            "MemoryMax=4G",
        ] {
            assert!(unit.contains(directive), "missing {directive}");
        }
    }

    #[test]
    fn unit_denies_agent_credential_stores_and_limits_writable_state() {
        let unit = unit_text();
        let writable = unit
            .lines()
            .find(|line| line.starts_with("ReadWritePaths="))
            .expect("writable-path policy");
        for forbidden in [".codex", ".claude", ".gemini", ".config", ".npm"] {
            assert!(
                !writable.contains(forbidden),
                "agent or broad config path remained writable: {forbidden}"
            );
        }
        assert_eq!(writable, "ReadWritePaths=/home/labby/.labby");
        assert!(unit.contains(
            "InaccessiblePaths=-/home/labby/.codex -/home/labby/.claude -/home/labby/.gemini"
        ));
        assert!(unit.contains("Environment=LABBY_STDIO_SANDBOX_REQUIRED=1"));
    }

    #[test]
    fn unit_path_lives_under_systemd_system_dir() {
        assert_eq!(
            unit_path(),
            PathBuf::from("/etc/systemd/system/labby.service")
        );
    }

    #[test]
    fn parses_systemctl_show_properties() {
        let props = parse_systemctl_show(
            "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=123\n",
        );

        assert_eq!(
            non_empty_prop(&props, "ActiveState").as_deref(),
            Some("active")
        );
        assert_eq!(non_empty_prop(&props, "MainPID").as_deref(), Some("123"));
        assert_eq!(parse_main_pid(&props), Some(123));
    }

    #[test]
    fn lifecycle_port_uses_only_persisted_daemon_sources() {
        assert_eq!(
            configured_local_port_from(Some("9876"), Some(7777)),
            Ok(9876)
        );
        assert_eq!(configured_local_port_from(None, Some(7777)), Ok(7777));
        assert_eq!(configured_local_port_from(None, None), Ok(8765));
    }

    #[test]
    fn conflicting_invoking_process_port_cannot_change_daemon_default() {
        const CHILD: &str = "LABBY_TEST_HOST_PORT_CHILD";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(std::env::var("LABBY_MCP_HTTP_PORT").unwrap(), "65534");
            assert_eq!(configured_local_port_from(None, None), Ok(8765));
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dispatch::setup::host_service::tests::conflicting_invoking_process_port_cannot_change_daemon_default",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("LABBY_MCP_HTTP_PORT", "65534")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn invalid_higher_precedence_ports_fail_closed() {
        assert_eq!(
            configured_local_port_from(Some("bad"), Some(7777)),
            Err(
                "service environment LABBY_MCP_HTTP_PORT must be an integer from 1 through 65535"
                    .to_string()
            )
        );
        assert_eq!(
            configured_local_port_from(Some("0"), None),
            Err(
                "service environment LABBY_MCP_HTTP_PORT must be an integer from 1 through 65535"
                    .to_string()
            )
        );
    }

    #[test]
    fn detects_port_holder_pid() {
        let holder = r#"LISTEN 0 4096 127.0.0.1:8765 0.0.0.0:* users:(("labby",pid=12345,fd=17))"#;

        assert!(holder_contains_pid(holder, 12345));
        assert!(!holder_contains_pid(holder, 1234));
        assert!(holder_contains_pid_data(holder));
    }

    #[test]
    fn detects_redacted_port_holder_without_pid_data() {
        let holder = "LISTEN 0 128 127.0.0.1:8765 0.0.0.0:*";

        assert!(!holder_contains_pid_data(holder));
    }

    #[test]
    fn preflight_blocks_docker_labby_master() {
        let err = preflight_decision("install", 8765, Some(true), None, None, None).unwrap_err();

        assert_eq!(err.kind(), "conflict");
        assert!(err.to_string().contains("labby-master"));
    }

    #[test]
    fn preflight_blocks_foreign_port_holder() {
        let holder = r#"LISTEN 0 4096 127.0.0.1:8765 0.0.0.0:* users:(("other",pid=54321,fd=17))"#;
        let err = preflight_decision(
            "restart",
            8765,
            Some(false),
            Some(holder),
            Some("active"),
            Some(12345),
        )
        .unwrap_err();

        assert_eq!(err.kind(), "conflict");
        assert!(
            err.to_string()
                .contains("local port 8765 is already in use")
        );
    }

    #[test]
    fn preflight_allows_active_service_pid_holder() {
        let holder = r#"LISTEN 0 4096 127.0.0.1:8765 0.0.0.0:* users:(("labby",pid=12345,fd=17))"#;

        preflight_decision(
            "restart",
            8765,
            Some(false),
            Some(holder),
            Some("active"),
            Some(12345),
        )
        .unwrap();
    }

    #[test]
    fn docker_container_missing_matches_lowercase_and_uppercase_no_object() {
        let upper_case = ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message:
                "command failed: docker inspect ...\nstderr: Error: No such object: labby-master"
                    .into(),
        };
        let lower_case = ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: "command failed: docker inspect ...\nstderr: no such object: labby-master"
                .into(),
        };
        let other = ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: "command failed: docker inspect ...\nstderr: Error: No such container".into(),
        };

        assert!(docker_container_missing(&upper_case));
        assert!(docker_container_missing(&lower_case));
        assert!(!docker_container_missing(&other));
    }

    #[test]
    fn readiness_from_non_service_pid_is_not_owned() {
        let holder = r#"LISTEN 0 4096 127.0.0.1:8765 0.0.0.0:* users:(("other",pid=54321,fd=17))"#;

        assert!(!readiness_owner_matches_from(Some(12345), Some(holder)));
        assert!(readiness_owner_matches_from(Some(54321), Some(holder)));
        assert!(!readiness_owner_matches_from(None, Some(holder)));
    }
}
