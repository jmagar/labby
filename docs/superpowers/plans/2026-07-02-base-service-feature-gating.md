# Base-Service Feature Gating Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put the three ungated base services — `stash`, `acp`, and `nodes` (fleet) — behind cargo features so `cargo build -p labby --no-default-features --features gateway` produces a gateway-only binary without them (~32k LOC excluded), while the default all-features build is byte-for-byte behaviorally unchanged. Docs generation (`labby docs`, `src/docs/`) deliberately stays always-on: it is registry-driven, documents whatever services are compiled in (including the gateway's own action catalog), and drags no dependencies.

**Architecture:** Mirror the existing `#[cfg(feature = "gateway")]` / `#[cfg(feature = "marketplace")]` gating pattern exactly: gate module declarations at each surface (lib.rs / dispatch.rs / cli.rs / api mounts / mcp services / registry.rs), gate `AppState` fields and `serve` startup wiring, and add config-without-feature rejection helpers mirroring `reject_protected_routes_without_gateway()`. Three new features: `stash`, `acp`, `nodes`, all members of `all`.

**Tech Stack:** Rust 2024, cargo features, clap, axum, existing `cargo check --no-default-features` CI slice matrix.

## Global Constraints

- The `all` feature remains the default and the authoritative build. Every new feature MUST be added to `all` in `crates/labby/Cargo.toml` so `cargo build --all-features` and the default build are unchanged.
- Slice checks compile with `--all-targets`, so integration tests referencing a gated service MUST get a `#![cfg(feature = "<name>")]` crate-level attribute.
- Slice builds are allowed to have dead-code warnings (CI sets `RUSTFLAGS: ""` for slice jobs — see `.github/workflows/ci.yml:186-217`). The all-features build must stay warning-clean (`just lint` / lefthook pre-commit runs clippy `-D warnings`).
- No `mod.rs` files. Modern module style only (`foo.rs` sibling to `foo/`).
- Feature names (exact): `stash`, `acp`, `nodes`.
- New feature dependencies (exact): `marketplace` requires `acp`, `nodes`, and `stash` (because `dispatch/marketplace/acp_dispatch.rs` uses `crate::acp`, marketplace's remote plugin install pushes RPCs to fleet nodes via `dispatch/node/send.rs` from `api/services/marketplace.rs` and `dispatch/marketplace/mcp_dispatch.rs`, and `dispatch/marketplace/stash_bridge.rs` uses `crate::dispatch::stash` internals — the `nodes` dependency was discovered during implementation). The `marketplace` feature on main ALREADY requires `gateway` (spawn-guard/SSRF checks live in labby-gateway) — preserve the existing `"gateway"` entry and its comment when appending. `acp` owns the `agent-client-protocol` dependency via `dep:agent-client-protocol`.
- Commit style: conventional prefixes as used in this repo (`feat:`, `fix:`, `ci:`).
- Repo rule: create a bead before writing code (`bd create`), claim it, close it at the end.
- The slice verification command used throughout (run from the repo root):
  ```bash
  RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets
  ```
- The all-features verification commands used throughout:
  ```bash
  cargo check --workspace --all-features
  just test    # cargo nextest run --workspace --all-features
  ```

---

### Task 0: Bead + baseline

**Files:**
- None modified. Baseline verification only.

**Interfaces:**
- Consumes: clean `claude/awesome-faraday-9d424e` worktree.
- Produces: a claimed bead ID (referenced in later commit messages) and a recorded green baseline.

- [ ] **Step 1: Create and claim the bead**

```bash
bd create --title="Feature-gate base services (stash, acp, nodes) for gateway-only builds" \
  --description="Gate the three ungated base services behind cargo features so --no-default-features --features gateway excludes them (~32k LOC). Mirrors the existing gateway/marketplace cfg pattern. all-features build unchanged. docs-gen stays always-on (registry-driven, documents the gateway too)." \
  --type=task --priority=2
bd update <returned-id> --claim
```

- [ ] **Step 2: Record the green baseline**

```bash
cargo check --workspace --all-features
RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets
```

Expected: both succeed (warnings OK on the slice check). If either fails, STOP — fix main first; this plan assumes a green start.

- [ ] **Step 3: Confirm the fat is present in the gateway slice (the "failing test" for this whole plan)**

```bash
cargo tree -p labby --no-default-features --features gateway | grep agent-client-protocol
cargo run -p labby --no-default-features --features gateway -- stash --help >/dev/null && echo "stash present"
cargo run -p labby --no-default-features --features gateway -- nodes --help >/dev/null && echo "nodes present"
```

Expected: `agent-client-protocol v0.13.1` appears; both `present` lines print. By the end of Task 3 all three probes must come up empty/fail.

---

### Task 1: Gate `stash`

**Files:**
- Modify: `crates/labby/Cargo.toml` (features table, ~line 143)
- Modify: `crates/labby/src/dispatch.rs:23`
- Modify: `crates/labby/src/cli.rs:27,100-101,132`
- Modify: `crates/labby/src/api/services.rs` (the `pub mod stash;` line)
- Modify: `crates/labby/src/api/router.rs:1145`
- Modify: `crates/labby/src/registry.rs` (stash registration block, ~lines 572-583)
- Possibly modify: any test file the slice check flags (see Step 7)

**Interfaces:**
- Consumes: existing `crate::dispatch::stash::{catalog::ACTIONS, dispatch::dispatch}`.
- Produces: cargo feature `stash` (empty feature, `[]`); `marketplace` feature now lists `"stash"`. Later tasks rely on the exact feature name `stash`.

- [ ] **Step 1: Declare the feature and wire dependents in `crates/labby/Cargo.toml`**

In the `[features]` table:

```toml
all = ["labby-apis/all", "lab-admin", "acp_registry", "deploy", "gateway", "marketplace", "stash"]
# Depends on `gateway` because marketplace's stdio-install validation reuses
# the shared spawn-guard/SSRF checks, which now live in labby-gateway (moved
# out of labby-runtime so labby-auth/labby-codemode don't pull them in).
# Also requires `stash`: fork persistence goes through dispatch/marketplace/stash_bridge.rs.
marketplace = ["labby-apis/mcpregistry", "labby-runtime/marketplace", "gateway", "stash"]
stash = []
```

(`all` gets `stash` appended; `marketplace` keeps its existing `"gateway"` entry and comment and gets `"stash"` appended; new `stash = []` line in alphabetical position.)

- [ ] **Step 2: Gate the dispatch module** — `crates/labby/src/dispatch.rs:23`:

```rust
#[cfg(feature = "stash")]
pub mod stash;
```

- [ ] **Step 3: Gate the CLI surface** — `crates/labby/src/cli.rs`, three edits:

Line 27:
```rust
#[cfg(feature = "stash")]
pub mod stash;
```

Lines 100-101 (enum variant):
```rust
    /// Component versioning and deployment.
    #[cfg(feature = "stash")]
    Stash(stash::StashArgs),
```

Line 132 (dispatch arm):
```rust
        #[cfg(feature = "stash")]
        Command::Stash(args) => stash::run(args, format).await,
```

- [ ] **Step 4: Gate the HTTP surface** — two files.

`crates/labby/src/api/services.rs` (the bare `pub mod stash;` line):
```rust
#[cfg(feature = "stash")]
pub mod stash;
```

`crates/labby/src/api/router.rs:1145` — the mount is a method call in a builder chain (`.nest("/stash", services::stash::routes(state.clone()))`). Break it out of the chain into a gated statement, following the shape already used for gateway mounts in this file:

```rust
    #[cfg(feature = "stash")]
    {
        v1 = v1.nest("/stash", services::stash::routes(state.clone()));
    }
```

(If line 1145 sits mid-chain on a `let` binding, split the chain: bind the router up to the line before, apply the gated `nest` as a statement, then continue. Look at how `#[cfg(feature = "gateway")]` mounts around line 1074-1097 are structured and copy that shape.)

- [ ] **Step 5: Gate the registry registration** — `crates/labby/src/registry.rs`, the block commented `// stash is always-on (no feature flag). Manages versioned component snapshots.` (~lines 572-583). Replace the comment and gate the block:

```rust
    // stash is feature-gated: versioned component snapshots (required by marketplace).
    #[cfg(feature = "stash")]
    {
        let meta = labby_apis::stash::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::stash::catalog::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::stash::dispatch::dispatch),
        });
    }
```

- [ ] **Step 6: Verify all-features is unchanged**

```bash
cargo check --workspace --all-features
```

Expected: PASS with no new warnings (`stash` is in `all`, so nothing is actually excluded).

- [ ] **Step 7: Run the slice check and fix stragglers**

```bash
RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets 2>&1 | grep -E '^error' | head -30
```

Expected: compile errors pointing at every remaining unconditional reference to `crate::dispatch::stash` or `cli::stash` — e.g. registry unit tests asserting stash is registered, or an integration test in `crates/labby/tests/`. For each:
- Unit test inside a shared file → wrap the test fn in `#[cfg(feature = "stash")]`.
- Whole integration test file about stash → add `#![cfg(feature = "stash")]` as the first line.
- A shared code path (non-test) referencing stash → gate the statement with `#[cfg(feature = "stash")]` blocks, same shape as Steps 2-5.

Re-run until zero errors (warnings are fine).

- [ ] **Step 8: Verify the red probe went green**

```bash
cargo run -p labby --no-default-features --features gateway -- stash --help
```

Expected: exits non-zero with `unrecognized subcommand 'stash'`.

- [ ] **Step 9: Run the full all-features test suite**

```bash
just test
```

Expected: PASS (identical to baseline).

- [ ] **Step 10: Commit**

```bash
git add crates/labby/Cargo.toml crates/labby/src crates/labby/tests
git commit -m "feat: gate stash behind a cargo feature (bead <id>)"
```

---

### Task 2: Gate `acp`

**Files:**
- Modify: `crates/labby/Cargo.toml:78` (make `agent-client-protocol` optional) and the features table
- Modify: `crates/labby/src/lib.rs:22`
- Modify: `crates/labby/src/dispatch.rs:1`
- Modify: `crates/labby/src/api/services.rs` (the `pub mod acp;` line)
- Modify: `crates/labby/src/api/router.rs:1109`
- Modify: `crates/labby/src/api/state.rs:8,68,149,315-323`
- Modify: `crates/labby/src/cli/serve.rs:328-336,474`
- Modify: `crates/labby/src/registry.rs` (acp registration block, ~lines 558-569)
- Modify: `crates/labby/tests/acp_docker_provider_config.rs`, `crates/labby/tests/acp_backend_contract.rs`

**Interfaces:**
- Consumes: feature name `stash` already exists (Task 1) — needed because this step also edits the `marketplace` feature line.
- Produces: cargo feature `acp = ["dep:agent-client-protocol"]`; `marketplace` now lists both `"acp"` and `"stash"`. `AppState.acp_registry` and `with_acp_registry()` exist only under `#[cfg(feature = "acp")]`.

- [ ] **Step 1: Cargo.toml — optional dep + feature wiring**

Line 78, change:
```toml
agent-client-protocol = { version = "=0.13.1", features = ["unstable"], optional = true }
```

Features table:
```toml
acp = ["dep:agent-client-protocol"]
all = ["labby-apis/all", "lab-admin", "acp", "acp_registry", "deploy", "gateway", "marketplace", "stash"]
marketplace = ["labby-apis/mcpregistry", "labby-runtime/marketplace", "gateway", "acp", "stash"]
```

(`marketplace` keeps the existing `"gateway"` entry and its comment; `"acp"` is appended alongside the `"stash"` added in Task 1. NOTE — as-shipped: Task 3/4 additionally appends `"nodes"` because marketplace's remote plugin install uses `dispatch::node::send`; the final line is `marketplace = ["labby-apis/mcpregistry", "labby-runtime/marketplace", "gateway", "acp", "nodes", "stash"]`.)

Note: `acp` and `acp_registry` are distinct, unrelated features — do not merge them. `acp_registry` is the marketplace-facing agent installer SDK; `acp` is the chat/session runtime being gated here.

- [ ] **Step 2: Gate the two module trees**

`crates/labby/src/lib.rs:22`:
```rust
#[cfg(feature = "acp")]
pub mod acp;
```

`crates/labby/src/dispatch.rs:1`:
```rust
#[cfg(feature = "acp")]
pub mod acp;
```

- [ ] **Step 3: Gate the HTTP surface**

`crates/labby/src/api/services.rs`:
```rust
#[cfg(feature = "acp")]
pub mod acp;
```

`crates/labby/src/api/router.rs:1109` — same chain-breaking treatment as Task 1 Step 4:
```rust
    #[cfg(feature = "acp")]
    {
        v1 = v1.nest("/acp", services::acp::routes(state.clone()));
    }
```

- [ ] **Step 4: Gate `AppState`** — `crates/labby/src/api/state.rs`, four edits:

Line 8 (import):
```rust
#[cfg(feature = "acp")]
use crate::acp::registry::AcpSessionRegistry;
```

Line 68 (field — keep the doc comment):
```rust
    /// Shared ACP session registry for browser chat/session routes.
    #[cfg(feature = "acp")]
    pub acp_registry: Arc<AcpSessionRegistry>,
```

Line 149 (default init inside `from_registry`):
```rust
            #[cfg(feature = "acp")]
            acp_registry: Arc::new(AcpSessionRegistry::new()),
```

Lines 315-323 (builder — keep the invariant doc comment):
```rust
    #[cfg(feature = "acp")]
    #[must_use]
    pub fn with_acp_registry(mut self, registry: Arc<AcpSessionRegistry>) -> Self {
        self.acp_registry = registry;
        self
    }
```

- [ ] **Step 5: Gate the serve wiring** — `crates/labby/src/cli/serve.rs`, two edits:

Lines 328-336, wrap the registry creation block:
```rust
    // Create the ACP session registry before the HTTP/stdio split so both transports
    // share the same process-global dispatch slot (intra-process only — stdio and
    // HTTP modes are mutually exclusive within one process).
    #[cfg(feature = "acp")]
    let acp_registry = {
        let acp_registry = Arc::new(crate::acp::registry::AcpSessionRegistry::from_env().await);
        crate::dispatch::acp::install_registry(Arc::clone(&acp_registry));
        acp_registry.restore_from_db().await;
        tracing::info!(
            subsystem = "acp",
            phase = "ready",
            "ACP session registry installed"
        );
        acp_registry
    };
```

Lines 471-474, break `.with_acp_registry(...)` out of the builder chain (mirror the `#[cfg(feature = "gateway")]` block at lines 488-491):
```rust
    let mut state = AppState::from_registry(registry)
        .with_config(config.clone())
        .with_http_bind_host(host.clone());
    #[cfg(feature = "acp")]
    {
        state = state.with_acp_registry(Arc::clone(&acp_registry));
    }
```

- [ ] **Step 6: Gate the registry registration** — `crates/labby/src/registry.rs`, the block commented `// acp is always-on (no feature flag). MCP and CLI surfaces are Phase 2.` (~lines 558-569):

```rust
    // acp is feature-gated: chat/session runtime for the web UI (required by marketplace).
    #[cfg(feature = "acp")]
    {
        let meta = labby_apis::acp::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
            kind: registered_service_kind(meta.name, meta.category),
            status: "available",
            actions: crate::dispatch::acp::catalog::ACTIONS,
            dispatch: dispatch_fn!(crate::dispatch::acp::dispatch::dispatch),
        });
    }
```

- [ ] **Step 7: Gate the ACP integration tests** — add as the FIRST line of each file:

`crates/labby/tests/acp_docker_provider_config.rs` and `crates/labby/tests/acp_backend_contract.rs`:
```rust
#![cfg(feature = "acp")]
```

- [ ] **Step 8: Verify all-features unchanged**

```bash
cargo check --workspace --all-features
```

Expected: PASS, no new warnings.

- [ ] **Step 9: Slice check and fix stragglers**

```bash
RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets 2>&1 | grep -E '^error' | head -30
```

Fix remaining unconditional `crate::acp` references per the Task 1 Step 7 three-way rule. Known verified consumers are only: `api/state.rs`, `cli/serve.rs`, `dispatch/marketplace/acp_dispatch.rs` (covered by the `marketplace → acp` feature dep). Anything else the compiler finds gets the same treatment.

- [ ] **Step 10: Verify the dependency actually dropped (red probe green)**

```bash
cargo tree -p labby --no-default-features --features gateway | grep agent-client-protocol || echo "GONE"
```

Expected: `GONE`.

- [ ] **Step 11: Verify the marketplace slice still compiles (it now pulls acp+stash transitively)**

```bash
RUSTFLAGS="" cargo check -p labby --no-default-features --features marketplace --all-targets
```

Expected: PASS.

- [ ] **Step 12: Full test suite + commit**

```bash
just test
git add crates/labby/Cargo.toml crates/labby/src crates/labby/tests
git commit -m "feat: gate ACP runtime behind acp feature, drop agent-client-protocol from gateway slice (bead <id>)"
```

---

### Task 3: Gate `nodes` (fleet)

The biggest task. The `node` module is both the controller-side fleet machinery (enrollment, NodeStore, fleet websocket) and the node-agent runtime (`run_node_mode`, ws_client, update, install). Two submodules stay always-on because keep-side code uses them: `node/identity.rs` (hostname + role resolution — used by `api/router.rs:1311` and serve startup) and nothing else.

**Files:**
- Modify: `crates/labby/Cargo.toml` (features table)
- Modify: `crates/labby/src/node.rs` (gate all submodules except `identity`)
- Modify: `crates/labby/src/dispatch.rs:16`
- Modify: `crates/labby/src/cli.rs:20,79-80,120`
- Modify: `crates/labby/src/cli/logs.rs` (gate `Search`/`Forward` subcommands, keep `Local`)
- Modify: `crates/labby/src/dispatch/logs.rs:4` (gate `pub mod forward;`)
- Modify: `crates/labby/src/mcp/services.rs:13`
- Modify: `crates/labby/src/api.rs:36`
- Modify: `crates/labby/src/api/router.rs:1066,1539-1553`
- Modify: `crates/labby/src/api/state.rs:12-13,62-64,146-147,210-218`
- Modify: `crates/labby/src/cli/serve.rs:44,46,219-226,243-277,518-519` and `run_node_mode` (~line 1249)
- Modify: `crates/labby/src/registry.rs` (device registration, ~lines 533-541)
- Modify: 8 test files (Step 11)

**Interfaces:**
- Consumes: nothing from earlier tasks (independent).
- Produces: cargo feature `nodes = []`. `node::identity` remains always-on. New helper `fn node_role_without_nodes(resolved: &...) -> Result<NodeRole>` in `cli/serve.rs` under `#[cfg(not(feature = "nodes"))]`. The exact enum variants used below (`NodeRole::NonMaster`, the controller-side counterpart, and `resolved_runtime.role`'s type) must be read from `crates/labby/src/config.rs` before writing Step 6 — the serve code at `cli/serve.rs:65-78` shows `ServeRole::{Controller, Node}` mapping to `crate::config::NodeRuntimeRole`, and line 224 matches `crate::config::NodeRole::NonMaster`.

- [ ] **Step 1: Declare the feature** — `crates/labby/Cargo.toml` `[features]`:

```toml
all = ["labby-apis/all", "lab-admin", "acp", "acp_registry", "deploy", "gateway", "marketplace", "nodes", "stash"]
nodes = []
```

Note: an empty orphan feature `node-runtime = []` already exists in this table with zero `cfg` references in src. Leave it alone in this task (removing it is a separate cleanup decision).

- [ ] **Step 2: Gate the node submodules** — `crates/labby/src/node.rs`. Keep `identity` ungated; gate everything else. `update` is already gated on `deploy` — it additionally needs `nodes` since `cli/nodes.rs` (its only consumer, line 81) is nodes-gated:

```rust
#[cfg(feature = "nodes")]
pub mod checkin;
#[cfg(feature = "nodes")]
pub mod config_scan;
#[cfg(feature = "nodes")]
pub mod enrollment;
#[cfg(feature = "nodes")]
pub mod health;
pub mod identity;
#[cfg(feature = "nodes")]
pub mod install;
#[cfg(feature = "nodes")]
pub mod log_collect;
#[cfg(feature = "nodes")]
pub mod log_event;
#[cfg(feature = "nodes")]
pub mod log_store;
#[cfg(feature = "nodes")]
pub mod master_client;
#[cfg(feature = "nodes")]
pub mod oauth;
#[cfg(feature = "nodes")]
pub mod queue;
#[cfg(feature = "nodes")]
pub mod runtime;
#[cfg(feature = "nodes")]
pub mod store;
#[cfg(feature = "nodes")]
pub mod sysmetrics;
#[cfg(feature = "nodes")]
pub mod token;
#[cfg(all(feature = "nodes", feature = "deploy"))]
pub mod update;
#[cfg(feature = "nodes")]
pub mod ws_client;
```

- [ ] **Step 3: Gate dispatch + MCP + api module decls**

`crates/labby/src/dispatch.rs:16`:
```rust
#[cfg(feature = "nodes")]
pub mod node;
```

`crates/labby/src/mcp/services.rs:13`:
```rust
#[cfg(feature = "nodes")]
pub mod nodes;
```

`crates/labby/src/api.rs:36`:
```rust
#[cfg(feature = "nodes")]
pub mod nodes;
```

- [ ] **Step 4: Gate the CLI surface** — `crates/labby/src/cli.rs`:

Line 20:
```rust
#[cfg(feature = "nodes")]
pub mod nodes;
```

Lines 79-80 (enum variant):
```rust
    /// Query nodes from the configured controller.
    #[cfg(feature = "nodes")]
    Nodes(nodes::NodesArgs),
```

Line 120 (dispatch arm):
```rust
        #[cfg(feature = "nodes")]
        Command::Nodes(args) => nodes::run(args, format, &config).await,
```

- [ ] **Step 5: Gate the fleet halves of the logs CLI** — `crates/labby/src/cli/logs.rs`. The `Local` subcommand stays (local log store is a keep-feature); `Search` (queries the fleet master) and `Forward` (node→master syslog forwarding) are fleet features.

On the `LogsCommand` enum variants `Search { device, query }` and `Forward(...)`, add:
```rust
    #[cfg(feature = "nodes")]
```
(one attribute above each of the two variants; `Local(LocalLogsArgs)` stays ungated.)

In `run()` (line 130), gate the two match arms:
```rust
pub async fn run(args: LogsArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    match args.command {
        #[cfg(feature = "nodes")]
        LogsCommand::Search { device, query } => {
            let value = search_logs(config, &device, &query).await?;
            print(&value, format)?;
            Ok(ExitCode::SUCCESS)
        }
        LogsCommand::Local(local) => run_local(local, format, config).await,
        #[cfg(feature = "nodes")]
        LogsCommand::Forward(args) => run_forward(args, config).await,
    }
}
```

Gate the two helper fns (`search_logs` at line 142 and `run_forward` wherever defined in this file) with `#[cfg(feature = "nodes")]` above each.

And `crates/labby/src/dispatch/logs.rs:4`:
```rust
#[cfg(feature = "nodes")]
pub mod forward;
```

- [ ] **Step 6: Gate serve startup** — `crates/labby/src/cli/serve.rs`. Five edits.

(a) Imports, lines 44 and 46:
```rust
#[cfg(feature = "nodes")]
use crate::node::enrollment::store::EnrollmentStore;
```
```rust
#[cfg(feature = "nodes")]
use crate::node::runtime::NodeRuntime;
```

(b) Role resolution, lines 219-226. Current code:
```rust
    let node_runtime = NodeRuntime::from_config(resolved_runtime, config, Some(port))?;
    let node_role = node_runtime.role();

    // Early return for node (non-controller) processes: skip the full
    // controller startup (registry build, OAuth, gateway, logs system, web UI, etc.).
    if matches!(node_role, crate::config::NodeRole::NonMaster) {
        return run_node_mode(transport, args.command.as_ref(), config, node_runtime, port).await;
    }
```

Replace with:
```rust
    #[cfg(feature = "nodes")]
    let node_runtime = NodeRuntime::from_config(resolved_runtime, config, Some(port))?;
    #[cfg(feature = "nodes")]
    let node_role = node_runtime.role();
    #[cfg(not(feature = "nodes"))]
    let node_role = node_role_without_nodes(&resolved_runtime)?;

    // Early return for node (non-controller) processes: skip the full
    // controller startup (registry build, OAuth, gateway, logs system, web UI, etc.).
    #[cfg(feature = "nodes")]
    if matches!(node_role, crate::config::NodeRole::NonMaster) {
        return run_node_mode(transport, args.command.as_ref(), config, node_runtime, port).await;
    }
```

And add the helper next to `reject_protected_routes_without_gateway` (~line 1240), mirroring its pattern. Read `crates/labby/src/config.rs` first to confirm the exact field/variant names on `ResolvedNodeRuntime` (its `.role` is logged at serve.rs:214) and `NodeRole`; the shape is:

```rust
/// Mirror of `reject_protected_routes_without_gateway`: a build without the
/// `nodes` feature can only run as a controller. Fail loudly if config asks
/// for node mode instead of silently ignoring it.
#[cfg(not(feature = "nodes"))]
fn node_role_without_nodes(
    resolved: &crate::config::ResolvedNodeRuntime,
) -> Result<crate::config::NodeRole> {
    let role = crate::config::NodeRole::from(resolved.role);
    if matches!(role, crate::config::NodeRole::NonMaster) {
        anyhow::bail!(
            "node (non-controller) role is configured but this labby build does not include fleet support (built without the `nodes` feature)"
        );
    }
    Ok(role)
}
```

(If no `From<NodeRuntimeRole> for NodeRole` impl exists, match on `resolved.role`'s two variants explicitly — `Controller => <master variant>`, `Node => bail!(...)` — using the variant names from config.rs.)

(c) Node log store + NodeStore + EnrollmentStore construction, lines ~243-277. Wrap the whole block (from `let log_retention_days = ...` through the `EnrollmentStore::open(...)` binding) in a single gated block that yields the two stores:
```rust
    #[cfg(feature = "nodes")]
    let (node_store, enrollment_store) = {
        let log_retention_days = config
            .node
            .as_ref()
            .and_then(|n| n.log_retention_days)
            .unwrap_or(crate::node::log_store::DEFAULT_RETENTION_DAYS);
        let node_log_db_path = node_runtime.home_dir().join(".lab/node-logs.sqlite");
        let node_store = match crate::node::log_store::SqliteNodeLogStore::open(
            node_log_db_path.clone(),
            log_retention_days,
        )
        .await
        {
            Ok(log_store) => {
                tracing::info!(
                    path = %node_log_db_path.display(),
                    retention_days = log_retention_days,
                    "node log store opened"
                );
                Arc::new(NodeStore::with_log_store(log_store))
            }
            Err(err) => {
                tracing::warn!(
                    path = %node_log_db_path.display(),
                    error = %err,
                    "node log store unavailable; falling back to in-memory store"
                );
                Arc::new(NodeStore::default())
            }
        };
        let enrollment_store = Arc::new(
            EnrollmentStore::open(node_runtime.home_dir().join(".lab/node-enrollments.json"))
                .await
                .context("open node enrollment store")?,
        );
        (node_store, enrollment_store)
    };
```
(Preserve the existing code verbatim inside the block; only the wrapper is new. Check whether `NodeStore` is imported at the top of serve.rs and gate that import too.)

(d) State wiring, lines 518-519:
```rust
    #[cfg(feature = "nodes")]
    {
        state = state.with_node_store(Arc::clone(&node_store));
        state = state.with_enrollment_store(Arc::clone(&enrollment_store));
    }
```

(e) Gate `run_node_mode` (~line 1249) with `#[cfg(feature = "nodes")]` above the fn. `state.with_node_role(node_role)` at line 594 stays ungated — `node_role` exists on both paths.

- [ ] **Step 7: Gate `AppState` fleet fields** — `crates/labby/src/api/state.rs`:

Lines 12-13 (imports):
```rust
#[cfg(feature = "nodes")]
use crate::node::enrollment::store::EnrollmentStore;
#[cfg(feature = "nodes")]
use crate::node::store::NodeStore;
```

Lines 62-64 (fields — keep doc comments):
```rust
    /// Shared fleet state store for node runtime ingestion.
    #[cfg(feature = "nodes")]
    pub node_store: Option<Arc<NodeStore>>,
    /// Shared durable enrollment store for fleet websocket admission control.
    #[cfg(feature = "nodes")]
    pub enrollment_store: Option<Arc<EnrollmentStore>>,
```

Lines 146-147 (defaults):
```rust
            #[cfg(feature = "nodes")]
            node_store: None,
            #[cfg(feature = "nodes")]
            enrollment_store: None,
```

Lines 210-218 (builders):
```rust
    #[cfg(feature = "nodes")]
    #[must_use]
    pub fn with_node_store(mut self, store: Arc<NodeStore>) -> Self {
        self.node_store = Some(store);
        self
    }

    #[cfg(feature = "nodes")]
    #[must_use]
    pub fn with_enrollment_store(mut self, store: Arc<EnrollmentStore>) -> Self {
        self.enrollment_store = Some(store);
        self
    }
```

- [ ] **Step 8: Gate the HTTP routes** — `crates/labby/src/api/router.rs`:

Line 1066 — current: `let mut v1 = Router::new().nest("/nodes", super::nodes::routes(state.clone()));` Replace with:
```rust
    let mut v1 = Router::new();
    #[cfg(feature = "nodes")]
    {
        v1 = v1.nest("/nodes", super::nodes::routes(state.clone()));
    }
```

Lines 1539-1553 — the public enrollment/websocket mounts (`.nest("/v1/nodes", super::nodes::public_routes(...))`, `.nest("/v1/fleet", ...)`, the two `.route(... websocket_upgrade)` lines with their comments). Break these out of their chain into one gated block, same shape as above. Preserve the `// POST /v1/nodes/hello is self-registration...` and `// GET /v1/nodes/ws is outside bearer-auth middleware by design...` comments inside the gated block.

`api/router.rs:1311` (`crate::node::identity::resolve_local_hostname()`) stays untouched — `identity` is always-on.

- [ ] **Step 9: Gate the registry registration** — `crates/labby/src/registry.rs`, the `device` block (~lines 533-541):

```rust
    #[cfg(feature = "nodes")]
    reg.register(RegisteredService {
        name: "device",
        description: "Manage fleet device enrollments",
        category: "bootstrap",
        kind: RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: crate::mcp::services::nodes::ACTIONS,
        dispatch: dispatch_fn!(crate::mcp::services::nodes::dispatch),
    });
```

- [ ] **Step 10: Verify all-features unchanged**

```bash
cargo check --workspace --all-features
```

Expected: PASS, no new warnings.

- [ ] **Step 11: Gate the fleet integration tests** — add `#![cfg(feature = "nodes")]` as the first line of each:

```
crates/labby/tests/device_master_only.rs
crates/labby/tests/device_cli.rs
crates/labby/tests/device_api.rs
crates/labby/tests/device_runtime.rs
crates/labby/tests/nodes_cli.rs
crates/labby/tests/nodes_runtime.rs
crates/labby/tests/nodes_master_only.rs
crates/labby/tests/nodes_api.rs
```

- [ ] **Step 12: Slice check and fix stragglers**

```bash
RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets 2>&1 | grep -E '^error' | head -40
```

Expected: errors for any remaining unconditional `crate::node::` references (e.g. `crates/labby/tests/architecture_orchestrator.rs`, registry unit tests, serve helpers this plan missed). Apply the Task 1 Step 7 three-way rule to each. Re-run until zero errors.

- [ ] **Step 13: Verify probes**

```bash
cargo run -p labby --no-default-features --features gateway -- nodes --help
cargo run -p labby --no-default-features --features gateway -- logs --help
```

Expected: `nodes` → `unrecognized subcommand`; `logs --help` still works and lists `local` but NOT `search`/`forward`.

- [ ] **Step 14: Full test suite + commit**

```bash
just test
git add crates/labby/Cargo.toml crates/labby/src crates/labby/tests
git commit -m "feat: gate fleet/node subsystem behind nodes feature (bead <id>)"
```

---

### Task 4: CI matrix, feature-contract docs, final verification

**Files:**
- Modify: `.github/workflows/ci.yml:186-217` (slice matrix + comment)
- Modify: `crates/labby/Cargo.toml` (feature-contract comment block, ~lines 132-142)
- Modify: `CLAUDE.md` (root — the "Build assumption" and feature-slice sentences)
- Modify: `Justfile` (new convenience target)

**Interfaces:**
- Consumes: features `stash`, `acp`, `nodes` from Tasks 1-3.
- Produces: CI coverage for the new slices; updated authoritative docs.

- [ ] **Step 1: Extend the CI slice matrix** — `.github/workflows/ci.yml`. In the `feature-slices` job:

Change the matrix line:
```yaml
        slice: [gateway, marketplace, fs, deploy, acp_registry, acp, nodes, stash]
```

And delete the now-false sentence in the job comment: `` `stash` has no feature flag (always-on) so it is absent. `` Replace it with:
```yaml
    # Base capabilities (acp, nodes, stash) are feature-gated too so a
    # gateway-only build can exclude them; their slices are compile checks only.
```

- [ ] **Step 2: Update the feature-contract comment** — `crates/labby/Cargo.toml`, the block above `[features]`. Replace these two lines:

```
# - `doctor`, `setup`, `serve` base, `nodes`, `logs`, `stash`, and `acp` are base control-plane
#   or UI-coupled capabilities, not standalone slices in this feature table.
```

with:

```
# - `acp`, `nodes`, and `stash` are feature-gated base capabilities: not standalone
#   products, but excludable so a gateway-only build stays lean. `marketplace`
#   requires `acp` + `stash`. `doctor`, `setup`, `logs` (local), docs generation,
#   and the `serve` base remain always-on.
```

- [ ] **Step 3: Update root `CLAUDE.md`** — in the paragraph beginning "Standalone Cargo product slices currently include", append one sentence:

```
Base capabilities `acp`, `nodes`, and `stash` are feature-gated (all included in
`all`); a gateway-only build (`--no-default-features --features gateway`)
excludes them.
```

- [ ] **Step 4: Add a Justfile convenience target** — append to `Justfile`:

```make
# Compile check for the lean gateway-only slice (base services excluded).
check-gateway-slice:
    RUSTFLAGS="" cargo check -p labby --no-default-features --features gateway --all-targets
```

- [ ] **Step 5: Final full verification**

```bash
just lint
just test
just check-gateway-slice
RUSTFLAGS="" cargo check -p labby --no-default-features --features marketplace --all-targets
RUSTFLAGS="" cargo check -p labby --no-default-features --features "acp" --all-targets
RUSTFLAGS="" cargo check -p labby --no-default-features --features "nodes" --all-targets
RUSTFLAGS="" cargo check -p labby --no-default-features --features "stash" --all-targets
```

Expected: all PASS. Then re-run the Task 0 Step 3 probes — all three must now show the service absent:

```bash
cargo tree -p labby --no-default-features --features gateway | grep agent-client-protocol || echo "acp dep GONE"
for cmd in stash nodes; do
  cargo run -q -p labby --no-default-features --features gateway -- $cmd --help 2>/dev/null && echo "$cmd STILL PRESENT" || echo "$cmd gone"
done
cargo run -q -p labby --no-default-features --features gateway -- docs --help >/dev/null && echo "docs still present (correct — always-on)"
```

Expected: `acp dep GONE`, `stash gone`, `nodes gone`, `docs still present (correct — always-on)`.

- [ ] **Step 6: Commit + close the bead**

```bash
git add .github/workflows/ci.yml crates/labby/Cargo.toml CLAUDE.md Justfile
git commit -m "ci: cover base-capability feature slices; document gateway-only build (bead <id>)"
bd close <id>
git push
```

---

## Out of Scope (explicitly)

- **Web UI trimming** (`apps/gateway-admin` chat/marketplace/registry/nodes/design-system pages, ~27k TS LOC) — separate plan; the Rust gating stands alone.
- **labby-apis module gating** (`acp`, `stash`, `device_runtime` type modules, ~1k LOC of pure types) — cheap but low-value; the heavy deps are all in the binary crate.
- **labby-auth `upstream-oauth-rmcp` conditionalization and `tokio-tungstenite` optionalization** — flagged earlier as slimming opportunities, but they are gateway-KEEP code; untouched here.
- **Removing the orphan `node-runtime = []` feature** — noted in Task 3 Step 1; separate cleanup.
- **`doctor`, `setup`, `logs` (local), `oauth`, docs generation (`labby docs` / `src/docs/`)** — deliberately kept always-on. Doctor/setup/logs/oauth serve gateway operations, deployment, and web-UI auth; docs generation is registry-driven and documents the gateway's own action catalog (`docs/artifacts.rs` already handles feature slices with its own `#[cfg(feature = "gateway")]` branches).
