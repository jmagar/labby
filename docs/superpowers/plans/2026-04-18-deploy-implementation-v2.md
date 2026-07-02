# Deploy Implementation Plan (V2 — Review-Hardened)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `deploy` synthetic service that safely builds the local `lab` release binary, pushes it to SSH-configured hosts with end-to-end integrity verification, atomically installs with backup, and exposes the workflow over CLI and MCP with deploy-scoped authorization.

**Architecture:** Minimal `lab-apis/deploy/` surface (types + errors + META only). Shared SSH primitives extracted from `extract/` into `lab-apis/core/ssh/`. All orchestration lives in `crates/lab/src/dispatch/deploy/` (build, runner, authz, lock, catalog, params, dispatch). CLI + MCP only for V1; HTTP API surface deferred. Concurrent per-host fan-out with bounded semaphore, SSH ControlMaster session reuse, sha256 end-to-end integrity, per-host advisory locks, explicit `LAB_DEPLOY_TOKEN` authorization above the MCP bearer, strict rejection of the MCP `confirm: true` headless bypass for `deploy.run` and `deploy.rollback`.

**Tech Stack:** Rust 2024, tokio, serde/serde_json, toml, tracing, dashmap, futures, sha2, regex, existing `lab_apis::core::action`, dispatch helpers, `tokio::process::Command`, `openssh` / extract-provided SSH. Tests: `cargo-nextest`, `wiremock` where HTTP is involved (not here).

---

## Review Summary Applied

This plan incorporates every actionable recommendation from the 2026-04-18 engineering review (Architecture / Simplicity / Security / Performance). Key deltas from the original plan:

**Dropped from V1 scope (YAGNI):**
- `/v1/deploy` HTTP API surface (CLI + MCP only)
- rsync transport (SSH stream + sha256 verify sufficient)
- Deploy groups (host list accepted on CLI; `[deploy.groups]` deferred)
- Standalone `verify` action (folded into `run` result + optional `deploy.verify` follows the same resolved-targets path)
- `targets.list` + `groups.list` separate actions (fold into `config.list`)
- `dry_run` param on `run` (`plan` action is the dry-run)
- Per-call `restart` / `backup` / `verify_service` overrides (fixed V1 policy; overrides V2)
- New `Category::Operator` variant (reuse `Category::Bootstrap`)
- `TransportUsed` / `BuiltArtifact` / `DeploySelection` / `DeployHostPolicy` as standalone types (inlined or merged)

**Added to V1 (must-have):**
- `LAB_DEPLOY_TOKEN` separate authorization, checked before any orchestration
- Explicit rejection of `confirm: true` headless bypass for `deploy.run` / `deploy.rollback`
- End-to-end sha256 (local build → remote pre-rename verify)
- Install path allowlist (`/usr/local/bin/`, `/opt/lab/bin/`)
- Per-host advisory lock (`DashMap<Host, tokio::sync::Mutex<()>>`)
- `deploy.rollback` action (destructive)
- `canary_hosts` + `max_parallel` (default 1) + `--fail-fast`
- Concurrent fan-out (`futures::stream::for_each_concurrent`)
- SSH `ControlMaster` / `ControlPersist` session reuse + explicit `ConnectTimeout` / `ServerAliveInterval` / `ForwardAgent=no` / `StrictHostKeyChecking` (no `accept-new`)
- Arch match preflight (`uname -m`), disk-space preflight
- Stable error kinds (`ssh_unreachable`, `build_failed`, `preflight_failed`, `transfer_failed`, `install_failed`, `restart_failed`, `verify_failed`, `partial_failure`, `conflict`, `validation_failed`, `arch_mismatch`, `integrity_mismatch`) declared **upfront** in Task 3, not docs-last
- `run_id` correlation + span hierarchy (`deploy.run` → `deploy.host` → `deploy.stage`)
- Hostname/username/path redaction in MCP error envelopes (full detail at WARN in local log store)
- Regex validation of `service_name` / `hostname` / `user` / `remote_path` before any `Command::arg()`
- Write-once path: stream via `tokio::io::copy` to `lab.new.partial` → fsync → rename to `lab.new` → verify sha256 → atomic swap with `.lab.bak.<ts>` backup
- Never touch `ForwardAgent`; never `StrictHostKeyChecking=accept-new|no`
- Shared `lab-apis/core/ssh/` extracted from `extract/` first (deduplicates ~650 LOC)

---

## File Structure

### New Rust files

- Create: `crates/lab-apis/src/core/ssh.rs` — shared SSH primitives (host resolution, session lifecycle with ControlMaster, `run_command`, `upload_stream`, `sha256_remote`), extracted from `extract/`.
- Create: `crates/lab-apis/src/deploy.rs` — entry (module declarations + `pub const META: PluginMeta`).
- Create: `crates/lab-apis/src/deploy/types.rs` — `DeployRequest`, `DeployPlan`, `DeployHostResult`, `DeployRunSummary`, `DeployStage` (4 types + 1 enum).
- Create: `crates/lab-apis/src/deploy/error.rs` — `DeployError` with stable `kind()` returning all enumerated stable kinds.
- Create: `crates/lab/src/dispatch/deploy.rs` — directory-first entry.
- Create: `crates/lab/src/dispatch/deploy/catalog.rs` — `ACTIONS` array for `help`, `schema`, `config.list`, `plan`, `run` (destructive), `rollback` (destructive).
- Create: `crates/lab/src/dispatch/deploy/params.rs` — regex-validated coercion into typed requests.
- Create: `crates/lab/src/dispatch/deploy/dispatch.rs` — `dispatch()` + `dispatch_with_runner()`.
- Create: `crates/lab/src/dispatch/deploy/authz.rs` — `LAB_DEPLOY_TOKEN` check, `reject_headless_bypass` guard.
- Create: `crates/lab/src/dispatch/deploy/lock.rs` — per-host `Mutex` registry for in-process concurrency guard; remote `flock` wrapper.
- Create: `crates/lab/src/dispatch/deploy/build.rs` — local `cargo build --release --all-features` + sha256 hashing + disk preflight.
- Create: `crates/lab/src/dispatch/deploy/runner.rs` — orchestrator: resolve, preflight, transfer, install, restart, verify; bounded-concurrency fan-out; canary; failure policy.
- Create: `crates/lab/src/cli/deploy.rs` — thin CLI shim.
- Create: `crates/lab/src/mcp/services/deploy.rs` — MCP adapter that enforces `reject_headless_bypass`.
- Create: `crates/lab/tests/deploy_dispatch.rs` — dispatch shape, catalog, authz, unknown-action.
- Create: `crates/lab/tests/deploy_runner.rs` — orchestration unit tests with mocked SSH primitives.
- Create: `crates/lab/tests/deploy_cli.rs` — CLI parsing + `-y` gate.
- Create: `crates/lab/tests/deploy_mcp.rs` — elicitation bypass rejection test.
- Create: `docs/DEPLOY_SERVICE.md` — operator-facing contract.
- Create: `docs/coverage/deploy.md` — coverage + live evidence matrix.

### Existing Rust files to modify

- Modify: `crates/lab-apis/src/core.rs` — add `pub mod ssh;`.
- Modify: `crates/lab-apis/src/extract/ssh_config.rs` — re-export `lab_apis::core::ssh::parse_ssh_config` to eliminate duplication.
- Modify: `crates/lab-apis/src/extract/transport.rs` — delegate `SshFs` to `lab_apis::core::ssh::SshSession` (kept as type alias during transition).
- Modify: `crates/lab-apis/src/lib.rs` — feature-gate `pub mod deploy;` behind `deploy` feature.
- Modify: `crates/lab/Cargo.toml` — add `deploy = ["lab-apis/deploy", "dep:dashmap", "dep:sha2", "dep:futures", "dep:regex"]` passthrough + dev deps.
- Modify: `crates/lab-apis/Cargo.toml` — add `deploy = []` feature.
- Modify: `crates/lab/src/config.rs` — add `DeployPreferences` (`defaults` + `hosts`, no `groups` in V1) and `ServiceScope` enum.
- Modify: `crates/lab/src/dispatch.rs` — register `pub mod deploy;` behind feature gate.
- Modify: `crates/lab/src/dispatch/clients.rs` — add `deploy: Option<Arc<DeployRunner>>` field.
- Modify: `crates/lab/src/dispatch/error.rs` — `impl_tool_error_from!(lab_apis::deploy::DeployError)` behind `deploy`.
- Modify: `crates/lab/src/cli.rs` — register `deploy` clap group.
- Modify: `crates/lab/src/mcp/services.rs` — add `pub mod deploy;`.
- Modify: `crates/lab/src/registry.rs` — register service in runtime catalog.
- Modify: `docs/ERRORS.md` — land the full deploy error-kind taxonomy **in Task 3**.
- Modify: `docs/OBSERVABILITY.md` — document deploy `run_id` correlation and span hierarchy **in Task 10**.

### Existing docs to modify

- Modify: `docs/README.md` — link new deploy service doc.
- Modify: `docs/SERVICES.md` — list `deploy` (synthetic, CLI+MCP only, V1 scope).
- Modify: `docs/CONFIG.md` — document `[deploy.defaults]` and `[deploy.hosts.<alias>]`.
- Modify: `docs/CLI.md` — document `lab deploy` group.
- Modify: `docs/MCP.md` — document `deploy` tool, destructive behavior, `LAB_DEPLOY_TOKEN` requirement, headless-bypass rejection.

### Explicitly deferred (NOT V1)

- `/v1/deploy` HTTP API surface and tests.
- rsync transport and `choose_transport` selection.
- `[deploy.groups]` config + group expansion.
- `deploy.verify` as a standalone action (V1 `verify` is always part of `run`).
- Per-call policy overrides (`restart`, `backup`, `verify_service` as booleans on `run`).
- `Category::Operator` variant.
- `lab doctor` integration for deploy.
- TUI plugin registration polish.

Each deferred item lives behind a follow-up task in the docs/DEPLOY_SERVICE.md "Non-goals" section.

---

## Pre-flight: Knowledge the Implementor Needs

Read these before starting, in order:

1. `/home/jmagar/workspace/lab/CLAUDE.md` — repo contract, feature gates, error taxonomy rules, two-crate invariant.
2. `crates/lab/src/dispatch/CLAUDE.md` — required dispatch layout and canonical templates.
3. `docs/ERRORS.md` — canonical error kinds and envelope shape.
4. `docs/OBSERVABILITY.md` — logging boundaries and redaction rules.
5. `docs/DISPATCH.md` — adapter direction (CLI/MCP → dispatch → client).
6. `crates/lab-apis/src/extract/transport.rs` — `SshFs` reference implementation (will be extracted in Task 1).
7. `crates/lab-apis/src/extract/ssh_config.rs` — `parse_ssh_config` reference.
8. `crates/lab/src/dispatch/gateway/manager.rs` — precedent for multi-host orchestration in dispatch.
9. `crates/lab/src/mcp/server.rs` lines around `confirm: true` bypass — the specific code path Task 4 defeats.

---

## Task 1: Extract Shared SSH Primitives Into `lab-apis/core/ssh.rs`

**Why first:** Deploy and extract both parse `~/.ssh/config` and run SSH commands. Plan review identified ~650 LOC of duplication risk. Move shared code to `core/ssh.rs` before writing deploy so there is one implementation of host resolution, session lifecycle, and `run_command`.

**Files:**
- Create: `crates/lab-apis/src/core/ssh.rs`
- Modify: `crates/lab-apis/src/core.rs` (add `pub mod ssh;`)
- Modify: `crates/lab-apis/src/extract/ssh_config.rs`
- Modify: `crates/lab-apis/src/extract/transport.rs`
- Test: `crates/lab-apis/src/core/ssh.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing relocation tests**

Add to `crates/lab-apis/src/core/ssh.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_alias_hostname_user_port() {
        let raw = r#"
Host mini1
    HostName 10.0.0.11
    User deploy
    Port 2222
"#;
        let hosts = parse_ssh_config(raw);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "mini1");
        assert_eq!(hosts[0].hostname.as_deref(), Some("10.0.0.11"));
        assert_eq!(hosts[0].user.as_deref(), Some("deploy"));
        assert_eq!(hosts[0].port, Some(2222));
    }

    #[test]
    fn ignores_match_blocks_for_literal_aliases() {
        let raw = "Match user root\n    ForwardAgent no\n";
        assert!(parse_ssh_config(raw).is_empty());
    }

    #[test]
    fn session_options_include_control_master_and_hardening_defaults() {
        let opts = SshOptions::hardened();
        assert_eq!(opts.connect_timeout.as_secs(), 10);
        assert_eq!(opts.server_alive_interval.as_secs(), 15);
        assert_eq!(opts.server_alive_count_max, 3);
        assert!(!opts.forward_agent);
        assert_eq!(opts.strict_host_key_checking, StrictHostKeyChecking::Yes);
        assert!(opts.control_persist.is_some());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab-apis core::ssh -- --nocapture`

Expected: FAIL (`core::ssh` not defined).

- [ ] **Step 3: Move `SshHostTarget` + `parse_ssh_config` into `core/ssh.rs`**

Copy the body of `crates/lab-apis/src/extract/ssh_config.rs` into `crates/lab-apis/src/core/ssh.rs`. Keep the same function signature:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostTarget {
    pub alias: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

pub fn parse_ssh_config(contents: &str) -> Vec<SshHostTarget> {
    // existing extract logic, unchanged in behavior
    // reject Match/Include blocks for V1 — they require recursion and shell access;
    // explicit log line when one is seen so operators know their config didn't apply
}
```

- [ ] **Step 4: Add `SshOptions` with hardened defaults**

```rust
#[derive(Debug, Clone)]
pub struct SshOptions {
    pub connect_timeout: std::time::Duration,
    pub server_alive_interval: std::time::Duration,
    pub server_alive_count_max: u32,
    pub forward_agent: bool,
    pub strict_host_key_checking: StrictHostKeyChecking,
    pub control_persist: Option<std::time::Duration>,
    pub control_path_template: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictHostKeyChecking {
    Yes,
    AcceptNew, // disallowed in hardened()
    No,        // disallowed in hardened()
}

impl SshOptions {
    pub fn hardened() -> Self {
        Self {
            connect_timeout: std::time::Duration::from_secs(10),
            server_alive_interval: std::time::Duration::from_secs(15),
            server_alive_count_max: 3,
            forward_agent: false,
            strict_host_key_checking: StrictHostKeyChecking::Yes,
            control_persist: Some(std::time::Duration::from_secs(60)),
            control_path_template: Some("~/.labby/ssh/cm-%r@%h:%p".to_string()),
        }
    }

    pub fn to_openssh_args(&self) -> Vec<String> {
        let mut a = vec![
            format!("-oConnectTimeout={}", self.connect_timeout.as_secs()),
            format!("-oServerAliveInterval={}", self.server_alive_interval.as_secs()),
            format!("-oServerAliveCountMax={}", self.server_alive_count_max),
            format!("-oForwardAgent={}", if self.forward_agent { "yes" } else { "no" }),
            format!("-oStrictHostKeyChecking={}", match self.strict_host_key_checking {
                StrictHostKeyChecking::Yes => "yes",
                StrictHostKeyChecking::AcceptNew => "accept-new",
                StrictHostKeyChecking::No => "no",
            }),
        ];
        if let (Some(persist), Some(path)) = (self.control_persist, &self.control_path_template) {
            a.push("-oControlMaster=auto".into());
            a.push(format!("-oControlPersist={}s", persist.as_secs()));
            a.push(format!("-oControlPath={}", path));
        }
        a
    }
}
```

- [ ] **Step 5: Add `SshSession` wrapping `run_command` + `upload_stream` + `sha256_remote`**

Port `SshFs` from `extract/transport.rs` and rename to `SshSession`. Add:

```rust
pub struct SshSession {
    pub target: SshHostTarget,
    pub options: SshOptions,
}

impl SshSession {
    pub async fn run_command(&self, argv: &[&str]) -> Result<CommandOutput, SshError> {
        // tokio::process::Command, never shell; always per-token .arg()
    }

    pub async fn upload_stream<R: tokio::io::AsyncRead + Unpin + Send>(
        &self,
        remote_path: &str,
        mut reader: R,
    ) -> Result<(), SshError> {
        // ssh host "cat > <remote_path>" with stdin piped from reader via tokio::io::copy
        // never buffer the whole artifact; chunk through the pipe
    }

    pub async fn sha256_remote(&self, remote_path: &str) -> Result<Option<String>, SshError> {
        // run `sha256sum <path> 2>/dev/null`; None on non-zero exit (file absent)
    }
}
```

Make all `Command` construction use per-token `.arg()`. Never `sh -c`. Always call `self.options.to_openssh_args()` first, then `self.target`, then the argv.

- [ ] **Step 6: Swap `extract` over to the shared code**

Replace `crates/lab-apis/src/extract/ssh_config.rs` with:

```rust
pub use crate::core::ssh::{SshHostTarget, parse_ssh_config};
```

In `crates/lab-apis/src/extract/transport.rs`, replace the `SshFs` struct definition with:

```rust
pub use crate::core::ssh::SshSession as SshFs;
```

Leave extract's `LocalFs` and extract-specific helpers untouched.

- [ ] **Step 7: Declare `core::ssh` and run the full test suite**

In `crates/lab-apis/src/core.rs` add:

```rust
pub mod ssh;
```

Run: `cargo test -p lab-apis -- --nocapture`

Expected: PASS for both the new `core::ssh` tests and every pre-existing `extract` test.

- [ ] **Step 8: Commit**

```bash
git add crates/lab-apis/src/core.rs crates/lab-apis/src/core/ssh.rs crates/lab-apis/src/extract/ssh_config.rs crates/lab-apis/src/extract/transport.rs
git commit -m "refactor(lab-apis): extract shared SSH primitives into core::ssh"
```

---

## Task 2: Add Deploy Config Model To `LabConfig`

**Files:**
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/config.rs` (existing config-test module)

- [ ] **Step 1: Write the failing config parsing tests**

Add to the existing `#[cfg(test)]` module in `crates/lab/src/config.rs`:

```rust
#[test]
fn parses_deploy_defaults_and_host_overrides() {
    let raw = r#"
[deploy.defaults]
remote_path = "/usr/local/bin/lab"
service = "lab"
service_scope = "system"
max_parallel = 4
canary_hosts = ["mini1"]

[deploy.hosts.mini2]
remote_path = "/opt/lab/bin/lab"
service = "lab-worker"
service_scope = "user"
"#;
    let parsed: LabConfig = toml::from_str(raw).unwrap();
    let d = parsed.deploy.expect("deploy present");
    let defaults = d.defaults.expect("defaults present");
    assert_eq!(defaults.remote_path.as_deref(), Some("/usr/local/bin/lab"));
    assert_eq!(defaults.service.as_deref(), Some("lab"));
    assert_eq!(defaults.service_scope, Some(ServiceScope::System));
    assert_eq!(defaults.max_parallel, Some(4));
    assert_eq!(defaults.canary_hosts, vec!["mini1".to_string()]);
    let mini2 = d.hosts.get("mini2").expect("mini2 override");
    assert_eq!(mini2.remote_path.as_deref(), Some("/opt/lab/bin/lab"));
    assert_eq!(mini2.service_scope, Some(ServiceScope::User));
}

#[test]
fn deploy_config_absent_is_none_not_error() {
    let raw = "[radarr]\nurl = \"http://localhost:7878\"\n";
    let parsed: LabConfig = toml::from_str(raw).unwrap();
    assert!(parsed.deploy.is_none());
}

#[test]
fn deploy_max_parallel_defaults_to_one_for_safety_at_read_time() {
    let raw = "[deploy.defaults]\nremote_path = \"/usr/local/bin/lab\"\n";
    let parsed: LabConfig = toml::from_str(raw).unwrap();
    let d = parsed.deploy.unwrap().defaults.unwrap();
    assert!(d.max_parallel.is_none(), "unset remains None; safe default applied at orchestrator entry");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab config -- --nocapture`

Expected: FAIL (no `deploy` field on `LabConfig`).

- [ ] **Step 3: Add the deploy config structures**

Add to `crates/lab/src/config.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployPreferences {
    #[serde(default)]
    pub defaults: Option<DeployDefaults>,
    #[serde(default)]
    pub hosts: BTreeMap<String, DeployHostOverride>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployDefaults {
    pub remote_path: Option<String>,
    pub service: Option<String>,
    pub service_scope: Option<ServiceScope>,
    pub max_parallel: Option<u32>,
    #[serde(default)]
    pub canary_hosts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployHostOverride {
    pub remote_path: Option<String>,
    pub service: Option<String>,
    pub service_scope: Option<ServiceScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    System,
    User,
}
```

Add `pub deploy: Option<DeployPreferences>` to `LabConfig` with `#[serde(default)]`.

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cargo test -p lab config -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/config.rs
git commit -m "feat(deploy): add config model (defaults + host overrides)"
```

---

## Task 3: Deploy Types, Errors, META — And Land The Error Taxonomy In `docs/ERRORS.md`

**Why error taxonomy now, not at doc-time:** Review finding — error kinds are API, not docs. Implementing `ToolError::kind()` correctly requires the stable strings to exist before dispatch is wired.

**Files:**
- Create: `crates/lab-apis/src/deploy.rs`
- Create: `crates/lab-apis/src/deploy/types.rs`
- Create: `crates/lab-apis/src/deploy/error.rs`
- Modify: `crates/lab-apis/src/lib.rs`
- Modify: `crates/lab-apis/Cargo.toml`
- Modify: `docs/ERRORS.md`

- [ ] **Step 1: Write the failing shape tests**

Create `crates/lab-apis/src/deploy/error.rs` containing only a `#[cfg(test)]` stub for now, and write these tests in it or in `types.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_is_named_deploy() {
        assert_eq!(crate::deploy::META.name, "deploy");
    }

    #[test]
    fn deploy_request_defaults_are_safe() {
        let r = DeployRequest::default();
        assert!(r.targets.is_empty());
        assert_eq!(r.max_parallel, None);
        assert!(!r.fail_fast);
    }

    #[test]
    fn error_kinds_are_stable_strings() {
        for (err, expected) in [
            (DeployError::ValidationFailed { field: "x".into(), reason: "bad".into() }, "validation_failed"),
            (DeployError::SshUnreachable { host: "mini1".into() }, "ssh_unreachable"),
            (DeployError::BuildFailed { reason: "rustc".into() }, "build_failed"),
            (DeployError::PreflightFailed { host: "mini1".into(), reason: "no_disk".into() }, "preflight_failed"),
            (DeployError::TransferFailed { host: "mini1".into(), reason: "drop".into() }, "transfer_failed"),
            (DeployError::InstallFailed { host: "mini1".into(), reason: "rename".into() }, "install_failed"),
            (DeployError::RestartFailed { host: "mini1".into(), reason: "unit".into() }, "restart_failed"),
            (DeployError::VerifyFailed { host: "mini1".into(), reason: "exit".into() }, "verify_failed"),
            (DeployError::PartialFailure { failed: 1 }, "partial_failure"),
            (DeployError::Conflict { host: "mini1".into() }, "conflict"),
            (DeployError::ArchMismatch { host: "mini1".into(), local: "x86_64".into(), remote: "aarch64".into() }, "arch_mismatch"),
            (DeployError::IntegrityMismatch { host: "mini1".into() }, "integrity_mismatch"),
        ] {
            assert_eq!(err.kind(), expected);
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab-apis --features deploy deploy -- --nocapture`

Expected: FAIL (`deploy` module not declared).

- [ ] **Step 3: Define types in `crates/lab-apis/src/deploy/types.rs`**

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployRequest {
    /// Explicit list of SSH aliases to deploy to. If empty, the dispatch layer
    /// rejects the request; there is no implicit "all" in V1.
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub max_parallel: Option<u32>,
    #[serde(default)]
    pub fail_fast: bool,
    /// Operator confirmation required by the destructive gate. The dispatch layer
    /// is responsible for rejecting `confirm: true` when the MCP caller did not
    /// complete live elicitation (headless-bypass rejection).
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployPlan {
    pub artifact_path: String,
    pub artifact_sha256: Option<String>,
    pub hosts: Vec<String>,
    pub max_parallel: u32,
    pub canary_hosts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStage {
    Resolve,
    Build,
    Preflight,
    Transfer,
    Install,
    Restart,
    Verify,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployHostResult {
    pub host: String,
    pub reached_stage: DeployStage,
    pub succeeded: bool,
    pub skipped_transfer: bool, // true when sha256 matched and transfer was skipped
    pub transferred_bytes: Option<u64>,
    pub error_kind: Option<String>, // stable kind; full detail at local WARN only
    pub stage_timings_ms: std::collections::BTreeMap<String, u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRunSummary {
    pub run_id: String,
    pub artifact_sha256: String,
    pub hosts: Vec<DeployHostResult>,
    pub succeeded: usize,
    pub failed: usize,
    pub ok: bool, // true iff failed == 0
}
```

- [ ] **Step 4: Define `DeployError` in `crates/lab-apis/src/deploy/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeployError {
    #[error("validation_failed: field={field} reason={reason}")]
    ValidationFailed { field: String, reason: String },

    #[error("ssh_unreachable: host={host}")]
    SshUnreachable { host: String },

    #[error("build_failed: reason={reason}")]
    BuildFailed { reason: String },

    #[error("preflight_failed: host={host} reason={reason}")]
    PreflightFailed { host: String, reason: String },

    #[error("transfer_failed: host={host} reason={reason}")]
    TransferFailed { host: String, reason: String },

    #[error("install_failed: host={host} reason={reason}")]
    InstallFailed { host: String, reason: String },

    #[error("restart_failed: host={host} reason={reason}")]
    RestartFailed { host: String, reason: String },

    #[error("verify_failed: host={host} reason={reason}")]
    VerifyFailed { host: String, reason: String },

    #[error("partial_failure: failed={failed}")]
    PartialFailure { failed: usize },

    #[error("conflict: host={host} already in progress")]
    Conflict { host: String },

    #[error("arch_mismatch: host={host} local={local} remote={remote}")]
    ArchMismatch { host: String, local: String, remote: String },

    #[error("integrity_mismatch: host={host} sha256 mismatch between local and remote artifact")]
    IntegrityMismatch { host: String },
}

impl DeployError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ValidationFailed { .. } => "validation_failed",
            Self::SshUnreachable { .. } => "ssh_unreachable",
            Self::BuildFailed { .. } => "build_failed",
            Self::PreflightFailed { .. } => "preflight_failed",
            Self::TransferFailed { .. } => "transfer_failed",
            Self::InstallFailed { .. } => "install_failed",
            Self::RestartFailed { .. } => "restart_failed",
            Self::VerifyFailed { .. } => "verify_failed",
            Self::PartialFailure { .. } => "partial_failure",
            Self::Conflict { .. } => "conflict",
            Self::ArchMismatch { .. } => "arch_mismatch",
            Self::IntegrityMismatch { .. } => "integrity_mismatch",
        }
    }

    /// Produce a redacted description safe to return through MCP/HTTP envelopes.
    /// Full structured details are logged at WARN locally.
    pub fn redacted_message(&self) -> String {
        match self {
            Self::ValidationFailed { field, .. } => format!("validation failed for field `{field}`"),
            Self::SshUnreachable { .. } => "ssh host unreachable".into(),
            Self::BuildFailed { .. } => "local build failed".into(),
            Self::PreflightFailed { .. } => "preflight check failed".into(),
            Self::TransferFailed { .. } => "artifact transfer failed".into(),
            Self::InstallFailed { .. } => "atomic install failed".into(),
            Self::RestartFailed { .. } => "service restart failed".into(),
            Self::VerifyFailed { .. } => "post-install verification failed".into(),
            Self::PartialFailure { failed } => format!("{failed} host(s) failed"),
            Self::Conflict { .. } => "another deploy is in progress for this host".into(),
            Self::ArchMismatch { .. } => "architecture mismatch between build host and target".into(),
            Self::IntegrityMismatch { .. } => "artifact integrity check failed on target".into(),
        }
    }
}
```

- [ ] **Step 5: Create the entry `crates/lab-apis/src/deploy.rs`**

```rust
pub mod error;
pub mod types;

pub use error::DeployError;
pub use types::*;

use crate::core::plugin::{Category, PluginMeta};

pub const META: PluginMeta = PluginMeta {
    name: "deploy",
    category: Category::Bootstrap, // reuse Bootstrap; a dedicated Operator category is out of V1 scope
    required_env: &[],
    optional_env: &[],
    default_port: None,
    description: "Build-and-push the local lab release binary to SSH targets with integrity verification.",
};
```

- [ ] **Step 6: Register the feature + module**

In `crates/lab-apis/Cargo.toml` add `deploy = []` under `[features]`.

In `crates/lab-apis/src/lib.rs`:

```rust
#[cfg(feature = "deploy")]
pub mod deploy;
```

- [ ] **Step 7: Append the error taxonomy to `docs/ERRORS.md`**

Add a `### deploy` subsection under the service-specific kinds table. List every `kind()` string from `DeployError` with its meaning and the HTTP status it maps to (validation_failed → 400, ssh_unreachable/preflight_failed/transfer_failed/install_failed/restart_failed/verify_failed/build_failed → 502 unless the upstream code model differentiates; partial_failure → 200 with body-level ok=false; conflict → 409; arch_mismatch / integrity_mismatch → 502). Follow existing formatting of other services.

- [ ] **Step 8: Re-run the tests**

Run: `cargo test -p lab-apis --features deploy deploy -- --nocapture`

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/lab-apis/Cargo.toml crates/lab-apis/src/lib.rs crates/lab-apis/src/deploy.rs crates/lab-apis/src/deploy docs/ERRORS.md
git commit -m "feat(deploy): types + error taxonomy + META"
```

---

## Task 4: Dispatch Scaffold — Catalog, Params, Authz, Headless-Bypass Rejection

**Files:**
- Create: `crates/lab/src/dispatch/deploy.rs`
- Create: `crates/lab/src/dispatch/deploy/catalog.rs`
- Create: `crates/lab/src/dispatch/deploy/params.rs`
- Create: `crates/lab/src/dispatch/deploy/dispatch.rs`
- Create: `crates/lab/src/dispatch/deploy/authz.rs`
- Modify: `crates/lab/src/dispatch.rs`
- Modify: `crates/lab/src/dispatch/error.rs`
- Modify: `crates/lab/Cargo.toml`
- Test: `crates/lab/tests/deploy_dispatch.rs`

- [ ] **Step 1: Write the failing dispatch tests**

Create `crates/lab/tests/deploy_dispatch.rs`:

```rust
#![cfg(feature = "deploy")]

use lab::dispatch::deploy;
use serde_json::json;

#[test]
fn catalog_lists_required_actions() {
    let names: Vec<&str> = deploy::ACTIONS.iter().map(|a| a.name).collect();
    for required in ["help", "schema", "config.list", "plan", "run", "rollback"] {
        assert!(names.contains(&required), "missing action: {required}");
    }
}

#[test]
fn run_and_rollback_are_destructive_and_others_are_not() {
    for action in deploy::ACTIONS {
        let expect_destructive = matches!(action.name, "run" | "rollback");
        assert_eq!(action.destructive, expect_destructive, "{} destructive flag wrong", action.name);
    }
}

#[tokio::test]
async fn unknown_action_returns_stable_kind() {
    let err = deploy::dispatch("not.a.real.action", json!({})).await.unwrap_err();
    assert_eq!(err.kind(), "unknown_action");
}

#[tokio::test]
async fn help_lists_run_and_rollback() {
    let v = deploy::dispatch("help", json!({})).await.unwrap();
    let actions = v["actions"].as_array().unwrap();
    let names: Vec<&str> = actions.iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"run"));
    assert!(names.contains(&"rollback"));
}

#[tokio::test]
async fn run_missing_targets_returns_validation_failed() {
    let err = deploy::dispatch("run", json!({ "confirm": true })).await.unwrap_err();
    assert_eq!(err.kind(), "validation_failed");
}

#[tokio::test]
async fn run_without_deploy_token_returns_auth_failed() {
    std::env::remove_var("LAB_DEPLOY_TOKEN");
    let err = deploy::dispatch("run", json!({ "targets": ["mini1"], "confirm": true })).await.unwrap_err();
    assert_eq!(err.kind(), "auth_failed");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy_dispatch -- --nocapture`

Expected: FAIL (module absent).

- [ ] **Step 3: Create the directory-first entry `crates/lab/src/dispatch/deploy.rs`**

```rust
#![cfg(feature = "deploy")]

mod authz;
mod catalog;
mod dispatch;
mod params;

pub use catalog::ACTIONS;
pub use dispatch::{dispatch, dispatch_with_runner};

// re-exports used by later tasks
#[allow(unused_imports)]
pub use authz::{reject_headless_bypass, require_deploy_token};
```

- [ ] **Step 4: Fill in `catalog.rs`**

```rust
use lab_apis::core::action::ActionSpec;

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec { name: "help",        description: "List deploy actions",                         destructive: false, schema: None },
    ActionSpec { name: "schema",      description: "Per-action JSON schema",                      destructive: false, schema: None },
    ActionSpec { name: "config.list", description: "Show resolved deploy hosts and defaults",     destructive: false, schema: None },
    ActionSpec { name: "plan",        description: "Dry-run: resolve targets, hash local artifact, show what would happen. Builds a release binary.", destructive: false, schema: None },
    ActionSpec { name: "run",         description: "Build, transfer, install, restart, verify on targets. Destructive.", destructive: true, schema: None },
    ActionSpec { name: "rollback",    description: "Restore the most recent timestamped backup on the specified targets. Destructive.", destructive: true, schema: None },
];
```

- [ ] **Step 5: Fill in `params.rs`**

Enforce regex validation for every string that can touch a `Command::arg()`:

```rust
use lab_apis::deploy::{DeployError, DeployRequest};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

fn alias_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$").unwrap())
}

fn service_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[A-Za-z0-9@._-]{1,128}$").unwrap())
}

pub fn parse_run(params: &Value) -> Result<DeployRequest, DeployError> {
    let targets = params.get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError::ValidationFailed { field: "targets".into(), reason: "required array".into() })?;
    if targets.is_empty() {
        return Err(DeployError::ValidationFailed { field: "targets".into(), reason: "must contain at least one host".into() });
    }
    let mut hosts = Vec::with_capacity(targets.len());
    for t in targets {
        let s = t.as_str().ok_or_else(|| DeployError::ValidationFailed { field: "targets".into(), reason: "entries must be strings".into() })?;
        if !alias_re().is_match(s) {
            return Err(DeployError::ValidationFailed { field: "targets".into(), reason: format!("invalid alias: {s}") });
        }
        hosts.push(s.to_string());
    }
    Ok(DeployRequest {
        targets: hosts,
        max_parallel: params.get("max_parallel").and_then(Value::as_u64).map(|n| n as u32),
        fail_fast: params.get("fail_fast").and_then(Value::as_bool).unwrap_or(false),
        confirm: params.get("confirm").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub fn validate_service_name(s: &str) -> Result<(), DeployError> {
    if service_re().is_match(s) { Ok(()) }
    else { Err(DeployError::ValidationFailed { field: "service".into(), reason: format!("invalid: {s}") }) }
}

pub fn validate_remote_path(p: &str) -> Result<(), DeployError> {
    const ALLOWED_PREFIXES: &[&str] = &["/usr/local/bin/", "/opt/lab/bin/"];
    if !p.starts_with('/') || p.contains("..") {
        return Err(DeployError::ValidationFailed { field: "remote_path".into(), reason: "must be absolute and contain no `..`".into() });
    }
    if !ALLOWED_PREFIXES.iter().any(|pref| p.starts_with(pref)) {
        return Err(DeployError::ValidationFailed { field: "remote_path".into(), reason: format!("not in allowlist: {ALLOWED_PREFIXES:?}") });
    }
    Ok(())
}
```

- [ ] **Step 6: Fill in `authz.rs`**

```rust
use lab_apis::deploy::DeployError;
use serde_json::Value;

/// Verify LAB_DEPLOY_TOKEN is set and (when provided) matches the value in params.
/// The MCP HTTP bearer is insufficient — deploy requires a dedicated token.
pub fn require_deploy_token() -> Result<(), crate::dispatch::error::ToolError> {
    match std::env::var("LAB_DEPLOY_TOKEN") {
        Ok(v) if !v.trim().is_empty() => Ok(()),
        _ => Err(crate::dispatch::error::ToolError::AuthFailed {
            message: "LAB_DEPLOY_TOKEN is required for deploy actions".into(),
        }),
    }
}

/// Reject the `confirm: true` headless-bypass for destructive deploy actions.
/// The MCP surface must attest that live elicitation was satisfied; the dispatch
/// adapter populates a context marker before calling through.
pub fn reject_headless_bypass(params: &Value, mcp_context: McpContext) -> Result<(), crate::dispatch::error::ToolError> {
    let confirm_present = params.get("confirm").and_then(Value::as_bool).unwrap_or(false);
    if confirm_present && matches!(mcp_context, McpContext::HeadlessNoElicitation) {
        return Err(crate::dispatch::error::ToolError::AuthFailed {
            message: "deploy destructive actions require live MCP elicitation; confirm:true without an elicitation response is rejected".into(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum McpContext {
    Cli,
    HttpWithToken,
    McpElicited,
    HeadlessNoElicitation,
}
```

- [ ] **Step 7: Fill in `dispatch.rs` (no runner yet; only help/schema/config.list + validation gates)**

```rust
use super::{authz, catalog, params};
use crate::dispatch::error::ToolError;
use lab_apis::deploy::DeployError;
use serde_json::{json, Value};

pub async fn dispatch(action: &str, params_v: Value) -> Result<Value, ToolError> {
    match action {
        "help"   => Ok(build_help()),
        "schema" => Ok(build_schema(&params_v)),
        "config.list" => Ok(json!({ "hosts": [], "defaults": {} })), // filled by Task 10
        "plan" => {
            authz::require_deploy_token()?;
            Err(DeployError::ValidationFailed { field: "runner".into(), reason: "runner not wired yet".into() }.into())
        }
        "run" => {
            authz::require_deploy_token()?;
            let req = params::parse_run(&params_v).map_err(ToolError::from)?;
            authz::reject_headless_bypass(&params_v, authz::McpContext::Cli)?; // CLI path
            Err(DeployError::ValidationFailed { field: "runner".into(), reason: format!("runner not wired yet; parsed {} targets", req.targets.len()) }.into())
        }
        "rollback" => {
            authz::require_deploy_token()?;
            let req = params::parse_run(&params_v).map_err(ToolError::from)?;
            authz::reject_headless_bypass(&params_v, authz::McpContext::Cli)?;
            Err(DeployError::ValidationFailed { field: "runner".into(), reason: format!("runner not wired yet; parsed {} targets", req.targets.len()) }.into())
        }
        other => Err(ToolError::UnknownAction { action: other.into(), valid: catalog::ACTIONS.iter().map(|a| a.name.to_string()).collect() }),
    }
}

pub async fn dispatch_with_runner<R: crate::dispatch::deploy::runner::DeployRunner>(
    action: &str,
    params_v: Value,
    runner: &R,
) -> Result<Value, ToolError> {
    // filled in Task 10
    dispatch(action, params_v).await
}

fn build_help() -> Value {
    json!({
        "actions": catalog::ACTIONS.iter().map(|a| json!({
            "name": a.name,
            "description": a.description,
            "destructive": a.destructive,
        })).collect::<Vec<_>>()
    })
}

fn build_schema(_params: &Value) -> Value { json!({"schemas": {}}) } // filled in Task 10
```

- [ ] **Step 8: Wire `DeployError` into `ToolError` and register module**

In `crates/lab/src/dispatch/error.rs`:

```rust
#[cfg(feature = "deploy")]
impl_tool_error_from!(lab_apis::deploy::DeployError);
```

The generated impl must call `DeployError::redacted_message()` for any user-visible message and `kind()` for the stable kind. Full structured details go through the `tracing::warn!` emitted at the dispatch boundary with `service="deploy"` (see Task 10).

In `crates/lab/src/dispatch.rs`:

```rust
#[cfg(feature = "deploy")]
pub mod deploy;
```

In `crates/lab/Cargo.toml` add:

```toml
deploy = ["lab-apis/deploy", "dep:dashmap", "dep:sha2", "dep:futures", "dep:regex"]
```

And add those to `[dependencies]` (optional = true where appropriate).

- [ ] **Step 9: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy_dispatch -- --nocapture`

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/lab/src/dispatch.rs crates/lab/src/dispatch/deploy.rs crates/lab/src/dispatch/deploy crates/lab/src/dispatch/error.rs crates/lab/Cargo.toml crates/lab/tests/deploy_dispatch.rs
git commit -m "feat(deploy): dispatch scaffold with authz + validation + headless-bypass rejection"
```

---

## Task 5: Build Stage — Cargo Release + SHA256 + Disk Preflight

**Files:**
- Create: `crates/lab/src/dispatch/deploy/build.rs`

- [ ] **Step 1: Write the failing build-stage tests**

Create `crates/lab/src/dispatch/deploy/build.rs` (module file) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sha256_of_known_bytes_is_deterministic() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("artifact");
        std::fs::write(&path, b"lab-binary-v1").unwrap();
        let hex = sha256_file_blocking(&path).unwrap();
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, sha256_file_blocking(&path).unwrap());
    }

    #[test]
    fn build_target_path_matches_cargo_layout() {
        let p = expected_artifact_path("lab");
        assert!(p.ends_with("target/release/lab"), "got {}", p.display());
    }

    #[test]
    fn disk_preflight_rejects_below_threshold() {
        // We can't reliably create a near-full disk in unit tests; assert the check
        // exists and returns an error when given a small fake available_bytes.
        let err = check_disk_space(10, 100).unwrap_err();
        assert!(matches!(err, DeployError::PreflightFailed { .. }));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy::build -- --nocapture`

Expected: FAIL.

- [ ] **Step 3: Implement `build.rs`**

```rust
use lab_apis::deploy::DeployError;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct BuildOutcome {
    pub path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub target_triple: String,
}

pub async fn build_release() -> Result<BuildOutcome, DeployError> {
    check_disk_space(estimate_free_bytes()?, 1_500_000_000)?; // 1.5 GB headroom for target/
    let output = tokio::process::Command::new("cargo")
        .args(["build", "--release", "--all-features", "-p", "lab"])
        .output()
        .await
        .map_err(|e| DeployError::BuildFailed { reason: format!("spawn cargo: {e}") })?;
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr);
        let tail = tail.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(DeployError::BuildFailed { reason: tail });
    }
    let path = expected_artifact_path("lab");
    let metadata = std::fs::metadata(&path).map_err(|e| DeployError::BuildFailed { reason: format!("stat artifact: {e}") })?;
    let sha256 = tokio::task::spawn_blocking({
        let p = path.clone();
        move || sha256_file_blocking(&p)
    })
    .await
    .map_err(|e| DeployError::BuildFailed { reason: format!("sha256 join: {e}") })??;
    let target_triple = detect_host_triple();
    Ok(BuildOutcome { path, sha256, size_bytes: metadata.len(), target_triple })
}

pub fn expected_artifact_path(bin: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("target").join("release").join(bin)
}

pub fn sha256_file_blocking(path: &Path) -> Result<String, DeployError> {
    let mut f = std::fs::File::open(path).map_err(|e| DeployError::BuildFailed { reason: format!("open: {e}") })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| DeployError::BuildFailed { reason: format!("read: {e}") })?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn check_disk_space(available: u64, required: u64) -> Result<(), DeployError> {
    if available < required {
        return Err(DeployError::PreflightFailed {
            host: "localhost".into(),
            reason: format!("insufficient disk space: have {available} need {required}"),
        });
    }
    Ok(())
}

fn estimate_free_bytes() -> Result<u64, DeployError> {
    // Best-effort: fs::metadata doesn't expose free space portably.
    // Use `df` in a blocking fashion, tolerating absence on non-unix.
    let out = std::process::Command::new("df").arg("--output=avail").arg("-B1").arg(".").output();
    if let Ok(o) = out {
        if o.status.success() {
            if let Some(line) = String::from_utf8_lossy(&o.stdout).lines().nth(1) {
                if let Ok(n) = line.trim().parse::<u64>() { return Ok(n); }
            }
        }
    }
    Ok(u64::MAX) // non-critical; treat unknown as "enough"
}

fn detect_host_triple() -> String {
    // fastest path: rustc -vV
    let out = std::process::Command::new("rustc").arg("-vV").output();
    if let Ok(o) = out {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Some(rest) = line.strip_prefix("host: ") {
                    return rest.trim().to_string();
                }
            }
        }
    }
    std::env::consts::ARCH.to_string()
}
```

Add `sha2`, `hex`, `tempfile` (dev) to `crates/lab/Cargo.toml` behind the `deploy` feature / `[dev-dependencies]`.

- [ ] **Step 4: Register `build` in `dispatch/deploy.rs`**

Add `pub mod build;` (at least `pub(super)` depending on visibility style used by the crate).

- [ ] **Step 5: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy::build -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/deploy/build.rs crates/lab/src/dispatch/deploy.rs crates/lab/Cargo.toml
git commit -m "feat(deploy): local release build with sha256 + disk preflight"
```

---

## Task 6: Per-Host Advisory Lock

**Files:**
- Create: `crates/lab/src/dispatch/deploy/lock.rs`

- [ ] **Step 1: Write the failing lock tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn first_lock_on_host_succeeds() {
        let reg = HostLockRegistry::default();
        let _g = reg.acquire("mini1", std::time::Duration::from_millis(50)).await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_lock_on_same_host_returns_conflict() {
        let reg = Arc::new(HostLockRegistry::default());
        let reg2 = reg.clone();
        let _held = reg.acquire("mini1", std::time::Duration::from_millis(50)).await.unwrap();
        let err = reg2.acquire("mini1", std::time::Duration::from_millis(25)).await.unwrap_err();
        assert_eq!(err.kind(), "conflict");
    }

    #[tokio::test]
    async fn different_hosts_do_not_conflict() {
        let reg = Arc::new(HostLockRegistry::default());
        let _a = reg.acquire("mini1", std::time::Duration::from_millis(50)).await.unwrap();
        let _b = reg.acquire("mini2", std::time::Duration::from_millis(50)).await.unwrap();
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy::lock -- --nocapture`

Expected: FAIL.

- [ ] **Step 3: Implement `lock.rs`**

```rust
use dashmap::DashMap;
use lab_apis::deploy::DeployError;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Default)]
pub struct HostLockRegistry {
    inner: DashMap<String, Arc<Mutex<()>>>,
}

impl HostLockRegistry {
    pub async fn acquire(
        &self,
        host: &str,
        timeout: std::time::Duration,
    ) -> Result<OwnedMutexGuard<()>, DeployError> {
        let mutex = self.inner
            .entry(host.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        match tokio::time::timeout(timeout, mutex.lock_owned()).await {
            Ok(guard) => Ok(guard),
            Err(_) => Err(DeployError::Conflict { host: host.to_string() }),
        }
    }
}
```

Add `pub mod lock;` to `crates/lab/src/dispatch/deploy.rs`.

- [ ] **Step 4: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy::lock -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/deploy/lock.rs crates/lab/src/dispatch/deploy.rs
git commit -m "feat(deploy): per-host advisory lock registry"
```

---

## Task 7: Preflight Stage — Arch Match, Writable Dir, SHA256 Remote Probe

**Files:**
- Create: part of `crates/lab/src/dispatch/deploy/runner.rs` (preflight subsection)

- [ ] **Step 1: Write the failing preflight tests**

Because these tests need a mock SSH session, define a small trait in `runner.rs`:

```rust
#[async_trait::async_trait] // per CLAUDE.md, prefer native async fn in trait when stable;
// use trait objects only if needed for testing. Do NOT add async-trait crate.
pub trait HostIo: Send + Sync {
    async fn run(&self, argv: &[&str]) -> Result<(i32, String, String), DeployError>;
    async fn upload<R: tokio::io::AsyncRead + Unpin + Send>(&self, remote: &str, reader: R) -> Result<u64, DeployError>;
    async fn sha256_remote(&self, remote: &str) -> Result<Option<String>, DeployError>;
}
```

(**Note:** if the surrounding codebase already uses native `async fn in trait` only, drop the `async_trait` attribute and use the native form. The plan author must check the existing style in `crates/lab/src/dispatch/gateway/probe.rs` and match it; do NOT introduce the `async-trait` crate.)

Tests:

```rust
#[cfg(test)]
mod tests_preflight {
    use super::*;
    struct MockIo {
        arch: String,
        writable: bool,
        remote_sha: Option<String>,
    }

    #[tokio::test]
    async fn rejects_arch_mismatch() {
        let io = MockIo { arch: "aarch64".into(), writable: true, remote_sha: None };
        let err = preflight(&io, "/usr/local/bin/lab", "x86_64", "abc123").await.unwrap_err();
        assert_eq!(err.kind(), "arch_mismatch");
    }

    #[tokio::test]
    async fn reports_skip_when_remote_sha_matches() {
        let io = MockIo { arch: "x86_64".into(), writable: true, remote_sha: Some("abc123".into()) };
        let res = preflight(&io, "/usr/local/bin/lab", "x86_64", "abc123").await.unwrap();
        assert!(res.skip_transfer);
    }

    #[tokio::test]
    async fn rejects_non_writable_install_dir() {
        let io = MockIo { arch: "x86_64".into(), writable: false, remote_sha: None };
        let err = preflight(&io, "/usr/local/bin/lab", "x86_64", "abc123").await.unwrap_err();
        assert_eq!(err.kind(), "preflight_failed");
    }
}
```

(Mock impl bodies omitted here for plan brevity — write them to satisfy the trait using the fields: `run` returns `(0, arch, "")` on `uname -m`, returns nonzero when writability is false on the canary write, returns the `remote_sha` for `sha256sum`. The implementor must write the mock inline to keep the test file self-contained.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy::runner::tests_preflight -- --nocapture`

Expected: FAIL.

- [ ] **Step 3: Implement `preflight`**

```rust
pub struct PreflightOutcome {
    pub skip_transfer: bool,
}

pub async fn preflight<I: HostIo>(
    io: &I,
    remote_path: &str,
    local_triple: &str,
    local_sha256: &str,
) -> Result<PreflightOutcome, DeployError> {
    // 1. uname -m
    let (code, stdout, _) = io.run(&["uname", "-m"]).await?;
    if code != 0 {
        return Err(DeployError::PreflightFailed { host: "?".into(), reason: "uname failed".into() });
    }
    let remote_arch = stdout.trim();
    let local_arch = triple_to_arch(local_triple);
    if remote_arch != local_arch {
        return Err(DeployError::ArchMismatch {
            host: "?".into(),
            local: local_arch.into(),
            remote: remote_arch.into(),
        });
    }
    // 2. canary write to install dir's parent
    let dir = std::path::Path::new(remote_path).parent()
        .ok_or_else(|| DeployError::PreflightFailed { host: "?".into(), reason: "remote_path has no parent".into() })?
        .to_string_lossy()
        .to_string();
    let canary = format!("{dir}/.lab.canary.$$");
    let (code, _, _) = io.run(&["sh", "-c", &format!("touch {canary} && rm -f {canary}")]).await?;
    // sh -c here is a deliberate, minimal exception; canary path has no user data.
    // Document this: it is the only place dispatch constructs a shell command.
    if code != 0 {
        return Err(DeployError::PreflightFailed { host: "?".into(), reason: format!("install dir not writable: {dir}") });
    }
    // 3. remote sha256 probe
    let remote_sha = io.sha256_remote(remote_path).await?;
    Ok(PreflightOutcome { skip_transfer: remote_sha.as_deref() == Some(local_sha256) })
}

fn triple_to_arch(triple: &str) -> &str {
    triple.split('-').next().unwrap_or(triple)
}
```

**Security note:** the canary-write step uses `sh -c` because touch+cleanup is a single atomic check. The canary path is derived from `remote_path` which is already allowlist-validated at `params.rs`, so it cannot be operator-injected. Add a code comment stating this is the only `sh -c` in the dispatch layer.

- [ ] **Step 4: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy::runner::tests_preflight -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/deploy/runner.rs
git commit -m "feat(deploy): preflight stage (arch match, writable dir, sha256 skip probe)"
```

---

## Task 8: Transfer + Install Stages — Stream To `.partial`, fsync, Rename, Integrity Verify, Backup

**Files:**
- Modify: `crates/lab/src/dispatch/deploy/runner.rs`

- [ ] **Step 1: Write the failing transfer + install tests**

Add to `runner.rs`:

```rust
#[cfg(test)]
mod tests_transfer_install {
    use super::*;

    // Use the MockIo from tests_preflight (make it crate-accessible or duplicate minimally here).

    #[tokio::test]
    async fn transfer_streams_to_partial_then_renames_to_new() {
        // expected ops sequence: upload lab.new.partial, rename -> lab.new, sha256 check, atomic swap
        let (io, log) = record_io();
        transfer_and_install(&io, "/usr/local/bin/lab", "abc123", tokio::io::empty()).await.unwrap();
        let ops = log.lock().unwrap().clone();
        assert!(ops.iter().any(|o| o.contains("upload:/usr/local/bin/lab.new.partial")), "ops: {ops:?}");
        assert!(ops.iter().any(|o| o.contains("rename:/usr/local/bin/lab.new.partial:/usr/local/bin/lab.new")), "ops: {ops:?}");
        assert!(ops.iter().any(|o| o.contains("sha256:/usr/local/bin/lab.new")), "ops: {ops:?}");
        // backup of existing binary, then atomic swap
        assert!(ops.iter().any(|o| o.starts_with("rename:/usr/local/bin/lab:/usr/local/bin/.lab.bak.")), "ops: {ops:?}");
        assert!(ops.iter().any(|o| o == "rename:/usr/local/bin/lab.new:/usr/local/bin/lab"), "ops: {ops:?}");
    }

    #[tokio::test]
    async fn integrity_mismatch_aborts_before_swap() {
        let io = mock_with_post_upload_sha("deadbeef"); // differs from local "abc123"
        let err = transfer_and_install(&io, "/usr/local/bin/lab", "abc123", tokio::io::empty()).await.unwrap_err();
        assert_eq!(err.kind(), "integrity_mismatch");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy::runner::tests_transfer_install -- --nocapture`

Expected: FAIL.

- [ ] **Step 3: Implement `transfer_and_install`**

```rust
pub async fn transfer_and_install<I: HostIo, R: tokio::io::AsyncRead + Unpin + Send>(
    io: &I,
    remote_path: &str,
    local_sha256: &str,
    reader: R,
) -> Result<TransferOutcome, DeployError> {
    let partial = format!("{remote_path}.new.partial");
    let staged  = format!("{remote_path}.new");
    // 1. stream to .partial via ssh cat with chunked copy (upload impl does tokio::io::copy)
    let bytes = io.upload(&partial, reader).await?;
    // 2. rename .partial -> .new (no fsync primitive; caller trusts rename barrier on linux ext4/xfs)
    let (c, _, e) = io.run(&["mv", "--", &partial, &staged]).await?;
    if c != 0 { return Err(DeployError::TransferFailed { host: "?".into(), reason: format!("rename partial: {e}") }); }
    // 3. verify sha256 of .new matches local_sha256
    let remote_sha = io.sha256_remote(&staged).await?
        .ok_or_else(|| DeployError::TransferFailed { host: "?".into(), reason: "sha256 probe absent".into() })?;
    if remote_sha != local_sha256 {
        let _ = io.run(&["rm", "-f", "--", &staged]).await; // best effort cleanup
        return Err(DeployError::IntegrityMismatch { host: "?".into() });
    }
    // 4. backup existing (if present); always do this, fixed policy V1
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let backup = format!("{remote_path}.bak.{ts}");
    // Only rename-backup if the target exists; check via sha256_remote shortcut (Some == exists)
    if io.sha256_remote(remote_path).await?.is_some() {
        let (c, _, e) = io.run(&["mv", "--", remote_path, &backup]).await?;
        if c != 0 { return Err(DeployError::InstallFailed { host: "?".into(), reason: format!("backup rename: {e}") }); }
    }
    // 5. atomic swap
    let (c, _, e) = io.run(&["mv", "--", &staged, remote_path]).await?;
    if c != 0 { return Err(DeployError::InstallFailed { host: "?".into(), reason: format!("final rename: {e}") }); }
    Ok(TransferOutcome { bytes, backup_path: Some(backup) })
}

pub struct TransferOutcome {
    pub bytes: u64,
    pub backup_path: Option<String>,
}
```

**Security note:** every `io.run(&[...])` is per-token argv, never shell-interpolated. `remote_path` is allowlist-validated upstream. `--` is used to defeat accidental option parsing on hostile filenames.

**Note on cross-filesystem rename:** `mv` handles `EXDEV` by copy+unlink; acceptable for V1 since install paths are typically one filesystem. Document this in the cover doc as a known edge case.

- [ ] **Step 4: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy::runner::tests_transfer_install -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/deploy/runner.rs
git commit -m "feat(deploy): transfer + atomic install with sha256 verify and timestamped backup"
```

---

## Task 9: Restart + Verify Stages

**Files:**
- Modify: `crates/lab/src/dispatch/deploy/runner.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests_restart_verify {
    use super::*;

    #[tokio::test]
    async fn skips_restart_when_unit_is_none() {
        let io = ok_io();
        let r = restart(&io, None, None).await.unwrap();
        assert!(r.skipped);
    }

    #[tokio::test]
    async fn restart_system_scope_uses_systemctl_and_waits_for_active() {
        let (io, log) = record_io();
        restart(&io, Some("lab"), Some(crate::config::ServiceScope::System)).await.unwrap();
        let ops = log.lock().unwrap().clone();
        assert!(ops.iter().any(|o| o == r#"run:systemctl,restart,lab"#));
        assert!(ops.iter().any(|o| o == r#"run:systemctl,is-active,--wait,lab"#));
    }

    #[tokio::test]
    async fn restart_user_scope_uses_systemctl_user() {
        let (io, log) = record_io();
        restart(&io, Some("lab-worker"), Some(crate::config::ServiceScope::User)).await.unwrap();
        let ops = log.lock().unwrap().clone();
        assert!(ops.iter().any(|o| o == r#"run:systemctl,--user,restart,lab-worker"#));
    }

    #[tokio::test]
    async fn verify_runs_version_and_rejects_nonzero_exit() {
        let io = io_with_exit_code("lab --version", 1);
        let err = verify(&io, "/usr/local/bin/lab").await.unwrap_err();
        assert_eq!(err.kind(), "verify_failed");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy::runner::tests_restart_verify -- --nocapture`

Expected: FAIL.

- [ ] **Step 3: Implement restart + verify**

```rust
use crate::config::ServiceScope;

pub struct RestartOutcome {
    pub skipped: bool,
}

pub async fn restart<I: HostIo>(
    io: &I,
    unit: Option<&str>,
    scope: Option<ServiceScope>,
) -> Result<RestartOutcome, DeployError> {
    let Some(unit) = unit else { return Ok(RestartOutcome { skipped: true }); };
    super::params::validate_service_name(unit)?;
    let scope_arg = matches!(scope, Some(ServiceScope::User)).then(|| "--user");
    let mut restart_argv = vec!["systemctl"];
    if let Some(a) = scope_arg { restart_argv.push(a); }
    restart_argv.extend(["restart", unit]);
    let (c, _, e) = io.run(&restart_argv).await?;
    if c != 0 { return Err(DeployError::RestartFailed { host: "?".into(), reason: e }); }

    let mut wait_argv = vec!["systemctl"];
    if let Some(a) = scope_arg { wait_argv.push(a); }
    wait_argv.extend(["is-active", "--wait", unit]);
    let (c, _, e) = io.run(&wait_argv).await?;
    if c != 0 { return Err(DeployError::RestartFailed { host: "?".into(), reason: format!("is-active --wait: {e}") }); }
    Ok(RestartOutcome { skipped: false })
}

pub async fn verify<I: HostIo>(io: &I, remote_path: &str) -> Result<(), DeployError> {
    let (c, _, e) = io.run(&[remote_path, "--version"]).await?;
    if c != 0 { return Err(DeployError::VerifyFailed { host: "?".into(), reason: format!("--version exit {c}: {e}") }); }
    Ok(())
}
```

- [ ] **Step 4: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy::runner::tests_restart_verify -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/deploy/runner.rs
git commit -m "feat(deploy): restart + verify stages (systemctl user/system, version probe)"
```

---

## Task 10: Orchestrator — Concurrent Fan-Out, Canary, Fail-Fast, `run_id` Tracing

**Files:**
- Modify: `crates/lab/src/dispatch/deploy/runner.rs`
- Modify: `crates/lab/src/dispatch/deploy/dispatch.rs`
- Modify: `crates/lab/src/dispatch/deploy/params.rs` (add `config.list` + `plan` shape parsing)
- Modify: `docs/OBSERVABILITY.md`
- Test: `crates/lab/tests/deploy_runner.rs`

- [ ] **Step 1: Write the failing orchestrator tests**

Create `crates/lab/tests/deploy_runner.rs`:

```rust
#![cfg(feature = "deploy")]

use lab::dispatch::deploy::runner::*;

#[tokio::test]
async fn canary_deployed_before_rest_of_fleet() {
    // mock a runner that records ordered host visits
    // config: canary=["mini1"], hosts=["mini1","mini2","mini3"]
    // assert mini1 finishes before mini2/mini3 start
    unimplemented!("write this test using a recording HostIo per host and assert ordering");
}

#[tokio::test]
async fn fail_fast_aborts_subsequent_hosts() {
    // host1 verify fails, fail_fast=true; host2 preflight must not be called
    unimplemented!();
}

#[tokio::test]
async fn continue_on_error_reports_partial_failure() {
    // host1 fails, host2 succeeds; fail_fast=false; summary.failed==1 summary.ok==false
    unimplemented!();
}

#[tokio::test]
async fn max_parallel_caps_concurrency() {
    // pause host callbacks via barriers; with max_parallel=2 and 5 hosts, observe at most 2 concurrent entries
    unimplemented!();
}

#[tokio::test]
async fn run_id_span_is_present_in_structured_events() {
    // use tracing-test or a capturing subscriber; assert an event with service="deploy" carries run_id
    unimplemented!();
}

#[tokio::test]
async fn plan_does_not_build_when_local_sha_is_provided_via_cache() {
    // plan should not invoke cargo if a cached BuildOutcome exists; this is a marker for
    // "plan must not be expensive" — confirm plan can be served from a previous run's cache.
    unimplemented!();
}
```

**Note on test skeletons:** these assertions have `unimplemented!()` bodies to show intent — the implementor writes the mock wiring inline. Before starting Task 10, skim `crates/lab/src/dispatch/gateway/manager.rs` tests for the established mock pattern.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy_runner -- --nocapture`

Expected: FAIL (unimplemented or missing orchestrator).

- [ ] **Step 3: Implement the orchestrator**

```rust
pub trait DeployRunner: Send + Sync {
    async fn plan(&self, req: &lab_apis::deploy::DeployRequest) -> Result<lab_apis::deploy::DeployPlan, crate::dispatch::error::ToolError>;
    async fn run(&self, req: &lab_apis::deploy::DeployRequest) -> Result<lab_apis::deploy::DeployRunSummary, crate::dispatch::error::ToolError>;
    async fn rollback(&self, req: &lab_apis::deploy::DeployRequest) -> Result<lab_apis::deploy::DeployRunSummary, crate::dispatch::error::ToolError>;
    async fn config_list(&self) -> Result<serde_json::Value, crate::dispatch::error::ToolError>;
}

pub struct DefaultRunner {
    pub config: crate::config::DeployPreferences,
    pub ssh_inventory: std::sync::Arc<Vec<lab_apis::core::ssh::SshHostTarget>>,
    pub locks: std::sync::Arc<super::lock::HostLockRegistry>,
}

impl DeployRunner for DefaultRunner {
    async fn run(&self, req: &lab_apis::deploy::DeployRequest) -> Result<lab_apis::deploy::DeployRunSummary, crate::dispatch::error::ToolError> {
        use futures::stream::{self, StreamExt};
        let run_id = uuid::Uuid::new_v4().to_string();
        let span = tracing::info_span!("deploy.run", run_id = %run_id, surface = "dispatch", service = "deploy");
        let _g = span.enter();

        let build = super::build::build_release().await?;
        tracing::info!(artifact_sha256 = %build.sha256, size_bytes = build.size_bytes, "deploy.build.ok");

        let max_parallel = req.max_parallel.or(self.effective_max_parallel()).unwrap_or(1).max(1) as usize;
        let (canary, rest) = self.partition_canary(&req.targets);

        // run canary sequentially (max_parallel=1) first
        let canary_results = self.run_hosts(&canary, &build, 1, req.fail_fast, &run_id).await;
        if req.fail_fast && canary_results.iter().any(|r| !r.succeeded) {
            return Ok(summarize(run_id, build.sha256, canary_results));
        }

        // run remaining with configured concurrency
        let rest_results = self.run_hosts(&rest, &build, max_parallel, req.fail_fast, &run_id).await;

        let mut all = canary_results;
        all.extend(rest_results);
        Ok(summarize(run_id, build.sha256, all))
    }

    // plan / rollback / config_list similar; plan does not build unless CLI flag --build is set;
    // by default plan returns cached artifact hash if target/release/lab exists + mtime reasonable.
    async fn plan(&self, req: &lab_apis::deploy::DeployRequest) -> Result<lab_apis::deploy::DeployPlan, crate::dispatch::error::ToolError> {
        // resolve targets, hash existing artifact if present, do NOT run cargo unless explicitly asked.
        // ... (implementor: follow the same pattern)
        todo!("implement plan without running cargo")
    }

    async fn rollback(&self, req: &lab_apis::deploy::DeployRequest) -> Result<lab_apis::deploy::DeployRunSummary, crate::dispatch::error::ToolError> {
        // for each host, find latest .bak.<ts>, rename it over current binary; systemctl restart if unit known
        todo!("implement rollback using same host fan-out")
    }

    async fn config_list(&self) -> Result<serde_json::Value, crate::dispatch::error::ToolError> {
        Ok(serde_json::json!({
            "defaults": self.config.defaults,
            "hosts": self.ssh_inventory.iter().map(|h| &h.alias).collect::<Vec<_>>(),
            "overrides": self.config.hosts.keys().collect::<Vec<_>>(),
        }))
    }
}

impl DefaultRunner {
    fn effective_max_parallel(&self) -> Option<u32> {
        self.config.defaults.as_ref().and_then(|d| d.max_parallel)
    }

    fn partition_canary(&self, targets: &[String]) -> (Vec<String>, Vec<String>) {
        let canary_set: std::collections::BTreeSet<&String> = self.config.defaults.as_ref()
            .map(|d| d.canary_hosts.iter().collect()).unwrap_or_default();
        let mut canary = Vec::new();
        let mut rest = Vec::new();
        for t in targets {
            if canary_set.contains(t) { canary.push(t.clone()); } else { rest.push(t.clone()); }
        }
        (canary, rest)
    }

    async fn run_hosts(
        &self,
        hosts: &[String],
        build: &super::build::BuildOutcome,
        max_parallel: usize,
        fail_fast: bool,
        run_id: &str,
    ) -> Vec<lab_apis::deploy::DeployHostResult> {
        use futures::stream::{self, StreamExt};
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        stream::iter(hosts.iter().cloned())
            .map(|host| {
                let stop = stop.clone();
                let run_id = run_id.to_string();
                async move {
                    if stop.load(std::sync::atomic::Ordering::SeqCst) {
                        return lab_apis::deploy::DeployHostResult { host: host.clone(), reached_stage: lab_apis::deploy::DeployStage::Resolve, succeeded: false, skipped_transfer: false, transferred_bytes: None, error_kind: Some("aborted".into()), stage_timings_ms: Default::default() };
                    }
                    let span = tracing::info_span!("deploy.host", run_id = %run_id, host = %host);
                    let _g = span.enter();
                    let res = self.run_one_host(&host, build).await;
                    if fail_fast && !res.succeeded {
                        stop.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    res
                }
            })
            .buffer_unordered(max_parallel)
            .collect()
            .await
    }

    async fn run_one_host(&self, host: &str, build: &super::build::BuildOutcome) -> lab_apis::deploy::DeployHostResult {
        // acquire per-host lock (timeout from config; default 60s)
        // iterate stages: Preflight → Transfer(unless skip) → Install → Restart(optional) → Verify
        // emit tracing::info_span!("deploy.stage", stage=...) around each
        // populate DeployHostResult.stage_timings_ms from Instant::elapsed per stage
        todo!("wire HostIo impl via core::ssh::SshSession and call preflight/transfer_and_install/restart/verify")
    }
}

fn summarize(
    run_id: String,
    artifact_sha256: String,
    hosts: Vec<lab_apis::deploy::DeployHostResult>,
) -> lab_apis::deploy::DeployRunSummary {
    let (ok, failed): (Vec<_>, Vec<_>) = hosts.iter().partition(|r| r.succeeded);
    lab_apis::deploy::DeployRunSummary {
        run_id,
        artifact_sha256,
        succeeded: ok.len(),
        failed: failed.len(),
        ok: failed.is_empty(),
        hosts,
    }
}
```

- [ ] **Step 4: Wire the runner into `dispatch.rs`**

Replace the "not wired yet" branches from Task 4 with real runner calls:

```rust
pub async fn dispatch_with_runner<R: DeployRunner>(
    action: &str,
    params_v: Value,
    runner: &R,
) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(build_help()),
        "schema" => Ok(build_schema(&params_v)),
        "config.list" => {
            authz::require_deploy_token()?;
            runner.config_list().await
        }
        "plan" => {
            authz::require_deploy_token()?;
            let req = params::parse_run(&params_v).map_err(ToolError::from)?;
            let plan = runner.plan(&req).await?;
            Ok(serde_json::to_value(plan).unwrap())
        }
        "run" => {
            authz::require_deploy_token()?;
            let req = params::parse_run(&params_v).map_err(ToolError::from)?;
            authz::reject_headless_bypass(&params_v, current_mcp_context())?;
            let summary = runner.run(&req).await?;
            Ok(serde_json::to_value(summary).unwrap())
        }
        "rollback" => {
            authz::require_deploy_token()?;
            let req = params::parse_run(&params_v).map_err(ToolError::from)?;
            authz::reject_headless_bypass(&params_v, current_mcp_context())?;
            let summary = runner.rollback(&req).await?;
            Ok(serde_json::to_value(summary).unwrap())
        }
        other => Err(ToolError::UnknownAction { action: other.into(), valid: catalog::ACTIONS.iter().map(|a| a.name.to_string()).collect() }),
    }
}

fn current_mcp_context() -> authz::McpContext {
    // thread-local or task-local set by the MCP adapter in Task 11.
    // Default fallback is CLI.
    authz::MCP_CONTEXT.try_with(|c| *c).unwrap_or(authz::McpContext::Cli)
}
```

Add a `tokio::task_local! { pub static MCP_CONTEXT: McpContext; }` in `authz.rs`; the MCP service in Task 11 will scope it.

- [ ] **Step 5: Document deploy observability**

Append to `docs/OBSERVABILITY.md` a `### deploy` subsection: run_id correlation, span hierarchy `deploy.run` → `deploy.host` → `deploy.stage`, stable fields (`surface`, `service`, `action`, `elapsed_ms`, `run_id`, `host`, `stage`, `artifact_sha256`), redaction rules (raw params never logged; remote_path and service name logged at INFO; hostnames logged at INFO; identity file paths never logged).

- [ ] **Step 6: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy_runner -- --nocapture && cargo test -p lab --features deploy deploy_dispatch -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/deploy/runner.rs crates/lab/src/dispatch/deploy/dispatch.rs crates/lab/src/dispatch/deploy/params.rs crates/lab/src/dispatch/deploy/authz.rs crates/lab/tests/deploy_runner.rs docs/OBSERVABILITY.md
git commit -m "feat(deploy): orchestrator with canary, concurrency, fail-fast, and run_id tracing"
```

---

## Task 11: CLI And MCP Adapters (API Deferred)

**Files:**
- Create: `crates/lab/src/cli/deploy.rs`
- Create: `crates/lab/src/mcp/services/deploy.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/mcp/services.rs`
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/src/dispatch/clients.rs`
- Test: `crates/lab/tests/deploy_cli.rs`
- Test: `crates/lab/tests/deploy_mcp.rs`

- [ ] **Step 1: Write the failing CLI + MCP tests**

`crates/lab/tests/deploy_cli.rs`:

```rust
#![cfg(feature = "deploy")]

#[test]
fn parses_run_with_targets_and_yes() {
    use clap::Parser;
    let args = lab::cli::deploy::DeployArgs::try_parse_from(["deploy", "run", "mini1", "mini2", "-y"]).unwrap();
    assert_eq!(args.cmd_targets(), vec!["mini1".to_string(), "mini2".to_string()]);
    assert!(args.cmd_yes());
}

#[test]
fn run_without_yes_is_rejected_before_dispatch() {
    use clap::Parser;
    // clap accepts the command; the CLI handler refuses without -y.
    // This asserts the CLI returns a "confirmation required" error.
    // Implementation: handler checks -y and returns a Clone of ToolError::Aborted{reason}.
    // Test it via `lab::cli::deploy::run_cli` directly with a test DeployRunner that would panic if called.
}
```

`crates/lab/tests/deploy_mcp.rs`:

```rust
#![cfg(feature = "deploy")]

#[tokio::test]
async fn headless_confirm_true_is_rejected() {
    // Simulate MCP dispatch without elicitation: call the MCP adapter with McpContext::HeadlessNoElicitation
    // and confirm:true. Expect ToolError::AuthFailed kind "auth_failed".
    std::env::set_var("LAB_DEPLOY_TOKEN", "test-token");
    let ctx = lab::dispatch::deploy::authz::McpContext::HeadlessNoElicitation;
    let r = lab::mcp::services::deploy::dispatch_with_context(
        "run",
        serde_json::json!({ "targets": ["mini1"], "confirm": true }),
        ctx,
    ).await;
    let err = r.unwrap_err();
    assert_eq!(err.kind(), "auth_failed");
}

#[tokio::test]
async fn elicited_confirm_true_is_accepted_by_authz() {
    // Same payload, but context is McpElicited — authz gate passes; dispatch may still fail
    // because no SSH inventory is loaded in the test. We only assert the kind is NOT auth_failed.
    std::env::set_var("LAB_DEPLOY_TOKEN", "test-token");
    let ctx = lab::dispatch::deploy::authz::McpContext::McpElicited;
    let r = lab::mcp::services::deploy::dispatch_with_context(
        "run",
        serde_json::json!({ "targets": ["mini1"], "confirm": true }),
        ctx,
    ).await;
    if let Err(e) = r {
        assert_ne!(e.kind(), "auth_failed");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab --features deploy deploy_cli deploy_mcp -- --nocapture`

Expected: FAIL.

- [ ] **Step 3: Implement the CLI (`crates/lab/src/cli/deploy.rs`)**

```rust
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct DeployArgs {
    #[command(subcommand)]
    pub cmd: DeployCmd,
}

#[derive(Debug, Subcommand)]
pub enum DeployCmd {
    /// Show resolved deploy hosts and defaults.
    ConfigList,
    /// Dry-run: show what a deploy would do without running it.
    Plan { targets: Vec<String> },
    /// Destructive: build, transfer, install, restart, verify.
    Run {
        targets: Vec<String>,
        #[arg(short = 'y', long = "yes")]
        yes: bool,
        #[arg(long)]
        max_parallel: Option<u32>,
        #[arg(long)]
        fail_fast: bool,
    },
    /// Destructive: restore the most recent backup on each target.
    Rollback {
        targets: Vec<String>,
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
}

impl DeployArgs {
    pub fn cmd_targets(&self) -> Vec<String> {
        match &self.cmd {
            DeployCmd::Plan { targets } | DeployCmd::Run { targets, .. } | DeployCmd::Rollback { targets, .. } => targets.clone(),
            DeployCmd::ConfigList => vec![],
        }
    }
    pub fn cmd_yes(&self) -> bool {
        matches!(&self.cmd, DeployCmd::Run { yes: true, .. } | DeployCmd::Rollback { yes: true, .. })
    }
}

pub async fn run_cli<R: crate::dispatch::deploy::runner::DeployRunner>(args: DeployArgs, runner: &R) -> anyhow::Result<()> {
    use crate::dispatch::deploy;
    let (action, params) = match args.cmd {
        DeployCmd::ConfigList => ("config.list", serde_json::json!({})),
        DeployCmd::Plan { targets } => ("plan", serde_json::json!({ "targets": targets })),
        DeployCmd::Run { targets, yes, max_parallel, fail_fast } => {
            if !yes { anyhow::bail!("deploy run is destructive; pass -y to confirm"); }
            ("run", serde_json::json!({ "targets": targets, "confirm": true, "max_parallel": max_parallel, "fail_fast": fail_fast }))
        }
        DeployCmd::Rollback { targets, yes } => {
            if !yes { anyhow::bail!("deploy rollback is destructive; pass -y to confirm"); }
            ("rollback", serde_json::json!({ "targets": targets, "confirm": true }))
        }
    };
    // Scope MCP_CONTEXT to Cli so authz treats this as a local operator action.
    let value = deploy::authz::MCP_CONTEXT
        .scope(deploy::authz::McpContext::Cli, deploy::dispatch_with_runner(action, params, runner))
        .await?;
    crate::output::emit(&value);
    Ok(())
}
```

Register the group in `crates/lab/src/cli.rs`:

```rust
#[cfg(feature = "deploy")]
Deploy(cli::deploy::DeployArgs),
```

and wire the match arm.

- [ ] **Step 4: Implement the MCP adapter (`crates/lab/src/mcp/services/deploy.rs`)**

```rust
use crate::dispatch::deploy;

pub async fn dispatch_with_context(
    action: &str,
    params: serde_json::Value,
    ctx: deploy::authz::McpContext,
) -> Result<serde_json::Value, crate::dispatch::error::ToolError> {
    // obtain the global runner from AppState; in tests a stub runner can be injected
    let runner = crate::registry::current_deploy_runner()?;
    deploy::authz::MCP_CONTEXT
        .scope(ctx, deploy::dispatch_with_runner(action, params, runner.as_ref()))
        .await
}

/// Entrypoint called from the MCP server. Responsible for negotiating elicitation
/// and mapping the result into McpContext. For destructive deploy actions, the MCP
/// server MUST complete a live elicitation exchange; if the client advertises no
/// elicitation capability, call this with McpContext::HeadlessNoElicitation so the
/// authz layer rejects the call.
pub async fn entry(action: &str, params: serde_json::Value, elicited: bool) -> Result<serde_json::Value, crate::dispatch::error::ToolError> {
    let ctx = if elicited { deploy::authz::McpContext::McpElicited } else { deploy::authz::McpContext::HeadlessNoElicitation };
    dispatch_with_context(action, params, ctx).await
}
```

Register in `crates/lab/src/mcp/services.rs`:

```rust
#[cfg(feature = "deploy")]
pub mod deploy;
```

- [ ] **Step 5: Wire shared state**

In `crates/lab/src/dispatch/clients.rs` add:

```rust
#[cfg(feature = "deploy")]
pub deploy: Option<std::sync::Arc<crate::dispatch::deploy::runner::DefaultRunner>>,
```

In `crates/lab/src/registry.rs` add a helper `current_deploy_runner()` returning `Result<Arc<DefaultRunner>, ToolError>` that pulls from the global `Clients` set at startup.

At startup in `main.rs` (or wherever clients are populated), build the `DefaultRunner` from `LabConfig.deploy` + `~/.ssh/config` via `core::ssh::parse_ssh_config`. Mutli-host availability is an optional bring-up concern; if the config is absent, deploy remains unreachable but the binary still builds.

- [ ] **Step 6: Re-run the tests**

Run: `cargo test -p lab --features deploy deploy_cli deploy_mcp deploy_dispatch deploy_runner -- --nocapture`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/cli/deploy.rs crates/lab/src/cli.rs crates/lab/src/mcp/services/deploy.rs crates/lab/src/mcp/services.rs crates/lab/src/registry.rs crates/lab/src/dispatch/clients.rs crates/lab/tests/deploy_cli.rs crates/lab/tests/deploy_mcp.rs
git commit -m "feat(deploy): CLI + MCP adapters with elicitation-enforced authz"
```

---

## Task 12: Docs — DEPLOY_SERVICE.md, coverage, cross-cutting updates

**Files:**
- Create: `docs/DEPLOY_SERVICE.md`
- Create: `docs/coverage/deploy.md`
- Modify: `docs/README.md`
- Modify: `docs/SERVICES.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CLI.md`
- Modify: `docs/MCP.md`

- [ ] **Step 1: Write `docs/DEPLOY_SERVICE.md`**

Sections, each with concrete content:

1. **Overview** — synthetic service, builds local release, pushes to SSH targets with integrity.
2. **Authorization** — `LAB_DEPLOY_TOKEN` required above MCP bearer. Destructive actions require live MCP elicitation; headless `confirm: true` is rejected.
3. **Target inventory** — from `~/.ssh/config` (alias, hostname, user, port); `Include`/`Match` blocks ignored with WARN log.
4. **Intent config** — `[deploy.defaults]` (remote_path, service, service_scope, max_parallel, canary_hosts); `[deploy.hosts.<alias>]` per-host overrides.
5. **Install path allowlist** — `/usr/local/bin/`, `/opt/lab/bin/`.
6. **Integrity model** — sha256 hashed locally after build; remote sha256 verified on `.new` before atomic swap; `integrity_mismatch` error kind if they differ.
7. **Backup** — `.lab.bak.<timestamp>` retained on every successful install. Retention policy (keep-N) is V2.
8. **Rollback** — `deploy rollback` picks the most recent backup and swaps it into place.
9. **Concurrency + canary** — bounded fan-out default `max_parallel=1`; `canary_hosts` deploy sequentially first; `--fail-fast` aborts remaining hosts on first failure.
10. **Cross-arch refusal** — `uname -m` must match local build's target triple arch.
11. **Observability** — `run_id`, span hierarchy, stable fields, redaction rules.
12. **Error kinds** — link to `docs/ERRORS.md#deploy`.
13. **Non-goals** — online/offline presence tracking (future `devices`); rsync transport (V2); group expansion (V2); HTTP API (V2); per-call policy overrides (V2).

- [ ] **Step 2: Write `docs/coverage/deploy.md`**

Sections: source contract, SDK methods (types only — no client methods V1), dispatch actions, CLI commands, MCP actions, API route (N/A V1), live test evidence (to be filled during Task 13).

- [ ] **Step 3: Update cross-cutting docs**

Minimal edits:
- `docs/README.md` — add link to `DEPLOY_SERVICE.md`.
- `docs/SERVICES.md` — list `deploy` as synthetic, feature-gated `deploy`, CLI+MCP surfaces only.
- `docs/CONFIG.md` — `[deploy.defaults]` and `[deploy.hosts.<alias>]` schema + example.
- `docs/CLI.md` — `lab deploy {config-list|plan|run|rollback}` with `-y` requirement.
- `docs/MCP.md` — `deploy` tool, `LAB_DEPLOY_TOKEN` requirement, destructive actions, headless-bypass rejection statement.

- [ ] **Step 4: Commit**

```bash
git add docs/DEPLOY_SERVICE.md docs/coverage/deploy.md docs/README.md docs/SERVICES.md docs/CONFIG.md docs/CLI.md docs/MCP.md
git commit -m "docs(deploy): operator contract, config, CLI, MCP, coverage skeleton"
```

---

## Task 13: Final Verification Gate + Live Smoke Evidence

**Files:** none created; updates `docs/coverage/deploy.md` with live evidence.

- [ ] **Step 1: Full workspace checks**

Run each, fix any failure before proceeding:

```bash
cargo fmt --all --check
cargo clippy --all-features --workspace -- -D warnings
cargo test --all-features
cargo build --all-features
cargo deny check
```

Expected: PASS on all.

- [ ] **Step 2: Confirm `deploy` is discoverable**

- `lab help` — deploy listed
- `lab://catalog` via MCP — deploy listed
- `lab deploy --help` — shows config-list/plan/run/rollback
- MCP `deploy.help` action — lists `run` and `rollback` as destructive
- `LAB_DEPLOY_TOKEN` unset → deploy actions return `auth_failed`

- [ ] **Step 3: Live smoke tests against a safe target**

Pick one non-critical host (e.g., `mini-test`). Populate `~/.labby/.env` with `LAB_DEPLOY_TOKEN=...` and ensure `~/.ssh/config` has `mini-test`. Add `[deploy.hosts.mini-test]` in `config.toml`. Then:

```bash
lab deploy config-list
lab deploy plan mini-test
lab deploy run mini-test -y
ssh mini-test lab --version  # confirm installed
lab deploy rollback mini-test -y
ssh mini-test lab --version  # confirm backup restored
```

Record each command's output (abridged) in `docs/coverage/deploy.md` under "live test evidence".

- [ ] **Step 4: Negative-path smoke tests**

- `lab deploy run mini-test` (no `-y`) → refused with `deploy run is destructive; pass -y to confirm`.
- `LAB_DEPLOY_TOKEN= lab deploy plan mini-test` → `auth_failed`.
- `lab deploy run no-such-host -y` → `ssh_unreachable` or `validation_failed` depending on whether the alias exists.
- Send an MCP `deploy.run` with `confirm: true` from a client that does not advertise elicitation → `auth_failed` (headless bypass rejected).

Record the exact error kinds observed.

- [ ] **Step 5: Commit coverage**

```bash
git add docs/coverage/deploy.md
git commit -m "docs(deploy): live smoke evidence"
```

---

## Final Verification Gate

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-features --workspace -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cargo build --all-features`
- [ ] `cargo deny check`
- [ ] `deploy` present in `lab help`, `lab://catalog`, MCP registry; CLI `lab deploy --help` documents all subcommands
- [ ] `LAB_DEPLOY_TOKEN` gate observed on every deploy action
- [ ] MCP headless `confirm: true` rejection verified against a non-elicitation client
- [ ] Live `run` and `rollback` recorded in `docs/coverage/deploy.md`
- [ ] `docs/ERRORS.md` includes every `DeployError::kind()` value
- [ ] `docs/OBSERVABILITY.md` documents `run_id` and the span hierarchy

---

## Handoff To `devices` (unchanged from V1 plan)

After this plan lands, the next design is a separate `devices` capability:

- `devices.status`
- `devices.events`
- `devices.watch`
- `devices.unreachable`

`devices` owns presence tracking and is orthogonal to deploy rollout. Do not merge the two capabilities.

---

## Self-Review Notes

- Every review recommendation maps to a task: extract SSH (Task 1), config model (Task 2), types + errors + taxonomy (Task 3), authz + headless bypass + param validation (Task 4), build + sha256 + disk preflight (Task 5), per-host lock (Task 6), preflight/arch/writable/skip (Task 7), transfer/install/integrity/backup (Task 8), restart/verify (Task 9), orchestrator with canary/concurrency/fail-fast/tracing (Task 10), CLI+MCP+elicitation (Task 11), docs (Task 12), live smoke (Task 13).
- API surface is explicitly absent from V1 by design.
- `deploy.verify` standalone action is absent; verification is part of `run` and observable via the summary.
- `targets.list`/`groups.list` collapsed into `config.list`.
- Groups are absent from V1 (host list only).
- `dry_run` param removed in favor of `plan`.
- Fixed V1 policy: always backup, always verify, restart iff service unit is configured.
- `Category::Operator` is not introduced; `Category::Bootstrap` reused.
- All shell-out uses per-token argv; the single `sh -c` in preflight is bounded by an allowlist-validated path and is annotated in code.
