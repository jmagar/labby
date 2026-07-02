//! Local-only in-box provisioning for the Incus/bare-metal Labby gateway.
//!
//! This module is intentionally not routed through the setup action catalog:
//! callers reach it only through `labby setup --provision`.

use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::config::{ConfigScalarPatch, ConfigScalarValue};
use crate::dispatch::error::ToolError;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(600);
const CAPTURE_BYTES: usize = 16 * 1024;
const STALE_LOCK_AFTER: Duration = Duration::from_secs(60 * 60);
const LOCK_PATH: &str = "/var/lock/labby-provision.lock";
const LABBY_USER: &str = "labby";
const LABBY_HOME: &str = "/home/labby";
const LABBY_PATH: &str =
    "/home/labby/.local/bin:/home/labby/.cargo/bin:/usr/local/go/bin:/usr/local/bin:/usr/bin:/bin";
const TS_AUTHKEY_ENV: &str = "TS_AUTHKEY";
const TS_HOSTNAME_ENV: &str = "LABBY_TAILSCALE_HOSTNAME";
const TS_AUTHKEY_PATH: &str = "/run/labby-ts-authkey";
const LABBY_ZPROFILE: &str = "/home/labby/.zprofile";
const LABBY_IMAGE_YAML: &str = include_str!("../../../../../config/incus/labby-image.yaml");
const LABBY_USER_DIRS: &[&str] = &[
    "/home/labby/.labby",
    "/home/labby/.local/bin",
    "/home/labby/.cache",
    "/home/labby/.config",
    "/home/labby/.npm",
    "/home/labby/.codex",
    "/home/labby/.claude",
    "/home/labby/.gemini",
    "/home/labby/downloads",
];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProvisionOptions {
    pub dry_run: bool,
    pub yes: bool,
    pub skip_deps: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ProvisionOutcome {
    pub ok: bool,
    pub dry_run: bool,
    pub changed: bool,
    pub plan: String,
    pub executed: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Privilege {
    Root,
    Lab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProvisionAction {
    privilege: Privilege,
    label: Cow<'static, str>,
    kind: ActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionKind {
    AptFloor,
    LabUser,
    Node,
    UvPython,
    RustGo,
    AgentClis,
    TailscaleInstall,
    TailscaleJoin,
    ControllerConfig,
    HostService,
}

struct ProvisionLock {
    path: PathBuf,
    released: bool,
}

impl Drop for ProvisionLock {
    fn drop(&mut self) {
        if !self.released {
            drop(std::fs::remove_file(&self.path));
        }
    }
}

impl ProvisionLock {
    fn release(mut self) -> Result<(), ToolError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {
                self.released = true;
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.released = true;
                Ok(())
            }
            Err(err) => Err(io_error(err)),
        }
    }
}

#[derive(Debug)]
struct CommandCapture {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

pub(crate) async fn provision(options: ProvisionOptions) -> Result<ProvisionOutcome, ToolError> {
    let plan = build_plan(options.skip_deps);
    let rendered = render_plan(&plan);
    if options.dry_run {
        return Ok(ProvisionOutcome {
            ok: true,
            dry_run: true,
            changed: false,
            plan: rendered,
            executed: Vec::new(),
            skipped: Vec::new(),
        });
    }
    if !options.yes {
        return Err(ToolError::ConfirmationRequired {
            message: "setup --provision mutates packages, users, and systemd; pass --yes or confirm interactively".into(),
        });
    }

    let lock = acquire_lock()?;
    let mut executed = Vec::new();
    let mut skipped = Vec::new();
    for action in &plan {
        if action.is_done().await? {
            skipped.push(action.label.to_string());
            continue;
        }
        action.execute().await?;
        action.verify().await?;
        executed.push(action.label.to_string());
    }

    let outcome = ProvisionOutcome {
        ok: true,
        dry_run: false,
        changed: !executed.is_empty(),
        plan: rendered,
        executed,
        skipped,
    };
    lock.release()?;
    Ok(outcome)
}

pub(crate) fn provision_plan_text(skip_deps: bool) -> String {
    render_plan(&build_plan(skip_deps))
}

fn build_plan(skip_deps: bool) -> Vec<ProvisionAction> {
    let mut actions = Vec::new();
    if !skip_deps {
        actions.push(ProvisionAction {
            privilege: Privilege::Root,
            label: Cow::Owned(format!("apt install: {}", apt_floor().join(" "))),
            kind: ActionKind::AptFloor,
        });
    }
    actions.push(ProvisionAction {
        privilege: Privilege::Root,
        label: Cow::Borrowed("useradd labby (if absent)"),
        kind: ActionKind::LabUser,
    });
    if !skip_deps {
        actions.extend([
            ProvisionAction {
                privilege: Privilege::Lab,
                label: Cow::Borrowed("install node v24.x (official static tarball, on PATH)"),
                kind: ActionKind::Node,
            },
            ProvisionAction {
                privilege: Privilege::Lab,
                label: Cow::Borrowed("install uv + python (user-space)"),
                kind: ActionKind::UvPython,
            },
            ProvisionAction {
                privilege: Privilege::Lab,
                label: Cow::Borrowed("install rust + go toolchains"),
                kind: ActionKind::RustGo,
            },
            ProvisionAction {
                privilege: Privilege::Lab,
                label: Cow::Borrowed("install claude + codex + gemini (npm, user-space)"),
                kind: ActionKind::AgentClis,
            },
            ProvisionAction {
                privilege: Privilege::Root,
                label: Cow::Borrowed("install tailscale client"),
                kind: ActionKind::TailscaleInstall,
            },
        ]);
        if tailscale_authkey().is_some() {
            actions.push(ProvisionAction {
                privilege: Privilege::Root,
                label: Cow::Borrowed("join tailnet using TS_AUTHKEY"),
                kind: ActionKind::TailscaleJoin,
            });
        }
    }
    actions.push(ProvisionAction {
        privilege: Privilege::Root,
        label: Cow::Borrowed("set Labby node runtime role to controller"),
        kind: ActionKind::ControllerConfig,
    });
    actions.push(ProvisionAction {
        privilege: Privilege::Root,
        label: Cow::Borrowed(
            "write /etc/systemd/system/labby.service and systemctl enable --now labby",
        ),
        kind: ActionKind::HostService,
    });
    actions
}

fn render_plan(actions: &[ProvisionAction]) -> String {
    let mut out = String::from("labby setup --provision will:\n");
    for action in actions {
        out.push_str("  ");
        out.push_str(match action.privilege {
            Privilege::Root => "[root] ",
            Privilege::Lab => "[labby] ",
        });
        out.push_str(&action.label);
        out.push('\n');
    }
    out.push_str(
        "\nIt will NOT:\n  - install or modify Incus\n  - touch any package outside the list above\n  - modify anything on the host outside this container\n  - transmit anything off-box except explicit package/runtime downloads\n",
    );
    if actions
        .iter()
        .any(|action| action.kind == ActionKind::TailscaleJoin)
    {
        out.push_str("  - print or persist the Tailscale auth key after join\n");
    }
    out
}

impl ProvisionAction {
    async fn is_done(&self) -> Result<bool, ToolError> {
        match self.kind {
            ActionKind::AptFloor => apt_floor_installed().await,
            ActionKind::LabUser => lab_user_ready().await,
            ActionKind::Node => {
                lab_command_success("command -v node >/dev/null && command -v npm >/dev/null && command -v npx >/dev/null && node --version | grep -Eq '^v24\\.'").await
            }
            ActionKind::UvPython => {
                lab_command_success("command -v uv >/dev/null && command -v uvx >/dev/null && command -v python >/dev/null && command -v python3 >/dev/null && uv python find >/dev/null").await
            }
            ActionKind::RustGo => {
                lab_command_success("command -v rustup >/dev/null && command -v rustc >/dev/null && command -v cargo >/dev/null && command -v go >/dev/null && command -v gofmt >/dev/null")
                    .await
            }
            ActionKind::AgentClis => {
                lab_command_success(
                    "command -v claude >/dev/null && command -v codex >/dev/null && command -v gemini >/dev/null",
                )
                .await
            }
            ActionKind::TailscaleInstall => command_success("tailscale", &["version"]).await,
            ActionKind::TailscaleJoin => command_success("tailscale", &["ip", "-4"]).await,
            ActionKind::ControllerConfig => controller_config_ready().await,
            ActionKind::HostService => super::host_service::installed_and_ready().await,
        }
    }

    async fn execute(&self) -> Result<(), ToolError> {
        match self.kind {
            ActionKind::AptFloor => {
                run_checked("apt-get", &["update"]).await?;
                let mut args = vec!["install", "-y"];
                args.extend(apt_floor());
                run_checked("apt-get", &args).await?;
            }
            ActionKind::LabUser => {
                run_image_provision_action("labby-user").await?;
            }
            ActionKind::Node => {
                run_image_provision_action("node").await?;
            }
            ActionKind::UvPython => {
                run_image_provision_action("uv-python").await?;
            }
            ActionKind::RustGo => {
                run_image_provision_action("rust-go").await?;
            }
            ActionKind::AgentClis => {
                run_image_provision_action("agent-clis").await?;
            }
            ActionKind::TailscaleInstall => {
                run_image_provision_action("tailscale-install").await?;
            }
            ActionKind::TailscaleJoin => {
                install_and_join_tailscale().await?;
            }
            ActionKind::ControllerConfig => {
                ensure_controller_config().await?;
            }
            ActionKind::HostService => {
                drop(run_checked("systemctl", &["reset-failed", "labby.service"]).await);
                super::host_service::install().await?;
            }
        }
        Ok(())
    }

    async fn verify(&self) -> Result<(), ToolError> {
        if self.is_done().await? {
            Ok(())
        } else {
            Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("provision step did not verify: {}", self.label),
            })
        }
    }
}

fn acquire_lock() -> Result<ProvisionLock, ToolError> {
    let path = PathBuf::from(LOCK_PATH);
    match try_create_lock(&path) {
        Ok(lock) => Ok(lock),
        Err(err)
            if err.kind() == std::io::ErrorKind::AlreadyExists && cleanup_stale_lock(&path)? =>
        {
            try_create_lock(&path).map_err(io_error)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(ToolError::Conflict {
            message: lock_conflict_message(&path),
            existing_id: LOCK_PATH.to_string(),
        }),
        Err(err) => Err(io_error(err)),
    }
}

fn try_create_lock(path: &Path) -> std::io::Result<ProvisionLock> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            writeln!(file, "pid={}", std::process::id())?;
            Ok(ProvisionLock {
                path: path.to_path_buf(),
                released: false,
            })
        }
        Err(err) => Err(err),
    }
}

fn cleanup_stale_lock(path: &Path) -> Result<bool, ToolError> {
    let Some(pid) = lock_owner_pid(path) else {
        return Ok(false);
    };
    if Path::new(&format!("/proc/{pid}")).exists() {
        return Ok(false);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(err) => Err(io_error(err)),
    }
}

fn lock_owner_pid(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("pid=")?.parse().ok())
}

fn lock_conflict_message(path: &Path) -> String {
    let stale_note = std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .filter(|age| *age > STALE_LOCK_AFTER)
        .map(|age| {
            format!(
                "; lock is older than {} minutes, verify no provision run is active before removing it",
                age.as_secs() / 60
            )
        })
        .unwrap_or_default();
    format!("another labby setup --provision is running ({LOCK_PATH} exists{stale_note})")
}

async fn lab_user_ready() -> Result<bool, ToolError> {
    if !command_success("id", &["-u", LABBY_USER]).await? {
        return Ok(false);
    }
    let home_ok = std::fs::metadata(LABBY_HOME)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    if !home_ok {
        return Ok(false);
    }
    if LABBY_USER_DIRS.iter().any(|path| {
        !std::fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false)
    }) {
        return Ok(false);
    }
    let zprofile_ok = std::fs::read_to_string(LABBY_ZPROFILE)
        .map(|profile| profile.contains("LABBY managed PATH"))
        .unwrap_or(false);
    if !zprofile_ok {
        return Ok(false);
    }
    lab_command_success("test -w \"$HOME/.labby\" && test -w \"$HOME/.local/bin\"").await
}

async fn apt_floor_installed() -> Result<bool, ToolError> {
    for package in apt_floor() {
        let output = run_command("dpkg-query", &["-W", "-f=${db:Status-Abbrev}", package]).await?;
        if !output.status.success() || !output.stdout.starts_with("ii ") {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn controller_config_ready() -> Result<bool, ToolError> {
    let path = provision_config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(io_error(err)),
    };
    let cfg = toml::from_str::<crate::config::LabConfig>(&raw).map_err(|err| ToolError::Sdk {
        sdk_kind: "invalid_config".into(),
        message: format!("failed to parse {}: {err}", path.display()),
    })?;
    let Some(node) = cfg.node else {
        return Ok(false);
    };
    Ok(
        node.role == Some(crate::config::NodeRuntimeRole::Controller)
            && node
                .controller
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
    )
}

async fn ensure_controller_config() -> Result<(), ToolError> {
    let hostname = run_command("hostname", &[]).await?;
    if !hostname.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: "failed to read container hostname for controller config".into(),
        });
    }
    let controller = hostname.stdout.trim();
    let controller = if controller.is_empty() {
        "labby"
    } else {
        controller
    };
    let path = provision_config_path();
    crate::config::patch_config_scalars_checked(
        &path,
        &[
            ConfigScalarPatch::new(
                "node.role",
                ConfigScalarValue::String("controller".to_string()),
            ),
            ConfigScalarPatch::new(
                "node.controller",
                ConfigScalarValue::String(controller.to_string()),
            ),
        ],
        &[],
    )
    .map_err(|err| ToolError::Sdk {
        sdk_kind: "invalid_config".into(),
        message: format!("failed to update {}: {err}", path.display()),
    })?;
    let path_arg = path.to_string_lossy().to_string();
    run_checked("chown", &[&format!("{LABBY_USER}:{LABBY_USER}"), &path_arg]).await?;
    Ok(())
}

fn provision_config_path() -> PathBuf {
    crate::config::config_toml_path()
        .unwrap_or_else(|| PathBuf::from(format!("{LABBY_HOME}/.config/labby/config.toml")))
}

fn apt_floor() -> Vec<&'static str> {
    let packages = parse_image_install_packages(LABBY_IMAGE_YAML);
    assert!(
        !packages.is_empty(),
        "config/incus/labby-image.yaml must declare packages.sets action=install packages"
    );
    packages
}

fn parse_image_install_packages(yaml: &str) -> Vec<&str> {
    let mut in_packages_root = false;
    let mut in_sets = false;
    let mut in_install_set = false;
    let mut in_install_packages = false;
    let mut packages = Vec::new();

    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        match (indent, trimmed) {
            (0, "packages:") => {
                in_packages_root = true;
                in_sets = false;
                in_install_set = false;
                in_install_packages = false;
                continue;
            }
            (0, _) => {
                in_packages_root = false;
                in_sets = false;
                in_install_set = false;
                in_install_packages = false;
            }
            (2, "sets:") if in_packages_root => {
                in_sets = true;
                continue;
            }
            (4, "- action: install") if in_sets => {
                in_install_set = true;
                in_install_packages = false;
                continue;
            }
            (4, text) if in_sets && text.starts_with("- action:") => {
                in_install_set = false;
                in_install_packages = false;
                continue;
            }
            (6, "packages:") if in_install_set => {
                in_install_packages = true;
                continue;
            }
            _ => {}
        }

        if in_install_packages {
            if indent == 8 {
                if let Some(package) = trimmed.strip_prefix("- ") {
                    packages.push(package.trim_matches('"').trim_matches('\''));
                    continue;
                }
            }
            if indent <= 6 {
                in_install_packages = false;
            }
        }
    }

    packages
}

async fn run_image_provision_action(name: &str) -> Result<(), ToolError> {
    let script = image_provision_action(name).ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!(
            "missing LABBY_PROVISION_ACTION `{name}` in config/incus/labby-image.yaml"
        ),
    })?;
    run_checked("bash", &["-lc", &script]).await?;
    Ok(())
}

fn image_provision_action(name: &str) -> Option<String> {
    let marker = format!("# LABBY_PROVISION_ACTION: {name}");
    parse_image_action_scripts(LABBY_IMAGE_YAML)
        .into_iter()
        .find(|script| script.lines().any(|line| line.trim() == marker))
}

fn parse_image_action_scripts(yaml: &str) -> Vec<String> {
    let mut scripts = Vec::new();
    let mut lines = yaml.lines().peekable();

    while let Some(line) = lines.next() {
        if line.trim() != "action: |-" {
            continue;
        }

        let mut script = String::new();
        while let Some(next) = lines.peek().copied() {
            let trimmed = next.trim();
            let indent = next.len() - next.trim_start().len();
            if !trimmed.is_empty() && indent <= 4 {
                break;
            }
            let raw = lines.next().expect("peeked line exists");
            if raw.len() >= 6 {
                script.push_str(&raw[6..]);
            }
            script.push('\n');
        }
        scripts.push(script);
    }

    scripts
}

async fn install_and_join_tailscale() -> Result<(), ToolError> {
    let authkey = tailscale_authkey().ok_or_else(|| ToolError::MissingParam {
        param: TS_AUTHKEY_ENV.into(),
        message: "TS_AUTHKEY is required to join Tailscale during provisioning".into(),
    })?;
    let result = async {
        if !command_success("tailscale", &["version"]).await? {
            run_image_provision_action("tailscale-install").await?;
        }
        write_tailscale_authkey(&authkey)?;
        let auth_arg = format!("--auth-key=file:{TS_AUTHKEY_PATH}");
        if let Some(hostname) = tailscale_hostname() {
            run_checked(
                "tailscale",
                &["up", &auth_arg, &format!("--hostname={hostname}")],
            )
            .await?;
        } else {
            run_checked("tailscale", &["up", &auth_arg]).await?;
        }
        Ok(())
    }
    .await;
    drop(std::fs::remove_file(TS_AUTHKEY_PATH));
    result
}

fn tailscale_authkey() -> Option<String> {
    std::env::var(TS_AUTHKEY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn tailscale_hostname() -> Option<String> {
    std::env::var(TS_HOSTNAME_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_tailscale_authkey(authkey: &str) -> Result<(), ToolError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(TS_AUTHKEY_PATH).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io_error)?;
    }
    file.write_all(authkey.as_bytes()).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    Ok(())
}

async fn lab_command_success(script: &str) -> Result<bool, ToolError> {
    let script = lab_shell_script(script);
    command_success(
        "runuser",
        &["-u", LABBY_USER, "--", "sh", "-lc", script.as_str()],
    )
    .await
}

async fn command_success(program: &str, args: &[&str]) -> Result<bool, ToolError> {
    match run_command(program, args).await {
        Ok(output) => Ok(output.status.success()),
        Err(err) if command_not_found(&err) => Ok(false),
        Err(err) => Err(err),
    }
}

fn lab_shell_script(script: &str) -> String {
    format!(
        "export HOME={LABBY_HOME}; export PATH={LABBY_PATH}; export XDG_CONFIG_HOME={LABBY_HOME}/.config; export XDG_CACHE_HOME={LABBY_HOME}/.cache; cd {LABBY_HOME}; {script}"
    )
}

async fn run_checked(program: &str, args: &[&str]) -> Result<CommandCapture, ToolError> {
    let output = run_command(program, args).await?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!(
                "command failed: {}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
                command_display(program, args),
                output.status,
                redact_command_output(&output.stdout),
                redact_command_output(&output.stderr)
            ),
        })
    }
}

async fn run_command(program: &str, args: &[&str]) -> Result<CommandCapture, ToolError> {
    let display = command_display(program, args);
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("failed to run `{display}`: {err}"),
    })?;
    let child_id = child.id();
    let stdout = tokio::spawn(read_capped(child.stdout.take()));
    let stderr = tokio::spawn(read_capped(child.stderr.take()));
    let status = tokio::time::timeout(COMMAND_TIMEOUT, child.wait())
        .await
        .map_err(|_| {
            kill_process_tree(child_id);
            ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("command timed out after {COMMAND_TIMEOUT:?}: {display}"),
            }
        })?
        .map_err(io_error)?;
    let stdout = stdout
        .await
        .unwrap_or_else(|err| format!("failed to join command output reader: {err}"));
    let stderr = stderr
        .await
        .unwrap_or_else(|err| format!("failed to join command output reader: {err}"));
    Ok(CommandCapture {
        status,
        stdout,
        stderr,
    })
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
        text.insert_str(0, "…[truncated]\n");
    }
    text
}

#[cfg(unix)]
fn kill_process_tree(child_id: Option<u32>) {
    let Some(pid) = child_id else {
        return;
    };
    let group = format!("-{pid}");
    drop(
        std::process::Command::new("kill")
            .args(["-TERM", &group])
            .status(),
    );
    std::thread::sleep(Duration::from_millis(500));
    drop(
        std::process::Command::new("kill")
            .args(["-KILL", &group])
            .status(),
    );
}

#[cfg(not(unix))]
fn kill_process_tree(_child_id: Option<u32>) {}

fn command_display(program: &str, args: &[&str]) -> String {
    std::iter::once(program)
        .map(str::to_string)
        .chain(args.iter().map(|arg| redact_command_output(arg)))
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
        format!("{}…[truncated]", &joined[..cut])
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

fn command_not_found(err: &ToolError) -> bool {
    let message = err.to_string();
    message.contains("failed to run `")
        && (message.contains("No such file or directory") || message.contains("os error 2"))
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
    fn dry_run_plan_renders_privilege_and_non_actions() {
        let text = provision_plan_text(false);

        assert!(text.contains("[root] apt install: git openssh-client gh"));
        assert!(text.contains("ffmpeg"));
        assert!(text.contains("adb"));
        assert!(text.contains("android-sdk"));
        assert!(text.contains("[labby] install node v24.x"));
        assert!(text.contains("[labby] install rust + go toolchains"));
        assert!(text.contains("[labby] install claude + codex + gemini"));
        assert!(text.contains("[root] install tailscale client"));
        assert!(text.contains("[root] set Labby node runtime role to controller"));
        assert!(text.contains("[root] write /etc/systemd/system/labby.service"));
        assert!(text.contains("It will NOT:"));
        assert!(text.contains("install or modify Incus"));
    }

    #[test]
    fn apt_floor_is_derived_from_incus_image_yaml() {
        assert_eq!(
            apt_floor(),
            vec![
                "git",
                "openssh-client",
                "gh",
                "ca-certificates",
                "curl",
                "xz-utils",
                "python3",
                "zsh",
                "ffmpeg",
                "adb",
                "android-sdk",
                "android-sdk-platform-tools",
                "android-sdk-platform-tools-common",
                "android-sdk-build-tools",
            ]
        );
    }

    #[test]
    fn install_package_parser_ignores_non_install_sets() {
        let packages = parse_image_install_packages(
            r#"
packages:
  manager: apt
  sets:
    - action: install
      packages:
        - curl
        - adb
    - action: remove
      packages:
        - do-not-install

files:
  - generator: dump
"#,
        );

        assert_eq!(packages, vec!["curl", "adb"]);
    }

    #[test]
    fn provision_actions_are_derived_from_incus_image_yaml() {
        for name in [
            "labby-user",
            "node",
            "uv-python",
            "rust-go",
            "agent-clis",
            "tailscale-install",
        ] {
            let script = image_provision_action(name).expect("named action should exist");
            assert!(script.contains(&format!("LABBY_PROVISION_ACTION: {name}")));
        }
    }

    #[test]
    fn skip_deps_plan_is_service_only() {
        let text = provision_plan_text(true);

        assert!(!text.contains("apt install"));
        assert!(!text.contains("install node"));
        assert!(text.contains("useradd labby"));
        assert!(text.contains("systemctl enable --now labby"));
    }

    #[tokio::test]
    async fn dry_run_does_not_require_confirmation_or_lock() {
        let outcome = provision(ProvisionOptions {
            dry_run: true,
            yes: false,
            skip_deps: false,
        })
        .await
        .unwrap();

        assert!(outcome.ok);
        assert!(outcome.dry_run);
        assert!(!outcome.changed);
        assert!(outcome.executed.is_empty());
    }

    #[tokio::test]
    async fn mutation_without_yes_is_refused() {
        let err = provision(ProvisionOptions {
            dry_run: false,
            yes: false,
            skip_deps: true,
        })
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "confirmation_required");
    }

    #[test]
    fn redacts_sensitive_command_output() {
        let bearer = ["sk", "abcdefghijklmnopqrstuvwxyz"].join("-");
        let ts_key = ["tskey", "secret"].join("-");
        let redacted = redact_command_output(&format!(
            "Authorization: Bearer {bearer}\nTS_AUTHKEY={ts_key}\n"
        ));

        assert!(!redacted.contains(&bearer));
        assert!(!redacted.contains(&ts_key));
    }

    #[tokio::test]
    async fn failed_command_redacts_stdout_and_stderr() {
        let stdout_token = ["sk", "stdout-secret"].join("-");
        let stderr_token = ["tskey", "stderr-secret"].join("-");
        #[cfg(unix)]
        let (program, args) = {
            let script = format!(
                "printf 'Authorization: Bearer {stdout_token}'; printf 'TS_AUTHKEY={stderr_token}' >&2; exit 9"
            );
            ("sh", vec!["-c".to_string(), script])
        };
        #[cfg(windows)]
        let (program, args) = {
            let script = format!(
                "[Console]::Out.Write('Authorization: Bearer {stdout_token}'); [Console]::Error.Write('TS_AUTHKEY={stderr_token}'); exit 9"
            );
            (
                "pwsh",
                vec!["-NoProfile".to_string(), "-Command".to_string(), script],
            )
        };
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let err = run_checked(program, &args).await.unwrap_err().to_string();

        assert!(!err.contains(&stdout_token));
        assert!(!err.contains(&stderr_token));
        assert!(err.contains("Authorization: Bearer [redacted]"));
        assert!(err.contains("TS_AUTHKEY=[redacted]"));
    }

    #[test]
    fn provision_is_not_in_setup_action_catalog() {
        assert!(
            !super::super::catalog::ACTIONS
                .iter()
                .any(|action| action.name.contains("provision"))
        );
    }
}
