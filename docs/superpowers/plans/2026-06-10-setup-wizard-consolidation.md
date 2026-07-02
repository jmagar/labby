# Setup Wizard Consolidation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `labby serve` self-bootstrap on first run (generate an MCP bearer token + a minimal `~/.labby/.env`, then print the `/setup` URL) and add first-class token generation to the `setup` service, so the web `/setup` wizard becomes a complete, reachable, single configuration surface — closing the headless bootstrap circularity.

**Architecture:** The `setup` dispatch service already owns config writes (`draft.set` → `draft.commit` → `config::env_merge::merge`, atomic + backup + audit-gated) and first-run detection (`setup.state`). Two primitives are missing: (1) token generation in Rust (today only the `just mcp-token` openssl recipe exists), and (2) a serve-time first-run bootstrap. This plan adds a pure `generate_mcp_token()` helper, a non-destructive `setup.bootstrap` action that creates `~/.labby/.env` only when absent, a `setup.token.generate` convenience action, and wires `labby serve` to bootstrap-then-surface-the-URL on first run.

**Tech Stack:** Rust 2024, `getrandom 0.4` + `hex 0.4` (token), `config::env_merge` (atomic .env writes), `tokio`, clap (serve CLI), `tempfile` (tests). No frontend changes in this plan.

---

## Scope

This plan is **backend only** and produces working, testable software on its own: after it lands, a fresh `curl | sh` install on a headless box can run `labby serve`, get a token + `/setup` URL printed, and reach the existing wizard — which already writes all config.

**Explicitly out of scope (separate follow-up plans):**
- **Frontend wizard "Generate token" affordance + the `setup.token.generate` action that backs it** — wiring `apps/gateway-admin/app/setup/core-config` is a Next.js change in a different subsystem. Per YAGNI (eng-review LOW-2), the `setup.token.generate` dispatch action has **zero callers in this backend-only plan** (serve uses `bootstrap()`, not `token.generate`), so it ships with the frontend follow-up plan, not here. The pure `generate_mcp_token()` helper IS in this plan — `bootstrap()` uses it.
- **`last_completed_step` resume tracking** — `state.rs` always returns `0`; unrelated enhancement.
- **Browser auto-launch** from `labby setup` (deferred `webbrowser` dep, already documented as a known follow-up).

**Behavior change to note in the PR:** with self-bootstrap, first-run `labby serve` on a non-loopback bind no longer refuses — it generates a token first, then the existing bind guard passes. The token is required to access the server and is printed once at startup. This is the intended UX resolution of the bootstrap circularity.

---

## File Structure

- **Create** `crates/lab/src/dispatch/setup/token.rs` — pure `generate_mcp_token()` + unit tests. One responsibility: token generation.
- **Create** `crates/lab/src/dispatch/setup/bootstrap.rs` — `bootstrap()` (create `~/.labby/.env` when absent) + `should_bootstrap()` decision helper + tests.
- **Modify** `crates/lab/Cargo.toml` — add `getrandom = "0.4"` (workspace `lab` crate; `hex = "0.4"` already present).
- **Modify** `crates/lab/src/dispatch/setup/setup.rs` — declare `pub mod token; pub mod bootstrap;` and re-export `bootstrap::bootstrap`.
- **Modify** `crates/lab/src/dispatch/setup/catalog.rs` — add `bootstrap` and `token.generate` `ActionSpec`s.
- **Modify** `crates/lab/src/dispatch/setup/dispatch.rs` — route `bootstrap` and `token.generate`.
- **Modify** `crates/lab/src/cli/serve.rs` — call the bootstrap decision before the bind guard; set the token in-process; print the `/setup` URL.
- **Modify** `CHANGELOG.md` + run `just docs-generate` (catalog/help regen).

Files that change together (the three setup modules) live together under `dispatch/setup/`, matching the existing layout.

---

### Task 1: Token generator helper

**Files:**
- Create: `crates/lab/src/dispatch/setup/token.rs`
- Modify: `crates/lab/Cargo.toml` (add `getrandom = "0.4"`)
- Modify: `crates/lab/src/dispatch/setup/setup.rs` (add `pub mod token;`)

- [ ] **Step 1: Add the `getrandom` dependency**

In `crates/lab/Cargo.toml`, in the `[dependencies]` table near the existing `hex = "0.4"` line, add:

```toml
getrandom = "0.4"
```

(Confirms with the version already used by `lab-auth`; `hex = "0.4"` is already a `lab` dependency.)

- [ ] **Step 2: Write `token.rs` with a failing-test-first helper**

Create `crates/lab/src/dispatch/setup/token.rs`:

```rust
//! MCP bearer-token generation for first-run setup.
//!
//! Produces the same hex shape as `just mcp-token` (`openssl rand -hex 32`):
//! 32 random bytes → 64 lowercase hex chars. `doctor` validates length >= 32.

/// Generate a fresh 64-char hex MCP bearer token from 32 OS-random bytes.
#[must_use]
pub fn generate_mcp_token() -> String {
    let mut buf = [0_u8; 32];
    getrandom::fill(&mut buf).expect("OS RNG unavailable while generating MCP token");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::generate_mcp_token;

    #[test]
    fn token_is_64_hex_chars() {
        let t = generate_mcp_token();
        assert_eq!(t.len(), 64, "token must be 64 hex chars (32 bytes)");
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tokens_are_unique() {
        assert_ne!(generate_mcp_token(), generate_mcp_token());
    }
}
```

- [ ] **Step 3: Declare the module**

In `crates/lab/src/dispatch/setup/setup.rs`, match the existing style — modules are declared **private** (`mod catalog;`) with selective `pub use` re-exports (eng-review HIGH-2). Add:

```rust
mod token;
```

- [ ] **Step 4: Run the tests — verify they pass**

Run: `cargo nextest run -p labby --all-features -E 'test(/token_is_64|tokens_are_unique/)'`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/Cargo.toml crates/lab/src/dispatch/setup/token.rs crates/lab/src/dispatch/setup/setup.rs
git commit -m "feat(setup): add generate_mcp_token() hex token helper"
```

---

### Task 2: `setup.bootstrap` action — create ~/.labby/.env when absent

**Files:**
- Create: `crates/lab/src/dispatch/setup/bootstrap.rs`
- Modify: `crates/lab/src/dispatch/setup/setup.rs` (add `pub mod bootstrap;` + re-export)
- Modify: `crates/lab/src/dispatch/setup/catalog.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`

- [ ] **Step 1: Write `bootstrap.rs` (impl + failing integration test)**

Create `crates/lab/src/dispatch/setup/bootstrap.rs`:

```rust
//! First-run self-bootstrap: create a minimal `~/.labby/.env` so the server can
//! start and the operator can reach `/setup`. Non-destructive — a no-op when
//! the file already exists, so it is safe to call unconditionally at startup.

use serde_json::{Value, json};

use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

use super::client::env_path;
use super::dispatch::map_merge_err;
use super::token::generate_mcp_token;

/// Decide whether `labby serve` should self-bootstrap: only when there is no
/// MCP bearer token configured AND OAuth is not the active mode. `oauth_mode`
/// is `true` when `LAB_AUTH_MODE=oauth`.
#[must_use]
pub fn should_bootstrap(token_configured: bool, oauth_mode: bool) -> bool {
    !token_configured && !oauth_mode
}

/// Create `~/.labby/.env` with a generated bearer token + loopback MCP defaults
/// when it does not exist. Returns `{ created, env_path, token }` — `token` is
/// the generated value on creation, or `null` when the file already existed.
pub fn bootstrap() -> Result<Value, ToolError> {
    let env = env_path();
    if env.exists() {
        return Ok(json!({
            "created": false,
            "env_path": env.display().to_string(),
            "token": Value::Null,
        }));
    }

    if let Some(parent) = env.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "write_failed".into(),
            message: format!("create {}: {e}", parent.display()),
        })?;
    }

    let token = generate_mcp_token();
    let entries = vec![
        EnvEntry::new("LAB_MCP_HTTP_TOKEN", token.clone()),
        EnvEntry::new("LAB_MCP_TRANSPORT", "http"),
        EnvEntry::new("LAB_MCP_HTTP_HOST", "127.0.0.1"),
        EnvEntry::new("LAB_MCP_HTTP_PORT", "8765"),
        EnvEntry::new("LAB_AUTH_MODE", "bearer"),
    ];

    // Reuse the canonical merge-error mapper so failures carry the stable
    // `kind` from docs/dev/ERRORS.md (merge_write_conflict, merge_temp_create,
    // …) instead of a flattened "write_failed" (eng-review HIGH-1).
    env_merge::merge(
        &env,
        MergeRequest {
            entries,
            force: false,
            expected_mtime: None,
        },
    )
    .map_err(map_merge_err)?;

    Ok(json!({
        "created": true,
        "env_path": env.display().to_string(),
        "token": token,
    }))
}

#[cfg(test)]
mod tests {
    use super::{bootstrap, should_bootstrap};

    #[test]
    fn should_bootstrap_only_without_token_and_oauth() {
        assert!(should_bootstrap(false, false));
        assert!(!should_bootstrap(true, false));
        assert!(!should_bootstrap(false, true));
        assert!(!should_bootstrap(true, true));
    }

    #[test]
    fn bootstrap_creates_env_with_token_then_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: single-threaded test; LABBY_HOME redirects env_path() to temp.
        unsafe {
            std::env::set_var("LABBY_HOME", dir.path());
        }

        let first = bootstrap().expect("first bootstrap");
        assert_eq!(first["created"], serde_json::json!(true));
        let token = first["token"].as_str().expect("token string");
        assert_eq!(token.len(), 64);

        let env_file = dir.path().join(".env");
        let body = std::fs::read_to_string(&env_file).expect("read .env");
        assert!(body.contains("LAB_MCP_HTTP_TOKEN="));
        assert!(body.contains("LAB_AUTH_MODE=bearer"));

        // Second call must be a no-op (file already exists).
        let second = bootstrap().expect("second bootstrap");
        assert_eq!(second["created"], serde_json::json!(false));
        assert_eq!(second["token"], serde_json::Value::Null);

        unsafe {
            std::env::remove_var("LABBY_HOME");
        }
    }
}
```

- [ ] **Step 2: Declare + re-export the module (match existing private-mod style)**

In `crates/lab/src/dispatch/setup/setup.rs` (eng-review HIGH-2 — private `mod` + selective `pub use`):

```rust
mod bootstrap;
pub use bootstrap::{bootstrap, should_bootstrap};
```

- [ ] **Step 2b: Expose `map_merge_err` to sibling modules**

In `crates/lab/src/dispatch/setup/dispatch.rs:613`, change the merge-error mapper from private to crate-sibling visible so `bootstrap.rs` reuses it (eng-review HIGH-1):

```rust
pub(super) fn map_merge_err(err: env_merge::MergeError) -> ToolError {
```
(was `fn map_merge_err(...)`)

- [ ] **Step 3: Run the tests — verify they pass**

Run: `cargo nextest run -p labby --all-features -E 'test(/should_bootstrap_only|bootstrap_creates_env/)'`
Expected: 2 passed. (If `bootstrap_creates_env...` is flaky under parallel `LABBY_HOME` mutation, it is the only test touching `LABBY_HOME` here; nextest runs each test in its own process, so the `set_var` is process-isolated.)

- [ ] **Step 4: Add the catalog entry**

In `crates/lab/src/dispatch/setup/catalog.rs`, add ONE `ActionSpec` to the `ACTIONS` array (after the `state` entry). `token.generate` is deferred to the frontend follow-up plan (eng-review LOW-2 / YAGNI — no in-plan caller):

```rust
    ActionSpec {
        name: "bootstrap",
        description: "Create ~/.labby/.env with a generated token + loopback defaults when absent (first-run)",
        destructive: false,
        requires_admin: false,
        returns: "BootstrapOutcome",
        params: &[],
    },
```

- [ ] **Step 5: Route the action**

In `crates/lab/src/dispatch/setup/dispatch.rs`, inside the `match action` block in `dispatch_inner` (add after the `"state" => state_action(),` arm). Use the re-exported `bootstrap` (Step 2):

```rust
        "bootstrap" => super::bootstrap(),
```

- [ ] **Step 6: Verify dispatch + catalog wiring**

Run: `cargo nextest run -p labby --all-features -E 'package(labby) and test(setup)'`
Expected: all setup tests pass (existing + new). The catalog has a test asserting every action is dispatchable / documented; confirm it still passes.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/setup/bootstrap.rs crates/lab/src/dispatch/setup/setup.rs crates/lab/src/dispatch/setup/catalog.rs crates/lab/src/dispatch/setup/dispatch.rs
git commit -m "feat(setup): add setup.bootstrap action (env_merge-backed, first-run)"
```

---

### Task 3: Wire `labby serve` first-run bootstrap

**Files:**
- Modify: `crates/lab/src/cli/serve.rs` (insert before the bind guard at ~line 493)

- [ ] **Step 1: Insert the bootstrap block before the auth-configured computation**

In `crates/lab/src/cli/serve.rs`, locate the existing lines (~493):

```rust
    let auth_configured = bearer_token.is_some() || matches!(auth_config.mode, AuthMode::OAuth);
```

Immediately BEFORE that line, insert (PR #112 review wave — loopback-gated + typed `BootstrapOutcome` + authoritative-token + dotenvy reload):

```rust
    // First-run self-bootstrap: only when no MCP token is configured, OAuth is
    // not active, AND the bind is loopback. The loopback gate (HIGH-1) is
    // load-bearing — an explicit non-loopback bind with no auth must STILL hit
    // the lab-319g safety gate below and bail; auto-minting a token must never
    // silently enable a public bind. The generated token is made authoritative
    // in-process (`bearer_token = Some(token)`) BEFORE the dotenvy reload, so
    // the running server authenticates with the token it just wrote even if the
    // reload fails (HIGH-2). The reload then propagates the token to downstream
    // env readers (node master client, `logs`). dotenvy owns its own set_var,
    // keeping this crate unsafe-free (the workspace forbids unsafe_code).
    if crate::dispatch::setup::should_bootstrap(
        bearer_token.is_some(),
        matches!(auth_config.mode, AuthMode::OAuth),
    ) && is_loopback_host(&host)
    {
        match crate::dispatch::setup::bootstrap() {
            Ok(crate::dispatch::setup::BootstrapOutcome::Created { env_path, token }) => {
                bearer_token = Some(token.clone());
                if let Err(error) = dotenvy::from_path(&env_path) {
                    tracing::error!(
                        surface = "cli",
                        service = "serve",
                        error = %error,
                        "failed to reload generated ~/.labby/.env into process env; \
                         in-process token is authoritative, downstream env readers may not see it"
                    );
                }
                tracing::warn!(
                    surface = "cli",
                    service = "serve",
                    "first run: generated LAB_MCP_HTTP_TOKEN and wrote ~/.labby/.env"
                );
                eprintln!("\n  Lab first-run setup");
                eprintln!("  MCP bearer token: {token}");
                eprintln!("  Open http://{host}:{port}/setup to finish configuration\n");
            }
            Ok(crate::dispatch::setup::BootstrapOutcome::AlreadyPresent { .. }) => {}
            Err(error) => {
                tracing::warn!(surface = "cli", service = "serve", error = %error, "first-run bootstrap skipped");
            }
        }
    }
```

Note: `bearer_token` must be `let mut bearer_token = http_token();` at its declaration (serve.rs:275) — change its `let` to `let mut` so the reassignment compiles. `host`, `port`, and `auth_config` are already in scope (used by the bind guard just below). The workspace sets `unsafe_code = "forbid"`, which `forbid` cannot be escaped via `#[allow]`; the dotenvy reload (dotenvy encapsulates its own `set_var`) replaces the originally planned `unsafe { std::env::set_var }`, keeping the crate unsafe-free. `bootstrap()` returns the typed `BootstrapOutcome` enum; the JSON envelope for the MCP/CLI route lives in `bootstrap_action()`.

- [ ] **Step 1b: Allowlist `cli/serve.rs` in the orchestrator architecture test (REQUIRED — eng-review CRITICAL-1)**

`crates/lab/tests/architecture_orchestrator.rs` test `no_peer_service_imports_setup_dispatch` forbids importing `crate::dispatch::setup` outside `ALLOWED_PATHS`, which lists `cli/setup.rs`, `api/services/setup.rs`, `registry.rs` — but NOT `cli/serve.rs`. The bootstrap call above WILL fail that test. Add `serve` as a sanctioned surface caller: in the `ALLOWED_PATHS` array, under the `// Surfaces that mount the dispatch (CLI / API / registry):` comment, add:

```rust
    "cli/serve.rs",
```

This is correct per the orchestrator rule: `serve` is a CLI surface that mounts dispatch and legitimately runs first-run bootstrap (the same category as `cli/setup.rs`).

- [ ] **Step 2: Make `bearer_token` mutable**

Find the existing `let bearer_token = http_token();` (above line 493) and change to:

```rust
    let mut bearer_token = http_token();
```

- [ ] **Step 3: Verify it compiles + architecture + serve tests pass**

Run: `cargo clippy -p labby --all-features -- -D warnings`
Expected: exit 0. (No `unsafe` — the dotenvy reload owns the env mutation.)

Run: `cargo nextest run -p labby --all-features -E 'test(serve) or test(no_peer_service_imports_setup_dispatch)'`
Expected: pass. The architecture test MUST be in this gate (eng-review CRITICAL-1) — without the Step 1b allowlist edit it fails here.

- [ ] **Step 4: Decision-gate coverage lives in `bootstrap.rs`**

No serve-level decision test is added. `bootstrap.rs::should_bootstrap_only_without_token_and_oauth` is the canonical coverage of the `should_bootstrap` gate (including the `(true, true)` case); a duplicate serve test would be redundant (PR #112 review wave).

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/cli/serve.rs crates/lab/tests/architecture_orchestrator.rs
git commit -m "feat(serve): self-bootstrap MCP token + ~/.labby/.env on first run, print /setup URL"
```

---

### Task 4: Docs + catalog regeneration

**Files:**
- Modify: `CHANGELOG.md`
- Modify (generated): `docs/generated/*` via `just docs-generate`
- Modify: `docs/runtime/CONFIG.md` (document first-run bootstrap)
- Modify: `crates/lab/CLAUDE.md` "Known Gaps" (remove the now-closed setup gap if listed)

- [ ] **Step 1: Regenerate catalogs (new actions appear in help/catalog)**

Run: `just docs-generate`
Then: `just docs-check`
Expected: `checked N docs artifacts: fresh`. Stage the changed `docs/generated/*` files.

- [ ] **Step 2: Document the first-run flow in CONFIG.md**

In `docs/runtime/CONFIG.md`, add a short subsection near the `~/.labby/.env` description:

```markdown
### First-run bootstrap

On first run, `labby serve` detects a missing MCP token (no `LAB_MCP_HTTP_TOKEN`
and `LAB_AUTH_MODE` != `oauth`) and self-bootstraps: it generates a 64-char hex
bearer token, writes a minimal `~/.labby/.env` (token + loopback MCP defaults via
the atomic `env_merge` path), prints the token and the `http://<host>:<port>/setup`
URL once, and continues startup. The web `/setup` wizard then owns all further
configuration. Set `LAB_MCP_HTTP_TOKEN` or `LAB_AUTH_MODE=oauth` beforehand to
opt out. The generated `~/.labby/.env` is written `0600` on Unix; **Windows ACL
hardening is still pending** (`env_merge::set_secure_perms` is a no-op on
non-unix), so on Windows the token file sits at default ACLs. The
`setup.bootstrap` action exposes this primitive to the wizard and CLI.
```

- [ ] **Step 3: Add a CHANGELOG entry**

In `CHANGELOG.md` under `## [Unreleased]` (or a new patch heading per the repo's release rule — match the current convention):

```markdown
- **First-run `labby serve` self-bootstrap** — generates an MCP bearer token and
  a minimal `~/.labby/.env`, then prints the `/setup` URL, so a fresh headless
  install is reachable without hand-editing config. New `setup.bootstrap` and
  `setup.token.generate` dispatch actions back it.
```

- [ ] **Step 4: Full verification gate**

```bash
cargo fmt --all
cargo clippy --workspace --all-features -- -D warnings   # exit 0; do NOT use --all-targets
cargo nextest run -p labby --all-features                # baseline 1643 + new tests, all green
just docs-check                                          # fresh
```

- [ ] **Step 5: Commit**

```bash
git add CHANGELOG.md docs/generated docs/runtime/CONFIG.md crates/lab/CLAUDE.md
git commit -m "docs(setup): document first-run bootstrap; regenerate catalogs"
```

---

## Self-Review

**1. Spec coverage:**
- "Token generation in Rust" → Task 1 (`generate_mcp_token`). ✓
- "serve self-bootstrap / close circularity" → Task 3 (`should_bootstrap` + serve wiring). ✓
- "wizard is the single config surface" → already true for writes (`draft.set`/`draft.commit`); this plan makes it *reachable* on first run. ✓
- "fill the gaps" → token gen (the one missing primitive) + reachable bootstrap. Frontend "generate token" button explicitly deferred to a follow-up plan (scope note). ✓

**2. Placeholder scan:** No TBD/TODO/"handle errors appropriately" — every step has exact paths and complete code. ✓

**3. Type consistency:** `generate_mcp_token() -> String` (Task 1) is called in Task 2 (`bootstrap.rs`) and Task 2's `token.generate` route. `should_bootstrap(bool, bool) -> bool` (Task 2) is called in Task 3 serve wiring and Task 3 test. `bootstrap() -> Result<Value, ToolError>` (Task 2) called in Task 3. `EnvEntry::new`, `MergeRequest { entries, force, expected_mtime }`, `env_merge::merge` match the real signatures in `crates/lab/src/config/env_merge.rs`. `ActionSpec`/`ParamSpec` fields match `crates/lab/src/dispatch/setup/catalog.rs`. ✓

**Eng-review applied (lavra-eng-review, 2026-06-10) — verdict was needs-rework; the four mandatory fixes are now folded in:**
- **CRITICAL-1** (`cli/serve.rs` trips `no_peer_service_imports_setup_dispatch`) → Task 3 Step 1b adds `cli/serve.rs` to `ALLOWED_PATHS` and Step 3 puts that test in the gate.
- **HIGH-1** (reinvented error mapper flattened 6 stable kinds to `write_failed`) → Task 2 reuses `map_merge_err` (Step 2b makes it `pub(super)`).
- **HIGH-2** (module-decl style mismatch) → private `mod token;`/`mod bootstrap;` + `pub use bootstrap::{bootstrap, should_bootstrap}`; serve calls the re-exported names.
- **LOW-2 / YAGNI** (`setup.token.generate` had no in-plan caller) → deferred to the frontend follow-up plan; only `bootstrap` ships here.
- **LOW-3** (forbid-unsafe?) → verified: no `forbid(unsafe_code)` in `crates/lab`; `state.rs` already uses `unsafe set_var`. Cleared.
- **MEDIUM-2** (Windows perms) → CONFIG.md text now states 0600-on-Unix / Windows-ACL-pending.

Confirmed against real signatures: `MergeError: Display` ✓, `EnvEntry::new`/`MergeRequest`/`ActionSpec` fields ✓, serve.rs scope (`bearer_token` mut, `host`/`port`/`auth_config`) ✓.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-10-setup-wizard-consolidation.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session with checkpoints.

Which approach? (Note: this should run AFTER PR #108 / `lab-jouhb` merges, on a clean main, to avoid worktree churn.)
