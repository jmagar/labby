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

## Parent ↔ Runner stdio Protocol

Messages are newline-delimited JSON sent over the child's stdin/stdout.

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

The following invariants govern runner subprocess security. The **intended**
hardened state is listed; items marked "(planned)" are not yet implemented.

| Invariant | Current state | Intended state |
|-----------|--------------|----------------|
| No ambient network APIs | Enforced by QuickJS — no `fetch`, no `XMLHttpRequest`, no Node builtins | Same |
| No dynamic import of host modules | Enforced by QuickJS module resolver | Same |
| Process-group guard | `spawn_guard.rs` sets PGID; `killpg` on drop | Same |
| Env isolation | **Runner inherits labby env** | `env_clear` + explicit allowlist (SEC work item) |
| `PR_SET_DUMPABLE` | Not set | Set on Linux for runner child (SEC work item) |
| Artifact path containment | Enforced: `artifacts.rs` checks `canonicalize` + `starts_with(jail_root)`, rejects symlinks | Same |
| Artifact size cap | Enforced: 8 MiB default (`LAB_CODE_MODE_ARTIFACT_MAX_MIB`) | Same |
| Tool call budget | Enforced: `max_tool_calls` counter, emits `tool_call_limit_exceeded` | Same |

**Writing tests that assert on env isolation:** until `env_clear` lands, tests
that assert the runner child has a minimal environment MUST be marked `#[ignore]`
and include a comment explaining the intended vs. actual state.

---

## File Responsibilities

| File | Purpose |
|------|---------|
| `runner.rs` | Subprocess lifecycle: `spawn_runner()`, read/write loop, graceful shutdown. |
| `runner_drive.rs` | Higher-level driver: timeout wrapping, retry policy, `CodeModeHistory` tracking. |
| `runner_io.rs` | Framed stdio line protocol with the child process. |
| `execute.rs` | `execute()` entry point: build context, inject preamble, call driver, return result. |
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
- Keep `protocol.rs` as the single serialization-stable wire contract.
- Keep each file under 500 LOC; split following the existing pattern if a file grows.

---

## Related Docs

- `docs/dev/CODE_MODE.md` — surface documentation and examples (authoritative)
- `docs/dev/ERRORS.md` — `"timeout"`, `"tool_call_limit_exceeded"`, artifact kinds
- Parent: `crates/lab/src/dispatch/gateway/CLAUDE.md` — trust model, env inheritance
