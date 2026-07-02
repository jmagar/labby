//! CLI-only management helpers for the system `labby.service`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::dispatch::error::ToolError;

const SERVICE_NAME: &str = "labby.service";
const LABBY_HOME: &str = "/home/labby";
const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(300);
const CAPTURE_BYTES: usize = 16 * 1024;

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

pub(crate) async fn unit() -> Result<String, ToolError> {
    Ok(unit_text().to_string())
}

pub(crate) async fn install() -> Result<HostServiceOutcome, ToolError> {
    let port = preflight_port_available("install").await?;
    let path = unit_path();
    let text = unit_text();
    std::fs::create_dir_all(unit_dir()).map_err(io_error)?;
    let changed = std::fs::read_to_string(&path).ok().as_deref() != Some(text);
    if changed {
        atomic_write(&path, text.as_bytes())?;
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    append_optional_verify(&path, &mut stdout, &mut stderr).await?;
    let daemon = run_systemctl(&["daemon-reload"]).await?;
    stdout.push_str(&daemon.stdout);
    stderr.push_str(&daemon.stderr);
    let enable = run_systemctl(&["enable", SERVICE_NAME]).await?;
    stdout.push_str(&enable.stdout);
    stderr.push_str(&enable.stderr);
    let restart = run_systemctl(&["restart", SERVICE_NAME]).await?;
    stdout.push_str(&restart.stdout);
    stderr.push_str(&restart.stderr);
    if let Err(err) = poll_ready(port).await {
        stderr.push_str(&format!("\nreadiness failed: {err}"));
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "installed {SERVICE_NAME}, but local readiness did not pass: {err}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            ),
        });
    }
    Ok(HostServiceOutcome {
        ok: true,
        changed,
        message: format!("{SERVICE_NAME} installed and running"),
        unit_path: path,
        stdout,
        stderr,
    })
}

pub(crate) async fn status() -> Result<HostServiceStatus, ToolError> {
    let path = unit_path();
    let port = configured_local_port();
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
    let port = configured_local_port();
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
EnvironmentFile=-/home/labby/.labby/.env
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/home/labby/.labby /home/labby/.local /home/labby/.cache /home/labby/.config /home/labby/.npm /home/labby/.codex /home/labby/.claude /home/labby/.gemini /home/labby/downloads
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
    let dir = path.parent().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("cannot determine parent directory for `{}`", path.display()),
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(io_error)?;
    std::io::Write::write_all(&mut temp, bytes).map_err(io_error)?;
    temp.as_file_mut().sync_all().map_err(io_error)?;
    temp.persist(path).map_err(|err| io_error(err.error))?;
    if let Ok(dir_file) = std::fs::File::open(dir) {
        drop(dir_file.sync_all());
    }
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
    let port = configured_local_port();
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

fn configured_local_port() -> u16 {
    let env_file_port = env_file_value("LAB_MCP_HTTP_PORT");
    let config_port = crate::config::load_toml(&crate::config::toml_candidates())
        .ok()
        .and_then(|config| config.mcp.port);
    let process_port = std::env::var("LAB_MCP_HTTP_PORT").ok();
    configured_local_port_from(
        env_file_port.as_deref(),
        config_port,
        process_port.as_deref(),
    )
}

fn configured_local_port_from(
    service_env: Option<&str>,
    config_port: Option<u16>,
    process_env: Option<&str>,
) -> u16 {
    service_env
        .and_then(|value| value.parse().ok())
        .or(config_port)
        .or_else(|| process_env.and_then(|value| value.parse().ok()))
        .unwrap_or(8765)
}

fn env_file_value(key: &str) -> Option<String> {
    let path = Path::new(LABBY_HOME).join(".labby/.env");
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
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
                redact_secret_like_segments(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn redact_secret_like_segments(input: &str) -> String {
    input
        .split_whitespace()
        .map(|segment| {
            let looks_secret = segment.starts_with("sk-")
                || segment.starts_with("ghp_")
                || segment.starts_with("github_pat_")
                || segment.starts_with("glpat-")
                || segment.starts_with("xoxb-")
                || segment.starts_with("xoxp-")
                || segment.starts_with("tskey-")
                || segment.starts_with("eyJ");
            if looks_secret {
                "[redacted]".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    message.contains("No such object: labby-master") || message.contains("No such container")
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
    fn service_env_port_wins_over_process_env_port() {
        assert_eq!(
            configured_local_port_from(Some("9876"), Some(7777), Some("1234")),
            9876
        );
        assert_eq!(
            configured_local_port_from(None, Some(7777), Some("1234")),
            7777
        );
        assert_eq!(configured_local_port_from(None, None, Some("1234")), 1234);
        assert_eq!(
            configured_local_port_from(Some("bad"), Some(7777), Some("1234")),
            7777
        );
        assert_eq!(
            configured_local_port_from(Some("bad"), None, Some("1234")),
            1234
        );
        assert_eq!(
            configured_local_port_from(Some("bad"), None, Some("also-bad")),
            8765
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
    fn readiness_from_non_service_pid_is_not_owned() {
        let holder = r#"LISTEN 0 4096 127.0.0.1:8765 0.0.0.0:* users:(("other",pid=54321,fd=17))"#;

        assert!(!readiness_owner_matches_from(Some(12345), Some(holder)));
        assert!(readiness_owner_matches_from(Some(54321), Some(holder)));
        assert!(!readiness_owner_matches_from(None, Some(holder)));
    }
}
