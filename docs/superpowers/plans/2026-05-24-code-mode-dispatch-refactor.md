# Code Mode Dispatch Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Code Mode execution, search, and schema business logic out of `mcp/server.rs` into gateway dispatch, then expose a native `labby gateway code` CLI adapter.

**Architecture:** Add a small dispatch-owned `CodeModeBroker` that uses explicit caller and surface policy types. MCP and CLI become adapters: they construct caller/surface inputs, call the broker, and serialize the existing response structs. The sandbox runner child stays in `dispatch/gateway/code_mode.rs`; parent orchestration moves there too with cancellation-safe process cleanup.

**Tech Stack:** Rust 2024, Tokio process/io, `serde_json`, existing `ToolRegistry`, `GatewayManager`, `ActionSpec`, `ToolError`, Clap CLI, existing nextest/fmt/clippy workflow.

---

## File Structure

- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
  - Owns `CodeModeBroker`, `CodeModeCaller`, `CodeModeSurface`, execution parent loop, search/schema helpers, comparator, confirmed downstream param handling, runner process guard helpers, and unit tests.
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
  - Keeps manager-owned config/search/upstream resolution APIs; add narrow helpers only when the broker needs policy-safe upstream search/schema or coalesced health updates.
- Modify: `crates/lab/src/mcp/server.rs`
  - Removes Code Mode business helpers. Keeps tool registration, auth adapter checks, MCP request parsing, catalog notification adapter, and `CallToolResult` envelope conversion.
- Modify: `crates/lab/src/cli/gateway.rs`
  - Adds `gateway code search|schema|exec` as a thin CLI adapter over `CodeModeBroker`.
- Modify: `crates/lab/tests/code_mode_runner.rs`
  - Keeps child-runner protocol tests; add cleanup tests only if they need binary process coverage.
- Modify: `docs/services/GATEWAY.md`
  - Documents dispatch-owned Code Mode and CLI examples.
- Modify: `crates/lab/src/mcp/CLAUDE.md`
  - Removes stale MCP-owned Code Mode exception.
- Possibly modify: `docs/dev/DISPATCH.md`
  - Add a short Code Mode ownership note if needed.

## Task 1: Dispatch Broker Execution Core

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/mcp/server.rs`

- [ ] **Step 1: Add caller and surface policy types**

Add these public dispatch types near the existing Code Mode response structs:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeCaller {
    TrustedLocal,
    Scoped { scopes: Vec<String>, subject: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    Mcp,
    Cli,
}

impl CodeModeCaller {
    pub fn can_read(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes.iter().any(|scope| {
                matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin")
            }),
        }
    }

    pub fn can_execute(&self) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } => scopes.iter().any(|scope| {
                matches!(scope.as_str(), "lab" | "lab:admin")
            }),
        }
    }

    pub fn can_execute_action(&self, destructive: bool) -> bool {
        match self {
            Self::TrustedLocal => true,
            Self::Scoped { scopes, .. } if destructive => {
                scopes.iter().any(|scope| scope == "lab:admin")
            }
            Self::Scoped { scopes, .. } => scopes.iter().any(|scope| {
                matches!(scope.as_str(), "lab" | "lab:admin")
            }),
        }
    }

    pub fn subject(&self) -> Option<&str> {
        match self {
            Self::TrustedLocal => None,
            Self::Scoped { subject, .. } => subject.as_deref(),
        }
    }
}
```

Run: `cargo test -p labby --lib dispatch::gateway::code_mode --all-features`

Expected: PASS or compile errors only from unused types until later steps.

- [ ] **Step 2: Add broker skeleton and config validation test**

Add `CodeModeBroker` with references to the registry and optional manager:

```rust
pub struct CodeModeBroker<'a> {
    registry: &'a crate::registry::ToolRegistry,
    gateway_manager: Option<&'a crate::dispatch::gateway::manager::GatewayManager>,
}

impl<'a> CodeModeBroker<'a> {
    pub fn new(
        registry: &'a crate::registry::ToolRegistry,
        gateway_manager: Option<&'a crate::dispatch::gateway::manager::GatewayManager>,
    ) -> Self {
        Self { registry, gateway_manager }
    }
}
```

Add a test in `code_mode.rs`:

```rust
#[tokio::test]
async fn execute_rejects_disabled_code_mode() {
    let registry = crate::registry::ToolRegistry::new();
    let broker = CodeModeBroker::new(&registry, None);

    let err = broker
        .execute(
            "await callTool(\"lab::gateway.gateway.servers\", {})",
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            crate::config::CodeModeConfig {
                enabled: false,
                timeout_ms: 30_000,
                max_tool_calls: 12,
            },
        )
        .await
        .expect_err("disabled Code Mode should fail");

    match err {
        ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "code_mode_disabled"),
        other => panic!("expected code_mode_disabled, got {other:?}"),
    }
}
```

Run: `cargo test -p labby --lib dispatch::gateway::code_mode::tests::execute_rejects_disabled_code_mode --all-features`

Expected: FAIL because `execute` does not exist.

- [ ] **Step 3: Implement minimal `execute` validation**

Implement:

```rust
pub async fn execute(
    &self,
    code: &str,
    caller: CodeModeCaller,
    surface: CodeModeSurface,
    config: crate::config::CodeModeConfig,
) -> Result<CodeModeExecutionResponse, ToolError> {
    if !config.enabled {
        return Err(ToolError::Sdk {
            sdk_kind: "code_mode_disabled".to_string(),
            message: "Code Mode execution is disabled".to_string(),
        });
    }
    if !caller.can_execute() {
        return Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "code_execute requires one of scopes: lab, lab:admin".to_string(),
        });
    }
    self.execute_sandboxed(
        code,
        config.max_tool_calls.max(1),
        std::time::Duration::from_millis(config.timeout_ms.max(1)),
        caller,
        surface,
    )
    .await
}
```

Temporarily implement `execute_sandboxed` to return `invalid_param` so the disabled test can pass:

```rust
async fn execute_sandboxed(
    &self,
    _code: &str,
    _max_tool_calls: usize,
    _timeout: std::time::Duration,
    _caller: CodeModeCaller,
    _surface: CodeModeSurface,
) -> Result<CodeModeExecutionResponse, ToolError> {
    Err(ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: "Code Mode snippet must call callTool(id, params) at least once".to_string(),
    })
}
```

Run: `cargo test -p labby --lib dispatch::gateway::code_mode::tests::execute_rejects_disabled_code_mode --all-features`

Expected: PASS.

- [ ] **Step 4: Move runner parent loop into dispatch**

Move these helpers from `mcp/server.rs` into `code_mode.rs` and adapt them onto `CodeModeBroker`:

```text
execute_code_mode_sandboxed
code_mode_call_tool_id_before_deadline
code_mode_call_tool_id
code_mode_call_lab_action
code_mode_call_upstream_tool
write_runner_input
terminate_code_mode_runner
```

Required changes while moving:

```rust
child.kill_on_drop(true);
```

Set runner stderr to `Stdio::null()` unless a bounded stderr drain is added:

```rust
.stderr(Stdio::null())
```

Replace `auth: Option<&AuthContext>` and `subject` with `CodeModeCaller`. Replace MCP visibility checks with `CodeModeSurface` policy helpers in dispatch.

Run: `cargo test -p labby --lib dispatch::gateway::code_mode --all-features`

Expected: compile errors identify the exact MCP-only dependencies still to remove.

- [ ] **Step 5: Strip broker control params before downstream dispatch**

Add helper:

```rust
fn strip_code_mode_control_params(mut params: Value) -> Value {
    if let Value::Object(map) = &mut params {
        map.remove("confirm");
    }
    params
}
```

In the Lab action path:

```rust
let confirmed = params.get("confirm").and_then(Value::as_bool) == Some(true);
if is_destructive && !confirmed {
    return Err(ToolError::Sdk {
        sdk_kind: "confirmation_required".to_string(),
        message: format!("action `{action_name}` is destructive - pass {{\"confirm\":true}} in params"),
    });
}
let params = strip_code_mode_control_params(params);
```

Add a test using a fake registered service that records params and assert `confirm` is absent.

Run: `cargo test -p labby --lib dispatch::gateway::code_mode::tests::execute_strips_confirm_before_dispatch --all-features`

Expected: PASS.

- [ ] **Step 6: Wire MCP `code_execute` to broker**

In `mcp/server.rs`, replace the direct helper call with:

```rust
let broker = CodeModeBroker::new(&self.registry, self.gateway_manager.as_deref());
let caller = CodeModeCaller::Scoped {
    scopes: auth.scopes.clone(),
    subject: subject.map(str::to_string),
};
let response = broker
    .execute(&code, caller, CodeModeSurface::Mcp, manager.code_mode_config().await)
    .await;
```

Keep MCP-specific auth extraction and `CallToolResult` conversion in `mcp/server.rs`.

Run: `rg "execute_code_mode_sandboxed|code_mode_call_tool_id|code_mode_call_lab_action|code_mode_call_upstream_tool" crates/lab/src/mcp/server.rs`

Expected: no matches, except comments being actively deleted.

## Task 2: Dispatch Search and Schema

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
- Modify: `crates/lab/src/mcp/server.rs`

- [ ] **Step 1: Move comparator and built-in candidate search**

Move `compare_code_mode_search_candidates` and `search_builtin_code_mode_candidates` into `CodeModeBroker`.

Broker method signature:

```rust
pub async fn search(
    &self,
    query: &str,
    top_k: usize,
    caller: CodeModeCaller,
    surface: CodeModeSurface,
) -> Result<Vec<CodeModeSearchCandidate>, ToolError>
```

Add test:

```rust
#[tokio::test]
async fn search_expands_builtin_matches_to_action_candidates() {
    let registry = completion_test_registry();
    let broker = CodeModeBroker::new(&registry, None);

    let results = broker
        .search("movie.search", 10, CodeModeCaller::TrustedLocal, CodeModeSurface::Cli)
        .await
        .unwrap();

    assert_eq!(results.first().map(|r| r.id.as_str()), Some("lab::radarr.movie.search"));
    assert!(results.iter().all(|r| r.schema_available));
}
```

Run: `cargo test -p labby --lib dispatch::gateway::code_mode::tests::search_expands_builtin_matches_to_action_candidates --all-features`

Expected: PASS after the helper moves.

- [ ] **Step 2: Move schema response construction**

Move these helpers into `CodeModeBroker`:

```text
code_mode_schema_for_lab_action
code_mode_schema_for_upstream_tool
code_mode_schema_response
```

Broker method signature:

```rust
pub async fn schema(
    &self,
    id: &str,
    caller: CodeModeCaller,
    surface: CodeModeSurface,
) -> Result<CodeModeSchemaResponse, ToolError>
```

Add tests for:

```rust
#[tokio::test]
async fn schema_returns_lab_action_bindings() {
    let registry = completion_test_registry();
    let broker = CodeModeBroker::new(&registry, None);

    let schema = broker
        .schema("lab::radarr.movie.search", CodeModeCaller::TrustedLocal, CodeModeSurface::Cli)
        .await
        .unwrap();

    assert_eq!(schema.kind, "lab_action");
    assert!(schema.bindings.typescript.contains("callTool(\"lab::radarr.movie.search\""));
}
```

Run: `cargo test -p labby --lib dispatch::gateway::code_mode::tests::schema_returns_lab_action_bindings --all-features`

Expected: PASS.

- [ ] **Step 3: Preserve upstream search and fallback behavior**

Have broker `search` call `GatewayManager::search_tools` when a manager exists, then convert results to `CodeModeSearchCandidate::upstream_tool`.

Do not allow upstream semantic failures or index warming to hide built-in candidates. The built-in path must return candidates even if upstream search returns `index_warming`, `timeout`, or semantic fallback warnings.

Add test:

```rust
#[tokio::test]
async fn search_keeps_builtin_candidates_when_upstream_search_is_unavailable() {
    let registry = completion_test_registry();
    let broker = CodeModeBroker::new(&registry, None);

    let results = broker
        .search("movie.search", 10, CodeModeCaller::TrustedLocal, CodeModeSurface::Mcp)
        .await
        .unwrap();

    assert!(results.iter().any(|r| r.id == "lab::radarr.movie.search"));
}
```

Run: `cargo test -p labby --lib dispatch::gateway::code_mode::tests::search_keeps_builtin_candidates_when_upstream_search_is_unavailable --all-features`

Expected: PASS.

- [ ] **Step 4: Wire MCP `code_search` and `code_schema` to broker**

Replace MCP-owned search/schema logic with broker calls. MCP still performs MCP surface auth checks before the broker call.

Run:

```bash
rg "search_builtin_code_mode_candidates|code_mode_schema_response|code_mode_schema_for_lab_action|code_mode_schema_for_upstream_tool|compare_code_mode_search_candidates" crates/lab/src/mcp/server.rs
```

Expected: no matches.

Run: `cargo nextest run -p labby --all-features code_mode`

Expected: focused Code Mode tests pass.

## Task 3: Native CLI Adapter

**Files:**
- Modify: `crates/lab/src/cli/gateway.rs`
- Possibly modify: `crates/lab/src/output.rs`

- [ ] **Step 1: Add Clap command shape tests first**

Extend the existing `gateway_cli_parses_commands` test with:

```rust
assert!(Cli::try_parse_from(["lab", "gateway", "code", "search", "movie.search"]).is_ok());
assert!(Cli::try_parse_from(["lab", "gateway", "code", "schema", "lab::radarr.movie.search"]).is_ok());
assert!(Cli::try_parse_from([
    "lab",
    "gateway",
    "code",
    "exec",
    "--code",
    "await callTool(\"lab::gateway.gateway.servers\", {})",
]).is_ok());
assert!(Cli::try_parse_from([
    "lab",
    "gateway",
    "code",
    "exec",
    "--file",
    "snippet.js",
]).is_ok());
```

Run: `cargo test -p labby --lib cli::gateway::tests::gateway_cli_parses_commands --all-features`

Expected: FAIL before command definitions exist.

- [ ] **Step 2: Add CLI args**

Add:

```rust
Code(GatewayCodeArgs),
```

and:

```rust
#[derive(Debug, Args)]
pub struct GatewayCodeArgs {
    #[command(subcommand)]
    pub command: GatewayCodeCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayCodeCommand {
    Search { query: String, #[arg(long, default_value_t = 10)] top_k: usize },
    Schema { id: String },
    Exec {
        #[arg(long, conflicts_with = "file")]
        code: Option<String>,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
}
```

Run the parser test again.

Expected: PASS.

- [ ] **Step 3: Implement CLI routing directly to broker**

Before the normal `GatewayCommand` action-string match, handle `GatewayCommand::Code(args)`:

```rust
GatewayCommand::Code(args) => {
    return run_gateway_code(manager, args, format, config).await;
}
```

Implement `run_gateway_code` in the same file. It must construct:

```rust
let broker = CodeModeBroker::new(manager.registry(), Some(manager.as_ref()));
let caller = CodeModeCaller::TrustedLocal;
let surface = CodeModeSurface::Cli;
```

If `GatewayManager` does not expose the registry, add a small accessor returning `&ToolRegistry` or pass the CLI `registry` from `build_manager`.

For `exec --file`, read no more than 20 KiB:

```rust
const CODE_MODE_CLI_MAX_SOURCE_BYTES: u64 = 20 * 1024;
let metadata = std::fs::metadata(path)?;
if metadata.len() > CODE_MODE_CLI_MAX_SOURCE_BYTES {
    anyhow::bail!("Code Mode source file exceeds 20480 bytes");
}
```

Return broker results with existing JSON output conventions.

Run: `cargo check --manifest-path crates/lab/Cargo.toml --all-features`

Expected: PASS.

- [ ] **Step 4: Add CLI smoke tests**

Add a focused test that disabled config returns `code_mode_disabled` by calling the command handler with `CodeModeConfig { enabled: false, .. }` if a unit hook exists. If not, cover it with the broker test from Task 1 and a CLI parser test here.

Run: `cargo nextest run -p labby --all-features code_mode`

Expected: PASS.

- [ ] **Step 5: Manual CLI smoke**

Build:

```bash
cargo build --workspace --all-features
```

Run:

```bash
target/debug/labby gateway code --help
target/debug/labby gateway code search "gateway servers" --json
target/debug/labby gateway code schema "lab::gateway.gateway.servers" --json
target/debug/labby gateway code exec --code 'await callTool("lab::gateway.gateway.servers", {})' --json
```

Expected:
- help lists `search`, `schema`, and `exec`
- search returns Code Mode IDs
- schema includes TypeScript bindings
- exec returns `{"calls":[...]}` or a structured `code_mode_disabled` error if local config has Code Mode off

## Task 4: Docs, Greps, and Verification

**Files:**
- Modify: `docs/services/GATEWAY.md`
- Modify: `crates/lab/src/mcp/CLAUDE.md`
- Possibly modify: `docs/dev/DISPATCH.md`

- [ ] **Step 1: Update Code Mode docs**

In `docs/services/GATEWAY.md`, state:

```markdown
Code Mode is implemented in the gateway dispatch layer and exposed through two adapters:
MCP meta-tools (`code_search`, `code_schema`, `code_execute`) and the native CLI
(`labby gateway code search|schema|exec`). Both use the same schema-first IDs and
the same sandboxed parent-brokered execution path.
```

Add CLI examples using only read-only actions:

```bash
labby gateway code search "gateway servers" --json
labby gateway code schema "lab::gateway.gateway.servers" --json
labby gateway code exec --code 'await callTool("lab::gateway.gateway.servers", {})' --json
```

- [ ] **Step 2: Update MCP local guide**

In `crates/lab/src/mcp/CLAUDE.md`, replace the stale MCP-owned Code Mode exception with:

```markdown
Code Mode is shared gateway dispatch business logic. MCP owns only tool registration,
scope extraction, and MCP envelope conversion for `code_search`, `code_schema`, and
`code_execute`.
```

- [ ] **Step 3: Run dependency-direction greps**

Run:

```bash
rg "crate::mcp|rmcp::model::CallToolResult|crate::api::oauth::AuthContext|CallToolResult" crates/lab/src/dispatch/gateway/code_mode.rs
rg "execute_code_mode_sandboxed|code_mode_call_tool_id|code_mode_call_lab_action|code_mode_call_upstream_tool|search_builtin_code_mode_candidates|code_mode_schema_response|compare_code_mode_search_candidates" crates/lab/src/mcp/server.rs
```

Expected:
- first command has no matches
- second command has no helper implementation matches

- [ ] **Step 4: Run local negative controls**

Run CLI negative control:

```bash
target/debug/labby gateway code schema "bad-id" --json
```

Expected: structured `invalid_code_mode_id`.

Run suppressed/hidden upstream negative control if a suppressed fixture exists in tests. Otherwise rely on the dispatch unit test added in Task 2 and note that no local suppressed live upstream is configured.

- [ ] **Step 5: Full verification**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features -- -D warnings
cargo nextest run --workspace --all-features
```

Expected: all pass. If `apps/gateway-admin/out` is missing and causes unrelated `include_dir!` failures, build the frontend artifact using the repo's existing command and rerun.

- [ ] **Step 6: Post-deploy live smoke**

After the updated binary is deployed, run read-only mcporter calls against the existing bearer-configured `lab-prod` server:

```bash
mcporter call lab-prod.code_search '{"query":"gateway servers","top_k":5}'
mcporter call lab-prod.code_schema '{"id":"lab::gateway.gateway.servers"}'
mcporter call lab-prod.code_execute '{"code":"await callTool(\"lab::gateway.gateway.servers\", {})"}'
mcporter call lab-prod.code_schema '{"id":"not-a-code-mode-id"}'
```

Expected:
- search/schema/execute succeed for read-only Lab action
- invalid ID fails with `invalid_code_mode_id`
- no bearer token or raw secret-bearing tool output is copied into docs/session notes

## Self-Review

- Spec coverage: the four reviewed beads map to Tasks 1-4. Execution, search/schema, CLI, docs, local verification, and live smoke are covered.
- Placeholder scan: no `TBD`, unconstrained edge-case language, or unspecified test commands remain.
- Type consistency: `CodeModeBroker`, `CodeModeCaller`, and `CodeModeSurface` are introduced in Task 1 and reused by Tasks 2 and 3.
