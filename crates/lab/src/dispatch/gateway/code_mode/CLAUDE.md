# dispatch/gateway/code_mode/ — Code Mode Runner

This directory owns the JavaScript execution sandbox. Read before editing.

---

## Runtime — Javy/QuickJS via subprocess stdio (NOT Wasmtime)

The live Code Mode runner is a **Javy/QuickJS subprocess** communicated with
over a framed stdio line protocol. There is NO Wasmtime/fuel path on any live
code path. `wasm_runner.rs` is dead code kept for reference only.

Execution limits (QuickJS side):
- **30-second wall-clock timeout** — enforced by `runner_drive.rs` via `tokio::time::timeout`.
- **64 MiB memory limit** — enforced by the Javy runtime at start-up.
- **Stack depth limit** — enforced by QuickJS natively.

The emitted `ToolError` kind when the wall-clock timer fires is `"timeout"`.
`code_mode_fuel_exhausted` is NOT emitted by this runner; see `docs/dev/ERRORS.md`.

---

## Warm-runner pool (Perf H1)

The runner **process** is pooled and long-lived; the **JS runtime is rebuilt
per execution**. This amortizes the dominant fixed cost (fork + process startup)
while guaranteeing JS-state isolation by construction.

- **Runner loop.** `runner.rs` reads a `Start` → builds a FRESH `javy::Runtime`
  + context → installs the bridge globals → runs to settle → emits `Done`/`Error`
  → resets per-execution state and **loops back to read the next `Start`**. The
  process never exits except on stdin EOF (parent dropped the runner).
- **Fresh runtime per `Start` is the contract.** Never reuse a `javy::Runtime`
  across executions — a brand-new runtime has no globals, no
  `__labPendingToolCalls`, and no captured data from a prior caller. This is
  where cross-caller leakage would live.
- **Per-execution resets** (`runner.rs`): the `next_seq` counter resets to 0, and
  a fresh per-execution cwd jail subdir is created (the previous one removed) so a
  pooled process never accumulates working-directory state across runs.
- **Parent pool** (`pool.rs`, `pool/runner_handle.rs`, `pool/config.rs`): a
  bounded set of long-lived runner handles, one execution per runner at a time
  (`size` runners ⇒ `size` concurrent executions). Slot ownership uses an explicit
  free-list so concurrent checkouts never collide.
- **Disposition.** `drive_runner` classifies each run: clean `Done` or a
  per-execution `Error` → the runner is parked and **released** back to the pool
  (it stayed alive with a fresh runtime); a crash (EOF/exit), timeout, or protocol
  fault → the runner is **evicted** (killed) and the slot respawns next checkout.
- **Recycle-after-K.** A pooled runner is killed+respawned after `recycle_after`
  executions (default 100) as cheap insurance against native-side fragmentation.
- **Backpressure.** When all pooled slots are busy, a checkout spawns a bounded
  ephemeral (overflow) runner (`max_overflow` cap) — never unbounded growth, never
  an indefinite queue. Overflow is logged at `action = "pool.overflow"`.
- **Config + kill switch** (env, read at manager construction):
  - `LAB_CODE_MODE_POOL_SIZE` — pooled runners (default 2, clamped to 16).
    **`0` disables pooling** → the drive layer falls back to spawn-per-execution
    (byte-identical to the pre-pool path).
  - `LAB_CODE_MODE_POOL_RECYCLE_AFTER` — executions before recycle (default 100).
  - `LAB_CODE_MODE_POOL_MAX_OVERFLOW` — max simultaneous ephemeral runners
    (default 8).
- **Security invariants persist for the pooled process** because they are set
  once at spawn: `env_clear()`, `process_group(0)`/Job Object, `kill_on_drop`,
  `prctl(PR_SET_DUMPABLE, 0)`. The 64 MiB heap / 30 s wall-clock / stack limits are
  enforced PER EXECUTION (heap+stack by the fresh runtime; wall-clock by the parent
  `drive_runner` deadline, which kills+evicts on expiry rather than reusing a
  runtime interrupted mid-execution).

When the broker has no `GatewayManager` (some tests / standalone paths) there is
no pool; it spawns a one-shot runner directly (the handle's `Drop` kills it).

---

## Parent ↔ Runner stdio Protocol

Messages are newline-delimited JSON sent over the child's stdin/stdout. A single
runner process serves **multiple** `Start`→`Done`/`Error` cycles over its
lifetime (warm pool); it parks on the next `Start` read after each and exits only
on stdin EOF.

**Parent → runner (requests):**

```jsonc
// Execute a snippet
{ "kind": "execute", "id": "<uuid>", "code": "<js source>" }

// Call an upstream tool (broker request from runner)
{ "kind": "tool_result", "id": "<uuid>", "result": <json> }

// Signal graceful shutdown
{ "kind": "shutdown" }
```

**Runner → parent (responses/events):**

```jsonc
// Snippet completed
{ "kind": "done", "id": "<uuid>", "result": { "state": "json"|"undefined"|"error", ... } }

// Runner wants to call an upstream tool
{ "kind": "call_tool", "id": "<uuid>", "tool": "<upstream::name>", "params": <json> }

// Runner wants to write an artifact
{ "kind": "write_artifact", "id": "<uuid>", "path": "<rel path>", "content": "<string>", ... }

// Execution error (JS exception or internal runner error)
{ "kind": "error", "id": "<uuid>", "message": "<string>" }

// Runner ready (sent once on startup before any requests)
{ "kind": "ready" }
```

Wire types are defined in `protocol.rs`. Do not add fields to the wire protocol
without updating both sides and `protocol.rs`.

---

## Sandbox Containment Invariants

The following invariants govern runner subprocess security. All rows below are
**implemented** on the live code path — there are no remaining "(planned)"
items.

| Invariant | Current state |
|-----------|--------------|
| No ambient network APIs | Enforced by QuickJS — no `fetch`, no `XMLHttpRequest`, no Node builtins |
| No dynamic import of host modules | Enforced by QuickJS module resolver |
| Process-group guard | Runner spawned with `process_group(0)` (Unix) / Job Object (Windows); `kill_on_drop(true)`; `killpg` reaches grandchildren |
| Env isolation | **Implemented.** Runner spawned with `env_clear()` (`runner_drive.rs:163`) — the child inherits NO labby env at all (not even an allowlist), so `LAB_*` secrets and every other ambient var are excluded. |
| `PR_SET_DUMPABLE` | **Implemented.** `runner.rs:22` calls `prctl(PR_SET_DUMPABLE, 0)` as the runner's first act on Linux, blocking `/proc/<pid>/environ` readback. Failure is non-fatal and warns via stderr (drained into the parent's response logs). |
| Per-run cwd isolation | Each runner has a long-lived spawn `TempDir`; the runner creates a FRESH per-execution jail subdir under it on every `Start` and removes the previous one (`runner.rs::reset_execution_jail`), so a pooled process never accumulates cwd state across runs. The `TempDir` is removed when the runner handle drops. |
| Artifact path containment | Enforced: `artifacts.rs` checks `canonicalize` + `starts_with(jail_root)`, rejects symlinks |
| Artifact size cap | Enforced: 8 MiB default (`LAB_CODE_MODE_ARTIFACT_MAX_MIB`) |
| Tool call budget | Not enforced. Code Mode is bounded by wall-clock timeout, sandbox memory/stack, output/log/artifact caps, and host-side tool policy. |

**Writing tests that assert on env isolation:** `env_clear()` has landed, so a
test asserting the runner child has a minimal/empty environment reflects real
behavior and need NOT be `#[ignore]` when it can inspect the child hermetically
(e.g. via the runner's own reporting). Do not re-introduce an `#[ignore]` "until
env_clear lands" comment — that state is in the past.

---

## File Responsibilities

| File | Purpose |
|------|---------|
| `runner.rs` | Runner subprocess entry point: the warm-pool loop (read `Start` → fresh runtime → run → `Done`/`Error` → reset + park), per-execution seq + cwd-jail reset, `PR_SET_DUMPABLE`. |
| `runner_drive.rs` | Parent-side driver: acquires a runner (pool lease or standalone), drives the protocol loop, classifies the outcome (`Completed`/`ExecutionError`/`RunnerUnhealthy`), wall-clock timeout, and finalizes the lease (release vs evict). |
| `pool.rs` | `RunnerPool` + `RunnerLease`: bounded warm pool, free-list slot ownership, recycle-after-K, bounded ephemeral overflow, kill switch. |
| `pool/runner_handle.rs` | `PooledRunner`: one long-lived runner process + its stdin/lines/stderr-drain, process-group/Job-Object guard, spawn (`env_clear`, `process_group`, `kill_on_drop`). |
| `pool/config.rs` | `PoolConfig`: env-driven pool size / recycle / overflow knobs and the kill switch. |
| `runner_io.rs` | Framed stdio line protocol with the child process. |
| `execute.rs` | `execute()` entry point: build context, inject preamble, call driver, return result. Also owns mcp-ui widget capture: `extract_ui_link` records an upstream result's `_meta.ui` (last-wins, into the per-run `CodeModeBroker::ui_capture` sink) before the envelope is unwrapped, and `apply_ui_opt_in` surfaces it on the final response while preserving `{ __ui: <result> }` unwrapping compatibility. |
| `search.rs` | `search()` entry point: project catalog, call driver, return filtered tool list. |
| `preamble.rs` | Injects the `callTool` bridge stub and catalog proxy into the JS environment. |
| `protocol.rs` | Wire types for all parent↔runner messages (serialization-stable). |
| `schema.rs` | JSON Schema helpers for tool description injection. |
| `normalize.rs` | Result normalization after runner returns. |
| `truncate.rs` | Output size limiting before returning to caller. |
| `trace.rs` | Execution span helpers (`tracing`). |
| `types.rs` | Shared Code Mode types: `CodeModeRequest`, `CodeModeResult`. |
| `ts_signatures.rs` | **Live** TypeScript signature / `.d.ts` generator called by `types.rs::CodeModeCatalogEntry::upstream_tool`. NOT legacy shims. |
| `types_legacy.rs` | Thin re-export alias for `ts_signatures`. Kept for backward compatibility — do not add new code here. |
| `util.rs` | Small utilities: JS source wrapping, ID generation. |
| `artifacts.rs` | Artifact write handler: path containment check, size cap, atomic write. |
| `catalog_cache.rs` | Per-run catalog snapshot cache to avoid repeated pool reads. |
| `wrapper.rs` | Wraps caller snippets in the async IIFE harness expected by the runner. |
| `wasm_runner.rs` | **DEAD CODE.** Wasmtime runner stub. Never call into this. |

---

## Rules

- Do not call `wasm_runner.rs` from any live code path.
- Do not add `code_mode_fuel_exhausted` to new match arms; the live kind is `"timeout"`.
- Do not expose host network APIs to the runner child.
- Keep `protocol.rs` as the single serialization-stable wire contract. The
  mcp-ui `{ __ui: <result> }` wrapper is a **host-side return convention**
  detected on the runner's returned `result` — it adds **no** new parent↔runner
  wire fields.
- Keep each file under 500 LOC; split following the existing pattern if a file grows.

---

## Related Docs

- `docs/dev/CODE_MODE.md` — surface documentation and examples (authoritative)
- `docs/dev/ERRORS.md` — `"timeout"` and artifact kinds
- Parent: `crates/lab/src/dispatch/gateway/CLAUDE.md` — trust model, runner env isolation (`env_clear`)
