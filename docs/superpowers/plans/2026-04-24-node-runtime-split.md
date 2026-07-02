# Node Runtime Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `lab serve` and `nodes update` into explicit controller and node behavior so deployed nodes do not initialize or ship controller-only surfaces.

**Architecture:** Make the runtime split first, then the artifact split. `serve` branches immediately after role resolution: controller mode keeps the existing control plane path, node mode starts only the outbound node runtime and optional loopback health server. Cargo feature slimming follows after behavior is correct, because Cargo feature unification means a real slim binary requires gating compile roots, not just skipping runtime code.

**Tech Stack:** Rust 2024, clap, axum for controller HTTP, raw tokio TCP for node loopback health, tokio-tungstenite for node WebSocket, Cargo features/profiles, systemd restart/readiness verification.

---

## Research Summary

Primary references:

- Cargo features: optional dependencies are enabled via feature groups, `dep:` avoids exposing implicit dependency features, and features are additive/unified across dependency uses. Source: Cargo Book Features, https://doc.rust-lang.org/cargo/reference/features.html
- Cargo profiles: custom profiles inherit from another profile, output goes under a profile-named directory in `target`, and LTO trades link time for optimization. Source: Cargo Book Profiles, https://doc.rust-lang.org/cargo/reference/profiles.html
- Workspace dependencies: inherited workspace dependencies may be marked optional in member crates, but workspace dependency definitions themselves cannot be optional. Source: Cargo Book Specifying Dependencies, https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html
- systemd readiness: `Type=notify` waits for `READY=1`, while `Type=simple` considers the service started when the process is launched. Source: systemd.service, https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html
- sd_notify: readiness notifications are used by systemd only for `Type=notify` or `Type=notify-reload`. Source: sd_notify, https://www.freedesktop.org/software/systemd/man/sd_notify.html

Code facts:

- `serve` currently resolves role in `crates/lab/src/cli/serve.rs`, but only after building the full registry.
- The correct runtime split point is immediately after `node_role` is known and before upstream OAuth, gateway discovery, gateway manager installation, logs bootstrap, web asset build, marketplace sync, and router construction.
- The current router gates many controller routes with `AppState::is_master()`, but that only prevents exposure. It does not prevent controller startup work.
- `nodes update` currently builds one all-features artifact once and deploys it to every role.
- `node/ws_client.rs` currently depends on `dispatch::upstream::transport::websocket` helpers, which creates a hidden controller compile edge for a future node-only binary.

Scope controls:

- Cross-compilation support should be model-ready but is not required for the first production rollout unless the live fleet contains non-host architectures.
- The first rollout may assume Linux `x86_64` hosts if the live config audit confirms that all targets match the controller build architecture.
- Generic `deploy run` role-aware artifacts are a follow-up after `nodes update --all` is production-ready.

## File Map

- Modify `crates/lab/src/config.rs`: add explicit node role config and deploy artifact profile config.
- Modify `crates/lab/src/cli/serve.rs`: add role CLI option, resolve role before controller startup, split controller/node startup paths.
- Modify `crates/lab/src/node/identity.rs`: centralize role resolution from CLI override, `[node].role`, `[node].controller`, and legacy `[device].master`.
- Modify `crates/lab/src/node/runtime.rs`: expose a single node startup helper for metadata, bootstrap logs, and WebSocket loop.
- Create `crates/lab/src/node/health.rs`: minimal raw loopback HTTP health/readiness server for node mode if HTTP health is enabled.
- Modify `crates/lab/src/api/router.rs`: add explicit controller router and node-health router entry points, or keep shared router only for controller mode.
- Modify `crates/lab/src/node/ws_client.rs`: move shared WebSocket backoff helpers out of `dispatch::upstream`.
- Create `crates/lab/src/net/backoff.rs`: shared jitter/backoff helper usable by node and upstream code.
- Modify `crates/lab/src/dispatch/deploy/build.rs`: replace single `build_release()` with role/profile-aware artifact builder.
- Modify `crates/lab/src/node/update.rs`: classify targets by artifact role, build each role once, verify readiness by role.
- Modify `crates/lab/src/dispatch/deploy/runner.rs`: support role/profile artifacts for generic deploy after `nodes update` is production-ready.
- Modify `crates/lab-apis/src/deploy/types.rs`: represent per-artifact/per-host artifact identity in plan and run summaries after `nodes update` is production-ready.
- Modify `crates/lab/Cargo.toml`: add feature groups and optional dependencies for `node-runtime` and `controller`.
- Modify `crates/lab-apis/Cargo.toml` and `crates/lab-apis/src/lib.rs`: make extract/marketplace/controller-only SDK pieces feature-gated where needed.
- Update docs: `docs/NODE_RUNTIME_CONTRACT.md`, `docs/DEVICE_RUNTIME.md`, `docs/DEPLOY.md`, `docs/CLI.md`.
- Update stale `device` naming in docs where it conflicts with the controller/node contract.

## Task 1: Role Resolution Contract in Config and CLI

**Files:**

- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/node/identity.rs`
- Test: `crates/lab/tests/node_config.rs`
- Test: `crates/lab/tests/nodes_runtime.rs`
- Test: `crates/lab/tests/nodes_cli.rs`

- [ ] **Step 1: Add failing config tests**

Add tests for:

```rust
#[test]
fn parses_node_role_controller() {
    let config: LabConfig = toml::from_str(r#"
        [node]
        role = "controller"
        controller = "node-a"
    "#).unwrap();
    assert_eq!(config.node.unwrap().role, Some(NodeRuntimeRole::Controller));
}

#[test]
fn parses_node_role_node() {
    let config: LabConfig = toml::from_str(r#"
        [node]
        role = "node"
        controller = "node-a"
    "#).unwrap();
    assert_eq!(config.node.unwrap().role, Some(NodeRuntimeRole::Node));
}
```

Run: `cargo test --manifest-path crates/lab/Cargo.toml --test node_config node_role`

Expected: fail because `NodeRuntimeRole` and `NodePreferences.role` do not exist.

- [ ] **Step 2: Add failing CLI parse tests**

Add tests for:

```rust
Cli::try_parse_from(["lab", "serve", "--role", "node"]).unwrap();
Cli::try_parse_from(["lab", "serve", "--role", "controller"]).unwrap();
```

Run: `cargo test --manifest-path crates/lab/Cargo.toml --test nodes_cli serve_role`

Expected: fail because `ServeArgs.role` does not exist.

- [ ] **Step 3: Implement role config and CLI enum**

Add to `config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRuntimeRole {
    Controller,
    Node,
}
```

Add `#[serde(default)] pub role: Option<NodeRuntimeRole>` to `NodePreferences`.

Add a clap `ValueEnum` bridge in `serve.rs`:

```rust
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ServeRole {
    Controller,
    Node,
}
```

Add `#[arg(long, value_enum)] pub role: Option<ServeRole>` to `ServeArgs`.

- [ ] **Step 4: Centralize role resolution**

Add a resolver in `node/identity.rs`:

```rust
pub fn resolve_runtime_role_from_config(
    local_host: &str,
    config: &LabConfig,
    override_role: Option<NodeRuntimeRole>,
) -> Result<ResolvedNodeRuntime>
```

Resolution order:

1. CLI override
2. `[node].role`
3. hostname comparison against `config.controller_host()`
4. legacy fallback to current behavior only when no `[node]` config exists

Map `Controller` to existing `NodeRole::Master` and `Node` to `NodeRole::NonMaster` for now.

Explicit failure rule:

- If the resolved role is `Node`, a controller host must be configured through `[node].controller` or legacy `[device].master`.
- `--role node` without a controller host must fail before startup with a clear configuration error.
- `[node].role = "node"` without a controller host must fail before startup with a clear configuration error.

- [ ] **Step 5: Wire `serve` to new resolver before registry construction**

Move local hostname and role resolution before `build_default_registry()` / `filter_registry()`.

Replace the current direct call to `resolve_runtime_role(... config.device.master ...)` in `serve.rs` with `resolve_runtime_role_from_config(&local_host, config, args.role.map(Into::into))`.

`build_default_registry()` must live in the controller path after the node-mode early return. Node mode must not build or filter the controller registry.

- [ ] **Step 6: Verify targeted tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --test node_config node_role
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_cli serve_role
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_runtime
```

Expected: pass.

## Task 2: Split `serve` Runtime Before Controller Startup

**Files:**

- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/node/runtime.rs`
- Create: `crates/lab/src/node/health.rs`
- Modify: `crates/lab/src/node.rs`
- Test: existing `serve` tests in `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/tests/nodes_master_only.rs`

- [ ] **Step 1: Add node-mode startup tests**

Add tests that construct node-mode serve state and assert:

- web asset preparation is not called
- gateway manager is not installed
- upstream discovery is not run
- marketplace registry sync is not started
- MCP is not mounted

If direct hooks are hard, factor pure helpers first:

```rust
fn startup_plan_for_role(role: NodeRole, transport: Transport) -> StartupPlan
```

Expected node plan:

```rust
StartupPlan {
    api_server_enabled: false,
    health_server_enabled: true,
    web_server_enabled: false,
    mcp_server_enabled: false,
    gateway_client_enabled: false,
    marketplace_sync_enabled: false,
}
```

- [ ] **Step 2: Add node runtime helper**

In `node/runtime.rs`, add:

```rust
pub async fn start_background_tasks(&self) {
    if let Err(error) = self.upload_initial_metadata().await { ... }
    if let Err(error) = self.collect_and_queue_bootstrap_logs().await { ... }
    if let Err(error) = self.spawn_ws_flush_loop().await { ... }
}
```

This consolidates the current spawned block in `serve.rs`.

- [ ] **Step 3: Create minimal node health server**

Create `node/health.rs` with:

- `run_loopback_health_server(port: u16) -> Result<ExitCode>`
- bind address must be `127.0.0.1:<port>`
- routes: `GET /health`, `GET /ready`
- no `/v1/*`
- no `/mcp`
- no web fallback

Implement this as a tiny raw `tokio::net::TcpListener` HTTP responder, not an `axum` router. This keeps the future `node-runtime` feature free of `axum`, `tower`, and `tower-http`.

Responses:

- `/health`: `HTTP/1.1 200 OK` with `{"ok":true}`
- `/ready`: `HTTP/1.1 200 OK` with `{"ready":true}`
- anything else: `HTTP/1.1 404 Not Found`

Default behavior:

- Node health is enabled by default.
- Node health binds to `127.0.0.1:<mcp.port>` using the same port value already used for controller health defaults.
- A later config option may disable node health, but this rollout should keep it on for systemd/deploy verification.

- [ ] **Step 4: Branch in `serve.rs` after role resolution**

Immediately after `node_role` is known:

```rust
if matches!(node_role, NodeRole::NonMaster) {
    return run_node_mode(args, config, node_runtime, port).await;
}
```

`run_node_mode` should:

- reject `lab serve mcp --stdio` unless we explicitly design node stdio
- start node background tasks
- keep the process alive through the loopback health server or a foreground node runtime driver

Do not call:

- `build_default_registry()`
- `resolve_auth()`
- `build_upstream_oauth_runtime()`
- `discover_all_with_in_process_peers()`
- `install_gateway_manager()`
- `bootstrap_running_log_system()`
- `ensure_web_assets_are_fresh()`
- `RegistryStore::open()`
- `resolve_workspace_root_from_env()`
- `run_http()`

- [ ] **Step 5: Keep controller path behavior unchanged**

Everything currently after the split remains controller-only:

- registry
- upstream OAuth
- upstream discovery
- gateway manager
- logs system
- OAuth state
- web asset build
- marketplace registry sync
- fs workspace
- full router
- MCP server

- [ ] **Step 6: Verify runtime tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_master_only
cargo test --manifest-path crates/lab/Cargo.toml --lib cli::serve
```

Expected: pass.

## Task 3: Remove Node Compile Edges Into Controller Dispatch

**Files:**

- Create: `crates/lab/src/net.rs`
- Create: `crates/lab/src/net/backoff.rs`
- Modify: `crates/lab/src/lib.rs`
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/src/node/ws_client.rs`
- Modify: `crates/lab/src/dispatch/upstream/transport/websocket.rs`

- [ ] **Step 1: Add failing compile check target**

Run:

```bash
cargo check -p lab --no-default-features --features node-runtime
```

Expected initially: fail because `node-runtime` does not exist.

- [ ] **Step 2: Move shared backoff helpers**

Move `jitter_delay` and `reprobe_backoff` from `dispatch::upstream::transport::websocket` to `net::backoff`.

Update both callers:

```rust
use crate::net::backoff::{jitter_delay, reprobe_backoff};
```

- [ ] **Step 3: Gate `node/update.rs` behind deploy**

In `node.rs`, gate:

```rust
#[cfg(feature = "deploy")]
pub mod update;
```

This prevents node-only builds from pulling deploy orchestration.

- [ ] **Step 4: Add initial `node-runtime` feature**

In `crates/lab/Cargo.toml`, add:

```toml
node-runtime = []
```

Do not make dependencies optional yet in this task; this task is about removing obvious module-level controller edges first.

- [ ] **Step 5: Verify no dispatch-upstream dependency from node runtime**

Run:

```bash
rg -n "dispatch::upstream|dispatch::deploy|crate::api" crates/lab/src/node
```

Expected: no node runtime path depends on controller-only dispatch or API modules, except deploy-gated `node/update.rs`.

## Task 4: Faster Deploy Profiles

**Files:**

- Modify: root `Cargo.toml`
- Modify: `crates/lab/src/dispatch/deploy/build.rs`
- Test: build unit tests in `build.rs`

- [ ] **Step 1: Add custom deploy profiles**

Add:

```toml
[profile.controller-deploy]
inherits = "release"
lto = "off"
codegen-units = 16
incremental = false

[profile.node-deploy]
inherits = "release"
lto = "off"
codegen-units = 16
incremental = false
```

Rationale from Cargo docs: custom profile output goes under a profile-named directory in `target`, and disabling LTO avoids the expensive whole-program link step.

- [ ] **Step 2: Verify artifact paths**

Expected paths:

- `target/controller-deploy/lab`
- `target/node-deploy/lab`

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --lib dispatch::deploy::build
```

Expected: current path tests still pass after adding support for custom profile path calculation.

## Task 5: Role-Specific Build Artifact Model

**Files:**

- Modify: `crates/lab/src/dispatch/deploy/build.rs`
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/tests/node_config.rs`
- Test: deploy build unit tests in `build.rs`

- [ ] **Step 1: Add artifact role config tests**

Add TOML tests:

```toml
[deploy.defaults]
artifact_role = "node"
target_triple = "x86_64-unknown-linux-gnu"

[deploy.hosts.node-a]
artifact_role = "controller"
```

Expected parsed roles:

- defaults role `Node`
- host `node-a` role `Controller`

- [ ] **Step 2: Add artifact model types**

In `config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Controller,
    Node,
}
```

Add optional `artifact_role`, `target_triple`, and `build_timeout_secs` to deploy defaults and host overrides.

- [ ] **Step 3: Replace `build_release()` API**

Introduce:

```rust
pub struct ArtifactProfile {
    pub role: ArtifactRole,
    pub target_triple: String,
    pub bin: String,
    pub cargo_features: Vec<String>,
    pub cargo_profile: String,
}

pub async fn build_artifact(profile: &ArtifactProfile) -> Result<BuildOutcome, DeployError>
```

Keep `build_release()` as a compatibility wrapper only if existing callers still need it during migration.

- [ ] **Step 4: Profile-key rebuild reuse**

Reuse key must include:

- role
- target triple
- binary name
- cargo profile
- feature set

Artifact path must use Cargo profile output directory:

- controller deploy profile: `target/controller-deploy/lab`
- node deploy profile: `target/node-deploy/lab`
- cross target: `target/<triple>/<profile>/lab`

- [ ] **Step 5: Add build timeout**

Wrap cargo invocation with `tokio::time::timeout`.

Build timeout applies only to artifact construction. Rollout/stage timeout starts after artifacts are built or reused.

- [ ] **Step 6: Verify build tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --lib dispatch::deploy::build
cargo test --manifest-path crates/lab/Cargo.toml --test node_config artifact_role
```

Expected: pass.

## Task 6: Controller Connected-State API

**Files:**

- Modify: `crates/lab/src/api/nodes/fleet.rs`
- Modify: `crates/lab/src/api/nodes.rs`
- Modify: `crates/lab/src/node/master_client.rs`
- Test: `crates/lab/tests/nodes_api.rs`
- Test: `crates/lab/tests/nodes_cli.rs`

- [ ] **Step 1: Add failing connected-state API test**

Add a controller API test that proves a node record response distinguishes:

- known inventory record
- currently connected WebSocket session

Expected response shape:

```json
{
  "id": "controller",
  "connected": true
}
```

Use the existing fleet store/session test harness where possible.

- [ ] **Step 2: Add `MasterClient::node_connected`**

Add:

```rust
pub async fn node_connected(&self, node_id: &str) -> Result<bool>
```

This must return `true` only when the controller reports an active WebSocket connection for that node. Inventory presence alone is not sufficient.

- [ ] **Step 3: Keep compatibility for existing `fetch_device`**

Do not remove `fetch_device`; keep CLI/API behavior stable. `node_connected` is a stricter rollout-verification helper.

- [ ] **Step 4: Verify API/client tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_api connected
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_cli
```

Expected: pass.

## Task 7: Role-Based `nodes update`

**Files:**

- Modify: `crates/lab/src/node/update.rs`
- Modify: `crates/lab/src/node/master_client.rs`
- Test: unit tests in `node/update.rs`
- Test: `crates/lab/tests/nodes_cli.rs`

- [ ] **Step 1: Add target classification tests**

Cases:

- `--all` on controller classifies local controller as `Controller`
- remote SSH targets classify as `Node`
- host override can force `Controller`
- explicit remote target does not implicitly update local controller

- [ ] **Step 2: Add artifact role to effective target config**

Extend `EffectiveTargetConfig`:

```rust
artifact_role: ArtifactRole,
target_triple: Option<String>,
build_timeout_secs: Option<u64>,
```

- [ ] **Step 3: Build needed artifacts once**

Replace single artifact build with:

```rust
let profiles = required_artifact_profiles(&resolved_targets);
let artifacts = build_artifacts_once(profiles).await?;
```

Remote nodes use node artifact. Local controller uses controller artifact.

- [ ] **Step 4: Verify node reconnect with retry**

Replace immediate `fetch_device(...).is_ok()` with a helper:

```rust
async fn wait_for_controller_node_connected(client: &MasterClient, node_id: &str, timeout: Duration) -> Result<()>
```

This helper must poll `MasterClient::node_connected`; it must not use `fetch_device(...).is_ok()` as a proxy.

- [ ] **Step 5: Verify controller readiness by HTTP and WS**

Controller verification must:

- run installed binary `--version`
- call `/health`
- call `/ready`
- verify `/v1/nodes/ws` listener accepts WebSocket upgrade or at least TCP/HTTP upgrade reaches the handler
- report the backup binary path in JSON if controller health/readiness fails after install

Do not hardcode `8765`; derive from config.

Recovery behavior:

- If local controller update installs successfully but readiness fails, the result JSON must include the previous binary backup path.
- Do not automatically roll back in this phase unless the current deploy backup mechanism already provides a safe, tested restore path.
- Document the manual recovery command in `docs/DEPLOY.md`.

- [ ] **Step 6: Verify targeted tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --lib node::update
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_cli
```

Expected: pass.

## Task 8: Readiness and systemd Contract

**Files:**

- Modify: `crates/lab/src/node/update.rs`
- Modify: deploy service/unit docs if present
- Modify: `docs/DEPLOY.md`
- Modify: `docs/NODE_RUNTIME_CONTRACT.md`

- [ ] **Step 1: Stop treating `systemctl is-active` as readiness**

Keep `systemctl is-active` only as process liveness.

Readiness must be proven by:

- `/health`
- `/ready`
- WebSocket listener for controller
- controller-observed node connection for nodes

- [ ] **Step 2: Decide `Type=notify` follow-up**

Do not block this rollout on `sd_notify`, but document it as the better future systemd contract:

- `Type=notify`
- `READY=1` sent after HTTP/WS listener bind and initial startup completes

- [ ] **Step 3: Verify restart waits use application probes**

Add retry/backoff around controller and node readiness probes. Timeouts should be configurable and reported in rollout output.

## Task 9: Controller Self-Update Recovery Contract

**Files:**

- Modify: `crates/lab/src/node/update.rs`
- Modify: `docs/DEPLOY.md`
- Modify: `docs/NODE_RUNTIME_CONTRACT.md`
- Test: unit tests in `node/update.rs`

- [ ] **Step 1: Add recovery-output test**

Add a unit test for local controller update failure after install:

- install succeeds
- restart succeeds or partially succeeds
- readiness probe fails
- result JSON includes `backup_path`
- result JSON includes failed stage and recovery hint

- [ ] **Step 2: Return local backup path from install**

Change local install helper so it returns backup metadata:

```rust
struct LocalInstallOutcome {
    backup_path: Option<PathBuf>,
}
```

Remote install can keep existing behavior initially unless remote backup reporting is already easy to expose.

- [ ] **Step 3: Surface recovery hint**

If controller readiness fails after local install, include a hint equivalent to:

```text
sudo install -m 755 <backup_path> <remote_path> && sudo systemctl restart lab
```

Use the actual configured service scope and restart model when rendering the hint.

- [ ] **Step 4: Document manual recovery**

In `docs/DEPLOY.md`, add a short controller self-update recovery section:

- where backups are written
- how to restore
- how to restart
- how to verify `/health` and `/ready`

- [ ] **Step 5: Verify recovery tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --lib node::update recovery
```

Expected: pass.

## Task 10: Cargo Feature Split, Part A: Feature Groups and Obvious Gates

**Files:**

- Modify: `crates/lab/Cargo.toml`
- Modify: `crates/lab/src/node.rs`
- Modify: `crates/lab/src/lib.rs`
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/src/dispatch.rs`

- [ ] **Step 1: Add feature groups while preserving default behavior**

Add:

```toml
node-runtime = [
  "lab-apis/device_runtime",
]

controller = [
  "cli",
  "api",
  "mcp",
  "gateway",
  "oauth",
  "marketplace",
  "extract",
  "doctor",
  "logs",
  "acp",
]

services-all = [
  "radarr",
  "sonarr",
  ...
]

all = [
  "controller",
  "services-all",
  "deploy",
  "fs",
  "lab-admin",
  "mcpregistry",
  "acp_registry",
]
```

Keep `default = ["all"]`.

- [ ] **Step 2: Gate obvious compile roots**

Gate:

- `node::update` behind `deploy`
- `dispatch::gateway` behind `gateway`
- `dispatch::marketplace` behind `marketplace`
- `dispatch::upstream` behind `gateway`

- [ ] **Step 3: Verify all-features still passes**

Run:

```bash
cargo check -p lab --all-features
```

Expected: pass.

## Task 11: Cargo Feature Split, Part B: Optional Controller Dependencies

**Files:**

- Modify: `crates/lab/Cargo.toml`
- Modify: `crates/lab/src/api.rs`
- Modify: `crates/lab/src/mcp.rs`
- Modify: `crates/lab/src/oauth.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/tui.rs`

- [ ] **Step 1: Make dependencies optional in `crates/lab`**

Make optional:

- `lab-auth`
- `rmcp`
- `jsonwebtoken`
- `rusqlite`
- `r2d2`
- `r2d2_sqlite`
- `oauth2`
- `axum`
- `tower`
- `tower-http`
- `utoipa`
- `utoipa-scalar`
- `ratatui`
- `crossterm`
- `clap_complete`
- `tabled`
- `dialoguer`

Keep `tokio`, `futures`, `reqwest`, `serde`, `serde_json`, `anyhow`, `tracing`, `tokio-tungstenite`, `uuid`, and node queue dependencies available to node runtime.

- [ ] **Step 2: Gate controller compile roots**

Gate:

- `api` behind `api`
- `mcp` behind `mcp`
- `oauth::upstream` behind `oauth`
- `tui` behind `tui`
- controller CLI commands behind matching features

- [ ] **Step 3: Decide node binary entrypoint**

Use the same `lab` binary with `--no-default-features --features node-runtime` unless this proves more tangled than a separate `lab-node` binary.

The node-runtime CLI surface only needs enough to run node mode. It must not expose controller commands.

- [ ] **Step 4: Verify node-runtime excludes controller deps**

Run:

```bash
cargo check -p lab --no-default-features --features node-runtime
cargo tree -p lab --no-default-features --features node-runtime -i axum
cargo tree -p lab --no-default-features --features node-runtime -i rmcp
cargo tree -p lab --no-default-features --features node-runtime -i rusqlite
```

Expected:

- node-runtime check passes
- `axum`, `rmcp`, and `rusqlite` are absent

## Task 12: Cargo Feature Split, Part C: `lab-apis` Optional Controller SDK Dependencies

**Files:**

- Modify: `crates/lab-apis/Cargo.toml`
- Modify: `crates/lab-apis/src/lib.rs`

- [ ] **Step 1: Gate extract dependencies**

Make `russh`, `russh-sftp`, `russh-config`, and `quick-xml` optional behind `extract`.

- [ ] **Step 2: Gate SQLite dependency**

Move unconditional `rusqlite` behind the feature that actually needs it.

- [ ] **Step 3: Gate marketplace and MCP registry SDK modules if not needed by node**

Marketplace and MCP registry should not be always-on in node-runtime unless a node-local operation explicitly requires them.

- [ ] **Step 4: Verify lab-apis no-default dependency absence**

Run:

```bash
cargo check -p lab-apis --no-default-features
cargo tree -p lab-apis --no-default-features -i russh
cargo tree -p lab-apis --no-default-features -i rusqlite
```

Expected:

- no-default check passes
- `russh` and `rusqlite` are absent

## Task 13: Generic Deploy Runner Artifact Split

**Files:**

- Modify: `crates/lab/src/dispatch/deploy/runner.rs`
- Modify: `crates/lab/src/dispatch/deploy/stages.rs`
- Modify: `crates/lab-apis/src/deploy/types.rs`
- Test: `crates/lab/tests/deploy_runner.rs`

This task intentionally comes after role-based `nodes update` and live rollout. `nodes update` is the production path that must work first; generic deploy runner support is the follow-up that makes the lower-level deploy API match the same artifact model.

- [ ] **Step 1: Add failing deploy runner tests**

Add tests:

- deploy plan includes per-host artifact role/profile
- deploy run builds controller artifact once and node artifact once
- node hosts receive node artifact sha
- controller host receives controller artifact sha
- transfer skip uses selected artifact sha

- [ ] **Step 2: Carry artifact role in `HostJob`**

Extend `HostJob` with resolved artifact profile.

- [ ] **Step 3: Build artifact map before job execution**

Change `run_jobs` from one `Arc<BuildOutcome>` to:

```rust
BTreeMap<ArtifactProfileKey, Arc<BuildOutcome>>
```

Each job looks up its artifact before running the pipeline.

- [ ] **Step 4: Update plan and summary types**

Replace single summary artifact sha with either:

- `artifacts: Vec<DeployArtifactSummary>`
- per-host artifact fields

Prefer both: summary-level unique artifacts and host-level selected artifact role/sha.

- [ ] **Step 5: Verify deploy runner tests**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml --test deploy_runner
```

Expected: pass.

## Task 14: Documentation Consistency Cleanup

**Files:**

- Modify: `docs/DEVICE_RUNTIME.md`
- Modify: `docs/NODE_RUNTIME_CONTRACT.md`
- Modify: `docs/DEPLOY.md`
- Modify: `docs/CLI.md`

- [ ] **Step 1: Remove stale device route examples**

Update docs that still refer to `/v1/device/*` routes as current node runtime behavior.

Use `/v1/nodes/*` only for controller compatibility/operator routes, and describe node-to-controller delivery as WebSocket-first.

- [ ] **Step 2: Normalize naming**

Prefer:

- `controller`
- `node`
- `controller host`
- `node runtime`

Keep `master` / `non-master` only when documenting legacy config compatibility.

- [ ] **Step 3: Document feature/artifact split**

Add docs explaining:

- controller artifact
- node artifact
- node health default
- live rollout verification
- manual recovery path

- [ ] **Step 4: Verify docs references**

Run:

```bash
rg -n "/v1/device|non-master|master-only|device runtime|device-token|device-enrollments" docs
```

Expected: remaining matches are either historical notes or explicitly marked legacy compatibility.

## Task 15: Live Config Audit Before Rollout

**Files:**

- No code changes unless audit discovers invalid config.

- [ ] **Step 1: Audit local controller config**

Check:

```bash
target/debug/lab --json config show
```

If no config command exists, inspect `~/.labby/config.toml` directly.

Required:

- `[node].controller` is set and matches the controller host.
- `[node].role` is either absent on controller or explicitly `controller`.
- deploy defaults have a restart policy.
- controller host override has `artifact_role = "controller"` or is inferable.
- remote node host overrides have `artifact_role = "node"` or are inferable.

- [ ] **Step 2: Audit target architecture assumptions**

Check every configured update target:

```bash
ssh <target> uname -m
```

Expected for first rollout:

- every target is compatible with the host-built artifact
- if any target differs, stop and add cross-compilation work before rollout

- [ ] **Step 3: Audit restart policies and install paths**

For each target:

- install path exists or parent is writable through configured privilege model
- restart policy matches actual service manager
- controller has a tested backup path

- [ ] **Step 4: Audit current connectivity**

Before update:

```bash
target/debug/lab --json nodes list
```

Expected:

- controller API is reachable
- expected nodes are currently connected or known
- disconnected nodes are explicitly accepted as rollout targets before proceeding

## Task 16: Full Verification and Live Rollout

**Files:**

- No code changes unless failures are found.

- [ ] **Step 1: Run targeted checks**

```bash
cargo test --manifest-path crates/lab/Cargo.toml --test node_config
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_cli
cargo test --manifest-path crates/lab/Cargo.toml --test nodes_master_only
cargo test --manifest-path crates/lab/Cargo.toml --test deploy_runner
```

- [ ] **Step 2: Run feature checks**

```bash
cargo check -p lab --no-default-features --features node-runtime
cargo check -p lab --no-default-features --features controller
cargo check --workspace --all-features
cargo test --no-run --message-format short --workspace --all-features
```

- [ ] **Step 3: Run full repo verification**

```bash
just check
just build
just test
just lint
```

- [ ] **Step 4: Run live node update**

```bash
target/debug/lab --json nodes update --all
```

Expected:

- controller artifact is reused or built once
- node artifact is reused or built once
- remotes receive node artifact
- local controller receives controller artifact
- artifact role/profile is visible in rollout JSON for every target
- no node starts Web UI, MCP, gateway, marketplace, registry sync, or full API
- controller comes back healthy and ready
- nodes reconnect over WebSocket
- final JSON has `"ok": true`

- [ ] **Step 5: If live rollout fails, debug systematically**

Collect:

```bash
systemctl status lab --no-pager --lines=80
journalctl -u lab --since '10 minutes ago' --no-pager
target/debug/lab --json nodes list
```

Continue fixing until targeted verification, full verification, and live rollout pass.
