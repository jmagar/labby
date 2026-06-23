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
| `manager/code_mode_runtime.rs` | Code Mode request dispatch to the runner subprocess. |
| `manager/code_mode_resolve.rs` | Upstream catalog projection for Code Mode discovery. |
| `manager/persist.rs` | `save_config()`, `load_config()`, atomic TOML write. |
| `manager/imports.rs` | `import_from_discovery`, `tombstone_upstream`. |
| `manager/import_matchers.rs` | `ImportTombstoneSelector`, `partition_discovered_for_import`, `discovered_is_tombstoned`. |
| `manager/virtual_servers.rs` | Manager-level virtual-server CRUD delegating to `virtual_servers.rs` helpers. |
| `manager/protected_routes.rs` | Manager-level protected-route management, index rebuild. |
| `manager/oauth_resources.rs` | Upstream OAuth resource/token management. |
| `manager/views.rs` | `list()`, `get()`, `status()`, `test()`, `discovered_tools/resources/prompts()`, `client_config()`. |

### `code_mode/` — extracted to the `labby-codemode` crate

**The Code Mode JS execution kernel, broker, result-shaping helpers, and snippet
engine were extracted into the client-neutral `labby-codemode` crate** (runner,
runner_drive, runner_io, protocol, pool, artifacts, wrapper, preamble, util,
execute broker, schema, normalize, truncate, trace, types, ts_signatures, plus
the snippet store). The crate is injected with a tool source via the
`labby_codemode::CodeModeHost` trait and carries **no** gateway/upstream
vocabulary. Wasmtime / `wasm_runner.rs` were dropped (dead code). See
`crates/labby-codemode/CLAUDE.md` for the sandbox containment invariants.

What stays in `dispatch/gateway/code_mode/` is the gateway's **thin adapter**:

| Module | Responsibilities |
|--------|-----------------|
| `code_mode.rs` (parent) | Re-exports the crate's public surface under the historical `code_mode::*` paths (with `CodeModeCapabilityFilter`/`CodeModeCatalogEntry`/`split_upstream_tool` aliases), and owns the host-side `CatalogRenderCache` / `SnippetMetadataCache`. |
| `code_mode/code_mode_host.rs` | `impl CodeModeHost for GatewayManager` — the upstream→`ToolDescriptor` binding, `callTool` over the `UpstreamPool`, mcp-ui capture + result unwrap, upstream-error classification, snippet source resolution, OAuth-subject / runtime-owner derivation. This is the one place gateway/upstream vocabulary is legitimately reintroduced. |
| `code_mode/search.rs` | Host-side discovery catalog projection + render cache (`build_tools_render`). |
| `code_mode/catalog_cache.rs` | One-shot CLI on-disk catalog cache (gateway-config fingerprinted). |

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
   forward. Integration tests in `crates/labby/tests/gateway_stdio_spawn.rs` that
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

**Runner process hardening is IMPLEMENTED (not "planned").** The Code Mode
runner subprocess is spawned with `env_clear()` (`pool/runner_handle.rs`, in
`PooledRunner::spawn` — the child inherits *no* labby environment, so `LAB_*`
secrets cannot leak) and on
Linux calls `prctl(PR_SET_DUMPABLE, 0)` as its first act (`runner.rs:22`) to
block `/proc/<pid>/environ` readback. Combined with the per-run `TempDir` cwd
and the process-group/`killpg` guard, this is the current hardened state. See
`code_mode/CLAUDE.md` § "Sandbox Containment Invariants" for the full table.

---

## OAuth Lifecycle

Outbound OAuth (upstream MCP servers protected by OAuth) is coordinated by:

1. `dispatch/gateway/oauth.rs` — action-level entry points (`oauth.start`, `oauth.status`, `oauth.callback`).
2. `dispatch/gateway/oauth_lifecycle.rs` — core orchestration: PKCE flow, token storage, refresh.
3. `manager/oauth_resources.rs` — per-upstream token/credential management within the manager.
4. `crates/labby-auth/src/upstream/` — the reusable `UpstreamOauthManager` (wire-level OAuth client).

Full flow documented in `docs/services/UPSTREAM.md`. Stable error kinds for OAuth
failures are in `docs/dev/ERRORS.md` under "Upstream OAuth Kinds".

---

## `GatewayManager` — sanctioned in-dispatch stateful runtime component

`GatewayManager` (`manager.rs` + the `manager/` impl split) is a **de-facto SDK
client that intentionally lives in dispatch, not in `labby-apis`.** This is a
sanctioned pattern, not a Golden-Rule violation — call it out explicitly so it
is not mistaken for a layout mistake during review or onboarding:

- It is the gateway analogue of the **`upstream` connection pool**: a stateful
  runtime object (`RwLock<LabConfig>` + a live `Arc<UpstreamPool>` of spawned
  stdio children) that cannot be a stateless `labby-apis` HTTP client. The pool is
  in-process, file-backed, and inherently single-instance (see the
  horizontal-scaling note in `02-security-performance.md` / Perf-C1).
- Dispatch arms over it are correctly **thin**: `to_json(manager.foo().await?)`.
  The complexity is encapsulated inside `GatewayManager`, which is the right
  place for it given the runtime state it owns.
- **No `labby-apis` counterpart is expected.** Unlike pure upstream API services
  (radarr, unraid, …), the gateway's "client" manages local process and config
  state, which the `labby-apis` layer (no `tokio`-spawned children, no config
  files) deliberately does not own.

`config.rs` persistence (`toml_edit` round-trips, atomic write, `fd_lock`) is a
property of this single-instance runtime component and stays with it; it is not
a separable SDK concern that belongs in `labby-apis`.

## Module Size Rule

**Target: no source file in this tree should exceed 500 LOC of non-data logic.**
The `manager/` and `pool/` impl-splits exist specifically to enforce this for
behavioral code. Two clarifications keep this rule and the actual code honest:

1. **Pure `ActionSpec` data tables are EXEMPT.** `catalog.rs` is a flat
   `&[ActionSpec]` literal — a declarative data table, not logic. It is allowed
   to exceed 500 LOC; splitting it would fragment a single source of truth for
   no maintainability gain. (Current size ~1030 LOC, almost entirely the
   action table.)

2. **Behavioral files that currently exceed 500 LOC are a tracked DEFERRED
   follow-up, not a silent exception.** As of this writing the over-limit
   behavioral files are:

   | File | LOC | Status |
   |------|-----|--------|
   | `config.rs` | ~2218 | DEFERRED split — by concern (load / render / mutation) using the `manager/` impl-split pattern. |
   | `dispatch.rs` | ~2636 | DEFERRED split — extract the `handle_*` action groups into submodules. |
   | `code_mode/runner_drive.rs` | ~818 | DEFERRED split — separate spawn/IO from timeout/retry/history. |

   These splits were intentionally left out of the current architecture sweep to
   keep the change reviewable; do not treat their current size as license to add
   more. New behavioral code in this tree must still aim for ≤500 LOC and split
   following the established pattern:

- Keep the struct definition in the parent `.rs` file.
- Move method bodies into named child modules as `impl StructName` blocks.
- Document the new module in this file.

---

## Related Docs

- `docs/dev/CODE_MODE.md` — Code Mode surface and runner spec (authoritative)
- `docs/dev/ERRORS.md` — stable error kind contract (authoritative)
- `docs/surfaces/TRANSPORT.md` — auth layer, MCP/HTTP transport security (authoritative)
- `docs/services/UPSTREAM.md` — upstream OAuth flow
- `crates/labby-gateway/src/upstream/CLAUDE.md` — upstream pool internals
