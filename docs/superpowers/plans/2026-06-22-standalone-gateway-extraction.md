# Standalone Gateway Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the current Labby gateway, gateway web UI, and outbound upstream OAuth runtime into standalone-owned crates and a new `lab-gatewayd` binary that can run the gateway without unrelated Labby services.

**Architecture:** Build the split in dependency order: `lab-runtime` for contracts, `lab-auth` for reusable outbound OAuth mechanics, `lab-codemode` for the client-neutral Javy/QuickJS Code Mode execution kernel, `lab-gateway` for gateway/upstream runtime (which depends on `lab-codemode` and implements its `CodeModeHost` trait), `lab-gateway-web` for pure static asset lookup/headers, and `lab-gatewayd` for CLI/API/MCP/auth/web/process entrypoints. Labby stays compatible through temporary forwarding shims only; the standalone binary is the product proof.

> **Code Mode is NOT folded into `lab-gateway`.** It is extracted into its own client-neutral `lab-codemode` crate; `lab-gateway` depends on it and implements `CodeModeHost`. The full `lab-codemode` task/step breakdown lives in the companion sub-plan [`2026-06-22-code-mode-crate-extraction.md`](2026-06-22-code-mode-crate-extraction.md). Task 5 below is amended accordingly.

**Tech Stack:** Rust 2024, Cargo workspace resolver 3, Tokio, serde, thiserror, rmcp 1.7, axum 0.8, lab-auth SQLite storage, Next.js static export from `apps/gateway-admin`.

## Global Constraints

- The standalone gateway binary is the canonical product target; Labby compatibility is transitional only.
- `lab-runtime` contains contracts, DTOs, and pure helpers only; it must not depend on `axum`, `clap`, `rmcp`, `javy`, `wasmtime`, `utoipa`, or Labby product registry builders.
- Do not duplicate `ActionSpec`. Use the existing `lab_apis::core::ActionSpec` unless a later compile error proves a move is required; if it moves, re-export it from `lab-apis` so there is still one catalog metadata type.
- Extract gateway config by dependency closure and TOML roundtrip fixtures. Do not hand-copy a shortlist of DTOs and lose serde defaults such as omitted `proxy_resources` / `proxy_prompts` defaulting to `true`.
- `lab-gateway` owns runtime behavior only; it must not import Labby, Clap commands, Axum route modules, Lab MCP server/session modules, web serving, or Labby default registry construction.
- `lab-gateway` must preserve daemon-scoped state. The standalone daemon owns one long-lived `Arc<GatewayManager>` and one long-lived `Arc<UpstreamPool>`; handlers must not construct managers or pools per request.
- `lab-auth` owns reusable outbound upstream OAuth mechanics; standalone binary owns browser/API route handling, sessions, `AuthContext`, cookies, and admin checks.
- Move upstream OAuth cache/types before moving the upstream pool, or introduce a real token/refresh provider trait first. Do not ship a temporary upstream pool path with subject-scoped OAuth disabled.
- `lab-gateway-web` owns Rust asset embedding, static asset resolution, content types, and cache headers only. Axum routing, node-role policy, auth policy, and SPA fallback order stay in `lab-gatewayd` and temporary Labby wrappers.
- Keep gateway admin routes authenticated. Do not mount `/v1/gateway` admin routes without configured API auth, route-layer auth middleware, handler-level `AuthContext`, and admin-scope checks.
- HTTP MCP and stdio MCP are separate trust paths. HTTP MCP must always receive an `AuthContext`; missing auth is trusted only for stdio construction.
- Preserve stdio spawn hardening: `env_clear`, allowlist/overlay behavior, stderr drain, spawn lock, process-group/job cleanup, relay subject isolation, and `proxy_resources` gating.
- Validate persisted stdio specs at standalone config load, reload, import, and immediately before connect. Fail closed for invalid commands unless an explicit documented bypass is set.
- Preserve Code Mode hardening: Javy/QuickJS subprocess path, `internal code-mode-runner`, 30s timeout, 64 MiB heap, temp cwd, process cleanup, Linux `PR_SET_DUMPABLE=0`, and timeout kind `timeout`.
- Preserve the current synchronous runner entrypoint shape: `run_code_mode_runner_stdio() -> std::process::ExitCode`. Do not invent an async runner API as part of the move.
- Code Mode is extracted into its own `lab-codemode` crate exposing a client-neutral `CodeModeHost` trait (`list_tools` / `call_tool` / `resolve_snippet` / `config` / history) with neutral `ToolDescriptor` / `ToolScope` types. `lab-gateway` depends on `lab-codemode` and implements that trait; it does NOT contain Code Mode runtime. `lab-codemode` carries the Javy kernel + snippet engine and must contain no `upstream`/`gateway`/client vocabulary (a grep of `lab-codemode/src` for `upstream`/`gateway` must be empty). The dead `wasmtime` dependency is dropped entirely. Details: companion sub-plan `2026-06-22-code-mode-crate-extraction.md`.
- Preserve outbound OAuth semantics: AAD binding `(upstream, subject, client_id)`, shared `gateway` subject behavior, single-flight refresh, resource/issuer checks, and stable error kinds.
- Redact or fingerprint OAuth subject values in all route and manager logs; do not log raw `auth.sub`.
- Normal gateway builds must not enable dead Wasmtime/fuel paths unless explicitly test/dev-gated.
- Prefer manifest and `cargo tree -e features` dependency gates over broad source-string scans. Keep source scans only for high-value symbols such as `build_default_registry`.
- Replace long-running smoke commands with bounded spawn/probe/teardown tests.

---

## File Structure

Create these crates and keep responsibilities narrow:

- `crates/lab-runtime/`
  - `Cargo.toml`: internal contract crate dependencies.
  - `src/lib.rs`: public module exports.
  - `src/error.rs`: moved `ToolError` stable envelope and helpers.
  - `src/gateway_config.rs`: gateway config DTO dependency closure moved from `crates/lab/src/config.rs`, including serde defaults and roundtrip fixtures.
  - `src/redact.rs`: surface-neutral redaction helpers.
  - `src/path_safety.rs`: surface-neutral path helpers.
  - `src/process.rs`: Unix process helper contracts that are safe to share.

- `crates/lab-codemode/` (client-neutral Code Mode kernel — see companion sub-plan `2026-06-22-code-mode-crate-extraction.md` for the full task/step breakdown)
  - `Cargo.toml`: `tokio`, `serde`, `serde_json`, `thiserror`, `javy`, `nix`, `tempfile`, `tracing`, `lab-runtime`. NO `wasmtime` / `axum` / `rmcp` / `clap` / `upstream` / `gateway` dependencies.
  - `src/lib.rs`: public API — `CodeModeBroker<H>`, `CodeModeHost`, execute types, `run_code_mode_runner_stdio`.
  - `src/host.rs`: `CodeModeHost` trait (`list_tools` / `call_tool` / `resolve_snippet` / `config` / history) + neutral `ToolDescriptor` / `ToolScope` types.
  - `src/runner.rs`, `src/runner_io.rs`, `src/protocol.rs`, `src/pool*`: Javy/QuickJS subprocess kernel + warm pool (host-configurable spawn).
  - `src/{execute,broker,preamble,schema,ts_signatures,normalize,truncate,trace,types,artifacts,util,wrapper}.rs`: broker + shaping helpers.
  - `src/snippet/**`: snippet engine (store/types/resolution). The snippet MCP/HTTP/CLI surface stays in Labby as a thin adapter.
  - `CLAUDE.md`: sandbox/trust invariants (merged from the two existing code_mode CLAUDE.md files).

- `crates/lab-gateway/`
  - `Cargo.toml`: runtime dependencies, including `rmcp`, `reqwest`, `tokio`, `lab-runtime`, `lab-auth`, `lab-codemode`. (Javy lives in `lab-codemode`, not here.)
  - `src/lib.rs`: gateway runtime public API.
  - `src/upstream.rs` and `src/upstream/**`: moved upstream pool.
  - `src/gateway.rs` and `src/gateway/**`: moved gateway manager/catalog/dispatch/projection/runtime.
  - `src/code_mode_host.rs`: `impl CodeModeHost for GatewayManager` — projects upstream tools into `ToolDescriptor` and routes `call_tool` to the pool. (Code Mode runtime itself lives in `lab-codemode`.)
  - Optional `src/registry.rs`: create only if the compiler proves a concrete injected-peer trait is needed; do not invent `GatewayRegisteredService` upfront.
  - `src/tests/**`: runtime-only tests moved from Labby when they do not need product adapters.
  - `CLAUDE.md`: new crate ownership and trust-boundary guide.

- `crates/lab-gateway-web/`
  - `Cargo.toml`: asset resolver dependencies.
  - `build.rs`: embed `apps/gateway-admin/out` into this crate's `OUT_DIR`.
  - `src/lib.rs`: public asset API.
  - `src/assets.rs`: embedded asset table and lookup.
  - `src/fs_assets.rs`: configured filesystem asset resolution.
  - `src/response.rs`: content type and cache-policy helpers.

- `crates/lab-gatewayd/`
  - `Cargo.toml`: binary dependencies.
  - `src/main.rs`: process entrypoint.
  - `src/cli.rs`: Clap args for `serve`, `mcp`, and hidden `internal code-mode-runner`.
  - `src/config.rs`: standalone config loading and conversion to runtime DTOs.
  - `src/state.rs`: gateway-only HTTP/MCP state.
  - `src/router.rs`: standalone route composition.
  - `src/api_gateway.rs`: `/v1/gateway` action route.
  - `src/api_upstream_oauth.rs`: `/v1/gateway/oauth` and browser callback routes.
  - `src/mcp_stdio.rs`: trusted stdio MCP server/session wiring.
  - `src/mcp_http.rs`: HTTP MCP server/session wiring that requires `AuthContext`.
  - `src/web.rs`: owns route precedence and calls `lab-gateway-web` for asset lookup.
  - `tests/**`: standalone runtime, auth, MCP, OAuth, Code Mode, and web parity tests.

Reviewed implementation order:

1. Task 1: create `lab-runtime` contracts and config roundtrip fixtures.
2. Task 2: move upstream OAuth runtime/cache/types into `lab-auth`.
3. Task 3: create `lab-gateway` and move upstream pool against the real `lab-auth` APIs.
4. Task 4: extract pure web asset lookup into `lab-gateway-web`.
5. Task 5: extract the Code Mode kernel into `lab-codemode` (per the companion sub-plan), then move `GatewayManager` + dispatch into `lab-gateway` and implement `CodeModeHost` on `GatewayManager`. Do NOT fold Code Mode into `lab-gateway`.
6. Task 6: build `lab-gatewayd` after a route-parity inventory.
7. Task 7: detach or mark Labby shims.
8. Task 8: run parity, feature, cache, watcher, and timing validation.

Labby compatibility files to touch only as shims or removals:

- `crates/lab/src/dispatch/gateway.rs`
- `crates/lab/src/dispatch/gateway/**`
- `crates/lab/src/dispatch/gateway/code_mode/**` (becomes re-export shims over `lab-codemode`)
- `crates/lab/src/dispatch/snippets/**` (snippet surface kept in Labby; engine delegates to `lab-codemode`)
- `crates/lab/src/dispatch/upstream.rs`
- `crates/lab/src/dispatch/upstream/**`
- `crates/lab/src/oauth/upstream.rs`
- `crates/lab/src/oauth/upstream/**`
- `crates/lab/src/api/web.rs`
- `crates/lab/build.rs`
- `crates/lab/src/cli/serve.rs`
- `crates/lab/src/cli/gateway.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/api/services/gateway.rs`
- `crates/lab/src/api/upstream_oauth.rs`
- `crates/lab/src/mcp/**`
- `crates/lab/src/registry.rs`

---

### Task 1: Create `lab-runtime` Contracts

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/lab-runtime/Cargo.toml`
- Create: `crates/lab-runtime/src/lib.rs`
- Create: `crates/lab-runtime/src/error.rs`
- Create: `crates/lab-runtime/src/gateway_config.rs`
- Create: `crates/lab-runtime/src/redact.rs`
- Create: `crates/lab-runtime/src/path_safety.rs`
- Create: `crates/lab-runtime/src/process.rs`
- Modify: `crates/lab/src/dispatch/error.rs`
- Modify: `crates/lab/src/dispatch/helpers.rs`
- Modify: `crates/lab/src/dispatch/redact.rs`
- Modify: `crates/lab/src/dispatch/path_safety.rs`
- Test: `crates/lab/tests/architecture_boundaries.rs`

**Interfaces:**
- Consumes: existing `ToolError`, existing `lab_apis::core::ActionSpec`, redaction helpers, path safety helpers, gateway config DTO dependency closure from `crates/lab/src/config.rs`.
- Produces:
  - `lab_runtime::error::ToolError`
  - `lab_runtime::gateway_config::{CodeModeConfig, GatewayImportMode, UpstreamConfig, ImportSource, UpstreamOauthConfig, UpstreamOauthRegistration, ProtectedMcpRouteConfig, ProtectedMcpRouteTarget, VirtualServerConfig, WebPreferences}`
  - `lab_runtime::redact::{redact_secret, redact_env_value}`
  - `lab_runtime::redact::fingerprint_subject`
  - `lab_runtime::path_safety::{normalize_relative_asset_path, is_within_root}`

- [ ] **Step 1: Add the crate to the workspace**

Edit `Cargo.toml` so the workspace members are exactly:

```toml
members = [
  "crates/lab-apis",
  "crates/lab",
  "crates/lab-auth",
  "crates/lab-winjob",
  "crates/lab-runtime",
]
```

- [ ] **Step 2: Create `crates/lab-runtime/Cargo.toml`**

```toml
[package]
name = "lab-runtime"
description = "Internal Lab runtime contracts shared by gateway extraction crates."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
url.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Create module exports**

Create `crates/lab-runtime/src/lib.rs`:

```rust
#![forbid(unsafe_code)]

pub mod error;
pub mod gateway_config;
pub mod path_safety;
pub mod process;
pub mod redact;
```

Do not create `src/action.rs` unless the implementer chooses to move `ActionSpec` wholesale out of `lab-apis`. The default path is to keep `ActionSpec` in `lab-apis` and import it from there.

- [ ] **Step 4: Move `ToolError` with a compatibility re-export**

Move the contents of `crates/lab/src/dispatch/error.rs` into `crates/lab-runtime/src/error.rs` unchanged except for module paths. Replace `crates/lab/src/dispatch/error.rs` with:

```rust
pub use lab_runtime::error::ToolError;
```

- [ ] **Step 5: Move gateway config by dependency closure**

Move `UpstreamConfig` and every directly referenced gateway config type it needs from `crates/lab/src/config.rs` into `crates/lab-runtime/src/gateway_config.rs`. Include the current serde attributes, defaults, and helper functions for omitted `proxy_resources` and `proxy_prompts`.

Add this fixture test in `crates/lab-runtime/src/gateway_config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::UpstreamConfig;

    #[test]
    fn omitted_proxy_flags_default_to_true() {
        let toml = r#"
name = "axon"
url = "https://axon.example/mcp"
"#;
        let cfg: UpstreamConfig = toml::from_str(toml).expect("deserialize upstream config");
        assert_eq!(cfg.proxy_resources, Some(true));
        assert_eq!(cfg.proxy_prompts, Some(true));
    }
}
```

Add `toml` as a dev-dependency if `lab-runtime` does not already depend on it:

```toml
[dev-dependencies]
toml.workspace = true
```

- [ ] **Step 6: Add the Labby dependency**

In `crates/lab/Cargo.toml`, add:

```toml
lab-runtime = { path = "../lab-runtime" }
```

- [ ] **Step 7: Write the dependency-boundary test**

Add this test to `crates/lab/tests/architecture_boundaries.rs`:

```rust
#[test]
fn lab_runtime_has_no_transport_or_heavy_runtime_dependencies() {
    let manifest = std::fs::read_to_string("../../crates/lab-runtime/Cargo.toml")
        .expect("read lab-runtime manifest");
    for banned in ["axum", "clap", "rmcp", "javy", "wasmtime", "utoipa", "labby"] {
        assert!(
            !manifest.contains(banned),
            "lab-runtime must not depend on {banned}"
        );
    }
}
```

- [ ] **Step 8: Run the first checks**

Run:

```bash
cargo check -p lab-runtime
cargo test -p lab-runtime omitted_proxy_flags_default_to_true
cargo check -p labby --all-features
cargo test -p labby --all-features architecture_boundaries
```

Expected: all commands pass.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/lab-runtime crates/lab/Cargo.toml crates/lab/src/dispatch/error.rs crates/lab/tests/architecture_boundaries.rs
git commit -m "feat: add lab-runtime gateway contracts"
```

---

### Task 2: Move Upstream Proxy Pool Into `lab-gateway`

**Reviewed-order note:** Execute Task 3 before this task. Task numbering is retained because the Beads graph uses `lab-zz6a7.2` for upstream, but the commit-safe implementation order is OAuth runtime first, then upstream pool.

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/lab-gateway/Cargo.toml`
- Create: `crates/lab-gateway/src/lib.rs`
- Move: `crates/lab/src/dispatch/upstream.rs` to `crates/lab-gateway/src/upstream.rs`
- Move: `crates/lab/src/dispatch/upstream/**` to `crates/lab-gateway/src/upstream/**`
- Modify: `crates/lab/src/dispatch/upstream.rs`
- Modify: `crates/lab/src/mcp/in_process_peer.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/tests/gateway_stdio_spawn.rs`
- Test: `crates/lab/tests/gateway_schema_resources.rs`
- Test: `crates/lab/tests/architecture_boundaries.rs`

**Interfaces:**
- Consumes:
  - `lab_runtime::gateway_config::UpstreamConfig`
  - `lab_auth::upstream::cache::OauthClientCache` from Task 3.
  - injected `InProcessConnector`.
- Produces:
  - `lab_gateway::upstream::UpstreamPool`
  - `lab_gateway::upstream::{InProcessConnector, InProcessRegistration}`

- [ ] **Step 1: Add the crate to the workspace**

Edit root `Cargo.toml` to include:

```toml
"crates/lab-gateway",
```

- [ ] **Step 2: Create `crates/lab-gateway/Cargo.toml`**

```toml
[package]
name = "lab-gateway"
description = "Standalone gateway runtime: upstream proxying, gateway manager, and Code Mode."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[dependencies]
lab-apis = { path = "../lab-apis", default-features = false }
lab-runtime = { path = "../lab-runtime" }
lab-auth = { path = "../lab-auth" }
tokio = { workspace = true, features = ["process"] }
tokio-util = { version = "0.7", features = ["rt", "compat", "codec"] }
futures.workspace = true
reqwest.workspace = true
rmcp = { workspace = true }
serde.workspace = true
serde_json = { workspace = true, features = ["preserve_order"] }
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
tempfile.workspace = true
url.workspace = true
# Code Mode (javy) lives in `lab-codemode`, added as a path dep in Task 5; not here.
process-wrap = { version = "9.1.0", default-features = false, features = ["tokio1", "process-group"] }
nix = { version = "0.31", default-features = false, features = ["signal", "process"] }
bytes = "1"
sse-stream = "0.2"
tokio-tungstenite = { version = "0.29", features = ["rustls-tls-webpki-roots"] }
dashmap = "6"
arc-swap = "1"
strip-ansi-escapes = "0.2"

[target.'cfg(windows)'.dependencies]
lab-winjob = { path = "../lab-winjob" }

[dev-dependencies]
wiremock.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Create `crates/lab-gateway/src/lib.rs`**

```rust
#![forbid(unsafe_code)]

pub mod upstream;
```

- [ ] **Step 4: Keep existing action and registry metadata until a concrete seam is required**

Do not invent `GatewayRegisteredService` in this task. Preserve the current `RegisteredService` shape through an adapter in Labby, or introduce the smallest trait only after the compiler identifies the exact required methods. Keep action metadata on the existing `lab_apis::core::ActionSpec`.

- [ ] **Step 5: Move upstream files and repair imports**

After moving files, replace imports like:

```rust
use crate::config::UpstreamConfig;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::registry::RegisteredService;
```

with:

```rust
use lab_auth::upstream::cache::OauthClientCache;
use lab_runtime::gateway_config::UpstreamConfig;
```

If this task is attempted before Task 3, stop. Do not create a temporary OAuth-disabled adapter path; protected upstream calls and Code Mode must keep subject-scoped OAuth behavior.

- [ ] **Step 6: Replace Labby upstream module with a re-export**

Create `crates/lab/src/dispatch/upstream.rs`:

```rust
pub use lab_gateway::upstream::*;
```

- [ ] **Step 7: Add a crate-boundary test**

Add a manifest/dependency gate to `crates/lab/tests/architecture_boundaries.rs`:

```rust
#[test]
fn lab_gateway_manifest_does_not_depend_on_product_surfaces() {
    let manifest = std::fs::read_to_string("../../crates/lab-gateway/Cargo.toml")
        .expect("read lab-gateway manifest");
    for banned in ["axum", "clap", "utoipa", "labby"] {
        assert!(
            !manifest.contains(banned),
            "lab-gateway runtime crate must not depend on {banned}"
        );
    }
}
```

Keep one targeted symbol scan for Labby registry fallback:

```rust
#[test]
fn lab_gateway_does_not_call_labby_default_registry() {
    let root = std::path::Path::new("../../crates/lab-gateway/src");
    let mut offenders = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.expect("walk lab-gateway");
        if entry.path().extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(entry.path()).expect("read rust file");
            if text.contains("build_default_registry") {
                offenders.push(entry.path().display().to_string());
            }
        }
    }
    assert!(offenders.is_empty(), "lab-gateway must not call build_default_registry: {offenders:?}");
}
```

Use broad source scans only for this high-value symbol; use manifests and `cargo tree` for dependency boundaries.

- [ ] **Step 8: Run upstream-focused checks**

Run:

```bash
cargo check -p lab-gateway
cargo test -p labby --all-features architecture_boundaries
cargo nextest run -p labby --all-features -E 'test(gateway_stdio)' -- --include-ignored
cargo tree -p lab-gateway -e features
cargo check -p labby --all-features
```

Expected: all commands pass; stdio tests still prove env allowlist and process cleanup; `cargo tree -p lab-gateway -e features` shows no normal Wasmtime, Axum, or Clap dependency path.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/lab-gateway crates/lab/Cargo.toml crates/lab/src/dispatch/upstream.rs crates/lab/tests/architecture_boundaries.rs crates/lab/tests/gateway_stdio_spawn.rs crates/lab/tests/gateway_schema_resources.rs
git commit -m "feat: move upstream pool into lab-gateway"
```

---

### Task 3: Move Outbound Upstream OAuth Runtime Into `lab-auth`

**Reviewed-order note:** Execute this task immediately after Task 1 and before Task 2. The upstream pool currently depends on `OauthClientCache`; moving that cache first avoids a temporary OAuth-disabled gateway path.

**Files:**
- Modify: `crates/lab-auth/Cargo.toml`
- Modify: `crates/lab-auth/src/lib.rs`
- Create: `crates/lab-auth/src/upstream.rs`
- Move: `crates/lab/src/oauth/upstream/cache.rs` to `crates/lab-auth/src/upstream/cache.rs`
- Move: `crates/lab/src/oauth/upstream/encryption.rs` to `crates/lab-auth/src/upstream/encryption.rs`
- Move: `crates/lab/src/oauth/upstream/manager.rs` to `crates/lab-auth/src/upstream/manager.rs`
- Move: `crates/lab/src/oauth/upstream/refresh.rs` to `crates/lab-auth/src/upstream/refresh.rs`
- Move: `crates/lab/src/oauth/upstream/store.rs` to `crates/lab-auth/src/upstream/store.rs`
- Move: `crates/lab/src/oauth/upstream/types.rs` to `crates/lab-auth/src/upstream/types.rs`
- Move: `crates/lab/src/oauth/upstream/runtime.rs` to `crates/lab-auth/src/upstream/runtime.rs`
- Modify: `crates/lab/src/oauth/upstream.rs`
- Modify: `crates/lab/src/dispatch/gateway/oauth.rs`
- Modify: `crates/lab/src/dispatch/gateway/oauth_lifecycle.rs`
- Modify: `crates/lab/src/dispatch/gateway/manager/oauth_resources.rs`
- Test: `crates/lab/tests/upstream_oauth.rs`
- Test: `crates/lab/src/api/upstream_oauth.rs`

**Interfaces:**
- Consumes:
  - `lab_runtime::gateway_config::{UpstreamConfig, UpstreamOauthConfig, UpstreamOauthRegistration}`
  - `lab_auth::SqliteStore`
- Produces:
  - `lab_auth::upstream::cache::OauthClientCache`
  - `lab_auth::upstream::manager::UpstreamOauthManager`
  - `lab_auth::upstream::runtime::UpstreamOauthRuntime`
  - `lab_auth::upstream::types::{UpstreamOauthError, UpstreamOauthState}`

- [ ] **Step 1: Add feature-gated rmcp support to `lab-auth`**

In `crates/lab-auth/Cargo.toml`, add:

```toml
[features]
default = []
upstream-oauth-rmcp = ["dep:rmcp-client"]

[dependencies.rmcp-client]
package = "rmcp"
version = "1.7"
default-features = false
features = ["client", "auth", "transport-streamable-http-client-reqwest"]
optional = true
```

If `lab-auth` already has a `[features]` table, merge these entries without changing existing feature names.

- [ ] **Step 2: Export the upstream module**

In `crates/lab-auth/src/lib.rs`, add:

```rust
pub mod upstream;
```

Create `crates/lab-auth/src/upstream.rs`:

```rust
pub mod cache;
pub mod encryption;
pub mod manager;
pub mod refresh;
pub mod runtime;
pub mod store;
pub mod types;
```

Update the feature line to reference the alias:

```toml
upstream-oauth-rmcp = ["dep:rmcp-client"]
```

- [ ] **Step 3: Move runtime files and replace Labby config imports**

Replace imports like:

```rust
use crate::config::{UpstreamConfig, UpstreamOauthConfig, UpstreamOauthRegistration};
```

with:

```rust
use lab_runtime::gateway_config::{
    UpstreamConfig,
    UpstreamOauthConfig,
    UpstreamOauthRegistration,
};
```

Any moved code that references rmcp should import the alias as:

```rust
use rmcp_client as rmcp;
```

Replace `axum::http::StatusCode` usage inside moved `types.rs` with transport-neutral methods:

```rust
impl UpstreamOauthError {
    #[must_use]
    pub const fn http_status_code(&self) -> u16 {
        match self {
            Self::NeedsReauth { .. } => 401,
            Self::StateInvalid { .. }
            | Self::ResourceMismatch { .. }
            | Self::IssuerMismatch { .. }
            | Self::UnsupportedMethod { .. } => 400,
            Self::Internal { .. } => 500,
        }
    }
}
```

- [ ] **Step 4: Preserve AAD encryption semantics**

Keep the upstream OAuth encryption helper with AAD. Add this focused test in `crates/lab-auth/src/upstream/encryption.rs`:

```rust
#[test]
fn aad_prevents_cross_row_token_open() {
    let key = [7_u8; 32];
    let plaintext = b"refresh-token";
    let first = aad_for_row("axon", "gateway", "client-a");
    let second = aad_for_row("axon", "other-subject", "client-a");
    let sealed = seal_with_aad(&key, plaintext, &first).expect("seal token");
    let opened = open_with_aad(&key, &sealed, &second);
    assert!(opened.is_err(), "token must not open under a different subject AAD");
}
```

- [ ] **Step 5: Add OAuth subject redaction helper**

Create a helper in `lab-auth` or `lab-runtime` and use it in moved OAuth route/manager logging:

```rust
#[must_use]
pub fn fingerprint_subject(subject: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(subject.as_bytes());
    format!("sha256:{:02x}{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2], digest[3])
}
```

Replace logs that include raw `auth.sub` with the fingerprint:

```rust
let subject_fingerprint = lab_auth::upstream::types::fingerprint_subject(auth.sub.as_str());
tracing::debug!(subject = %subject_fingerprint, "upstream oauth status requested");
```

- [ ] **Step 6: Replace Labby OAuth module with a compatibility re-export**

Create `crates/lab/src/oauth/upstream.rs`:

```rust
pub use lab_auth::upstream::*;
```

- [ ] **Step 7: Run OAuth checks**

Run:

```bash
cargo check -p lab-auth --features upstream-oauth-rmcp
cargo test -p lab-auth --features upstream-oauth-rmcp aad_prevents_cross_row_token_open
cargo test -p lab-auth --features upstream-oauth-rmcp fingerprint_subject
cargo nextest run -p labby --all-features upstream_oauth
cargo check -p lab-gateway
cargo check -p labby --all-features
```

Expected: all commands pass; OAuth tests still cover resource indicator, S256, issuer binding, dynamic registration, restart state lookup, and secret-free refresh logs.

- [ ] **Step 8: Commit**

```bash
git add crates/lab-auth crates/lab/src/oauth/upstream.rs crates/lab/src/dispatch/gateway/oauth.rs crates/lab/src/dispatch/gateway/oauth_lifecycle.rs crates/lab/src/dispatch/gateway/manager/oauth_resources.rs crates/lab/tests/upstream_oauth.rs crates/lab/src/api/upstream_oauth.rs
git commit -m "feat: move upstream oauth runtime into lab-auth"
```

---

### Task 4: Extract Gateway Web Assets Into `lab-gateway-web`

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/lab-gateway-web/Cargo.toml`
- Create: `crates/lab-gateway-web/build.rs`
- Create: `crates/lab-gateway-web/src/lib.rs`
- Create: `crates/lab-gateway-web/src/assets.rs`
- Create: `crates/lab-gateway-web/src/fs_assets.rs`
- Create: `crates/lab-gateway-web/src/response.rs`
- Modify: `crates/lab/build.rs`
- Modify: `crates/lab/src/api/web.rs`
- Modify: `.github/workflows/ci.yml`
- Test: `crates/lab/src/api/router.rs` web tests

**Interfaces:**
- Produces:
  - `lab_gateway_web::AssetSource`
  - `lab_gateway_web::AssetResponse`
  - `lab_gateway_web::serve_asset(path: &str, source: &AssetSource) -> Result<AssetResponse, AssetError>`

- [ ] **Step 1: Add the crate to the workspace**

Edit root `Cargo.toml` to include:

```toml
"crates/lab-gateway-web",
```

- [ ] **Step 2: Create `crates/lab-gateway-web/Cargo.toml`**

```toml
[package]
name = "lab-gateway-web"
description = "Gateway admin static asset embedding and serving helpers."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
build = "build.rs"

[features]
default = []

[dependencies]
serde.workspace = true
thiserror.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Move build-script asset generation**

Create `crates/lab-gateway-web/build.rs` by copying the web asset portions of `crates/lab/build.rs`. Preserve these behaviors:

```rust
const ASSET_DIR: &str = "../../apps/gateway-admin/out";
const GENERATED_FILE: &str = "embedded_web_assets.rs";

fn main() {
    let out_dir = std::env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo");
    let generated_path = std::path::PathBuf::from(out_dir).join(GENERATED_FILE);
    let asset_dir = std::path::Path::new(ASSET_DIR);
    watch_asset_tree_or_existing_parent(asset_dir);
    write_embedded_assets(asset_dir, &generated_path);
}
```

Keep `watch_asset_tree_or_existing_parent` behavior from the current Labby build script so missing `apps/gateway-admin/out` does not make every no-op Rust build stale.

- [ ] **Step 4: Create the public asset API**

Create `crates/lab-gateway-web/src/lib.rs`:

```rust
#![forbid(unsafe_code)]

pub mod assets;
pub mod fs_assets;
pub mod response;

pub use assets::{embedded_asset, embedded_asset_paths};
pub use fs_assets::AssetSource;
pub use response::{AssetError, AssetResponse, serve_asset};
```

- [ ] **Step 5: Preserve route precedence and symlink escape tests**

Move or duplicate pure asset tests from `crates/lab/src/api/router.rs` into `lab-gateway-web` where they do not require Axum or `AppState`:

```rust
#[test]
fn filesystem_assets_reject_symlink_escape() {}

#[test]
fn embedded_assets_serve_when_present() {}
```

Keep route precedence tests in `lab-gatewayd` and temporary Labby wrappers because route order is product policy, not asset lookup.

- [ ] **Step 6: Make Labby consume the web crate through a wrapper**

Keep `crates/lab/src/api/web.rs` as the Labby wrapper that owns `AppState` and node-role policy, but replace its internal filesystem/embedded lookup with `lab_gateway_web::serve_asset`:

```rust
let source = lab_gateway_web::AssetSource::from_configured_dir_or_embedded(assets_dir);
let asset = lab_gateway_web::serve_asset(request_path, &source)?;
```

Remove Labby-local `include!(concat!(env!("OUT_DIR"), "/embedded_web_assets.rs"));` after the new crate owns embedded assets.

- [ ] **Step 7: Run web checks**

Run:

```bash
cargo check -p lab-gateway-web
pnpm --dir apps/gateway-admin build
cargo test -p lab-gateway-web filesystem_assets_reject_symlink_escape
cargo test -p lab-gateway-web embedded_assets_serve_when_present
cargo test -p labby --all-features v1_routes_win_over_spa_fallback
cargo check -p labby --all-features
```

Expected: all commands pass; a missing `apps/gateway-admin/out` before `pnpm build` still produces an empty embedded asset table and does not poison every no-op Rust build.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/lab-gateway-web crates/lab/build.rs crates/lab/src/api/web.rs crates/lab/src/api/router.rs .github/workflows/ci.yml
git commit -m "feat: extract gateway web assets"
```

---

### Task 5: Extract Code Mode Into `lab-codemode`; Move GatewayManager + Dispatch Into `lab-gateway`

> **Amendment (supersedes the original fold-into-gateway approach):** Code Mode is extracted into its own client-neutral `lab-codemode` crate, NOT moved into `lab-gateway/src/code_mode/**`. The full `lab-codemode` task/step breakdown lives in the companion sub-plan [`2026-06-22-code-mode-crate-extraction.md`](2026-06-22-code-mode-crate-extraction.md). The steps below cover: (a) creating `lab-codemode` and moving the kernel + snippet engine into it (per the sub-plan), (b) adding `lab-codemode` as a `lab-gateway` dependency, and (c) implementing `CodeModeHost for GatewayManager`. `lab-gateway` exports NO `code_mode` module.

**Files:**
- Modify: `crates/lab-gateway/src/lib.rs`
- Move: `crates/lab/src/dispatch/gateway.rs` to `crates/lab-gateway/src/gateway.rs`
- Move: `crates/lab/src/dispatch/gateway/**` to `crates/lab-gateway/src/gateway/**`
- Move: `crates/lab/src/dispatch/gateway/code_mode/**` to `crates/lab-codemode/src/**` (kernel + broker + shaping — per companion sub-plan)
- Move: `crates/lab/src/dispatch/snippets/store.rs` (snippet engine) to `crates/lab-codemode/src/snippet/**`; keep the `snippets` surface in Labby as a thin adapter
- Create: `crates/lab-codemode/**` (new crate)
- Create: `crates/lab-gateway/src/code_mode_host.rs` (`impl CodeModeHost for GatewayManager`)
- Modify: `crates/lab/src/dispatch/gateway.rs`
- Modify: `crates/lab/src/cli/internal.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/api/services/gateway.rs`
- Modify: `crates/lab/src/mcp/call_tool.rs`
- Test: `crates/lab/tests/code_mode_runner.rs`
- Test: `crates/lab/tests/architecture_orchestrator.rs`

**Interfaces:**
- Consumes:
  - `lab_gateway::upstream::UpstreamPool`
  - `lab_auth::upstream::*`
  - `lab_runtime::*`
- Produces:
- `lab_gateway::gateway::{GatewayManager, GatewayManagerConfig, dispatch_with_manager, gateway_actions}`
  - `lab_codemode::{run_code_mode_runner_stdio, CodeModeBroker, CodeModeHost, ToolDescriptor, ToolScope}`
  - `lab_gateway::code_mode_host` (`impl CodeModeHost for GatewayManager`)

- [ ] **Step 1: Create `lab-codemode`, depend on it, and export gateway modules**

First create the client-neutral `lab-codemode` crate by executing the companion sub-plan `2026-06-22-code-mode-crate-extraction.md` (moves the Javy kernel, broker, shaping helpers, and snippet engine; defines the `CodeModeHost` trait; adds `crates/lab-codemode` to the workspace). Add it as a `lab-gateway` dependency in `crates/lab-gateway/Cargo.toml`:

```toml
lab-codemode = { path = "../lab-codemode" }
```

Then update `crates/lab-gateway/src/lib.rs` — `lab-gateway` exports NO `code_mode` runtime module, only the host impl:

```rust
#![forbid(unsafe_code)]

pub mod code_mode_host; // impl CodeModeHost for GatewayManager
pub mod gateway;
pub mod upstream;
```

- [ ] **Step 2: Move gateway runtime files**

After moving files, update imports:

```rust
use crate::dispatch::error::ToolError;
```

to:

```rust
use lab_runtime::error::ToolError;
```

Update upstream imports:

```rust
use crate::dispatch::upstream::UpstreamPool;
```

to:

```rust
use crate::upstream::UpstreamPool;
```

- [ ] **Step 3: Remove only default Labby registry construction from `GatewayManager`**

Do not redesign `GatewayManagerConfig` in this move. Preserve current fields such as `config_path`, service clients, runtime handle, notifier, OAuth resources, and process Code Mode flag. Make the smallest change that removes the fallback to `build_default_registry()` by requiring registry/service composition injection from the caller.

Current shape to preserve conceptually:

```rust
pub struct GatewayManagerConfig {
    pub config_path: PathBuf,
    pub registry: ToolRegistry,
    pub service_clients: SharedServiceClients,
    pub in_process_connector: Option<InProcessConnector>,
    pub oauth_runtime: Option<UpstreamOauthRuntime>,
    // Preserve remaining existing fields; do not drop them during relocation.
}
```

If `ToolRegistry` cannot move into `lab-gateway`, introduce a narrow trait that preserves the methods the current manager actually uses:

```rust
pub trait GatewayServiceRegistry: Send + Sync {
    fn service_names(&self) -> Vec<String>;
    fn get_service(&self, name: &str) -> Option<RegisteredService>;
}
```

Do not call `build_default_registry()` anywhere in `crates/lab-gateway`.

- [ ] **Step 4: Keep Labby compatibility through re-export**

Replace `crates/lab/src/dispatch/gateway.rs` with:

```rust
pub use lab_gateway::gateway::*;
```

- [ ] **Step 5: Expose the Code Mode runner entrypoint from `lab-codemode`**

The synchronous runner entrypoint lives in `lab-codemode` and keeps its return type unchanged:

```rust
// lab-codemode/src/lib.rs
pub fn run_code_mode_runner_stdio() -> std::process::ExitCode {
    runner::run_code_mode_runner_stdio()
}
```

Update `crates/lab/src/cli/internal.rs` so the Labby hidden command calls:

```rust
lab_codemode::run_code_mode_runner_stdio()
```

The runner pool's spawn command is host-configurable (program + args), defaulting to `std::env::current_exe()` + `["internal", "code-mode-runner"]`, so Labby and `lab-gatewayd` each re-invoke their own hidden subcommand. Test both paths.

- [ ] **Step 6: Drop Wasmtime entirely**

`wasm_runner.rs` is `#[cfg(test)]`-only dead code; the live runner is Javy/QuickJS over stdio. Do not carry `wasmtime` into `lab-codemode` or `lab-gateway`. In `crates/lab/Cargo.toml`, drop it from the feature and remove the dependency entry:

```toml
gateway = ["dep:javy"]
```

Neither `lab-codemode` nor `lab-gateway` declares `wasmtime`. (`javy` itself lives in `lab-codemode`; keep it in `lab` only as long as a transitional shim there references the kernel directly, then remove it.)

- [ ] **Step 7: Add boundary tests**

Add manifest and targeted-symbol checks to `crates/lab/tests/architecture_boundaries.rs`:

```rust
#[test]
fn lab_gateway_runtime_manifest_has_no_adapter_dependencies() {
    let manifest = std::fs::read_to_string("../../crates/lab-gateway/Cargo.toml")
        .expect("read lab-gateway manifest");
    for banned in ["axum", "clap", "utoipa", "labby"] {
        assert!(
            !manifest.contains(banned),
            "lab-gateway runtime crate must not depend on {banned}"
        );
    }
}

#[test]
fn lab_gateway_runtime_does_not_call_default_registry() {
    let root = std::path::Path::new("../../crates/lab-gateway/src");
    let mut offenders = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.expect("walk lab-gateway");
        if entry.path().extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(entry.path()).expect("read rust file");
            if text.contains("build_default_registry") {
                offenders.push(entry.path().display().to_string());
            }
        }
    }
    assert!(offenders.is_empty(), "lab-gateway must not call build_default_registry: {offenders:?}");
}
```

- [ ] **Step 8: Run runtime checks**

Run:

```bash
cargo check -p lab-gateway
cargo test -p labby --all-features architecture_boundaries
cargo nextest run -p labby --all-features code_mode_runner
cargo test -p labby --all-features gateway_schema_resources
cargo tree -p lab-gateway -e features
cargo check -p labby --all-features
```

Expected: all commands pass; Code Mode timeout kind remains `timeout`; runner exit codes remain intact; dead Wasmtime does not compile in the normal `gateway` feature; `cargo tree` shows no adapter dependencies.

- [ ] **Step 9: Commit**

```bash
git add crates/lab-gateway crates/lab/Cargo.toml crates/lab/src/dispatch/gateway.rs crates/lab/src/cli/internal.rs crates/lab/src/cli/serve.rs crates/lab/src/api/services/gateway.rs crates/lab/src/mcp/call_tool.rs crates/lab/tests/code_mode_runner.rs crates/lab/tests/architecture_boundaries.rs crates/lab/tests/architecture_orchestrator.rs
git commit -m "feat: move gateway runtime into lab-gateway"
```

---

### Task 6: Build Standalone `lab-gatewayd` Binary

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/lab-gatewayd/Cargo.toml`
- Create: `crates/lab-gatewayd/src/main.rs`
- Create: `crates/lab-gatewayd/src/cli.rs`
- Create: `crates/lab-gatewayd/src/config.rs`
- Create: `crates/lab-gatewayd/src/state.rs`
- Create: `crates/lab-gatewayd/src/router.rs`
- Create: `crates/lab-gatewayd/src/api_gateway.rs`
- Create: `crates/lab-gatewayd/src/api_upstream_oauth.rs`
- Create: `crates/lab-gatewayd/src/mcp_stdio.rs`
- Create: `crates/lab-gatewayd/src/mcp_http.rs`
- Create: `crates/lab-gatewayd/src/web.rs`
- Create: `crates/lab-gatewayd/tests/startup.rs`
- Create: `crates/lab-gatewayd/tests/auth.rs`
- Create: `crates/lab-gatewayd/tests/route_precedence.rs`
- Create: `crates/lab-gatewayd/tests/mcp.rs`
- Create: `crates/lab-gatewayd/tests/code_mode.rs`

**Interfaces:**
- Consumes:
  - `lab_gateway::gateway::GatewayManager`
  - `lab_gateway::upstream::UpstreamPool`
  - `lab_codemode::run_code_mode_runner_stdio`
  - `lab_auth::{AuthContext, SqliteStore}`
  - `lab_gateway_web`
- Produces:
  - binary `lab-gatewayd`
  - commands `lab-gatewayd serve`, `lab-gatewayd mcp`, `lab-gatewayd internal code-mode-runner`

- [ ] **Step 1: Add the crate to the workspace**

Edit root `Cargo.toml` to include:

```toml
"crates/lab-gatewayd",
```

- [ ] **Step 2: Create `crates/lab-gatewayd/Cargo.toml`**

```toml
[package]
name = "lab-gatewayd"
description = "Standalone Lab gateway daemon and MCP server."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[[bin]]
name = "lab-gatewayd"
path = "src/main.rs"
test = false

[dependencies]
lab-runtime = { path = "../lab-runtime" }
lab-auth = { path = "../lab-auth", features = ["upstream-oauth-rmcp"] }
lab-codemode = { path = "../lab-codemode" }
lab-gateway = { path = "../lab-gateway" }
lab-gateway-web = { path = "../lab-gateway-web" }
tokio.workspace = true
axum.workspace = true
tower.workspace = true
tower-http.workspace = true
rmcp.workspace = true
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
dotenvy.workspace = true
toml.workspace = true
url.workspace = true

[dev-dependencies]
tempfile.workspace = true

[lints]
workspace = true
```

- [ ] **Step 3: Create CLI shape**

Create `crates/lab-gatewayd/src/cli.rs`:

```rust
use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "lab-gatewayd")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Serve {
        #[arg(long, default_value = "127.0.0.1:8765")]
        bind: SocketAddr,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    Mcp {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        command: InternalCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    CodeModeRunner,
}
```

- [ ] **Step 4: Create main entrypoint**

Create `crates/lab-gatewayd/src/main.rs`:

```rust
mod api_gateway;
mod api_upstream_oauth;
mod cli;
mod config;
mod mcp_http;
mod mcp_stdio;
mod router;
mod state;
mod web;

use clap::Parser;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    let cli = cli::Cli::parse();
    if matches!(
        cli.command,
        cli::Command::Internal {
            command: cli::InternalCommand::CodeModeRunner
        }
    ) {
        return lab_codemode::run_code_mode_runner_stdio();
    }

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(?error, "lab-gatewayd failed");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: cli::Cli) -> anyhow::Result<()> {
    match cli.command {
        cli::Command::Serve { bind, config } => {
            let app_config = config::load_gateway_config(config.as_deref())?;
            let state = state::GatewayState::build(app_config).await?;
            let app = router::build_router(state)?;
            let listener = tokio::net::TcpListener::bind(bind).await?;
            axum::serve(listener, app).await?;
        }
        cli::Command::Mcp { config } => {
            let app_config = config::load_gateway_config(config.as_deref())?;
            mcp_stdio::serve_stdio(app_config).await?;
        }
        cli::Command::Internal { .. } => unreachable!("handled before async runtime work"),
    }
    Ok(())
}
```

- [ ] **Step 5: Inventory route parity before writing router code**

Create `crates/lab-gatewayd/tests/route_parity.rs` or a checked fixture that lists every Labby gateway-owned route the standalone daemon must preserve:

```text
GET  /auth/session
GET  /auth/upstream/callback/:provider
GET  /.well-known/oauth-authorization-server
GET  /.well-known/oauth-protected-resource
POST /mcp
GET  /mcp
POST /v1/gateway
GET  /v1/gateway/oauth/status
POST /v1/gateway/oauth/start
POST /v1/gateway/oauth/clear
GET  /_next/*
GET  /assets/*
GET  /*
```

For each route record: handler source, auth mode, required scope, response type, and fallback precedence. Do not implement `lab-gatewayd` router behavior from memory.

- [ ] **Step 6: Enforce auth-gated route mounting and handler-level admin checks**

In `crates/lab-gatewayd/src/router.rs`, make route construction return an error if gateway admin routes would mount without auth:

```rust
use axum::{routing::{get, post}, Router};

pub fn build_router(state: crate::state::GatewayState) -> anyhow::Result<axum::Router> {
    if !state.auth_configured() {
        anyhow::bail!("refusing to mount /v1/gateway without configured API auth");
    }

    let auth_layer = state.api_auth_layer()?;
    let protected_api = Router::new()
        .route("/v1/gateway", post(crate::api_gateway::dispatch_gateway))
        .route(
            "/v1/gateway/oauth/status",
            get(crate::api_upstream_oauth::status),
        )
        .route_layer(auth_layer);

    Ok(Router::new()
        .merge(crate::mcp_http::router(state.clone())?)
        .merge(protected_api)
        .merge(crate::api_upstream_oauth::public_callback_routes(state.clone()))
        .fallback(crate::web::serve_web)
        .with_state(state))
}
```

In `crates/lab-gatewayd/src/api_gateway.rs`, keep the same defense-in-depth pattern as Labby:

```rust
use axum::{extract::State, Extension, Json};
use lab_auth::AuthContext;

fn has_admin_scope(auth: &AuthContext) -> bool {
    auth.scopes.iter().any(|scope| scope == "lab:admin")
}

pub async fn dispatch_gateway(
    State(state): State<crate::state::GatewayState>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<lab_runtime::gateway_config::GatewayActionRequest>,
) -> Result<Json<serde_json::Value>, crate::error::ApiError> {
    if state.gateway_action_requires_admin(&req.action) && !has_admin_scope(&auth) {
        return Err(crate::error::ApiError::forbidden(format!(
            "action `{}` requires `lab:admin` scope",
            req.action
        )));
    }

    state.dispatch_gateway(req, auth).await.map(Json)
}
```

> **`GatewayActionRequest` DTO:** the standalone gateway API mirrors the existing
> MCP/HTTP `action + params` contract — `GatewayActionRequest { action: String,
> params: serde_json::Value }` (`#[serde(default)]` params). It introduces no new
> schema beyond what `lab_apis::core::ActionSpec` already governs; param
> validation stays in shared dispatch. Define it alongside the other gateway DTOs
> in `lab-runtime` (e.g. `lab_runtime::gateway_config`), not a new
> `lab_runtime::gateway` module — adjust the import path in the snippet above
> accordingly.

- [ ] **Step 7: Preserve daemon-scoped gateway state**

`GatewayState` must own one long-lived manager/pool pair:

```rust
#[derive(Clone)]
pub struct GatewayState {
    gateway: std::sync::Arc<lab_gateway::gateway::GatewayManager>,
    upstreams: std::sync::Arc<lab_gateway::upstream::UpstreamPool>,
    oauth_cache: std::sync::Arc<lab_auth::upstream::cache::OauthClientCache>,
}
```

Do not construct `GatewayManager`, `UpstreamPool`, OAuth caches, Code Mode runner pools, or asset watchers per request.

- [ ] **Step 8: Split stdio MCP and HTTP MCP trust boundaries**

`mcp_stdio::serve_stdio` may construct trusted-local contexts for local operator use. `mcp_http::router` must always run behind API auth and must pass a real `AuthContext` into MCP execution. It must never call admin/destructive MCP paths with `None` auth.

Add tests for:
- no bearer token: HTTP MCP rejects before dispatch
- `lab:read` token: read-only MCP calls work, admin/destructive actions fail
- `lab:admin` token: admin gateway actions work
- stdio MCP still supports trusted-local execution without HTTP bearer auth

- [ ] **Step 9: Add startup/auth tests**

Create `crates/lab-gatewayd/tests/auth.rs`:

```rust
#[test]
fn router_refuses_gateway_admin_without_auth() {
    let config = lab_gatewayd::config::GatewayDaemonConfig::for_test_without_auth();
    let result = lab_gatewayd::state::GatewayState::build_for_test(config)
        .and_then(lab_gatewayd::router::build_router);
    let err = result.expect_err("router must reject unauthenticated gateway admin mount");
    assert!(
        err.to_string().contains("refusing to mount /v1/gateway"),
        "unexpected error: {err}"
    );
}
```

If `lab-gatewayd` needs a library target for integration tests, add:

```toml
[lib]
path = "src/lib.rs"
```

and move module declarations from `main.rs` into `src/lib.rs`.

- [ ] **Step 10: Add route precedence and cache reuse tests**

Add route precedence tests proving these paths never fall through to the SPA:
- `/auth/session`
- `/auth/upstream/callback/:provider`
- `/.well-known/oauth-authorization-server`
- `/.well-known/oauth-protected-resource`
- `/mcp`
- `/v1/gateway`
- `/v1/gateway/oauth/status`

Add state reuse tests proving repeated calls reuse:
- OAuth subject connection cache
- relay session connection cache
- Code Mode runner pool
- asset watcher state

- [ ] **Step 11: Run standalone checks**

Run:

```bash
cargo check -p lab-gatewayd
cargo test -p lab-gatewayd router_refuses_gateway_admin_without_auth
cargo test -p lab-gatewayd startup_smoke_binds_health_and_shuts_down
cargo test -p lab-gatewayd route_precedence
cargo test -p lab-gatewayd http_mcp_auth_boundaries
cargo test -p lab-gatewayd daemon_scoped_caches_are_reused
cargo check -p labby --all-features
```

Expected: the check and tests pass; the startup smoke spawns a server on port `0`, probes health/readiness, and tears it down inside the test.

- [ ] **Step 12: Commit**

```bash
git add Cargo.toml crates/lab-gatewayd
git commit -m "feat: add standalone gateway daemon"
```

---

### Task 7: Detach or Mark Labby Gateway Shims

**Files:**
- Modify: `crates/lab/src/cli/gateway.rs`
- Modify: `crates/lab/src/cli/gateway/**`
- Modify: `crates/lab/src/api/services/gateway.rs`
- Modify: `crates/lab/src/api/upstream_oauth.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/mcp/**`
- Modify: `crates/lab/src/registry.rs`
- Modify: `plugins/labby/**`
- Modify: `docs/services/GATEWAY.md`
- Modify: `docs/surfaces/MCP.md`
- Modify: `docs/surfaces/TRANSPORT.md`

**Interfaces:**
- Consumes: `lab-gatewayd` canonical surfaces.
- Produces: Labby forwarding shims or removed gateway adapters, with removal tracked in Beads.

- [ ] **Step 1: Inventory remaining Labby gateway imports**

Run:

```bash
rg -n "dispatch::gateway|dispatch::upstream|oauth::upstream|api::web|GatewayManager|UpstreamPool|code-mode-runner" crates/lab plugins/labby docs
```

Expected: every remaining hit is either a compatibility shim, a test that is still Labby-specific, or documentation pointing to `lab-gatewayd`.

- [ ] **Step 2: Mark temporary shims in code**

For each Labby shim that must stay, add this exact comment above the shim entrypoint:

```rust
// Temporary gateway compatibility shim. Canonical implementation lives in
// lab-gatewayd/lab-gateway; remove this after lab-zz6a7 standalone parity closes.
```

- [ ] **Step 3: Track compatibility shim removal explicitly**

Create or update a Bead for shim removal after standalone parity closes:

```bash
bd create --title "Remove Labby gateway compatibility shims after lab-gatewayd parity" \
  --description "After lab-zz6a7 standalone gateway parity is validated, remove temporary Labby gateway CLI/API/MCP forwarding shims and make lab-gatewayd the only canonical gateway binary." \
  --type task \
  --priority 2
```

- [ ] **Step 4: Remove Labby default registry ownership from gateway runtime**

In `crates/lab/src/registry.rs`, keep Labby service registration for Labby only. Do not let `lab-gateway` call:

```rust
build_default_registry()
```

Add a test in `crates/lab/tests/architecture_boundaries.rs`:

```rust
#[test]
fn lab_gateway_does_not_reference_labby_default_registry() {
    let text = std::fs::read_to_string("../../crates/lab-gateway/src/gateway/manager/core.rs")
        .expect("read gateway manager core");
    assert!(
        !text.contains("build_default_registry"),
        "standalone gateway runtime must receive registry composition by injection"
    );
}
```

- [ ] **Step 5: Update docs to name canonical binary**

In gateway docs, use this wording:

```markdown
`lab-gatewayd` is the canonical standalone gateway binary. `labby` gateway
commands and routes, when present, are compatibility shims during the
lab-zz6a7 migration.
```

- [ ] **Step 6: Run compatibility checks**

Run:

```bash
cargo check -p lab-gatewayd
cargo check -p labby --all-features
cargo test -p labby --all-features architecture_boundaries
```

Expected: standalone and Labby compatibility both compile; boundary tests reject long-term Labby ownership from `lab-gateway`.

- [ ] **Step 7: Commit**

```bash
git add crates/lab plugins/labby docs
git commit -m "refactor: detach labby gateway ownership"
```

---

### Task 8: Final Parity, Benchmarks, and CI

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `docs/dev/ERRORS.md`
- Modify: `docs/surfaces/MCP.md`
- Modify: `docs/surfaces/TRANSPORT.md`
- Modify: `docs/generated/**`
- Modify: `crates/lab/tests/architecture_boundaries.rs`
- Modify: `crates/lab-gatewayd/tests/**`
- Modify: `crates/lab-gateway/tests/**`
- Modify: `crates/lab-gateway-web/tests/**`

**Interfaces:**
- Consumes: completed crate extraction and standalone binary.
- Produces: repeatable closeout proof and updated CI gates.

- [ ] **Step 1: Add CI checks for new crates**

In `.github/workflows/ci.yml`, add commands:

```yaml
- name: Check gateway runtime crates
  run: |
    cargo check -p lab-runtime
    cargo check -p lab-auth --features upstream-oauth-rmcp
    cargo check -p lab-gateway
    cargo check -p lab-gateway-web
    cargo check -p lab-gatewayd
    cargo tree -p lab-gateway -e features
```

- [ ] **Step 2: Run full standalone parity commands**

Run:

```bash
cargo check -p lab-runtime
cargo check -p lab-auth --features upstream-oauth-rmcp
cargo check -p lab-gateway
cargo test -p lab-gateway
cargo check -p lab-gateway-web
cargo check -p lab-gatewayd
cargo test -p lab-gatewayd
pnpm --dir apps/gateway-admin build
cargo fmt --all --check
cargo check -p labby --all-features
cargo tree -p lab-gateway -e features
```

Expected: all commands pass; `cargo tree -p lab-gateway -e features` shows no normal-path `wasmtime`, `axum`, or `clap` dependency.

- [ ] **Step 3: Measure no-op build timings**

Run each command twice and record the second run:

```bash
/usr/bin/time -f 'lab-gateway noop %e' cargo check -p lab-gateway
/usr/bin/time -f 'lab-gatewayd noop %e' cargo check -p lab-gatewayd
/usr/bin/time -f 'labby all-features noop %e' cargo check -p labby --all-features
```

Expected: record these as standalone crate timings only. Do not present standalone numbers as a direct Labby speedup unless the comparison command and cache state are also recorded.

- [ ] **Step 4: Measure gateway source-edit timing**

Run:

```bash
touch crates/lab-gateway/src/gateway/catalog.rs
/usr/bin/time -f 'lab-gateway source edit %e' cargo check -p lab-gateway
touch crates/lab-gateway/src/gateway/catalog.rs
/usr/bin/time -f 'lab-gatewayd after runtime edit %e' cargo check -p lab-gatewayd
```

Expected: record both numbers in the epic comment and compare them against fresh same-cache `labby --features gateway` and Labby all-features source-edit measurements, not only the old `2:07.60` and `2:39.25` snapshot.

- [ ] **Step 5: Verify standalone route and auth behavior**

Run the bounded startup/auth smoke test:

```bash
cargo test -p lab-gatewayd startup_smoke_binds_health_and_shuts_down
cargo test -p lab-gatewayd route_precedence
cargo test -p lab-gatewayd http_mcp_auth_boundaries
```

Expected:
- `/health` returns `200`.
- unauthenticated `/v1/gateway` returns `401` or `405`, not a successful admin response.
- authenticated `/auth/session` returns JSON with `sub: "static-bearer"` and an admin scope list containing `lab:admin`.
- `/auth/session`, `/auth/upstream/callback`, `/.well-known/*`, `/mcp`, and protected API routes do not fall through to the SPA fallback.

- [ ] **Step 6: Verify MCP initialize with a bounded stdio smoke**

Run the repo smoke helper or a test that spawns `lab-gatewayd mcp`, sends `initialize` and `tools/list`, then kills the child on timeout:

```bash
cargo test -p lab-gatewayd stdio_mcp_initialize_and_tools_list
```

Expected: the gateway MCP tool is visible, admin-scoped actions are enforced, stdio trust semantics are documented, and the test tears down the child process even on failure.

- [ ] **Step 7: Verify cache and watcher reuse**

Run:

```bash
cargo test -p lab-gatewayd daemon_scoped_caches_are_reused
cargo test -p lab-gatewayd asset_watcher_invalidates_on_rebuild
cargo test -p lab-gateway code_mode_runner_pool_reuse
cargo test -p lab-auth --features upstream-oauth-rmcp oauth_subject_connection_cache_reuse
```

Expected: repeated daemon calls reuse OAuth subject connections, relay sessions, Code Mode runner pool state, and the asset watcher while still invalidating changed assets.

- [ ] **Step 8: Add an epic closeout comment**

Run:

```bash
bd comment lab-zz6a7 --stdin <<'EOF'
CLOSEOUT: standalone gateway extraction validated. Recorded checks: cargo check -p lab-runtime, lab-auth with upstream OAuth, lab-gateway, lab-gateway-web, lab-gatewayd; cargo test -p lab-gateway and lab-gatewayd; route precedence and HTTP MCP auth boundaries; daemon cache reuse; pnpm gateway-admin build; cargo fmt; Labby all-features compatibility; cargo tree feature audit for lab-gateway. Recorded timings: lab-gateway no-op, lab-gatewayd no-op, lab-gateway source edit, lab-gatewayd after runtime edit, and fresh same-cache Labby compatibility measurements. Remaining Labby shims are tracked separately.
EOF
```

- [ ] **Step 9: Commit**

```bash
git add .github/workflows/ci.yml docs crates/lab/tests crates/lab-gateway/tests crates/lab-gateway-web/tests crates/lab-gatewayd/tests
git commit -m "test: validate standalone gateway extraction"
```

---

## Self-Review

**Spec coverage:** This plan covers all epic children: contracts (`lab-zz6a7.1`), upstream pool (`.2`), OAuth into `lab-auth` (`.3`), gateway runtime (`.4`) with Code Mode extracted to the client-neutral `lab-codemode` crate via the companion sub-plan (not folded into `lab-gateway`), standalone binary and Labby detach (`.5`), final validation (`.6`), and web assets (`.7`).

**Placeholder scan:** The plan avoids undefined crate names by choosing `lab-runtime`, `lab-gateway`, `lab-gateway-web`, and `lab-gatewayd`. It does not leave feature names or command names undecided.

**Type consistency:** Public interface names are consistent across tasks: `ToolError`, `UpstreamConfig`, `OauthClientCache`, `UpstreamOauthRuntime`, `GatewayManager`, `UpstreamPool`, `CodeModeHost`, `ToolDescriptor`, `ToolScope`, `AssetSource`, and `lab-gatewayd internal code-mode-runner`.
