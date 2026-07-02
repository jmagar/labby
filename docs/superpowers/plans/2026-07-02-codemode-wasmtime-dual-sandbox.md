# Code Mode Wasmtime Dual-Sandbox Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the native `javy::Runtime` execution inside the existing Code Mode runner subprocess with QuickJS-in-Wasm under Wasmtime, preserving the subprocess jail while adding Wasm linear-memory containment, epoch interruption, and fuel telemetry.

**Architecture:** Keep the existing parent-owned `CodeModeBroker` and `CodeModeHost` authority in the parent process. The child runner executes one QuickJS-in-Wasm instance per `Start` message and continues to emit the existing parent/runner stdio protocol messages for `callTool`, `writeArtifact`, and `codemode.run`; this avoids moving gateway credentials, upstream pools, snippets, or artifact store authority into the sandbox process. A shared Wasmtime `Engine`, compiled plugin `Module`, and epoch ticker are initialized once per runner subprocess, while every execution gets a fresh `Store`/`Instance`.

**Tech Stack:** Rust 2024, Tokio, Wasmtime, `javy-codegen`, `wasmtime-wizer`, existing Code Mode stdio protocol, existing `CodeModeBroker` / `CodeModeHost`, `cargo-nextest`, `cargo-deny`, `cargo-audit`.

## Global Constraints

- Base implementation from `origin/main` or a known-green integration branch. Do not stack this epic on `feat/codemode-semantic-search` until that branch passes `cargo check -p labby-gateway --all-features`; reviewers found source-level compile drift around `ToolsRender.fingerprint` and `CodeModeHost::semantic_rank`.
- Preserve the accepted rationale exactly: this is "defense-in-depth + graceful interruption", not "fixes a security hole" and not "the current design cannot cleanly kill a hung script".
- Preserve the outer subprocess boundary: `env_clear()`, process group / Job Object reaping, `kill_on_drop`, `PR_SET_DUMPABLE`, per-execution cwd jail, runner recycle-after-K, pool backpressure, and parent-side 30s wall-clock kill+evict remain intact.
- Per Code Mode execution, exactly one JS engine runs caller code: QuickJS compiled to Wasm and executed by Wasmtime inside the existing runner subprocess.
- Do not move `GatewayManager`, `UpstreamPool`, OAuth subjects, snippet storage, artifact persistence, or `CodeModeHost` implementations into the child process.
- The child process may use Wasmtime, but host calls still cross the existing parent/child stdio protocol. Do not implement `Linker::func_wrap_async` calls that directly call parent-side `CodeModeHost` from the child.
- Keep caller-facing error kind stable as `timeout` for Wasmtime fuel/epoch traps and OS wall-clock timeout. Add internal/log fields such as `trap_cause = "fuel_exhausted" | "epoch_interrupted" | "os_subprocess_timeout"` for operators.
- New public kind `code_mode_fuel_exhausted` is out of scope unless the error contract is deliberately changed in `docs/dev/ERRORS.md` before code lands.
- Exact dependency versions must be pinned after the research spike. Do not write broad `"46"` or `"46.x"` version requirements into `Cargo.toml`.
- No committed binary `.wasm` artifact in v1. Use build-from-source / build-time generation first; Git LFS and reproducible binary artifact flow are deferred unless measured build cost forces a different decision.
- Keep files under the crate's 500 LOC convention. Add new sibling files under `crates/labby-codemode/src/` rather than growing `runner.rs`, `runner_drive.rs`, `pool.rs`, or `execute.rs`.
- Every task must leave an independently testable state. Commit after each task.

---

## File Structure

- `crates/labby-codemode/CLAUDE.md`
  - Modify twice: first to remove stale implementation guidance after bead/spec cleanup, and finally to match the implemented runtime.
- `docs/dev/CODE_MODE.md`
  - Modify final public runtime docs.
- `docs/dev/ERRORS.md`
  - Modify timeout/trap-cause contract.
- `docs/dev/OBSERVABILITY.md`
  - Modify Code Mode trap/timeout logging fields.
- `crates/labby-codemode/Cargo.toml`
  - Add exact `wasmtime`, `javy-codegen`, and `wasmtime-wizer` dependencies after spike verification.
- `crates/labby-codemode/build.rs`
  - Create if the spike confirms the plugin module can be generated at build time.
- `crates/labby-codemode/src/wasm_plugin.rs`
  - Create: loads embedded plugin bytes, exposes cache-key/version metadata, validates a `wasmtime::Module`.
- `crates/labby-codemode/src/wasm_engine.rs`
  - Create: one `wasmtime::Engine`, epoch ticker handle, watchdog liveness state, and engine config per runner subprocess.
- `crates/labby-codemode/src/wasm_runner.rs`
  - Create: per-execution `Store`/`Instance`, JS snippet compile/link/instantiate/run, fuel/epoch/memory trap mapping, result/log extraction.
- `crates/labby-codemode/src/wasm_bridge.rs`
  - Create: Wasm guest memory readers/writers and bridge shims that emit existing `CodeModeRunnerOutput::{ToolCall, ArtifactWrite, SnippetResolve}` messages, then wait for existing `CodeModeRunnerInput` replies.
- `crates/labby-codemode/src/runner.rs`
  - Modify: initialize Wasmtime engine/plugin/ticker once before the `Start` loop; replace per-execution native `javy::Runtime` execution with `wasm_runner`.
- `crates/labby-codemode/src/runner_drive.rs`
  - Modify: classify reusable fuel/epoch traps as `ExecutionError`, classify dead watchdog / protocol faults as `RunnerUnhealthy`, preserve OS timeout kill+evict.
- `crates/labby-codemode/src/protocol.rs`
  - Prefer no changes. If a trap-cause field is unavoidable, add serde-defaulted fields only.
- `crates/labby-codemode/src/pool/runner_handle.rs`
  - Modify stale memory comment and add any runner-start health signal handling if needed.
- `crates/labby/tests/code_mode_runner.rs`
  - Modify/add parity, trap, reuse, host-call, and memory-bound tests.
- `deny.toml`
  - Modify only if `cargo deny check` proves the new dependency tree requires license/advisory/allowlist updates.

---

## Engineering Review Summary

### Architecture

Strengths:
- The corrected rationale and replace-not-parallel topology are sound.
- The outer subprocess jail remains the authoritative containment boundary.
- Engine/plugin/ticker per subprocess plus Store/Instance per execution preserves warm-pool economics and state isolation.

Critical concern:
- The prior bead text says Wasmtime linker imports call parent-side `CodeModeHost` directly while Wasmtime runs in the child subprocess. That is impossible without moving host authority into the child. This plan resolves it by keeping stdio IPC as the host-call transport.

### Simplicity

Over-engineering risks:
- A "fallback to today's native QuickJS path" kill switch would keep two engines and two bridge implementations alive. This plan rejects that as v1 scope.
- On-disk plugin cache, committed plugin artifact, LFS, and reproducible binary diffing are deferred until measured build cost demands them.

### Security

Missing protections to keep blocking:
- Exact dependency pins, `cargo deny check`, and `cargo audit` must run before production code lands.
- Wasm guest memory reads must enforce OOB, cap-before-copy, and UTF-8 validation before host allocation or artifact validation.
- Trap causes must be logged internally without creating new caller-facing error kinds.

### Performance

Bottlenecks to test:
- Engine/ticker must never be constructed per execution.
- Trap paths must prove subprocess reuse after fuel/epoch interruption.
- Benchmark output must split compile+link+instantiate, execution/fuel overhead, and total end-to-end latency through the pool.

### Failure Modes

| Codepath | Failure Mode | Rescued? | Test? | User Sees? | Logged? |
|---|---|---:|---:|---|---:|
| plugin build/load | pinned versions drift or plugin bytes stale | Y, hard build/start failure | Y | Code Mode unavailable/build fails | Y |
| engine startup | Engine/ticker created per execution | N | Y | latency regression | Y |
| watchdog | epoch ticker dies silently | partial, OS timeout remains | Y | slower timeout | Y |
| fuel trap | legitimate JSON-heavy snippet traps | partial, user can tune/retry | Y | `timeout` | Y |
| epoch trap | trap poisons subprocess | Y if runner reusable test passes | Y | possible latency spike | Y |
| host-call bridge | child tries direct parent host call | N | Y | every `callTool` fails | Y |
| Wasm memory read | bad ptr/len or non-UTF8 panics/allocates | N unless bounded read implemented | Y | structured rejection if fixed | Y |
| OS wall timeout | malicious/hung code ignores in-process traps | Y, kill+evict outer ring | existing + Y | `timeout` | Y |

Rows without both rescue and tests are plan blockers; Task 1 rewrites the beads to make these explicit before implementation.

### NOT In Scope

- Retiring the subprocess pool: the pool remains the outer containment and performance boundary.
- Native QuickJS fallback mode: keeping the old engine as an emergency rollback creates a dual-engine maintenance fork.
- Committed binary plugin artifact / Git LFS flow: defer unless build-time measurements make it necessary.
- Store/Instance reuse optimization: defer until benchmarks prove per-execution instantiation is material.
- New caller-facing `code_mode_fuel_exhausted` kind: keep `timeout` externally and use internal trap cause fields.
- Query/cache optimizations unrelated to Wasmtime: handle separately after the semantic-search branch is green.

---

## Task 1: Reconcile Issue And Bead Specs Before Code

**Files:**
- Modify: bead descriptions via `bd update`, not source code
- Modify: GitHub issue #168 body/comments only if needed after bead updates
- Test: `bd show lab-crav6`, `bd list --parent lab-crav6 --json`, `bd swarm validate lab-crav6`

**Interfaces:**
- Produces: an implementation-ready `lab-crav6` bead chain with no contradiction between description and comments.
- Produces: locked decisions:
  - Wasmtime runs inside the child subprocess.
  - Host calls use existing stdio IPC; no direct child-to-parent `CodeModeHost` calls.
  - Kill switch disables Wasmtime limits only, or is removed; it does not promise native QuickJS fallback.
  - Exact pins and security gates are blocking in early beads.

- [ ] **Step 1: Snapshot current epic and child text**

```bash
bd show lab-crav6 > /tmp/lab-crav6.before.txt
bd list --parent lab-crav6 --json > /tmp/lab-crav6.children.before.json
for id in lab-crav6.1 lab-crav6.2 lab-crav6.3 lab-crav6.4 lab-crav6.5 lab-crav6.6; do
  bd show "$id" > "/tmp/${id}.before.txt"
done
```

Expected: files exist in `/tmp` and include the stale text that still mentions "alongside", legacy `wizer`, broad "46.x", and native fallback.

- [ ] **Step 2: Update `lab-crav6.6` to stdio-backed Wasm bridge**

Replace the bead's direct `CodeModeHost`/`Linker::func_wrap_async` requirement with:

```markdown
The Wasmtime runner lives in the child subprocess. Parent-owned host authority stays in the parent process. The Wasm bridge must expose guest functions that serialize the same `CodeModeRunnerOutput::{ToolCall, ArtifactWrite, SnippetResolve}` messages the native runner emits today, then block/poll for the matching existing `CodeModeRunnerInput` reply. Do not call `CodeModeHost` directly from the child.
```

Run:

```bash
bd update lab-crav6.6 -d "$(python - <<'PY'
from pathlib import Path
text = Path('/tmp/lab-crav6.6.before.txt').read_text()
text = text.replace('Uses `Config::async_support(true)` + `Linker::func_wrap_async` — do not\\n  reimplement a second seq-numbered pending-operation map to shim around\\n  synchronous linker imports (this was the alternative architecture-strategist\\n  explicitly recommended against).', 'Uses the existing parent/runner stdio protocol for host authority. The child-side Wasm bridge emits the same `CodeModeRunnerOutput` messages and waits for the same `CodeModeRunnerInput` replies as the native runner. Do not move `CodeModeHost`/gateway authority into the child process.')
text += '\\n\\n## Engineering Review Reconciliation - 2026-07-02\\n\\nLocked correction: Wasmtime runs in the child subprocess, but host calls remain parent-owned over the existing stdio protocol. Direct async linker imports into parent-side `CodeModeHost` are out of scope and would violate the subprocess boundary. Required tests: callTool parity, writeArtifact parity, codemode.run parity, OOB ptr/len, cap-before-copy, non-UTF8 rejection, and guest-sourced path traversal.\\n'
print(text)
PY
)"
```

Expected: `bd show lab-crav6.6` no longer requires direct async `CodeModeHost` calls from the child.

- [ ] **Step 3: Update `lab-crav6.4` kill-switch wording**

Set the v1 switch contract to one of these explicit outcomes:

```markdown
V1 kill switch: `LAB_CODE_MODE_WASM_LIMITS=0` disables fuel/epoch enforcement while keeping the Wasmtime execution path. It does not restore the native `javy::Runtime` path. Native QuickJS fallback is a separate rollback strategy, not a runtime mode.
```

Run:

```bash
bd comments add lab-crav6.4 "DECISION: Kill switch wording corrected. V1 switch disables Wasmtime fuel/epoch enforcement only; it does not promise byte-identical native QuickJS fallback because the locked topology replaces native javy::Runtime rather than keeping two engines alive."
```

Expected: implementers cannot read the bead as requiring a dual-engine fallback.

- [ ] **Step 4: Update dependency wording in `lab-crav6.1`, `.2`, `.5`**

Add comments:

```bash
bd comments add lab-crav6.1 "DECISION: Spike starts from current cargo search pins javy-codegen =4.0.0, wasmtime =46.0.1, and wasmtime-wizer =46.0.1, then must prove API compatibility from current source/docs before Cargo.toml changes land. Broad 46.x family references are not implementation guidance."
bd comments add lab-crav6.2 "DECISION: Build-from-source build.rs/include_bytes flow is the v1 default. Startup/on-disk cache, committed plugin.wasm, Git LFS, and reproducible binary diffing are deferred unless measured build cost requires them."
bd comments add lab-crav6.5 "DECISION: Docs closeout must depend on lab-crav6.6 explicitly and must describe stdio-backed host calls, not direct CodeModeHost calls from the child subprocess."
```

Expected: comments are attached to each bead.

- [ ] **Step 5: Validate dependency chain**

```bash
bd swarm validate lab-crav6
```

Expected: valid sequential chain. If `lab-crav6.5` does not depend on `lab-crav6.6`, add the missing dependency:

```bash
bd dep add lab-crav6.5 lab-crav6.6
bd swarm validate lab-crav6
```

- [ ] **Step 6: Commit if bead metadata is repo-backed**

```bash
git status --short
git add .beads 2>/dev/null || true
git commit -m "docs: reconcile codemode wasmtime epic review findings"
```

Expected: commit succeeds if bead storage changed in-repo; if beads are not stored in the worktree, there is nothing to commit.

---

## Task 2: Research Spike And Version Lock

**Files:**
- Create: `docs/dev/CODE_MODE_WASMTIME_SPIKE.md`
- Modify: `crates/labby-codemode/Cargo.toml` only in a throwaway verification branch or after exact pins are known
- Test: `cargo check -p labby-codemode --all-features`, `cargo deny check`, `cargo audit`

**Interfaces:**
- Produces: exact dependency pins and API signatures for later tasks.
- Produces: a durable doc table with source links, version pins, invocation model, and local build proof.

- [ ] **Step 1: Verify current crate metadata from primary sources**

Use current docs/source, not memory:

```bash
cargo search javy-codegen --limit 5
cargo search wasmtime --limit 5
cargo search wasmtime-wizer --limit 5
```

Expected: identify current crate versions available on July 2, 2026.

- [ ] **Step 2: Create spike doc**

Create `docs/dev/CODE_MODE_WASMTIME_SPIKE.md` with this structure:

```markdown
# Code Mode Wasmtime Spike

## Version Pins

| Crate | Exact version | Why compatible | Source |
|---|---:|---|---|
| javy-codegen | =4.0.0 | Confirm dynamic-linking API against docs/source | cargo search on 2026-07-02, then docs.rs/GitHub verification |
| wasmtime | =46.0.1 | Confirm fuel/epoch/memory APIs against docs/source | cargo search on 2026-07-02, then docs.rs/GitHub verification |
| wasmtime-wizer | =46.0.1 | Confirm build-time Wizer invocation model against docs/source | cargo search on 2026-07-02, then docs.rs/GitHub verification |

## Dynamic Linking API

List exact module paths, function names, and signatures verified from docs/source.

## Wizer Invocation

State whether build.rs uses library API or CLI, and include the exact command/API call.

## Wasmtime Limits

List exact Config/Store/Engine methods for fuel, epoch, and memory limiting.

## Local Build Proof

Commands run, output summary, and open questions.
```

- [ ] **Step 3: Run a scratch compile proof**

Use a throwaway branch or temporary directory. Do not leave scratch production code in `crates/labby-codemode/src`.

```bash
mkdir -p /tmp/labby-wasmtime-spike
cd /tmp/labby-wasmtime-spike
cargo init --bin
cargo add javy-codegen@=4.0.0 wasmtime@=46.0.1 wasmtime-wizer@=46.0.1
cargo check
```

Expected: PASS with the exact pins recorded in the spike doc. If it fails, update the pins or stop the epic.

2026-07-02 result: the compile proof only passes when `deterministic-wasi-ctx`
is pinned to `=4.0.0`, because `javy-codegen 4.0.0` is internally on
Wasmtime/WASI 42 while newer `deterministic-wasi-ctx 4.0.4` pulls Wasmtime/WASI
46. The passing compile proof is not enough to proceed because the security gate
below fails on the Wasmtime/WASI 42 dependency tree.

- [ ] **Step 4: Run security gates after adding dependencies in the real repo**

```bash
cargo check -p labby-codemode --all-features
cargo deny check
cargo audit
```

Expected: PASS. If `cargo audit` is unavailable, install or document the exact reason it cannot run before proceeding.

2026-07-02 result: BLOCKED. `cargo check -p labby-codemode --all-features`
passes with the spike dependencies, but `cargo deny check` and `cargo audit`
flag RustSec advisories introduced by `javy-codegen 4.0.0`'s transitive
Wasmtime/WASI 42 dependencies. Do not proceed to Task 3 by adding advisory
ignores. The safe next step is one of:

- wait for a Javy release whose codegen path uses a patched Wasmtime/WASI line;
- move JS-to-Wasm compilation outside the production workspace dependency tree
  with a separately pinned and reviewed tool artifact;
- change the architecture away from Javy-to-Wasm.

- [ ] **Step 5: Commit**

```bash
git add docs/dev/CODE_MODE_WASMTIME_SPIKE.md docs/superpowers/plans/2026-07-02-codemode-wasmtime-dual-sandbox.md
git commit -m "docs(codemode): capture wasmtime spike blocker"
```

Expected: a docs-only commit. `crates/labby-codemode/Cargo.toml`, `Cargo.lock`,
and `deny.toml` should remain unchanged unless the security gate above passes.

---

## Task 3: Build And Load The Shared QuickJS Wasm Plugin

**Files:**
- Create: `crates/labby-codemode/build.rs`
- Create: `crates/labby-codemode/src/wasm_plugin.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/CLAUDE.md`
- Test: inline tests in `wasm_plugin.rs`

**Interfaces:**
- Produces: `wasm_plugin::plugin_bytes() -> &'static [u8]`
- Produces: `wasm_plugin::plugin_cache_key() -> &'static str`
- Produces: `wasm_plugin::compile_plugin_module(engine: &wasmtime::Engine) -> Result<wasmtime::Module, ToolError>`

- [ ] **Step 1: Write failing loader test**

Add to `crates/labby-codemode/src/wasm_plugin.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_bytes_are_non_empty_and_keyed() {
        assert!(!plugin_bytes().is_empty());
        assert!(plugin_cache_key().contains("javy-codegen"));
        assert!(plugin_cache_key().contains("wasmtime"));
        assert!(plugin_cache_key().contains("wasmtime-wizer"));
    }

    #[test]
    fn plugin_module_compiles_under_wasmtime() {
        let engine = wasmtime::Engine::default();
        let module = compile_plugin_module(&engine).expect("plugin module compiles");
        assert!(module.imports().count() > 0 || module.exports().count() > 0);
    }
}
```

Run:

```bash
cargo test -p labby-codemode wasm_plugin -- --nocapture
```

Expected: FAIL because the module does not exist yet.

- [ ] **Step 2: Implement `build.rs` and embedded bytes**

Use the exact invocation model from `docs/dev/CODE_MODE_WASMTIME_SPIKE.md`. The output file must live under `OUT_DIR`, and `wasm_plugin.rs` must use `include_bytes!(concat!(env!("OUT_DIR"), "/code_mode_plugin.wasm"))`.

If the spike proves Wizer is CLI-only, `build.rs` uses `std::process::Command` with explicit argv, no shell.

- [ ] **Step 3: Implement `wasm_plugin.rs`**

```rust
use crate::error::ToolError;

const PLUGIN_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/code_mode_plugin.wasm"));
const PLUGIN_CACHE_KEY: &str = concat!(
    "javy-codegen=", env!("LABBY_JAVY_CODEGEN_VERSION"),
    ";wasmtime=", env!("LABBY_WASMTIME_VERSION"),
    ";wasmtime-wizer=", env!("LABBY_WASMTIME_WIZER_VERSION")
);

#[must_use]
pub(crate) fn plugin_bytes() -> &'static [u8] {
    PLUGIN_BYTES
}

#[must_use]
pub(crate) fn plugin_cache_key() -> &'static str {
    PLUGIN_CACHE_KEY
}

pub(crate) fn compile_plugin_module(engine: &wasmtime::Engine) -> Result<wasmtime::Module, ToolError> {
    wasmtime::Module::from_binary(engine, plugin_bytes()).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to compile Code Mode Wasm plugin: {err}"),
    })
}
```

If the exact env constants cannot be exported from `build.rs`, replace them with `env!("CARGO_PKG_VERSION")` plus the literal dependency versions from the spike, for example `javy-codegen=4.0.0;wasmtime=46.0.1;wasmtime-wizer=46.0.1`.

- [ ] **Step 4: Register module and docs**

Add to `crates/labby-codemode/src/lib.rs`:

```rust
mod wasm_plugin;
```

Update `crates/labby-codemode/CLAUDE.md` File Responsibilities with `wasm_plugin.rs` and `build.rs`.

- [ ] **Step 5: Verify**

```bash
cargo test -p labby-codemode wasm_plugin -- --nocapture
cargo check -p labby-codemode --all-features
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/build.rs crates/labby-codemode/src/wasm_plugin.rs crates/labby-codemode/src/lib.rs crates/labby-codemode/CLAUDE.md
git commit -m "feat(codemode): build shared QuickJS Wasm plugin"
```

---

## Task 4: Engine, Epoch Ticker, And Watchdog Liveness

**Files:**
- Create: `crates/labby-codemode/src/wasm_engine.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: inline tests in `wasm_engine.rs`

**Interfaces:**
- Produces: `CodeModeWasmRuntime::new() -> Result<Self, ToolError>`
- Produces: `CodeModeWasmRuntime::engine(&self) -> &wasmtime::Engine`
- Produces: `CodeModeWasmRuntime::plugin_module(&self) -> &wasmtime::Module`
- Produces: `CodeModeWasmRuntime::watchdog_alive(&self) -> bool`

- [ ] **Step 1: Write failing singleton/liveness tests**

In `wasm_engine.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_constructs_engine_and_plugin_once() {
        let runtime = CodeModeWasmRuntime::new().expect("runtime");
        let engine_a = runtime.engine() as *const _;
        let engine_b = runtime.engine() as *const _;
        assert_eq!(engine_a, engine_b);
        assert!(runtime.watchdog_alive());
    }
}
```

Run:

```bash
cargo test -p labby-codemode wasm_engine -- --nocapture
```

Expected: FAIL because `wasm_engine.rs` does not exist.

- [ ] **Step 2: Implement engine config**

`CodeModeWasmRuntime::new()` configures:

```rust
let mut config = wasmtime::Config::new();
config.consume_fuel(true);
config.epoch_interruption(true);
config.async_support(false);
```

Do not enable `async_support(true)` in v1 because host calls remain stdio-backed, not direct async linker imports.

- [ ] **Step 3: Spawn one epoch ticker per runner subprocess**

Use a thread owned by `CodeModeWasmRuntime`:

```rust
let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
let alive_for_thread = alive.clone();
let engine_for_thread = engine.clone();
let ticker = std::thread::Builder::new()
    .name("labby-code-mode-epoch".to_string())
    .spawn(move || {
        while alive_for_thread.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            engine_for_thread.increment_epoch();
        }
    })
    .map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to spawn Code Mode epoch ticker: {err}"),
    })?;
```

Store the `JoinHandle` and stop flag. On drop, set `alive=false`; no blocking join in a panic path.

- [ ] **Step 4: Initialize once before runner loop**

In `runner.rs`, construct `CodeModeWasmRuntime::new()` after `PR_SET_DUMPABLE` and before `RUNNER_STATE.with(...)`.

Expected code shape:

```rust
let wasm_runtime = match super::wasm_engine::CodeModeWasmRuntime::new() {
    Ok(runtime) => runtime,
    Err(err) => {
        eprintln!("ERROR: failed to initialize Code Mode Wasm runtime: {err}");
        return ExitCode::FAILURE;
    }
};
```

Pass `&wasm_runtime` into the per-execution runner function in later tasks.

- [ ] **Step 5: Verify**

```bash
cargo test -p labby-codemode wasm_engine -- --nocapture
cargo check -p labby-codemode --all-features
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/wasm_engine.rs crates/labby-codemode/src/lib.rs crates/labby-codemode/src/runner.rs
git commit -m "feat(codemode): initialize Wasmtime engine per runner"
```

---

## Task 5: Wasm Bridge Over Existing Stdio Protocol

**Files:**
- Create: `crates/labby-codemode/src/wasm_bridge.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: inline tests in `wasm_bridge.rs`, plus `crates/labby/tests/code_mode_runner.rs`

**Interfaces:**
- Produces: bounded guest-memory helpers:
  - `read_guest_utf8(memory: &wasmtime::Memory, store: &mut wasmtime::Store<CodeModeGuestState>, ptr: u32, len: u32, max_len: usize) -> Result<String, CodeModeRunnerError>`
  - `read_guest_bytes_bounded(memory: &wasmtime::Memory, store: &mut wasmtime::Store<CodeModeGuestState>, ptr: u32, len: u32, max_len: usize) -> Result<Vec<u8>, CodeModeRunnerError>`
- Produces: bridge functions that emit existing `CodeModeRunnerOutput` messages and read matching existing `CodeModeRunnerInput` replies.

- [ ] **Step 1: Write failing guest-memory tests**

Add tests:

```rust
#[test]
fn guest_memory_read_rejects_oob_range() {
    // Construct a one-page Wasmtime memory, pass ptr near end with len past end,
    // assert kind == "invalid_param" and no panic.
}

#[test]
fn guest_memory_read_caps_before_copy() {
    // Pass len = max + 1 and assert rejection occurs before allocating len bytes.
}

#[test]
fn guest_memory_read_rejects_non_utf8() {
    // Write [0xff, 0xfe] into memory and assert structured invalid_param.
}
```

Run:

```bash
cargo test -p labby-codemode wasm_bridge -- --nocapture
```

Expected: FAIL until helpers exist.

- [ ] **Step 2: Implement bounded reads**

Implementation rule:

```rust
if len as usize > max_len {
    return Err(CodeModeRunnerError::invalid_param("guest argument exceeds limit"));
}
let end = (ptr as usize)
    .checked_add(len as usize)
    .ok_or_else(|| CodeModeRunnerError::invalid_param("guest pointer overflow"))?;
let data = memory.data(store);
if end > data.len() {
    return Err(CodeModeRunnerError::invalid_param("guest pointer out of bounds"));
}
let slice = &data[ptr as usize..end];
```

Only after these checks may code allocate/copy or call `std::str::from_utf8`.

- [ ] **Step 3: Implement bridge output parity**

For `callTool`, `writeArtifact`, and `codemode.run`, emit exactly the same `CodeModeRunnerOutput` variants currently emitted by `runner.rs`:

```rust
CodeModeRunnerOutput::ToolCall { seq, id, params }
CodeModeRunnerOutput::ArtifactWrite { seq, path, content, content_type }
CodeModeRunnerOutput::SnippetResolve { seq, name, input }
```

Then wait for the matching `CodeModeRunnerInput::{ToolResult, ToolError, SnippetResolved}` reply using the same `seq`.

- [ ] **Step 4: Add direct runner parity tests**

In `crates/labby/tests/code_mode_runner.rs`, adapt existing tests rather than duplicating a new harness:

- `code_mode_runner_tool_error_produces_json_encoded_error`
- `code_mode_runner_tool_error_does_not_abort_fan_out`
- `code_mode_runner_resolves_and_runs_snippet`
- artifact path traversal / artifact size cap tests

Expected: each test passes against the Wasmtime-backed runner with unchanged JSON protocol shape.

- [ ] **Step 5: Verify**

```bash
cargo test -p labby code_mode_runner_tool_error_produces_json_encoded_error -- --nocapture
cargo test -p labby code_mode_runner_resolves_and_runs_snippet -- --nocapture
cargo test -p labby-codemode wasm_bridge -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/wasm_bridge.rs crates/labby-codemode/src/lib.rs crates/labby-codemode/src/runner.rs crates/labby/tests/code_mode_runner.rs
git commit -m "feat(codemode): bridge Wasm host calls over runner protocol"
```

---

## Task 6: Wasmtime Execution, Fuel, Epoch, And Memory Limits

**Files:**
- Create: `crates/labby-codemode/src/wasm_runner.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: `crates/labby/tests/code_mode_runner.rs`

**Interfaces:**
- Produces: `wasm_runner::run_wasm_execution(runtime: &CodeModeWasmRuntime, start: CodeModeRunnerInput::Start) -> Result<CodeModeRunnerOutput, CodeModeRunnerError>`
- Produces: trap mapping to `kind = "timeout"` with internal trap cause.

- [ ] **Step 1: Add failing trap tests**

Add to `crates/labby/tests/code_mode_runner.rs`:

```rust
#[test]
fn wasm_runner_fuel_trap_returns_timeout_and_reuses_process() {
    let (mut child, mut stdin, mut stdout) = spawn_pooled_runner();
    let pid = child.id();

    writeln!(stdin, "{}", json!({
        "type": "start",
        "code": "async () => { while (true) {} }"
    })).expect("write runaway start");
    let err = read_protocol_line(&mut stdout);
    assert_eq!(err["type"], "error");
    assert_eq!(err["kind"], "timeout");

    let done = run_once(&mut stdin, &mut stdout, "async () => 42");
    assert_eq!(done["result"]["value"], json!(42));
    assert_eq!(child.id(), pid, "fuel trap should not evict the subprocess");
}
```

Add a second epoch-focused test if the spike identifies a low-fuel/long-wall snippet that Wasmtime/Javy actually supports.

Run:

```bash
cargo test -p labby wasm_runner_fuel_trap_returns_timeout_and_reuses_process -- --nocapture
```

Expected: FAIL until Wasmtime execution exists.

- [ ] **Step 2: Implement per-execution Store/Instance**

For each `Start`, create a fresh `Store`, set:

```rust
store.set_fuel(fuel_budget)?;
store.set_epoch_deadline(epoch_deadline_ticks);
```

The exact method names must come from `docs/dev/CODE_MODE_WASMTIME_SPIKE.md`; adjust to the pinned Wasmtime API without changing behavior.

- [ ] **Step 3: Apply memory limit**

Set a 64 MiB guest memory bound via the pinned Wasmtime API (`Store` resource limiter or config setting). The implementation must trap/reject allocations above the limit and surface a structured error.

- [ ] **Step 4: Map traps**

Map:

- fuel exhaustion -> `CodeModeRunnerOutput::Error { kind: "timeout", message: "Code Mode execution timed out" }` plus log field `trap_cause="fuel_exhausted"`.
- epoch interruption -> same caller kind plus `trap_cause="epoch_interrupted"`.
- memory limit -> `kind="invalid_param"` when caller-created allocation exceeds limits, unless Wasmtime only exposes it as trap; document exact behavior in `docs/dev/ERRORS.md`.

- [ ] **Step 5: Verify no native `javy::Runtime` construction on hot path**

Add a test or trace assertion that the Wasmtime path does not construct `javy::Runtime` per execution. If test instrumentation is awkward, remove the native runtime construction code in the same commit and rely on compile-time absence plus direct runner tests.

- [ ] **Step 6: Verify**

```bash
cargo test -p labby code_mode_runner -- --nocapture
cargo test -p labby-codemode -- --nocapture
cargo check -p labby-codemode --all-features
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/labby-codemode/src/wasm_runner.rs crates/labby-codemode/src/lib.rs crates/labby-codemode/src/runner.rs crates/labby/tests/code_mode_runner.rs
git commit -m "feat(codemode): execute snippets under Wasmtime"
```

---

## Task 7: Parent Driver Classification And Pool Reuse

**Files:**
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Modify: `crates/labby-codemode/src/pool/runner_handle.rs`
- Test: existing and new tests in `crates/labby/tests/code_mode_runner.rs`

**Interfaces:**
- Consumes: runner emits `Error { kind: "timeout", message }` for reusable fuel/epoch traps.
- Produces: fuel/epoch execution errors release the runner; OS timeout/protocol/watchdog failure evicts it.

- [ ] **Step 1: Add explicit pooled reuse test**

Add test:

```rust
#[test]
fn pooled_runner_survives_wasm_timeout_trap_but_os_timeout_evicts() {
    // First assertion: fuel/epoch trap returns Error and same PID serves next Start.
    // Second assertion: parent-side timeout still kills/evicts via existing driver test path.
}
```

Run:

```bash
cargo test -p labby pooled_runner_survives_wasm_timeout_trap_but_os_timeout_evicts -- --nocapture
```

Expected: FAIL until classification is adjusted.

- [ ] **Step 2: Update `DriveOutcome` handling**

Keep existing rules:

- `Done` -> `Completed` -> `lease.release()`
- runner-emitted `Error` -> `ExecutionError` -> `lease.release()`
- EOF, invalid protocol JSON, parent wall-clock timeout, dead watchdog -> `RunnerUnhealthy` -> `lease.evict()`

Add comments explaining why Wasmtime traps are runner-emitted errors and are therefore reusable.

- [ ] **Step 3: Update stale memory comment**

In `pool/runner_handle.rs`, replace "~24 at defaults" with "10 at defaults (`pool_size=2`, `max_overflow=8`)".

- [ ] **Step 4: Verify**

```bash
cargo test -p labby code_mode_runner -- --nocapture
cargo test -p labby-codemode -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/labby-codemode/src/runner_drive.rs crates/labby-codemode/src/pool/runner_handle.rs crates/labby/tests/code_mode_runner.rs
git commit -m "test(codemode): prove Wasmtime traps preserve runner reuse"
```

---

## Task 8: Benchmarks And Fuel Budget Corpus

**Files:**
- Create: `crates/labby-codemode/benches/wasmtime_runtime.rs` or `crates/labby-codemode/tests/wasmtime_bench.rs`
- Create: `docs/dev/CODE_MODE_WASMTIME_BENCHMARKS.md`
- Modify: `crates/labby-codemode/src/wasm_runner.rs`
- Test: benchmark command plus regression tests

**Interfaces:**
- Produces: documented p50/p95/p99/max table for at least 20 snippets.
- Produces: fuel budget constant derived as 10-50x empirical p99.

- [ ] **Step 1: Create benchmark corpus**

Add at least 20 snippets covering:

- existing Code Mode runner integration snippets
- JSON parse/map/filter/reduce over 1k, 5k, and 10k arrays
- parallel `Promise.all` callTool fan-out
- `codemode.run` snippet nesting
- artifact write path
- search/describe helper usage

- [ ] **Step 2: Implement non-enforcing fuel measurement**

Measurement pass:

```rust
store.set_fuel(u64::MAX)?;
// run snippet
let remaining = store.get_fuel()?;
let consumed = u64::MAX - remaining;
```

Use the exact pinned Wasmtime API names from the spike.

- [ ] **Step 3: Record results**

`docs/dev/CODE_MODE_WASMTIME_BENCHMARKS.md` must include:

```markdown
| Snippet | min | p50 | p95 | p99 | max | notes |
|---|---:|---:|---:|---:|---:|---|
```

Also record:

- compile+link+instantiate p50/p95/p99
- execution/fuel overhead p50/p95/p99
- total end-to-end through pool p50/p95/p99
- chosen fuel budget and multiplier

- [ ] **Step 4: Add regression tests**

Add:

- legitimate JSON-heavy snippet does not trap
- infinite loop traps within bounded wall-clock time
- p99 overhead threshold is under the agreed limit, or the test logs threshold failure and blocks the bead

- [ ] **Step 5: Verify**

```bash
cargo test -p labby-codemode wasmtime_bench -- --nocapture
cargo test -p labby code_mode_runner -- --nocapture
```

Expected: PASS and docs table populated with real numbers.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/benches crates/labby-codemode/tests docs/dev/CODE_MODE_WASMTIME_BENCHMARKS.md crates/labby-codemode/src/wasm_runner.rs
git commit -m "test(codemode): benchmark Wasmtime fuel budget"
```

---

## Task 9: Documentation, Error Contract, And Observability

**Files:**
- Modify: `crates/labby-codemode/CLAUDE.md`
- Modify: `crates/labby-gateway/src/gateway/CLAUDE.md`
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `docs/dev/ERRORS.md`
- Modify: `docs/dev/OBSERVABILITY.md`
- Modify: `deny.toml` only if required
- Test: docs grep, `just deny`, `just lint`

**Interfaces:**
- Produces: truthful docs for dual sandbox runtime.
- Produces: stable caller error contract with internal trap-cause logging.

- [ ] **Step 1: Remove stale "NOT Wasmtime" language**

Run:

```bash
rg -n "NOT Wasmtime|Do not reintroduce Wasmtime|dead Wasmtime|code_mode_fuel_exhausted|native QuickJS|Javy/QuickJS via subprocess stdio" crates docs
```

Expected: find stale references in `crates/labby-codemode/CLAUDE.md`, `crates/labby-gateway/src/gateway/CLAUDE.md`, and `docs/dev/ERRORS.md`.

- [ ] **Step 2: Update crate docs**

`crates/labby-codemode/CLAUDE.md` must state:

- runtime is QuickJS-in-Wasm via Wasmtime inside existing subprocess
- parent/runner stdio protocol remains host-call transport
- `Engine`/plugin/ticker once per subprocess
- `Store`/`Instance` fresh per execution
- fuel/epoch/memory rows in containment table
- `LAB_CODE_MODE_WASM_LIMITS=0` if implemented, with honest semantics
- no native QuickJS fallback unless explicitly kept

- [ ] **Step 3: Update public docs**

`docs/dev/CODE_MODE.md` must state:

- caller JS API is unchanged
- `callTool`, `writeArtifact`, `codemode.run`, `codemode.search`, `codemode.describe` behavior is unchanged
- runtime changed for containment/interruption only
- outer subprocess kill+evict remains final safety net

- [ ] **Step 4: Update error docs**

`docs/dev/ERRORS.md` must state:

- caller sees `timeout` for fuel exhaustion, epoch interruption, and OS wall-clock timeout
- logs include `trap_cause`
- `code_mode_fuel_exhausted` remains not emitted, or is removed from reserved language if the spec changed

- [ ] **Step 5: Update observability docs**

`docs/dev/OBSERVABILITY.md` must list Code Mode fields:

```text
surface
service = "code_mode"
action = "codemode"
elapsed_ms
kind
trap_cause
runner_pid
runner_reused
wasm_plugin_cache_key
```

- [ ] **Step 6: Verify docs and gates**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo deny check
cargo audit
rg -n "NOT Wasmtime|Do not reintroduce Wasmtime|dead Wasmtime" crates docs && exit 1 || true
```

Expected: all pass; final grep returns no stale prohibition language.

- [ ] **Step 7: Commit**

```bash
git add crates/labby-codemode/CLAUDE.md crates/labby-gateway/src/gateway/CLAUDE.md docs/dev/CODE_MODE.md docs/dev/ERRORS.md docs/dev/OBSERVABILITY.md deny.toml Cargo.lock
git commit -m "docs(codemode): document Wasmtime dual sandbox"
```

---

## Task 10: Full Verification And PR Prep

**Files:**
- No new source files expected
- Modify: `docs/sessions/2026-07-02-codemode-wasmtime-dual-sandbox.md` only if the execution workflow requires a session note

**Interfaces:**
- Produces: implementation-ready PR with evidence.

- [ ] **Step 1: Full gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo deny check
cargo audit
git diff --check
```

Expected: PASS.

- [ ] **Step 2: Manual smoke**

Run a simple Code Mode execution through the binary:

```bash
cargo run -p labby --all-features -- internal code-mode-runner
```

Then in a separate scripted runner test, send:

```json
{"type":"start","code":"async () => ({ ok: true, value: 2 + 2 })","proxy":""}
```

Expected:

```json
{"type":"done","result":{"state":"json","value":{"ok":true,"value":4}},"logs":[]}
```

- [ ] **Step 3: Update issue/bead comments**

```bash
bd comments add lab-crav6 "LEARNED: Wasmtime dual-sandbox implementation complete. Host authority stayed parent-owned over stdio IPC; Engine/plugin/ticker are per runner subprocess; Store/Instance are per execution; fuel/epoch traps reuse the runner and surface caller-facing timeout with internal trap_cause."
gh issue comment 168 --repo jmagar/labby --body "Implementation notes: host authority remains parent-owned over stdio IPC; Wasmtime runs inside the existing runner subprocess; fuel/epoch traps preserve runner reuse; docs and error contract updated. Verification: cargo fmt, clippy, nextest, deny, audit."
```

- [ ] **Step 4: Commit final notes**

```bash
git status --short
git add .
git commit -m "chore(codemode): record Wasmtime verification"
```

Expected: clean tree or only intentionally ignored artifacts.

---

## Self-Review

- Spec coverage: The plan covers all six child beads plus the new review blocker: reconcile stale bead bodies, exact dependency spike, plugin build, engine/ticker, bridge, execution limits, pool reuse, benchmarks, docs, errors, observability, and final verification.
- Placeholder scan: Clean. Version pins begin at the cargo-search-confirmed `javy-codegen=4.0.0`, `wasmtime=46.0.1`, and `wasmtime-wizer=46.0.1`; Task 2 still requires source-doc verification before those pins are committed.
- Type consistency: Produced interfaces are named consistently: `CodeModeWasmRuntime`, `compile_plugin_module`, `run_wasm_execution`, `wasm_bridge` bounded memory readers, caller-facing `timeout`, internal `trap_cause`.
