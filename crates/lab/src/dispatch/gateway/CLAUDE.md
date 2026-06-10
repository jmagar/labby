# dispatch/gateway/ — Gateway Dispatch Layer

The `gateway` service is the largest and most security-sensitive dispatch tree in `lab`.
It owns upstream MCP server management, Code Mode execution, OAuth lifecycle, protected
routes, and virtual-server projection. Read this file before editing anything under this
directory.

---

## File Map

### Top-level module files

| File | Purpose |
|------|---------|
| `gateway.rs` (parent) | Module entrypoint: `pub mod` declarations, `SHARED_GATEWAY_OAUTH_SUBJECT` constant, public re-exports. |
| `catalog.rs` | `ACTIONS: &[ActionSpec]` — single source of truth for the gateway action catalog. |
| `client.rs` | `GatewayClient` env-loading helpers (`client_from_env`, `require_client`, `not_configured_error`). |
| `params.rs` | Param coercion helpers for all gateway actions. |
| `dispatch.rs` | Top-level action router (`dispatch`, `dispatch_with_manager`). |
| `config.rs` | `GatewayConfig` loading, `load_gateway_config`, `save_gateway_config`. |
| `config_mutation.rs` | Mutation helpers used by `manager/config_ops.rs`. |
| `types.rs` | Shared view/projection types (`GatewayView`, `GatewayRuntimeView`, etc.). |
| `view_models.rs` | Narrower view types used by the admin UI (`ServerView`, `SurfaceStateView`, etc.). |
| `projection.rs` | Read-only projection helpers: `server_view_from_upstream`, `runtime_view`, `operator_visible_upstream_error`, secret-redaction. |
| `runtime.rs` | `GatewayRuntimeHandle` — thin `Arc<RwLock<Option<Arc<UpstreamPool>>>>` swap handle. |
| `shared.rs` | Cross-module shared helpers (e.g. `builtin_service_registry`). |
| `service_catalog.rs` | `service_meta(name)` — looks up `PluginMeta` for registered services. |
| `oauth.rs` | Outbound OAuth action dispatch helpers. |
| `oauth_lifecycle.rs` | OAuth lifecycle orchestration called by `manager/`. |
| `virtual_servers.rs` | Virtual-server CRUD helpers, `VirtualServerRecord`, `VirtualServerSource`. |
| `protected_routes.rs` | `ProtectedRouteIndex` — maps public path prefixes to upstream config records. |
| `discovery.rs` | Upstream discovery orchestration (top-level entry points). |
| `discovery/` | Discovery sub-modules. |

### `manager/` sub-modules

`manager.rs` declares the `GatewayManager` struct (fields only) and `mod` imports.
All method bodies live in child modules as additional `impl GatewayManager` blocks,
keeping each file under ~500 LOC.

| Module | Responsibilities |
|--------|-----------------|
| `manager/core.rs` | `new()`, `with_*` builder chain, `builtin_service_registry()`. |
| `manager/config_ops.rs` | Config reads/writes: `get_config`, `set_config`, `update_upstream`, `remove_upstream`, reload path. |
| `manager/pool_lifecycle.rs` | Pool bootstrap, `reload()`, `reload_pool()`, swap-and-drain. `GatewayCatalogSnapshot` / `diff_catalogs`. |
| `manager/code_mode_runtime.rs` | `execute()` / `search()` — Code Mode request dispatch to the runner subprocess. |
| `manager/code_mode_resolve.rs` | Upstream catalog projection for Code Mode `search` pre-pass. |
| `manager/persist.rs` | `save_config()`, `load_config()`, atomic TOML write. |
| `manager/imports.rs` | `import_from_discovery`, `tombstone_upstream`. |
| `manager/import_matchers.rs` | `ImportTombstoneSelector`, `partition_discovered_for_import`, `discovered_is_tombstoned`. |
| `manager/virtual_servers.rs` | Manager-level virtual-server CRUD delegating to `virtual_servers.rs` helpers. |
| `manager/protected_routes.rs` | Manager-level protected-route management, index rebuild. |
| `manager/oauth_resources.rs` | Upstream OAuth resource/token management. |
| `manager/views.rs` | `list()`, `get()`, `status()`, `test()`, `discovered_tools/resources/prompts()`, `client_config()`. |

### `code_mode/` sub-modules

| Module | Responsibilities |
|--------|-----------------|
| `code_mode.rs` (parent) | Module entrypoint, `CodeModeHistory`, shared types. |
| `code_mode/runner.rs` | Subprocess lifecycle: spawn, stdio framing, request/response loop, kill. |
| `code_mode/runner_drive.rs` | Higher-level runner driver: retry, timeout enforcement, history tracking. |
| `code_mode/runner_io.rs` | Framed stdin/stdout line protocol with the runner child. |
| `code_mode/execute.rs` | `execute()` entry point; builds the Code Mode context and calls the driver. |
| `code_mode/search.rs` | `search()` entry point; projects the catalog and calls the driver. |
| `code_mode/preamble.rs` | Preamble injection into the JS sandbox (catalog stubs, `callTool` bridge). |
| `code_mode/protocol.rs` | Wire types for the parent↔runner stdio protocol messages. |
| `code_mode/schema.rs` | JSON Schema helpers for Code Mode tool descriptions. |
| `code_mode/normalize.rs` | Result normalization / truncation. |
| `code_mode/truncate.rs` | Output size limiting. |
| `code_mode/trace.rs` | Execution tracing/span helpers. |
| `code_mode/types.rs` | Shared Code Mode types (`CodeModeRequest`, `CodeModeResult`, etc.). |
| `code_mode/types_legacy.rs` | Backward-compat shims for older Code Mode type shapes. |
| `code_mode/util.rs` | Small utilities (e.g. JS function wrapping). |
| `code_mode/artifacts.rs` | Artifact write handler — path containment, size cap, atomic write. |
| `code_mode/catalog_cache.rs` | Per-run catalog snapshot cache. |
| `code_mode/wrapper.rs` | JS source wrappers injected around caller snippets. |
| `code_mode/wasm_runner.rs` | **DEAD CODE — do not use.** Wasmtime/WASM runner path kept for reference only. The live runner is Javy/QuickJS via subprocess stdio. See trust-model note below. |

---

## Trust Model — READ BEFORE EDITING

**Gateway admin actions can spawn arbitrary local stdio commands with labby's full
process environment.** The following invariants are NON-NEGOTIABLE:

1. **HTTP must never be exposed without auth.** Every `/v1/gateway/*` route is
   gated behind bearer-token or OAuth middleware. See
   `docs/surfaces/TRANSPORT.md` for the full auth layer contract. Do not add
   unauthenticated gateway write routes.

2. **Stdio command spawning is intentional by design** — upstreams may be stdio
   MCP servers. Any operator who can write to the gateway config file or call
   `gateway.add` / `gateway.update` over a live authenticated connection can
   cause labby to spawn arbitrary commands. This is a documented trust boundary,
   not a vulnerability, but it means:
   - Never execute gateway add/update actions without confirmed auth.
   - The `destructive: true` annotation on remove/update actions triggers MCP
     elicitation and HTTP `confirm: true` gating.
   - `pool/connect_stdio.rs` sets a process-group guard so spawned children are
     reaped on drop.

3. **`env_clear` + `STDIO_ENV_ALLOWLIST` is the CURRENT hardened state.**
   `pool/connect_stdio.rs` calls `cmd.env_clear()` and then layers only the
   entries in `STDIO_ENV_ALLOWLIST` (PATH, HOME, TZ, SSL roots, and a small set
   of runtime-essential vars) before adding the upstream's declared `env` map.
   `LAB_*` secrets and every other ambient labby env var are excluded by default.
   To extend the allowlist, add entries to `STDIO_ENV_ALLOWLIST` in
   `pool/connect_stdio.rs` with a comment justifying why the var is safe to
   forward. Integration tests in `crates/lab/tests/gateway_stdio_spawn.rs` that
   assert on env behaviour are non-`#[ignore]` where hermetic (Linux `/proc`
   inspect) and `#[ignore]` where they require a built binary or external tool.

4. **`wasm_runner.rs` is dead code.** No Wasmtime/fuel path is wired. The live
   Code Mode runner is always Javy/QuickJS via subprocess stdio. If you see
   references to `code_mode_fuel_exhausted` in error-handling code, that is
   stale — the live emitted kind for execution-time budget exhaustion is
   `"timeout"`. See `docs/dev/ERRORS.md` for the canonical error kind contract.

---

## Code Mode — Runner Source of Truth

**The authoritative documentation for the Code Mode JS execution surface is
`docs/dev/CODE_MODE.md`.** The key facts for avoiding drift:

- **Runtime: Javy/QuickJS via subprocess stdio, NOT Wasmtime/fuel.**
- Bounded by: 30-second wall-clock timeout + 64 MiB memory + stack limit.
- The emitted `ToolError` kind on wall-clock expiry is `"timeout"`.
- `wasm_runner.rs` is kept for historical reference; it is not on any live code path.
- `code_mode/runner.rs` + `runner_drive.rs` + `runner_io.rs` are the live path.

Any doc or comment that says "Wasmtime", "fuel budget", or
`code_mode_fuel_exhausted` on an active code path is wrong and must be corrected.

---

## OAuth Lifecycle

Outbound OAuth (upstream MCP servers protected by OAuth) is coordinated by:

1. `dispatch/gateway/oauth.rs` — action-level entry points (`oauth.start`, `oauth.status`, `oauth.callback`).
2. `dispatch/gateway/oauth_lifecycle.rs` — core orchestration: PKCE flow, token storage, refresh.
3. `manager/oauth_resources.rs` — per-upstream token/credential management within the manager.
4. `crates/lab/src/oauth/upstream/` — the reusable `UpstreamOauthManager` (wire-level OAuth client).

Full flow documented in `docs/services/UPSTREAM.md`. Stable error kinds for OAuth
failures are in `docs/dev/ERRORS.md` under "Upstream OAuth Kinds".

---

## Module Size Rule

**No file in this tree may exceed 500 LOC (tests included).** The manager split
exists specifically to enforce this. If a module approaches 500 LOC, split it
following the pattern already established in `manager/` and `pool/`:

- Keep the struct definition in the parent `.rs` file.
- Move method bodies into named child modules as `impl StructName` blocks.
- Document the new module in this file.

---

## Related Docs

- `docs/dev/CODE_MODE.md` — Code Mode surface and runner spec (authoritative)
- `docs/dev/ERRORS.md` — stable error kind contract (authoritative)
- `docs/surfaces/TRANSPORT.md` — auth layer, MCP/HTTP transport security (authoritative)
- `docs/services/UPSTREAM.md` — upstream OAuth flow
- `crates/lab/src/dispatch/upstream/CLAUDE.md` — upstream pool internals
