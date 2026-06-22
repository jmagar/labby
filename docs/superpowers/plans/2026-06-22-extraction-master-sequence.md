# Gateway Extraction — Master Sequence

> **Purpose:** the single ordered execution plan that unifies the two extraction
> plan documents into one global sequence. It does **not** duplicate task bodies —
> each step links to the authoritative task in its owning plan. Execute strictly
> top to bottom; the dependency rationale for each step is stated inline.
>
> **Owning plans:**
> - Umbrella — [`2026-06-22-standalone-gateway-extraction.md`](2026-06-22-standalone-gateway-extraction.md) (`lab-runtime`, `lab-auth` finish, `lab-gateway`, `lab-gateway-web`, `lab-gatewayd`)
> - Sub-plan — [`2026-06-22-code-mode-crate-extraction.md`](2026-06-22-code-mode-crate-extraction.md) (`lab-codemode`)
> - Superseded — `2026-05-25-extract-gateway-server.md` (do not execute)

## Scope

In scope: the six-crate gateway stack — `lab-runtime`, `lab-auth` (finish),
`lab-codemode`, `lab-gateway`, `lab-gateway-web`, `lab-gatewayd`.

Explicitly **out of scope** (deferred, not part of this sequence): the ten REST
service clients (sonarr/radarr/prowlarr/overseerr/plex/sabnzbd/qbittorrent/
tautulli/tracearr/bazarr) and the native-dispatch *second* `CodeModeHost` that
would let Code Mode script them. Those are a future consumer of the extracted
`lab-codemode` kernel and get their own plan once this stack lands.

## The one ordering decision this sequence pins

**Extract `lab-codemode` and wire `CodeModeHost` onto `GatewayManager` while the
manager still lives in `lab` (Phase 2), before the gateway crate split moves the
manager out (Phase 6).** This means Code Mode is decoupled exactly once: the
later gateway split relocates an already-`CodeModeHost`-shaped manager instead of
moving Code Mode runtime a second time. Do not invert these.

## Dependency graph

```
lab-runtime ──┬─> lab-codemode ───────────────────────────┐
              ├─> lab-auth (upstream OAuth) ─> lab-gateway ┤
              │                                  (pool)    │
              └─> lab-gateway-web                          │
                                                           v
                              lab-gateway (GatewayManager + dispatch + host dep)
                                                           │
                                                           v
                                                     lab-gatewayd
                                                           │
                                                           v
                                              detach shims ─> validation
```

## Ordered execution

- [ ] **Phase 1 — `lab-runtime` contracts.**
  Run umbrella **Task 1**. Produces `ToolError`, gateway config DTOs (with serde
  defaults + roundtrip fixtures), redact/path-safety/process contracts.
  *Why first:* every other crate imports `ToolError` and/or the config DTOs from
  here. Root of the graph.

- [ ] **Phase 2 — `lab-codemode` kernel + host wiring (in place).**
  Run the entire **sub-plan** (`2026-06-22-code-mode-crate-extraction.md`,
  Tasks 1–8). Extract the Javy kernel, broker, shaping helpers, and snippet
  engine; define the client-neutral `CodeModeHost` trait (`list_tools` /
  `call_tool` / `resolve_snippet`, `ToolDescriptor` / `ToolScope`); implement
  `CodeModeHost for GatewayManager` **inside `lab`** (the manager has not moved
  yet); rewire the MCP, CLI, and `snippets` callers as thin adapters; drop
  `wasmtime`.
  *Why here:* needs only `lab-runtime`. Doing it now means the manager is already
  host-shaped before any crate split — see the pinned ordering decision above.
  *Exit check:* `cargo nextest run --all-features` green with Code Mode behaving
  identically across all three surfaces; `grep -r 'upstream\|gateway' lab-codemode/src` empty.

- [ ] **Phase 3 — `lab-auth` finish (outbound upstream OAuth).**
  Run umbrella **Task 3** (numbered Task 2 in that doc's file list but ordered
  third per its reviewed-order note). Move `oauth/upstream/{runtime,manager,cache,
  refresh,encryption,store,types}` into `lab-auth`; preserve AAD binding,
  single-flight refresh, resource/issuer checks, subject fingerprinting.
  *Why before the pool:* the upstream pool depends on `OauthClientCache`; moving
  the cache first avoids a temporary OAuth-disabled pool path.

- [ ] **Phase 4 — `lab-gateway` crate + upstream pool.**
  Run umbrella **Task 2**. Create `lab-gateway`; move the upstream pool against
  the real `lab-auth` upstream APIs from Phase 3. Do **not** add `javy` here.
  *Depends on:* `lab-runtime` (Phase 1), `lab-auth` upstream cache (Phase 3).

- [ ] **Phase 5 — `lab-gateway-web` assets.**
  Run umbrella **Task 4**. Pure embedded/filesystem asset lookup + content-type/
  cache headers; route precedence stays in the daemon/wrappers.
  *Independent:* only needs the workspace; may run any time after Phase 1.

- [ ] **Phase 6 — Move `GatewayManager` + dispatch into `lab-gateway`.**
  Run umbrella **Task 5** (as amended). Relocate the (already `CodeModeHost`-
  implementing) `GatewayManager` and dispatch into `lab-gateway`; add the
  `lab-codemode` path dependency; add `code_mode_host.rs`; remove the
  `build_default_registry()` fallback via injected registry composition.
  *Depends on:* Phases 2, 3, 4. `lab-gateway` exports no `code_mode` runtime
  module — only the host impl.

- [ ] **Phase 7 — `lab-gatewayd` standalone binary.**
  Run umbrella **Task 6** after the route-parity inventory. CLI (`serve` / `mcp` /
  hidden `internal code-mode-runner` → `lab_codemode::run_code_mode_runner_stdio`),
  daemon-scoped state (one `Arc<GatewayManager>` + `Arc<UpstreamPool>`),
  auth-gated `/v1/gateway`, split stdio vs HTTP MCP trust paths, web precedence.
  *Depends on:* all crates above.

- [ ] **Phase 8 — Detach / mark Labby shims.**
  Run umbrella **Task 7**. Convert remaining Labby gateway/upstream/oauth/web/
  code_mode entrypoints to forwarding shims (or remove), mark them, track removal.

- [ ] **Phase 9 — Parity, features, cache, timing validation + CI.**
  Run umbrella **Task 8**. Full check/test matrix across all new crates, bounded
  spawn/probe/teardown smokes, `cargo tree -e features` audit (no normal-path
  `wasmtime`/`axum`/`clap` in `lab-gateway`/`lab-codemode`), CI gates, closeout.

## Invariants that span the whole sequence

These hold at every phase; both owning plans carry the detail:

- Code Mode hardening: Javy/QuickJS subprocess, 30s wall-clock (`timeout` kind),
  64 MiB heap, temp cwd jail, `env_clear`, Linux `PR_SET_DUMPABLE=0`,
  process-group cleanup. No `wasmtime` on any normal path.
- Stdio spawn hardening (upstream pool): `env_clear` + allowlist overlay, stderr
  drain, spawn lock, process-group/job cleanup, relay subject isolation,
  `proxy_resources` gating; validate persisted specs and fail closed.
- Auth: `/v1/gateway` admin routes never mount without configured auth +
  route-layer middleware + handler `AuthContext` + admin-scope check. HTTP MCP
  always carries an `AuthContext`; only stdio MCP may construct trusted-local.
- Outbound OAuth: AAD `(upstream, subject, client_id)`, shared `gateway` subject,
  single-flight refresh, resource/issuer checks, stable error kinds; never log
  raw `auth.sub` (fingerprint instead).
- `lab-runtime` stays transport-free; `lab-codemode` stays client/transport-free
  (no `upstream`/`gateway` vocabulary); `lab-gateway` stays adapter-free (no
  `axum`/`clap`/`utoipa`, no `build_default_registry`).

## Operational gotchas (learned during execution)

- **Every new workspace crate needs a `config/Dockerfile` dep-cache entry**, or
  the "Container build + smoke" CI job fails with `failed to read
  crates/<crate>/Cargo.toml`. That layer COPYs each member's `Cargo.toml` and
  stubs its `src/` individually for a hardcoded list. For each new crate add: the
  `COPY crates/<crate>/Cargo.toml ...` line, `crates/<crate>/src` to the `mkdir`,
  `crates/<crate>/src/lib.rs` to the `touch`, and `cargo clean -p <crate>`. If the
  crate declares `build = "build.rs"`, also `echo 'fn main(){}' >
  crates/<crate>/build.rs` (lab-gateway-web needed this). **`cargo check
  --all-features` passing locally does NOT catch this** — only the in-image build
  does. (lab-codemode, lab-gateway, lab-gatewayd will each need this.)
- **`ToolError` move is not mechanical (orphan rule).** Moving `ToolError` to
  `lab-runtime` makes `impl IntoResponse for ToolError` (`api/error.rs`) and the
  `From<lab_apis::*>` impls illegal in `lab`. Resolution: the `lab-apis`-sourced
  `From` impls move into `lab-runtime` behind feature gates (optional `lab-apis`
  dep); the `marketplace::store::RegistryStoreError` `From` stays in `lab` (local
  source type); a local `ApiError(ToolError)` newtype in `api/error.rs` carries
  `IntoResponse`, threaded through ~82 handler return sites. `path_safety.rs`
  returns `ToolError`, so it can only move to `lab-runtime` AFTER `ToolError`.
- **`lab-auth` OAuth move has one `LabConfig` coupling:**
  `oauth/upstream/runtime.rs` takes `config: &LabConfig` (lines ~19, ~58). Since
  `lab-auth` cannot depend on `lab`, the Phase 3 move must pass the specific
  upstream-config data those functions need instead of the whole `LabConfig`.
- **`lab-runtime::process` is a consolidation, not a move** — there is no single
  `dispatch/process.rs`; process-group/`killpg` logic lives in
  `upstream/process_guard.rs` and `code_mode/pool/runner_handle.rs`. Treat as
  optional/deferred; the upstream guard travels with `lab-gateway` regardless.
- **Integrate worktree branches one at a time, verified.** Each agent branch
  conflicts on `Cargo.toml` (`members`), `crates/lab/Cargo.toml` (deps/features),
  and `crates/lab-runtime/src/lib.rs` (module decls) — all trivial "keep both"
  resolutions. `cargo check --all-features` green after each merge before the next.
