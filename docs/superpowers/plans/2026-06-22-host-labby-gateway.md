# Host Labby Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the primary Labby gateway runtime from the Docker dev container to a host-owned user service, while keeping Docker available only as an explicit prod-like/container smoke path.

**Architecture:** Labby should run where its stdio MCP tools, host credentials, SSH config, local binaries, and agent caches already live: the host user session. The host runtime is owned by `systemd --user`, while Docker remains a compatibility path that never bind-mounts over the running executable as the default developer workflow. Code Mode runner spawning also gains a stable executable resolver so a replaced binary produces either a working fallback or a clear restart-needed error.

**Tech Stack:** Rust 2024, Clap, Tokio, `systemctl --user`, Docker Compose, existing Lab setup dispatch layer, existing Code Mode runner pool.

**Engineering review decisions applied:**
- Host service lifecycle management is CLI-only for this change. Do not add `host_service.*` actions to the setup dispatch catalog, MCP, or HTTP API.
- The host service binds through normal Labby config/env. The generated unit must not hard-code `--host 0.0.0.0 --port 8765`.
- Code Mode runner resolution must not silently switch to another `labby` binary. Deleted/stale `current_exe()` returns clear restart guidance unless `LAB_CODE_MODE_RUNNER_EXE` is explicitly set and validated.
- Service management helpers must preserve exact `systemctl`/journal diagnostics and use timeouts so a wedged user systemd bus does not hang an agent workflow.
- Migration proof must verify the actual public MCP route and the host process backing it, not just `/health`.

## Global Constraints

- Preserve the current one-binary model: `labby serve` remains the hosted HTTP/API/MCP/Web runtime.
- Keep `lab-apis` pure: no `clap`, `rmcp`, `axum`, `anyhow`, filesystem service management, or env loading there.
- CLI files stay thin shims; host service behavior lives under `crates/lab/src/dispatch/setup/`.
- Host service install/uninstall/restart commands are destructive and require explicit CLI confirmation.
- Do not expose host service install, restart, uninstall, or status through MCP or HTTP in this implementation.
- Do not remove Docker Compose support; demote it to explicit container smoke/dev-container workflows.
- Validate with all-features Rust checks before completion.
- Do not edit `AGENTS.md` or `GEMINI.md`; update `CLAUDE.md` if agent memory changes are needed.

---

## File Structure

- Create: `crates/lab/src/dispatch/gateway/code_mode/runner_exe.rs`
  - Resolves the executable used for `labby internal code-mode-runner`.
  - Detects deleted/stale `current_exe()` paths.
  - Honors an explicit `LAB_CODE_MODE_RUNNER_EXE` override.
  - Does not fall back to unrelated Labby binaries automatically.
- Modify: `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`
  - Uses `runner_exe::resolve_runner_exe()` instead of calling `std::env::current_exe()` inline.
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
  - Declares the new `runner_exe` module.
- Modify: `crates/lab/src/dispatch/gateway/code_mode/pool/runner_handle.rs`
  - Improves the spawn error to include the attempted executable path.
- Create: `crates/lab/src/dispatch/setup/host_service.rs`
  - Generates and installs the `systemd --user` unit.
  - Runs status/restart/uninstall operations through typed CLI-only helpers.
  - Exposes serializable status and operation result types.
- Modify: `crates/lab/src/dispatch/setup.rs`
  - Declares `host_service`.
- Modify: `crates/lab/src/cli/setup.rs`
  - Adds `labby setup host-service ...` subcommands as thin CLI shims over `dispatch::setup::host_service`.
- Modify: `Justfile`
  - Adds host-first `host-sync`, `host-service-install`, `host-service-restart`, and `host-service-status`.
  - Keeps container workflows under explicit names.
- Modify: `README.md`
  - Documents host gateway as the normal local/node-a runtime.
  - Demotes the Docker dev container section.
- Modify: `CLAUDE.md`
  - Updates development instructions so agents prefer the host service for Labby gateway work.
- Create: `docs/runtime/HOST_GATEWAY.md`
  - Operator runbook for install, migration from container, rollback, verification, and known failure modes.

---

### Task 1: Code Mode Runner Executable Resolver

**Files:**
- Modify: `crates/lab/Cargo.toml`
  - Enable the existing `nix` dependency's `user` feature if required for current-user ownership checks.
- Create: `crates/lab/src/dispatch/gateway/code_mode/runner_exe.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/pool/runner_handle.rs`

**Interfaces:**
- Consumes: `ToolError::Sdk { sdk_kind, message }`
- Produces: `pub(super) fn resolve_runner_exe() -> Result<std::path::PathBuf, ToolError>`
- Produces for tests: `pub(super) fn resolve_runner_exe_from(current_exe: PathBuf, override_exe: Option<PathBuf>) -> Result<PathBuf, ToolError>`

- [ ] **Step 1: Write failing resolver tests**

Add this module to `crates/lab/src/dispatch/gateway/code_mode/runner_exe.rs` with the tests included first:

```rust
use std::path::{Path, PathBuf};

use crate::dispatch::error::ToolError;

pub(super) fn resolve_runner_exe() -> Result<PathBuf, ToolError> {
    let current = std::env::current_exe().map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to locate current executable for Code Mode runner: {err}"),
    })?;
    let override_exe = std::env::var_os("LAB_CODE_MODE_RUNNER_EXE").map(PathBuf::from);
    resolve_runner_exe_from(current, override_exe)
}

pub(super) fn resolve_runner_exe_from(
    current_exe: PathBuf,
    override_exe: Option<PathBuf>,
) -> Result<PathBuf, ToolError> {
    if let Some(path) = override_exe {
        let path = validate_operator_override(path)?;
        tracing::warn!(
            runner_exe = %path.display(),
            "using LAB_CODE_MODE_RUNNER_EXE override for Code Mode runner"
        );
        return Ok(path);
    }

    if is_usable_exe(&current_exe) {
        return Ok(current_exe);
    }

    Err(ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!(
            "Code Mode runner executable is stale or unavailable: `{}`; restart labby.service or set LAB_CODE_MODE_RUNNER_EXE to a validated labby binary",
            current_exe.display()
        ),
    })
}

fn validate_operator_override(path: PathBuf) -> Result<PathBuf, ToolError> {
    if !path.is_absolute() {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "LAB_CODE_MODE_RUNNER_EXE must be an absolute path".to_string(),
        });
    }
    let canonical = std::fs::canonicalize(&path).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!(
            "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but it cannot be resolved: {err}",
            path.display()
        ),
    })?;
    if !is_usable_exe(&canonical) {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but that file is not executable",
                canonical.display()
            ),
        });
    }
    reject_untrusted_permissions(&canonical)?;
    Ok(canonical)
}

fn is_usable_exe(path: &Path) -> bool {
    if path.to_string_lossy().ends_with(" (deleted)") {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn reject_untrusted_permissions(path: &Path) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to inspect `{}`: {err}", path.display()),
        })?;
        if meta.mode() & 0o022 != 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!(
                    "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but the file is group/world writable",
                    path.display()
                ),
            });
        }
        let current_uid = nix::unistd::Uid::current().as_raw();
        if meta.uid() != current_uid && meta.uid() != 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!(
                    "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but the file is not owned by the current user or root",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn uses_current_exe_when_it_is_usable() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("labby");
        std::fs::write(&current, b"binary").unwrap();
        #[cfg(unix)]
        make_executable(&current);

        let resolved = resolve_runner_exe_from(current.clone(), None).unwrap();

        assert_eq!(resolved, current);
    }

    #[test]
    fn deleted_current_exe_without_override_reports_restart_guidance() {
        let err = resolve_runner_exe_from(PathBuf::from("/usr/local/bin/labby (deleted)"), None)
            .unwrap_err();

        assert_eq!(err.kind(), "internal_error");
        assert!(err.to_string().contains("restart labby.service"));
    }

    #[test]
    fn override_must_be_absolute() {
        let err = resolve_runner_exe_from(
            PathBuf::from("/usr/local/bin/labby"),
            Some(PathBuf::from("target/debug/labby")),
        )
        .unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("absolute path"));
    }

    #[test]
    fn override_must_point_to_usable_file() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing-labby");

        let err = resolve_runner_exe_from(
            PathBuf::from("/usr/local/bin/labby"),
            Some(missing),
        )
        .unwrap_err();

        assert_eq!(err.kind(), "internal_error");
        assert!(err.to_string().contains("LAB_CODE_MODE_RUNNER_EXE"));
    }

    #[test]
    fn explicit_override_is_used_after_validation() {
        let temp = tempfile::tempdir().unwrap();
        let override_path = temp.path().join("labby");
        std::fs::write(&override_path, b"binary").unwrap();
        #[cfg(unix)]
        make_executable(&override_path);

        let resolved = resolve_runner_exe_from(
            PathBuf::from("/usr/local/bin/labby (deleted)"),
            Some(override_path.clone()),
        )
        .unwrap();

        assert_eq!(resolved, std::fs::canonicalize(override_path).unwrap());
    }
}
```

Review note: automatic fallback to `/usr/local/bin/labby` or `~/.local/bin/labby`
was rejected because it can mix parent and runner protocol versions after a
failed restart. A future protocol/version handshake can be added if automatic
fallback becomes necessary; this migration should prefer a loud restart-needed
error over a silent stale-binary mismatch.

- [ ] **Step 2: Run tests to verify the integration points fail**

Run:

```bash
cargo test -p labby runner_exe --all-features
```

Expected before module wiring: compilation fails because `runner_exe` is not declared.

- [ ] **Step 3: Wire the resolver into Code Mode**

In `crates/lab/src/dispatch/gateway/code_mode.rs`, add the module declaration next to the other Code Mode submodules:

```rust
mod runner_exe;
```

In `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`, replace the inline `current_exe()` block with:

```rust
let exe = super::runner_exe::resolve_runner_exe()?;
```

In `crates/lab/src/dispatch/gateway/code_mode/pool/runner_handle.rs`, replace the spawn error message with:

```rust
let mut child = cmd.spawn().map_err(|err| ToolError::Sdk {
    sdk_kind: "internal_error".to_string(),
    message: format!(
        "failed to spawn Code Mode runner from `{}`: {err}",
        exe.display()
    ),
})?;
```

- [ ] **Step 4: Run the focused tests**

Run:

```bash
cargo test -p labby runner_exe --all-features
```

Expected: PASS.

- [ ] **Step 5: Run the existing Code Mode runner tests**

Run:

```bash
cargo nextest run -p labby --all-features code_mode_runner
```

Expected: PASS.

- [ ] **Step 6: Verify diagnostics mention the executable path**

Run the focused tests or an existing Code Mode runner smoke with a deliberately
invalid override:

```bash
LAB_CODE_MODE_RUNNER_EXE=/tmp/missing-labby cargo test -p labby runner_exe --all-features
```

Expected: the failure message includes `LAB_CODE_MODE_RUNNER_EXE` and the
attempted path. Do not leave the override in the environment after this check.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs \
  crates/lab/Cargo.toml \
  crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs \
  crates/lab/src/dispatch/gateway/code_mode/runner_exe.rs \
  crates/lab/src/dispatch/gateway/code_mode/pool/runner_handle.rs
git commit -m "fix: resolve code mode runner executable robustly"
```

---

### Task 2: Host Systemd Service CLI Only

**Files:**
- Create: `crates/lab/src/dispatch/setup/host_service.rs`
- Modify: `crates/lab/src/dispatch/setup.rs`
- Modify: `crates/lab/src/cli/setup.rs`
- Modify only if tests reveal accidental exposure: `crates/lab/src/dispatch/setup/catalog.rs`, `crates/lab/src/dispatch/setup/dispatch.rs`, `crates/lab/src/api/services/setup.rs`

**Interfaces:**
- Consumes: local CLI invocation only.
- Produces: `labby setup host-service unit`
- Produces: `labby setup host-service install -y`
- Produces: `labby setup host-service status`
- Produces: `labby setup host-service restart -y`
- Produces: `labby setup host-service uninstall -y`
- Does not produce setup catalog actions, MCP actions, or HTTP API actions.

Review note: this replaces the earlier dispatch-based Task 2. Host lifecycle
management controls the process that serves the gateway and has access to host
credentials, SSH config, and local agent caches. `requires_admin` is not a
sufficient boundary for `systemctl --user` mutation on a service that can bind a
public interface, so the lifecycle surface stays local CLI-only.

- [ ] **Step 1: Add explicit non-exposure tests**

Before implementing the CLI, add tests that fail if host-service actions are
added to the shared setup action catalog:

```rust
#[test]
fn setup_catalog_does_not_expose_host_service_actions() {
    let catalog = crate::dispatch::setup::catalog::actions();
    assert!(
        catalog.iter().all(|action| !action.name.starts_with("host_service.")),
        "host-service lifecycle commands must remain CLI-only"
    );
}
```

If there is already a route-level setup API test helper, add a companion test
that a public `/v1/setup` request cannot call `host_service.restart` or
`host_service.uninstall`. If such a helper does not exist, the catalog
non-exposure test is the required guard for this plan.

- [ ] **Step 2: Create the host service helper module with safe unit text**

Create `crates/lab/src/dispatch/setup/host_service.rs`. Use
`crate::dispatch::error::ToolError`, not `crate::mcp::error::ToolError`.

Required unit behavior:
- `ExecStart=%h/.local/bin/labby serve`
- `EnvironmentFile=-%h/.labby/.env`
- `WorkingDirectory=%h`
- `Restart=on-failure`
- `RestartSec=3`
- `StartLimitIntervalSec=60`
- `StartLimitBurst=5`
- `KillSignal=SIGINT`
- No hard-coded `--host` or `--port`; binding stays in `~/.labby/.env` or Labby config.

Implement `unit_text()` as a fixed unit template instead of interpolating raw
paths. The durable binary path is `%h/.local/bin/labby`; `host-sync` is
responsible for copying that binary before restart.

Add unit tests:
- unit contains `ExecStart=%h/.local/bin/labby serve`
- unit does not contain `--host 0.0.0.0` or `--port 8765`
- unit contains `EnvironmentFile=-%h/.labby/.env`
- unit contains restart limit settings
- `unit_path(home)` resolves to `<home>/.config/systemd/user/labby.service`

- [ ] **Step 3: Implement typed CLI-only service operations**

In `host_service.rs`, implement:

```rust
pub(crate) async fn unit() -> Result<String, ToolError>;
pub(crate) async fn install() -> Result<HostServiceOutcome, ToolError>;
pub(crate) async fn status() -> Result<HostServiceStatus, ToolError>;
pub(crate) async fn restart() -> Result<HostServiceOutcome, ToolError>;
pub(crate) async fn uninstall() -> Result<HostServiceOutcome, ToolError>;
```

`HostServiceStatus` must be richer than `installed + active`:

```rust
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
    pub docker_labby_master_running: Option<bool>,
}
```

`HostServiceOutcome` must preserve diagnostics:

```rust
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct HostServiceOutcome {
    pub ok: bool,
    pub changed: bool,
    pub message: String,
    pub unit_path: PathBuf,
    pub stdout: String,
    pub stderr: String,
}
```

System command rules:
- Use `tokio::process::Command` or `tokio::task::spawn_blocking`; never block a Tokio worker with plain `std::process::Command` inside async code.
- Wrap every `systemctl --user` call in `tokio::time::timeout`.
- Return `ToolError::Sdk { sdk_kind: "internal_error", ... }` with the full command, exit status, stdout, and stderr on failure. Do not invent `systemctl_failed` unless `docs/dev/ERRORS.md` is updated in the same task.
- Use atomic unit writes: write a temp sibling, `sync_all` if practical, then rename.
- If `systemd-analyze --user verify <unit>` exists, run it before `daemon-reload`; if it is missing, skip with a diagnostic in the outcome.
- `uninstall()` must fail when `disable --now` fails unless the unit is already absent.

Readiness and port rules:
- Before `install()` or `restart()`, detect the holder of the configured local port when possible. If another `labby` process or Docker `labby-master` owns it, return a diagnostic instead of entering a restart loop.
- After `install()` and `restart()`, poll `http://127.0.0.1:8765/ready` with a deadline and include the last error in `stderr` if readiness fails.
- `status()` should report `/proc/<pid>/exe` when `main_pid` is available and flag paths ending in ` (deleted)`.

- [ ] **Step 4: Wire the module for local CLI use only**

In `crates/lab/src/dispatch/setup.rs`, add:

```rust
pub(crate) mod host_service;
```

Do not add `host_service.*` entries to `setup/catalog.rs`.
Do not add `host_service.*` arms to `setup/dispatch.rs`.
Do not add MCP or HTTP routes for host service lifecycle.

- [ ] **Step 5: Add CLI subcommands**

In `crates/lab/src/cli/setup.rs`, add:

```rust
#[derive(Debug, Args)]
pub struct HostServiceArgs {
    #[command(subcommand)]
    pub command: HostServiceCommand,
}

#[derive(Debug, Subcommand)]
pub enum HostServiceCommand {
    /// Print the user systemd unit that Labby would install.
    Unit,
    /// Install and start labby.service under systemd --user.
    Install {
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Read labby.service status.
    Status,
    /// Restart labby.service.
    Restart {
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Stop, disable, and remove labby.service.
    Uninstall {
        #[arg(short = 'y', long)]
        yes: bool,
    },
}
```

The CLI match arm calls `crate::dispatch::setup::host_service::{unit, install,
status, restart, uninstall}` directly and prints the returned serializable value
through the existing output helpers.

Destructive commands must require `-y/--yes` in the CLI shim:
- `install -y`
- `restart -y`
- `uninstall -y`

No `confirm` parameter is passed through JSON, because there is no dispatch
action and no remote elicitation path.

- [ ] **Step 6: Add parser, unit, and non-exposure tests**

Run and make pass:

```bash
cargo test -p labby host_service --all-features
cargo test -p labby parses_host_service_subcommands --all-features
cargo test -p labby setup_catalog_does_not_expose_host_service_actions --all-features
```

Expected:
- host service unit and helper tests PASS
- CLI parser tests PASS
- setup catalog non-exposure test PASS

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/setup.rs \
  crates/lab/src/dispatch/setup/host_service.rs \
  crates/lab/src/cli/setup.rs
git commit -m "feat: add local host labby service management"
```

---

### Superseded Task 2 Draft: Do Not Implement

This section is retained only as review context. It exposed host service
lifecycle through shared setup dispatch actions, which engineering review
rejected because the controls would become reachable through MCP or HTTP.

<details>
<summary>Superseded draft retained for context only</summary>

**Files:**
- Create: `crates/lab/src/dispatch/setup/host_service.rs`
- Modify: `crates/lab/src/dispatch/setup.rs`
- Modify: `crates/lab/src/dispatch/setup/catalog.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`
- Modify: `crates/lab/src/cli/setup.rs`

**Interfaces:**
- Consumes: `setup` dispatch action routing in `dispatch.rs`
- Produces: `host_service::unit_text(bin_path: &Path, working_dir: &Path) -> String`
- Produces dispatch actions:
  - `host_service.unit`
  - `host_service.install`
  - `host_service.status`
  - `host_service.restart`
  - `host_service.uninstall`
- Produces CLI:
  - `labby setup host-service unit`
  - `labby setup host-service install -y`
  - `labby setup host-service status`
  - `labby setup host-service restart -y`
  - `labby setup host-service uninstall -y`

- [ ] **Step 1: Write host service unit tests**

Create `crates/lab/src/dispatch/setup/host_service.rs` with this initial content:

```rust
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::mcp::error::ToolError;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct HostServiceStatus {
    pub installed: bool,
    pub active: Option<bool>,
    pub unit_path: PathBuf,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct HostServiceOutcome {
    pub ok: bool,
    pub changed: bool,
    pub message: String,
    pub unit_path: PathBuf,
}

pub(crate) fn unit_text(bin_path: &Path, working_dir: &Path) -> String {
    format!(
        r#"[Unit]
Description=Labby host gateway
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={} serve --host 0.0.0.0 --port 8765
WorkingDirectory={}
EnvironmentFile=-%h/.labby/.env
Restart=always
RestartSec=2
KillSignal=SIGINT

[Install]
WantedBy=default.target
"#,
        bin_path.display(),
        working_dir.display()
    )
}

pub(crate) fn unit_dir(home: &Path) -> PathBuf {
    home.join(".config/systemd/user")
}

pub(crate) fn unit_path(home: &Path) -> PathBuf {
    unit_dir(home).join("labby.service")
}

pub(crate) fn current_home() -> Result<PathBuf, ToolError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "HOME is not set; cannot manage user systemd service".to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_uses_host_binary_and_lab_env() {
        let unit = unit_text(
            Path::new("/home/jmagar/.local/bin/labby"),
            Path::new("/home/jmagar/workspace/lab"),
        );

        assert!(unit.contains("Description=Labby host gateway"));
        assert!(unit.contains("ExecStart=/home/jmagar/.local/bin/labby serve --host 0.0.0.0 --port 8765"));
        assert!(unit.contains("WorkingDirectory=/home/jmagar/workspace/lab"));
        assert!(unit.contains("EnvironmentFile=-%h/.labby/.env"));
        assert!(unit.contains("Restart=always"));
    }

    #[test]
    fn unit_path_lives_under_user_systemd_dir() {
        let home = Path::new("/home/example");

        assert_eq!(
            unit_path(home),
            PathBuf::from("/home/example/.config/systemd/user/labby.service")
        );
    }
}
```

- [ ] **Step 2: Run tests to verify the new unit helper passes**

Run:

```bash
cargo test -p labby host_service --all-features
```

Expected: PASS for the local helper tests.

- [ ] **Step 3: Implement install/status/restart/uninstall helpers**

Append these functions to `host_service.rs`:

```rust
pub(crate) fn install(confirm: bool) -> Result<HostServiceOutcome, ToolError> {
    if !confirm {
        return Err(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "host_service.install requires confirm=true".to_string(),
        });
    }
    let home = current_home()?;
    let path = unit_path(&home);
    let bin = home.join(".local/bin/labby");
    let cwd = std::env::current_dir().map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to resolve current directory: {err}"),
    })?;
    let text = unit_text(&bin, &cwd);
    std::fs::create_dir_all(unit_dir(&home)).map_err(io_error)?;
    let changed = std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str());
    if changed {
        std::fs::write(&path, text).map_err(io_error)?;
    }
    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", "--now", "labby.service"])?;
    Ok(HostServiceOutcome {
        ok: true,
        changed,
        message: "labby.service installed and running".to_string(),
        unit_path: path,
    })
}

pub(crate) fn status() -> Result<HostServiceStatus, ToolError> {
    let home = current_home()?;
    let path = unit_path(&home);
    let installed = path.is_file();
    let active = if installed {
        Some(run_systemctl_status(&["is-active", "--quiet", "labby.service"])?)
    } else {
        None
    };
    Ok(HostServiceStatus {
        installed,
        active,
        unit_path: path,
    })
}

pub(crate) fn restart(confirm: bool) -> Result<HostServiceOutcome, ToolError> {
    if !confirm {
        return Err(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "host_service.restart requires confirm=true".to_string(),
        });
    }
    run_systemctl(&["restart", "labby.service"])?;
    let home = current_home()?;
    Ok(HostServiceOutcome {
        ok: true,
        changed: true,
        message: "labby.service restarted".to_string(),
        unit_path: unit_path(&home),
    })
}

pub(crate) fn uninstall(confirm: bool) -> Result<HostServiceOutcome, ToolError> {
    if !confirm {
        return Err(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: "host_service.uninstall requires confirm=true".to_string(),
        });
    }
    let home = current_home()?;
    let path = unit_path(&home);
    let _ = run_systemctl(&["disable", "--now", "labby.service"]);
    let changed = if path.exists() {
        std::fs::remove_file(&path).map_err(io_error)?;
        true
    } else {
        false
    };
    run_systemctl(&["daemon-reload"])?;
    Ok(HostServiceOutcome {
        ok: true,
        changed,
        message: "labby.service uninstalled".to_string(),
        unit_path: path,
    })
}

fn run_systemctl(args: &[&str]) -> Result<(), ToolError> {
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(io_error)?;
    if output.status.success() {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "systemctl_failed".to_string(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn run_systemctl_status(args: &[&str]) -> Result<bool, ToolError> {
    let status = std::process::Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .map_err(io_error)?;
    Ok(status.success())
}

fn io_error(err: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: err.to_string(),
    }
}
```

- [ ] **Step 4: Add dispatch actions**

In `crates/lab/src/dispatch/setup.rs`, add:

```rust
pub(crate) mod host_service;
```

In `crates/lab/src/dispatch/setup/catalog.rs`, add these `ActionSpec` entries:

```rust
ActionSpec {
    name: "host_service.unit",
    description: "Render the user systemd unit for the host Labby gateway",
    destructive: false,
    requires_admin: false,
    returns: "string",
    params: &[],
},
ActionSpec {
    name: "host_service.install",
    description: "Install and start the host user systemd service for labby serve",
    destructive: true,
    requires_admin: true,
    returns: "HostServiceOutcome",
    params: &[ParamSpec {
        name: "confirm",
        ty: "boolean",
        required: true,
        description: "Must be true to install and start the service",
    }],
},
ActionSpec {
    name: "host_service.status",
    description: "Read the host user systemd service status for labby.service",
    destructive: false,
    requires_admin: false,
    returns: "HostServiceStatus",
    params: &[],
},
ActionSpec {
    name: "host_service.restart",
    description: "Restart the host user systemd service for labby serve",
    destructive: true,
    requires_admin: true,
    returns: "HostServiceOutcome",
    params: &[ParamSpec {
        name: "confirm",
        ty: "boolean",
        required: true,
        description: "Must be true to restart the service",
    }],
},
ActionSpec {
    name: "host_service.uninstall",
    description: "Disable and remove the host user systemd service for labby serve",
    destructive: true,
    requires_admin: true,
    returns: "HostServiceOutcome",
    params: &[ParamSpec {
        name: "confirm",
        ty: "boolean",
        required: true,
        description: "Must be true to stop and remove the service",
    }],
},
```

In `crates/lab/src/dispatch/setup/dispatch.rs`, add match arms:

```rust
"host_service.unit" => {
    let home = super::host_service::current_home()?;
    let unit = super::host_service::unit_text(
        &home.join(".local/bin/labby"),
        &std::env::current_dir().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to resolve current directory: {err}"),
        })?,
    );
    to_json(unit)
}
"host_service.install" => {
    to_json(super::host_service::install(parse_required_confirm(&params)?)?)
}
"host_service.status" => to_json(super::host_service::status()?),
"host_service.restart" => {
    to_json(super::host_service::restart(parse_required_confirm(&params)?)?)
}
"host_service.uninstall" => {
    to_json(super::host_service::uninstall(parse_required_confirm(&params)?)?)
}
```

Add this helper near the other parse helpers:

```rust
fn parse_required_confirm(params: &Value) -> Result<bool, ToolError> {
    let value = params.get("confirm").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "missing_param".to_string(),
        message: "missing required param `confirm`".to_string(),
    })?;
    parse_required_bool(value, "confirm")
}
```

- [ ] **Step 5: Add CLI subcommands**

In `crates/lab/src/cli/setup.rs`, add:

```rust
#[derive(Debug, Args)]
pub struct HostServiceArgs {
    #[command(subcommand)]
    pub command: HostServiceCommand,
}

#[derive(Debug, Subcommand)]
pub enum HostServiceCommand {
    /// Print the user systemd unit that Labby would install.
    Unit,
    /// Install and start labby.service under systemd --user.
    Install {
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Read labby.service status.
    Status,
    /// Restart labby.service.
    Restart {
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Stop, disable, and remove labby.service.
    Uninstall {
        #[arg(short = 'y', long)]
        yes: bool,
    },
}
```

Add the enum variant:

```rust
/// Manage the host user systemd Labby gateway service.
HostService(HostServiceArgs),
```

Add this match arm in `run_command`:

```rust
SetupCommand::HostService(args) => {
    run_host_service_command(args, format).await?;
}
```

Add this function:

```rust
async fn run_host_service_command(args: HostServiceArgs, format: OutputFormat) -> Result<()> {
    let (action, params) = match args.command {
        HostServiceCommand::Unit => ("host_service.unit", json!({})),
        HostServiceCommand::Install { yes } => {
            if !yes {
                anyhow::bail!("setup host-service install is destructive; pass -y/--yes to confirm");
            }
            ("host_service.install", json!({ "confirm": true }))
        }
        HostServiceCommand::Status => ("host_service.status", json!({})),
        HostServiceCommand::Restart { yes } => {
            if !yes {
                anyhow::bail!("setup host-service restart is destructive; pass -y/--yes to confirm");
            }
            ("host_service.restart", json!({ "confirm": true }))
        }
        HostServiceCommand::Uninstall { yes } => {
            if !yes {
                anyhow::bail!("setup host-service uninstall is destructive; pass -y/--yes to confirm");
            }
            ("host_service.uninstall", json!({ "confirm": true }))
        }
    };
    let value = crate::dispatch::setup::dispatch(action, params).await?;
    print(&value, format)?;
    Ok(())
}
```

- [ ] **Step 6: Add parser and catalog tests**

Extend `crates/lab/src/cli/setup.rs` tests:

```rust
#[test]
fn parses_host_service_subcommands() {
    for command in ["unit", "install", "status", "restart", "uninstall"] {
        let mut args = vec!["labby", "setup", "host-service", command];
        if matches!(command, "install" | "restart" | "uninstall") {
            args.push("-y");
        }
        let cli = crate::cli::Cli::try_parse_from(args).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        assert!(matches!(args.command, Some(SetupCommand::HostService(_))));
    }
}
```

Extend `setup_catalog_covers_dispatch_actions` in `crates/lab/src/dispatch/setup/dispatch.rs`:

```rust
"host_service.unit",
"host_service.install",
"host_service.status",
"host_service.restart",
"host_service.uninstall",
```

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test -p labby setup::host_service --all-features
cargo test -p labby parses_host_service_subcommands --all-features
cargo test -p labby setup_catalog_covers_dispatch_actions --all-features
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/setup.rs \
  crates/lab/src/dispatch/setup/host_service.rs \
  crates/lab/src/dispatch/setup/catalog.rs \
  crates/lab/src/dispatch/setup/dispatch.rs \
  crates/lab/src/cli/setup.rs
git commit -m "feat: add host labby service setup"
```

---

</details>

### Task 3: Host-First Developer Recipes And Documentation

**Files:**
- Modify: `Justfile`
- Modify: `README.md`
- Modify: `CLAUDE.md`
- Create: `docs/runtime/HOST_GATEWAY.md`

**Interfaces:**
- Consumes: `labby setup host-service ...` from Task 2.
- Produces: host-first recipes:
  - `just host-sync`
  - `just host-service-install`
  - `just host-service-restart`
  - `just host-service-status`
  - `just dev-container`
  - `just dev-container-debug`

- [ ] **Step 1: Update Justfile recipes**

Modify `Justfile` so the host service is the default development gateway path:
the host recipes must copy a durable binary to `~/.local/bin/labby`, not symlink
to `target/` or the current worktree.

```make
# Build release-fast binary when stale, update ~/.local/bin/labby, and restart
# the host user service. This is the preferred Labby gateway workflow because
# stdio MCP tools, SSH config, agent caches, and credentials live on the host.
host-sync:
    #!/usr/bin/env bash
    set -euo pipefail
    repo="$(pwd)"
    profile="{{local_release_profile}}"
    if command -v mold >/dev/null 2>&1; then
      export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-fuse-ld=mold"
    fi
    LAB_TARGET_DIR="${CARGO_TARGET_DIR:-target}"
    case "$LAB_TARGET_DIR" in
      /*) LABBY_BIN="$LAB_TARGET_DIR/$profile/labby" ;;
      *)  LABBY_BIN="$repo/$LAB_TARGET_DIR/$profile/labby" ;;
    esac
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-16}" cargo build --workspace --all-features --profile "$profile" --bin labby
    mkdir -p ~/.local/bin
    if [ -x ~/.local/bin/labby ]; then
      cp -f ~/.local/bin/labby ~/.local/bin/labby.prev
    fi
    install -D -m 755 "$LABBY_BIN" ~/.local/bin/labby.new
    mv ~/.local/bin/labby.new ~/.local/bin/labby
    if systemctl --user is-enabled --quiet labby.service; then
      systemctl --user restart labby.service
      ~/.local/bin/labby setup host-service status --json
      curl -fsS http://127.0.0.1:8765/ready >/dev/null
    else
      echo "labby.service is not installed; run: just host-service-install"
    fi

host-service-install:
    #!/usr/bin/env bash
    set -euo pipefail
    just host-sync
    ~/.local/bin/labby setup host-service install -y

host-service-restart:
    ~/.local/bin/labby setup host-service restart -y
    curl -fsS http://127.0.0.1:8765/ready >/dev/null

host-service-status:
    ~/.local/bin/labby setup host-service status --json

# Explicit container compatibility path. Prefer host-sync for normal gateway
# development; this remains useful for prod-like image smoke and Docker-specific
# ACP adapter changes.
dev-container: web-build build-release
    docker compose -f docker-compose.yml restart

dev-container-debug:
    #!/usr/bin/env bash
    set -euo pipefail
    nightly_rustc=$(rustup which --toolchain nightly rustc)
    RUSTC="$nightly_rustc" RUSTC_WRAPPER="" RUSTFLAGS="-C link-arg=-fuse-ld=mold -Z codegen-backend=cranelift" \
        cargo build -p labby --all-features
    install -D -m 755 target/debug/labby bin/labby
    docker compose -f docker-compose.yml restart
```

Keep `sync-container` and `container-sync` intact for now, but update comments so they are not described as the normal path.

- [ ] **Step 2: Update README command table**

In `README.md`, replace the dev command rows with:

```markdown
just host-service-install # install/start labby.service under systemd --user
just host-sync            # release-fast rebuild + install ~/.local/bin/labby + restart host service
just host-service-status  # inspect the host Labby gateway service
just dev-container        # explicit Docker compatibility/prod-like smoke path
just dev-container-debug  # explicit Docker debug binary path
```

Add this paragraph before the Dev Container section:

```markdown
### Host Gateway Runtime

The default local and node-a gateway runtime is the host user service:
`~/.local/bin/labby serve` managed by `systemd --user` as `labby.service`.
This keeps stdio MCP tools, SSH config, local binaries, agent caches, and
credentials in the same namespace as the gateway. Use `just host-service-install`
once, then `just host-sync` for ordinary Rust changes. Docker remains available
for prod-like image smoke and adapter-container work, but it is no longer the
preferred agent gateway runtime.
```

- [ ] **Step 3: Update CLAUDE.md runtime guidance**

In `CLAUDE.md`, replace the Docker-first development paragraph with:

```markdown
### Host Labby gateway

The normal local/node-a Labby gateway should run on the host as
`systemd --user` service `labby.service`, executing `~/.local/bin/labby serve`.
This is preferred over the Docker dev container because the gateway launches
stdio MCP tools and depends on host SSH config, agent credentials, local
binaries, and user caches. Use `just host-service-install` once, then
`just host-sync` after Rust changes.

The Docker Compose stack is still supported for prod-like image smoke and
Docker-specific ACP adapter work. Use `just dev-container` or
`just dev-container-debug` explicitly when testing that path.
```

- [ ] **Step 4: Create host gateway runbook**

Create `docs/runtime/HOST_GATEWAY.md`:

```markdown
# Host Labby Gateway

The preferred Labby gateway runtime is a host user service:

```bash
~/.local/bin/labby serve
```

It runs as `labby.service` under `systemd --user`. Bind host, port, auth, and
upstream gateway configuration continue to come from `~/.labby/.env` and Labby
config. Do not bake public bind settings into the systemd unit.

## Install

```bash
just host-service-install
systemctl --user --no-pager --full status labby.service
```

## Migrate From The Docker Dev Container

Stop the container before starting the host service because both runtimes bind
port `8765`:

```bash
docker compose -f docker-compose.yml stop labby-master
just host-service-install
curl -fsS http://127.0.0.1:8765/ready
labby gateway list
```

## Update The Running Host Gateway

```bash
just host-sync
curl -fsS http://127.0.0.1:8765/ready
labby gateway code exec --json --code 'async () => 1'
```

## Verify The Public MCP Route

```bash
TOKEN=$(awk -F= '/^LAB_MCP_HTTP_TOKEN=/{print $2}' ~/.labby/.env)
curl -fsS -H "Authorization: Bearer $TOKEN" https://lab.example.com/mcp
```

Then verify from Codex by calling the Labby Code Mode MCP tool through the
public MCP route with:

```javascript
async () => 1
```

Expected result:

```json
{"result":1}
```

Also prove the public route is backed by the host service:

```bash
pid=$(systemctl --user show labby.service --property=MainPID --value)
readlink "/proc/$pid/exe"
docker inspect -f '{{.State.Running}}' labby-master 2>/dev/null || true
```

Expected: `/proc/$pid/exe` points at `/home/jmagar/.local/bin/labby`, and the
Docker container is not the process answering the public route.

## Roll Back To Docker

```bash
systemctl --user disable --now labby.service
docker compose -f docker-compose.yml up -d labby-master --no-deps
curl -fsS http://127.0.0.1:8765/ready
```

## Known Failure Mode: Deleted Executable

If Code Mode reports `failed to spawn Code Mode runner` after replacing a
running binary, check:

```bash
pid=$(pgrep -u "$USER" -f 'labby serve' | head -n1)
readlink "/proc/$pid/exe"
```

If the path ends in `(deleted)`, restart the service:

```bash
systemctl --user restart labby.service
```
```

- [ ] **Step 5: Run docs and recipe smoke**

Run:

```bash
just --list | grep -E 'host-sync|host-service|dev-container'
```

Expected: all new recipes are listed.

Run:

```bash
cargo test -p labby parses_host_service_subcommands --all-features
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Justfile README.md CLAUDE.md docs/runtime/HOST_GATEWAY.md
git commit -m "docs: make host labby gateway the default"
```

---

### Task 4: Live Migration And End-To-End Verification

**Files:**
- No source files required.
- Uses: `~/.local/bin/labby`
- Uses: `~/.labby/.env`
- Uses: `~/.config/systemd/user/labby.service`
- Uses: Codex config entry for `mcp_servers.labby.url = "https://lab.example.com/mcp"`

**Interfaces:**
- Consumes: `just host-service-install`, `just host-sync`
- Produces: live host service running Labby on port `8765`
- Produces: verified Labby MCP Code Mode call through the public MCP route

- [ ] **Step 1: Capture rollback and port preflight state**

Run:

```bash
docker compose -f docker-compose.yml ps labby-master
ss -ltnp 'sport = :8765' || true
curl -fsS http://127.0.0.1:8765/ready || true
```

Expected: you know which process currently owns `8765`, and the container path
can still be restored if the host install fails.

- [ ] **Step 2: Stop the container runtime**

Run:

```bash
docker compose -f docker-compose.yml stop labby-master
```

Expected: container stops and frees host port `8765`.

- [ ] **Step 3: Install and start the host service**

Run:

```bash
just host-service-install
```

Expected: `labby.service` installs, starts, and reports a passing local
readiness check.

- [ ] **Step 4: Verify local readiness**

Run:

```bash
curl -fsS http://127.0.0.1:8765/ready
```

Expected: HTTP 200 readiness response.

- [ ] **Step 5: Verify the running executable is the durable host binary**

Run:

```bash
pid=$(systemctl --user show labby.service --property=MainPID --value)
readlink "/proc/$pid/exe"
```

Expected: path is `/home/jmagar/.local/bin/labby` and does not end with
` (deleted)`.

- [ ] **Step 6: Verify Code Mode locally**

Run:

```bash
labby gateway code exec --json --code 'async () => 1'
```

Expected:

```json
{"result":1,"calls":[],"logs":[]}
```

- [ ] **Step 7: Verify Synapse through Labby locally**

Run:

```bash
labby gateway code exec --json --code 'async () => {
  const result = await callTool("synapse::scout", { action: "nodes" });
  return { count: result.hosts.length, names: result.hosts.map(h => h.name).sort() };
}'
```

Expected: `count` is at least `1` and `names` includes `node-a`.

- [ ] **Step 8: Verify the public MCP route**

Use mcporter or an equivalent MCP client to initialize and call Code Mode
through `https://lab.example.com/mcp` with the bearer token from `~/.labby/.env`.
Do not treat public `/health` as sufficient proof.

Expected: MCP initialize succeeds, then the Code Mode call returns
`{"result":1}`.

- [ ] **Step 9: Verify Codex MCP tool path**

From Codex, call the Labby MCP Code Mode tool with:

```javascript
async () => 1
```

Expected: the MCP result succeeds with `{"result":1}` instead of `failed to spawn Code Mode runner`.

- [ ] **Step 10: Prove the public route is backed by the host service**

Run:

```bash
pid=$(systemctl --user show labby.service --property=MainPID --value)
readlink "/proc/$pid/exe"
docker inspect -f '{{.State.Running}}' labby-master 2>/dev/null || true
```

Expected: the running Labby process is `/home/jmagar/.local/bin/labby`; the
container is stopped or otherwise not serving the public MCP request.

- [ ] **Step 11: Validate rollback path**

Run the rollback commands from `docs/runtime/HOST_GATEWAY.md` in a controlled
window, confirm Docker can answer `/ready`, then switch back to the host
service. This proves the migration remains reversible before relying on it.

- [ ] **Step 12: Run full verification**

Run:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
```

Expected: PASS.

- [ ] **Step 13: Commit any final docs corrections**

If live migration reveals a command or path correction, update `docs/runtime/HOST_GATEWAY.md` and commit:

```bash
git add docs/runtime/HOST_GATEWAY.md
git commit -m "docs: record host gateway migration verification"
```

---

## Engineering Review Coverage

| Review finding | Plan response |
|---|---|
| Host service lifecycle was remotely reachable through setup dispatch | Task 2 is now CLI-only; setup catalog/dispatch/API/MCP exposure is explicitly forbidden and tested. |
| New dispatch snippets imported MCP-layer `ToolError` | Task 1 and Task 2 now require `crate::dispatch::error::ToolError`. |
| Code Mode fallback could spawn a stale or incompatible `labby` | Task 1 removed automatic fallback; only an explicit validated `LAB_CODE_MODE_RUNNER_EXE` override is allowed. |
| Systemd unit hard-coded public host/port and mutable cwd | Task 2 unit uses `%h/.local/bin/labby serve`, `%h` working directory, and config/env-owned bind settings. |
| `systemctl` calls could block async dispatch | Task 2 removes remote dispatch and requires async/timeout-wrapped process execution for CLI helpers. |
| Service status was too lossy | Task 2 requires `LoadState`, `ActiveState`, `SubState`, `MainPID`, `ExecMainStatus`, process exe, readiness, and Docker state. |
| Unit writes and path rendering were unsafe | Task 2 uses a fixed `%h` unit template and atomic writes, with optional `systemd-analyze --user verify`. |
| `host-sync` could leave a deleted parent executable after failed restart | Task 3 copies a durable binary, keeps a previous binary, and makes restart/readiness visible. |
| Public verification could pass while MCP was broken or still routed to Docker | Task 4 verifies MCP initialize/call through `https://lab.example.com/mcp` and checks the host service process. |
| Port conflicts and restart loops were under-specified | Task 2 and Task 4 add port-holder preflight, readiness polling, and systemd start-limit settings. |

### Failure Modes To Prove

| New path | Failure | Required rescue | User sees | Logged/returned |
|---|---|---|---|---|
| Code Mode runner resolver | Parent executable is deleted after `host-sync` but service restart failed | Restart `labby.service`; optional explicit override only after validation | Clear restart guidance instead of hang | Error includes stale path and `LAB_CODE_MODE_RUNNER_EXE` hint |
| Host service install | Port `8765` is still owned by Docker or manual Labby | Stop owner or roll back to Docker | Install fails before restart loop | Port owner and systemctl/journal diagnostics |
| Host service restart | Unit starts but `/ready` never passes | Restore previous binary or rollback to Docker | Restart command returns failure | Last readiness error plus service status |
| Public MCP route | SWAG still targets container or stale backend | Fix proxy/backend, then retry MCP initialize | Public MCP call fails despite local ready | MCP client error plus host/container process check |
| Uninstall/rollback | `systemctl --user disable --now` fails | Leave unit file intact and report failure | Rollback stops before deleting unit | Full `systemctl` stdout/stderr/status |

### Not In Scope

- Systemd sandboxing options such as `NoNewPrivileges` and `ProtectSystem`; add after compatibility testing.
- Socket activation or `Type=notify` as the default. `Type=simple` remains acceptable unless the `systemd` feature becomes part of the normal build.
- UI controls for host service status.
- New remote scopes more granular than `lab:admin`; this plan avoids the remote lifecycle surface entirely.
- Automatic runner fallback with protocol negotiation. The safer initial behavior is an explicit restart-needed error.

---

## Self-Review

**Spec coverage:** The plan covers the observed MCP failure, host-first gateway setup, Docker demotion, operator migration, and end-to-end verification through both local CLI and Codex MCP paths.

**Placeholder scan:** No placeholder markers or open-ended edge-case instructions remain.

**Type consistency:** `HostServiceOutcome`, `HostServiceStatus`, `resolve_runner_exe`, and `resolve_runner_exe_from` are introduced before later tasks consume them. Host service lifecycle commands are CLI-only and intentionally do not have dispatch action names.
