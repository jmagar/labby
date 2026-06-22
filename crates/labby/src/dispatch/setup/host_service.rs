//! CLI-only management helpers for the host `systemd --user` Labby service.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::dispatch::error::ToolError;

const SERVICE_NAME: &str = "labby.service";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(300);

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
    pub ready_owned_by_service: Option<bool>,
    pub docker_labby_master_running: Option<bool>,
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
    Ok(unit_text())
}

pub(crate) async fn install() -> Result<HostServiceOutcome, ToolError> {
    preflight_port_available("install").await?;
    let home = current_home()?;
    let path = unit_path(&home);
    let text = unit_text();
    std::fs::create_dir_all(unit_dir(&home)).map_err(io_error)?;
    let changed = std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str());
    if changed {
        atomic_write(&path, text.as_bytes())?;
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    append_optional_verify(&path, &mut stdout, &mut stderr).await?;
    let daemon = run_systemctl(&["daemon-reload"]).await?;
    stdout.push_str(&daemon.stdout);
    stderr.push_str(&daemon.stderr);
    let enable = run_systemctl(&["enable", "--now", SERVICE_NAME]).await?;
    stdout.push_str(&enable.stdout);
    stderr.push_str(&enable.stderr);
    if let Err(err) = poll_ready().await {
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
    let home = current_home()?;
    let path = unit_path(&home);
    let installed = path.is_file();
    let docker_labby_master_running = docker_labby_master_running().await;
    let ready_response = check_ready().await.unwrap_or(false);
    let mut load_state = None;
    let mut active_state = None;
    let mut sub_state = None;
    let mut main_pid = None;
    let mut exec_main_status = None;

    if installed {
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
        main_pid = non_empty_prop(&props, "MainPID").and_then(|value| {
            let pid = value.parse::<u32>().ok()?;
            (pid != 0).then_some(pid)
        });
        exec_main_status =
            non_empty_prop(&props, "ExecMainStatus").and_then(|value| value.parse().ok());
    }

    let process_exe = main_pid.and_then(process_exe);
    let ready_owned_by_service = if ready_response {
        Some(readiness_owner_matches(main_pid).await?)
    } else {
        Some(false)
    };
    let local_ready = Some(ready_response && ready_owned_by_service == Some(true));

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
        ready_owned_by_service,
        docker_labby_master_running,
    })
}

pub(crate) async fn restart() -> Result<HostServiceOutcome, ToolError> {
    preflight_port_available("restart").await?;
    let home = current_home()?;
    let path = unit_path(&home);
    let restart = run_systemctl(&["restart", SERVICE_NAME]).await?;
    let mut stderr = restart.stderr;
    if let Err(err) = poll_ready().await {
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
    let home = current_home()?;
    let path = unit_path(&home);
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

fn unit_text() -> String {
    r#"[Unit]
Description=Labby host gateway
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=%h/.local/bin/labby serve
WorkingDirectory=%h
Environment=PATH=%h/.local/bin:%h/.cargo/bin:%h/.local/share/mise/shims:/home/linuxbrew/.linuxbrew/bin:/usr/local/bin:/usr/bin:/bin
EnvironmentFile=-%h/.lab/.env
Restart=on-failure
RestartSec=3
StartLimitIntervalSec=60
StartLimitBurst=5
KillSignal=SIGINT

[Install]
WantedBy=default.target
"#
    .to_string()
}

fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}

fn unit_path(home: &Path) -> PathBuf {
    unit_dir(home).join(SERVICE_NAME)
}

fn current_home() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "HOME is not set; cannot manage user systemd service".to_string(),
        })
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
    match run_command("systemd-analyze", &["--user", "verify", path_to_str(path)?]).await {
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

async fn preflight_port_available(operation: &str) -> Result<(), ToolError> {
    if docker_labby_master_running().await == Some(true) {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "cannot {operation} {SERVICE_NAME}: Docker container `labby-master` is running; stop it before starting the host gateway"
            ),
        });
    }

    let port = configured_local_port();
    if let Some(holder) = port_holder(port).await?
        && !holder_can_be_host_service(&holder).await?
    {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "cannot {operation} {SERVICE_NAME}: local port {port} is already in use:\n{holder}"
            ),
        });
    }
    Ok(())
}

fn configured_local_port() -> u16 {
    let env_file_port = env_file_value("LAB_MCP_HTTP_PORT");
    let process_port = std::env::var("LAB_MCP_HTTP_PORT").ok();
    configured_local_port_from(env_file_port.as_deref(), process_port.as_deref())
}

fn configured_local_port_from(service_env: Option<&str>, process_env: Option<&str>) -> u16 {
    service_env
        .and_then(|value| value.parse().ok())
        .or_else(|| process_env.and_then(|value| value.parse().ok()))
        .unwrap_or(8765)
}

fn env_file_value(key: &str) -> Option<String> {
    let path = current_home().ok()?.join(".lab/.env");
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

async fn holder_can_be_host_service(holder: &str) -> Result<bool, ToolError> {
    let status = status().await?;
    if status.active_state.as_deref() != Some("active") {
        return Ok(false);
    }
    let Some(pid) = status.main_pid else {
        return Ok(false);
    };
    Ok(holder_contains_pid(holder, pid))
}

async fn readiness_owner_matches(main_pid: Option<u32>) -> Result<bool, ToolError> {
    let Some(pid) = main_pid else {
        return Ok(false);
    };
    let Some(holder) = port_holder(configured_local_port()).await? else {
        return Ok(false);
    };
    Ok(holder_contains_pid(&holder, pid))
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

async fn docker_labby_master_running() -> Option<bool> {
    match run_command(
        "docker",
        &["inspect", "-f", "{{.State.Running}}", "labby-master"],
    )
    .await
    {
        Ok(output) => Some(output.stdout.trim() == "true"),
        Err(err) if command_not_found(&err) => None,
        Err(_) => Some(false),
    }
}

async fn poll_ready() -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    let mut last_err = String::new();
    while tokio::time::Instant::now() < deadline {
        match check_ready().await {
            Ok(true) => match systemctl_main_pid().await {
                Ok(main_pid) => match readiness_owner_matches(main_pid).await {
                    Ok(true) => return Ok(()),
                    Ok(false) => {
                        last_err =
                            "ready endpoint responded, but the listener is not labby.service"
                                .to_string();
                    }
                    Err(err) => last_err = err.to_string(),
                },
                Err(err) => last_err = err.to_string(),
            },
            Ok(false) => last_err = "ready endpoint returned non-success".to_string(),
            Err(err) => last_err = err,
        }
        tokio::time::sleep(READY_POLL_INTERVAL).await;
    }
    Err(last_err)
}

async fn systemctl_main_pid() -> Result<Option<u32>, ToolError> {
    let output = run_systemctl(&["show", SERVICE_NAME, "--property=MainPID", "--no-pager"]).await?;
    let props = parse_systemctl_show(&output.stdout);
    Ok(non_empty_prop(&props, "MainPID").and_then(|value| {
        let pid = value.parse::<u32>().ok()?;
        (pid != 0).then_some(pid)
    }))
}

async fn check_ready() -> Result<bool, String> {
    let url = format!("http://127.0.0.1:{}/ready", configured_local_port());
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

async fn run_systemctl(args: &[&str]) -> Result<CommandCapture, ToolError> {
    let mut command_args = vec!["--user"];
    command_args.extend_from_slice(args);
    run_command("systemctl", &command_args).await
}

async fn run_command(program: &str, args: &[&str]) -> Result<CommandCapture, ToolError> {
    let command_display = command_display(program, args);
    let mut command = Command::new(program);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("command timed out after {COMMAND_TIMEOUT:?}: {command_display}"),
        })?
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to run `{command_display}`: {err}"),
        })?;
    let captured = CommandCapture {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    };
    if captured.status.success() {
        Ok(captured)
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "command failed: {command_display}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
                captured.status, captured.stdout, captured.stderr
            ),
        })
    }
}

fn command_display(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .chain(args.iter().copied())
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
    err.to_string().contains("No such file or directory")
        || err.to_string().contains("os error 2")
        || err.to_string().contains("not found")
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
    fn unit_uses_durable_host_binary_and_lab_env() {
        let unit = unit_text();

        assert!(unit.contains("Description=Labby host gateway"));
        assert!(unit.contains("ExecStart=%h/.local/bin/labby serve"));
        assert!(unit.contains("WorkingDirectory=%h"));
        assert!(unit.contains("Environment=PATH=%h/.local/bin:%h/.cargo/bin:%h/.local/share/mise/shims:/home/linuxbrew/.linuxbrew/bin:/usr/local/bin:/usr/bin:/bin"));
        assert!(unit.contains("EnvironmentFile=-%h/.lab/.env"));
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
    fn unit_path_lives_under_user_systemd_dir() {
        let home = Path::new("/home/example");

        assert_eq!(
            unit_path(home),
            PathBuf::from("/home/example/.config/systemd/user/labby.service")
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
    }

    #[test]
    fn service_env_port_wins_over_process_env_port() {
        assert_eq!(configured_local_port_from(Some("9876"), Some("1234")), 9876);
        assert_eq!(configured_local_port_from(None, Some("1234")), 1234);
        assert_eq!(configured_local_port_from(Some("bad"), Some("1234")), 1234);
        assert_eq!(
            configured_local_port_from(Some("bad"), Some("also-bad")),
            8765
        );
    }

    #[test]
    fn detects_port_holder_pid() {
        let holder = r#"LISTEN 0 4096 127.0.0.1:8765 0.0.0.0:* users:(("labby",pid=12345,fd=17))"#;

        assert!(holder_contains_pid(holder, 12345));
        assert!(!holder_contains_pid(holder, 1234));
    }
}
