# Code Mode Crate Extraction Implementation Plan

> For agentic workers: use `superpowers:executing-plans` (or
> `superpowers:subagent-driven-development`) to implement task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the Code Mode JavaScript execution kernel — the Javy/QuickJS
runner, its parent-side broker/driver, the result-shaping helpers, and the
snippet *engine* — into a standalone `lab-codemode` crate that exposes a generic
`CodeModeHost` trait. Labby's `gateway` becomes the first implementor of that
trait; any future server (e.g. a media/Servarr server that scripts REST APIs via
Code Mode) becomes a second implementor without depending on the gateway runtime.

**Relationship to the gateway extraction plan:** This plan is a dependency of,
and a refinement to, the Standalone Gateway Extraction plan. It lands **after**
that plan's Task 1 (`lab-runtime`, which owns the moved `ToolError`) and
**replaces** the part of that plan's Task 5 that moves Code Mode into
`lab-gateway/src/code_mode/**`. Instead, `lab-gateway` depends on `lab-codemode`
and implements `CodeModeHost`.

**Tech Stack:** Rust 2024, Cargo workspace resolver 3, Tokio, serde,
serde_json, thiserror, javy 7.x. No axum/rmcp/clap/anyhow in the new crate.

---

## Why this is its own crate (decision record)

Code Mode is a JS sandbox that is useless without a *tool-providing host*. Today
that host is `GatewayManager` (upstream MCP tools via the pool), and Code Mode is
explicitly upstream-MCP-only (`code_mode.rs:133` punts native Lab actions). A
second, non-gateway host is anticipated (Code Mode over native REST service
dispatch). If Code Mode were folded into `lab-gateway`, that second host would
have to pull in the entire gateway/upstream-pool/OAuth runtime just to run JS.
Therefore Code Mode is extracted as its own crate sitting below `lab-gateway`,
peer to `lab-runtime`/`lab-auth`, with hosts injected via a trait.

---

## Global Constraints

- `lab-codemode` must not depend on `axum`, `clap`, `rmcp`, `utoipa`, `wasmtime`,
  or any Labby product registry/router/MCP-server modules. Allowed runtime deps:
  `tokio`, `serde`, `serde_json`, `thiserror`, `javy`, `nix` (Unix hardening),
  `tempfile`, `tracing`, and `lab-runtime` (for `ToolError` and shared contracts).
- **Drop Wasmtime.** `wasm_runner.rs` is `#[cfg(test)]`-only dead code and the
  `wasmtime` dependency exists solely to compile it. Do not carry it into the new
  crate. If a reference copy is wanted, keep it out of the default build entirely.
- **Preserve runner hardening exactly:** `env_clear()` on spawn, process-group
  (`process_group(0)`) / Windows Job Object guard, `kill_on_drop`, Linux
  `prctl(PR_SET_DUMPABLE, 0)` as the runner's first act, per-execution temp cwd
  jail (create-fresh + remove-previous), 64 MiB heap, stack limit, and the 30s
  parent wall-clock deadline. The emitted `ToolError` kind on wall-clock expiry
  stays `"timeout"`. Do not reintroduce `code_mode_fuel_exhausted`.
- **Preserve the synchronous runner entrypoint shape:**
  `run_code_mode_runner_stdio() -> std::process::ExitCode`. Do not invent an async
  runner API as part of the move. The consuming binary wires this into its own
  hidden `internal code-mode-runner` subcommand.
- **Spawn must be host-configurable.** The warm pool re-execs `current_exe()` with
  `["internal", "code-mode-runner"]`. The crate must let the host supply the
  re-invocation (program + args), defaulting to `current_exe()` + the canonical
  args, so a different binary can host the runner. (`pool/runner_handle.rs`
  already supports a configurable program path for tests — generalize it.)
- **Keep `protocol.rs` the single serialization-stable wire contract.** The
  parent↔runner channel is always stdio (`ToolCall`, `SnippetResolve`,
  `tool_result`, etc.), independent of any host transport. The mcp-ui
  `{ __ui: <result> }` convention is host-side return detection — add no new wire
  fields.
- **`search`/`describe` are in-sandbox JS over the injected catalog** (no IPC, no
  wire message). Do not model them as host-trait methods. The host only provides
  the catalog they read and resolves `callTool`.
- **Snippet engine moves; snippet surface stays.** The store/types/resolution move
  into `lab-codemode`. The `snippets` MCP tool registration, HTTP route, CLI
  command, and `ACTIONS` catalog stay in Labby as a thin adapter over the crate.
  This also breaks today's bidirectional `code_mode ↔ snippets` dependency.
- **Do not duplicate `ToolError`.** Use the one moved to `lab-runtime` by the
  gateway plan's Task 1. `lab-codemode` depends on `lab-runtime` for it.
- **No client/transport vocabulary in the crate.** The crate must not export or
  reference `UpstreamTool`, `Upstream*`, `Gateway*`, `pool`, `manager`, or any
  API-client name. Tools are an opaque `id: &str` + JSON params; descriptors are
  the neutral `ToolDescriptor` (renamed from `CodeModeCatalogEntry`); the filter
  is the neutral `ToolScope` (renamed from `CodeModeCapabilityFilter`). Each host
  converts its own tool representation into `ToolDescriptor` inside its
  `CodeModeHost` impl, so the kernel never learns what its tool source is. A
  grep of `lab-codemode/src` for `upstream`/`gateway` must come back empty.
- Prefer manifest + `cargo tree -e features` checks over source-string scans for
  dependency gates. Replace any long-running smoke commands with bounded
  spawn/probe/teardown tests.

---

## `CodeModeHost` trait (tool-source-neutral)

The trait is the **only** thing the kernel knows about its tool source. Its
vocabulary is deliberately neutral — **no `upstream`, `gateway`, `pool`,
`manager`, or any client name appears in the crate or the trait.** A tool is
just an opaque string `id` + JSON params; a tool descriptor is a neutral
`ToolDescriptor` (the renamed, gateway-free successor of today's
`CodeModeCatalogEntry`). Hosts inject behavior; the kernel never reaches back
into a concrete type.

```text
trait CodeModeHost {
    // Discovery: the tool descriptors the sandbox's `tools` proxy +
    // in-sandbox search/describe read. Pure projection; no transport implied.
    async fn list_tools(&self, scope: &ToolScope) -> Result<Vec<ToolDescriptor>, ToolError>;

    // Optional render-cache hook; default impl just calls list_tools.
    async fn cached_tools_render(&self, scope: &ToolScope) -> Result<ToolsRender, ToolError> { .. }

    // Execution: route a callTool(id, params) to whatever the host's tool
    // source is (REST client, MCP proxy, in-memory stub — the kernel can't tell).
    async fn call_tool(&self, id: &str, params: Value, scope: &ToolScope) -> Result<Value, ToolError>;

    // Snippet resolution (engine lives in-crate; only source lookup is host-provided).
    async fn resolve_snippet(&self, name: &str, input: Value) -> Result<ResolvedSnippet, ToolError>;

    // Config + history sinks.
    async fn config(&self) -> CodeModeConfig;
    async fn record_history(&self, entry: CodeModeHistoryEntry);
    async fn record_source(&self, source: CodeModeExecutionSource);
}
```

Naming rules for this trait and its types:
- Methods are `list_tools` / `call_tool` / `resolve_snippet` — **not**
  `code_mode_catalog`, `resolve_upstream_tool`, or `resolve_and_call_tool`.
- The descriptor type is `ToolDescriptor`; do not export `UpstreamTool` or any
  `Upstream*` / `Gateway*` type from the crate.
- `ToolScope` is a neutral capability/visibility filter (the renamed
  `CodeModeCapabilityFilter`), carrying no gateway/route concepts.

`CodeModeBroker` becomes `CodeModeBroker<H: CodeModeHost>`. Exact signatures are
finalized in Task 3 by reading what the broker actually needs from the current
`GatewayManager` call sites (`execute.rs`, `runner_drive.rs`, `search.rs`,
`manager/code_mode_runtime.rs`, `manager/code_mode_resolve.rs`) — but the trait
that lands is named in neutral terms, and `GatewayManager` is merely Labby's
*implementor* of it, living outside the crate.

---

## File Structure

```
crates/lab-codemode/
  Cargo.toml            # tokio, serde, serde_json, thiserror, javy, nix, tempfile,
                        # tracing, lab-runtime. NO wasmtime/axum/rmcp/clap/anyhow.
  src/lib.rs            # public API: CodeModeBroker, CodeModeHost, execute types,
                        #   run_code_mode_runner_stdio, runner spawn config.
  src/host.rs           # CodeModeHost trait + neutral catalog types.
  src/broker.rs         # CodeModeBroker<H> (was code_mode.rs root).
  src/execute.rs        # single execute() entry + mcp-ui capture.
  src/runner.rs         # runner subprocess loop (synchronous entrypoint).
  src/runner_drive.rs   # parent-side driver: timeout, classify, lease finalize.
  src/runner_io.rs      # framed stdio line protocol.
  src/pool.rs           # warm pool; spawn made host-configurable.
  src/pool/             # runner_handle.rs, config.rs.
  src/protocol.rs       # serialization-stable wire types.
  src/preamble.rs       # JS preamble: callTool stub, search/describe, proxy.
  src/schema.rs         # JSON Schema helpers.
  src/ts_signatures.rs  # TS signature / .d.ts generator.
  src/normalize.rs      # result normalization.
  src/truncate.rs       # output caps.
  src/trace.rs          # tracing spans.
  src/types.rs          # CodeModeRequest/Result/Caller/Surface/CapabilityFilter, etc.
  src/artifacts.rs      # artifact write: path containment, size cap, atomic write.
  src/util.rs           # JS wrapping, id gen.
  src/wrapper.rs        # async IIFE harness.
  src/snippet/          # snippet ENGINE: store.rs, types, resolution.
  CLAUDE.md             # crate ownership + sandbox/trust invariants (moved from
                        #   the two code_mode CLAUDE.md files).
```

Labby files that become thin adapters / shims (no logic):
- `crates/lab/src/dispatch/gateway/code_mode*` → re-export shims, then the host
  impl for `GatewayManager` lands here (or in `lab-gateway` once that exists).
- `crates/lab/src/dispatch/snippets/{dispatch.rs,catalog.rs}` → keep surface;
  delegate engine calls to `lab-codemode`.
- `crates/lab/src/dispatch/snippets/store.rs` → moves into the crate; leave a
  re-export if needed transitionally.
- `crates/lab/src/cli/gateway/code.rs`, `crates/lab/src/mcp/call_tool_codemode.rs`
  → thin adapters constructing the broker over the host impl.
- `crates/lab/src/cli/internal.rs` → still wires `run_code_mode_runner_stdio()`
  (now re-exported from `lab-codemode`).

---

## Tasks (reviewed order)

- [ ] **Task 0 — Prereq gate.** Confirm the gateway plan's `lab-runtime` (Task 1)
  has landed with `ToolError` moved + re-exported. If not, that lands first.
- [ ] **Task 1 — Crate skeleton + move kernel.** Create `lab-codemode`; move
  `runner.rs`, `runner_io.rs`, `protocol.rs`, `artifacts.rs`, `wrapper.rs`,
  `preamble.rs`, `util.rs`. Drop `wasmtime`/`wasm_runner.rs`. Verify the runner
  builds and `run_code_mode_runner_stdio()` keeps its `-> ExitCode` shape.
- [ ] **Task 2 — Move shaping + types.** Move `schema.rs`, `ts_signatures.rs`,
  `normalize.rs`, `truncate.rs`, `trace.rs`, `types.rs`. Replace `UpstreamTool`
  in public catalog types with the crate-neutral `CodeModeCatalogEntry`-based
  type.
- [ ] **Task 3 — Define `CodeModeHost` + genericize broker.** Lift the trait from
  the real `GatewayManager` call sites; make `CodeModeBroker<H: CodeModeHost>`.
  Move `execute.rs`, `runner_drive.rs`, `pool*`, `search.rs` (catalog projection)
  in. Make pool spawn host-configurable (program/args injection, default
  `current_exe()`).
- [ ] **Task 4 — Move snippet engine.** Move `snippets/store.rs` (+ snippet types
  and resolution) into `src/snippet/`. Snippet *source lookup* surfaces through
  the host trait; leave the `snippets` MCP/HTTP/CLI surface in Labby as an
  adapter. Confirm the old bidirectional dependency is gone.
- [ ] **Task 5 — Implement host + rewire callers.** Implement `CodeModeHost` for
  `GatewayManager`. Convert the three call sites (MCP `call_tool_codemode.rs`,
  CLI `gateway/code.rs`, `snippets/dispatch.rs`) to thin adapters over the crate.
  Re-export `run_code_mode_runner_stdio()` for `cli/internal.rs`.
- [ ] **Task 6 — Docs + dead-doc cleanup.** Author `lab-codemode/CLAUDE.md`
  (merge the two code_mode CLAUDE.md files). Correct stale lines: the gateway
  CLAUDE.md "search() entry point", the root CLAUDE.md radarr/sonarr references,
  and any remaining Wasmtime/fuel mentions.
- [ ] **Task 7 — Tests.** Move runtime-only tests into the crate. Keep/port the
  hardening assertions: env isolation, `PR_SET_DUMPABLE`, timeout kind
  `"timeout"`, artifact path containment + size cap, warm-pool recycle/overflow.
  Replace any long-running smokes with bounded spawn/probe/teardown.
- [ ] **Task 8 — Parity validation.** `cargo build --all-features`,
  `cargo nextest run --all-features`, `cargo clippy -D warnings`, `cargo deny`.
  Verify Code Mode behaves identically across MCP, CLI, and `snippets.exec`.

---

## Out of scope (explicitly deferred)

- The ten REST service clients (sonarr/radarr/prowlarr/overseerr/plex/sabnzbd/
  qbittorrent/tautulli/tracearr/bazarr) and the native-dispatch `CodeModeHost`
  that would let Code Mode script them. None of those services exist in this repo
  today, and the extraction does not depend on them. They are a *second host* of
  the extracted kernel, planned separately once the crate exists.
- The standalone media server binary itself.
