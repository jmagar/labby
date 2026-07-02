# Lab Workspace Runtime Builder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce the first product-style runtime builder for the workspace filesystem service (`fs`) while preserving current `labby` behavior.

**Architecture:** Add an in-repo `workspace` product module that introduces a narrow, surface-neutral runtime API for the filesystem product. The runtime builder resolves workspace-root state from explicit runtime config, preserves startup errors for logging, and centralizes the HTTP route mount policy as a boolean decision. CLI, MCP registry, and HTTP router code continue to own their protocol adapters.

**Tech Stack:** Rust 2024, Axum, Tokio, existing `ToolRegistry`/`RegisteredService`, existing `dispatch::fs` and `api::services::fs` modules.

---

## File Structure

- Create `crates/lab/src/workspace.rs`
  - Public module entry point for the first product runtime seam.
  - Re-exports `WorkspaceRuntime` and `WorkspaceRuntimeBuilder` when `fs` is enabled.

- Create `crates/lab/src/workspace/runtime.rs`
  - Owns `WorkspaceRuntime`, `WorkspaceRuntimeBuilder`, `WorkspaceRuntimeConfig`, workspace-root resolution, and HTTP route mount policy.
  - Uses explicit `WorkspaceRuntimeConfig` input (`root`, `home`) for workspace-root resolution.
  - Does not implement file browsing logic and does not import MCP/API adapters.

- Modify `crates/lab/src/lib.rs`
  - Add `pub mod workspace;`.

- Modify `crates/lab/src/registry.rs`
  - Add a small constructor for bootstrap `RegisteredService` values.
  - Keep `fs` registration in the registry using the MCP-filtered action list.

- Modify `crates/lab/src/cli/serve.rs`
  - Use `WorkspaceRuntimeBuilder` to resolve and attach `workspace_root` to `AppState`.
  - Preserve existing startup log messages.

- Modify `crates/lab/src/api/router.rs`
  - Use `WorkspaceRuntime::should_mount_http_routes(...)` for `/v1/fs` mount policy.
  - Preserve the current security rule: do not mount `/v1/fs` when `LAB_WEB_UI_AUTH_DISABLED=true` and no API auth is configured.

- Test in `crates/lab/src/workspace/runtime.rs`
  - Builder resolves a configured absolute workspace root.
  - Builder expands `~` against explicit home.
  - Builder returns `None` for invalid workspace root.
  - HTTP route policy refuses unauthenticated disabled-auth mode.

---

### Task 1: Add A Bootstrap Service Constructor

**Files:**
- Modify: `crates/lab/src/registry.rs`

- [ ] **Step 1: Add a failing unit test for constructing a bootstrap service**

Add this test inside the existing `#[cfg(test)] mod tests` in `crates/lab/src/registry.rs`. If there is no test module near the bottom, create one at the end of the file.

```rust
#[cfg(test)]
mod workspace_runtime_constructor_tests {
    use super::*;

    fn noop_dispatch(
        _action: String,
        _params: serde_json::Value,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<serde_json::Value, crate::dispatch::error::ToolError>,
                > + Send,
        >,
    > {
        Box::pin(async { Ok(serde_json::Value::Null) })
    }

    #[test]
    fn bootstrap_operator_constructor_sets_available_status_for_actions() {
        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "demo.list",
            description: "Demo action",
            destructive: false,
            params: &[],
            returns: "null",
        }];

        let service = RegisteredService::bootstrap_operator(
            "demo",
            "Demo service",
            "bootstrap",
            ACTIONS,
            noop_dispatch,
        );

        assert_eq!(service.name, "demo");
        assert_eq!(service.category, "bootstrap");
        assert_eq!(service.kind, RegisteredServiceKind::BootstrapOperator);
        assert_eq!(service.status, "available");
        assert_eq!(service.actions[0].name, "demo.list");
    }

    #[test]
    fn bootstrap_operator_constructor_sets_stub_status_for_empty_actions() {
        let service = RegisteredService::bootstrap_operator(
            "demo",
            "Demo service",
            "bootstrap",
            &[],
            noop_dispatch,
        );

        assert_eq!(service.status, "stub");
    }
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo test -p labby bootstrap_operator_constructor_sets_available_status_for_actions --all-features
```

Expected: compile failure because `RegisteredService::bootstrap_operator` does not exist.

- [ ] **Step 3: Implement the constructor**

Add this `impl` block after the `impl std::fmt::Debug for RegisteredService` block in `crates/lab/src/registry.rs`:

```rust
impl RegisteredService {
    /// Construct a local/bootstrap/operator service registration.
    ///
    #[must_use]
    pub const fn bootstrap_operator(
        name: &'static str,
        description: &'static str,
        category: &'static str,
        actions: &'static [ActionSpec],
        dispatch: DispatchFn,
    ) -> Self {
        Self {
            name,
            description,
            category,
            kind: RegisteredServiceKind::BootstrapOperator,
            status: if actions.is_empty() { "stub" } else { "available" },
            actions,
            dispatch,
        }
    }
}
```

- [ ] **Step 4: Run the focused tests and verify they pass**

Run:

```bash
cargo test -p labby bootstrap_operator_constructor --all-features
```

Expected: both constructor tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/registry.rs
git commit -m "refactor: add bootstrap service registry constructor"
```

---

### Task 2: Add The Workspace Runtime Module

**Files:**
- Create: `crates/lab/src/workspace.rs`
- Create: `crates/lab/src/workspace/runtime.rs`
- Modify: `crates/lab/src/lib.rs`

- [ ] **Step 1: Write failing runtime tests**

Create `crates/lab/src/workspace.rs`:

```rust
//! Workspace product runtime seam.
//!
//! This module is the first extraction proof for product-style runtime
//! composition inside the existing `lab` crate. It composes the current
//! workspace filesystem adapters without moving them to an external crate yet.

#[cfg(feature = "fs")]
mod runtime;

#[cfg(feature = "fs")]
pub use runtime::{WorkspaceRuntime, WorkspaceRuntimeBuilder, WorkspaceRuntimeConfig};
```

Create `crates/lab/src/workspace/runtime.rs` with a surface-neutral runtime and tests first:

```rust
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceRuntime {
    workspace_root: Result<PathBuf, String>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceRuntimeConfig {
    pub root: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceRuntimeBuilder {
    config: WorkspaceRuntimeConfig,
}

impl WorkspaceRuntimeBuilder {
    #[must_use]
    pub fn new(config: WorkspaceRuntimeConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn build(self) -> WorkspaceRuntime {
        WorkspaceRuntime {
            workspace_root: resolve_workspace_root(&self.config).map_err(|error| error.to_string()),
        }
    }
}

impl WorkspaceRuntime {
    #[must_use]
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref().ok()
    }

    #[must_use]
    pub fn workspace_root_error(&self) -> Option<&str> {
        self.workspace_root.as_ref().err().map(String::as_str)
    }

    #[must_use]
    pub const fn should_mount_http_routes(
        web_ui_auth_disabled: bool,
        api_auth_configured: bool,
    ) -> bool {
        !web_ui_auth_disabled || api_auth_configured
    }
}

fn resolve_workspace_root(config: &WorkspaceRuntimeConfig) -> std::io::Result<PathBuf> {
    // Resolve `root` or default `home/.labby/stash`, expand `~`, create the
    // directory, canonicalize it, and reject non-directories/relative paths.
    canonicalize_workspace_dir(expand_configured_root(config)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_resolves_configured_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkspaceRuntimeConfig {
            root: Some(temp.path().to_path_buf()),
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path()).expect("canonical")
        );
    }

    #[test]
    fn builder_keeps_invalid_workspace_root_unset() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("not-a-dir");
        std::fs::write(&file, b"not a directory").expect("write");
        let config = WorkspaceRuntimeConfig {
            root: Some(file),
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert!(runtime.workspace_root().is_none());
        assert!(runtime.workspace_root_error().is_some());
    }

    #[test]
    fn builder_expands_tilde_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkspaceRuntimeConfig {
            root: Some(PathBuf::from("~/stash")),
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path().join("stash")).expect("canonical")
        );
    }

    #[test]
    fn mount_policy_refuses_disabled_auth_without_api_auth() {
        assert!(!WorkspaceRuntime::should_mount_http_routes(true, false));
    }

    #[test]
    fn mount_policy_allows_disabled_auth_when_api_auth_is_configured() {
        assert!(WorkspaceRuntime::should_mount_http_routes(true, true));
    }
}
```

Modify `crates/lab/src/lib.rs` by adding:

```rust
#[cfg(feature = "fs")]
pub mod workspace;
```

- [ ] **Step 2: Run the focused tests**

Run:

```bash
cargo test -p labby workspace::runtime::tests --all-features
```

Expected: tests compile or fail only on exact helper implementation details.

- [ ] **Step 3: Confirm workspace-root compatibility**

The serve adapter passes `config.workspace.root` and `HOME` into `WorkspaceRuntimeConfig`. Preserve the previous config behavior:

- `None` defaults to `$HOME/.labby/stash`.
- `~` and `~/...` expand against `$HOME`.
- relative configured roots are rejected after expansion.

- [ ] **Step 4: Run the focused tests and verify they pass**

Run:

```bash
cargo test -p labby workspace::runtime::tests --all-features
```

Expected: all workspace runtime tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/lib.rs crates/lab/src/workspace.rs crates/lab/src/workspace/runtime.rs
git commit -m "feat: add workspace runtime builder"
```

---

### Task 3: Preserve The Fs Registry Fragment

**Files:**
- Modify: `crates/lab/src/registry.rs`

- [ ] **Step 1: Add a characterization registry test**

Add this test in `crates/lab/src/registry.rs` under an existing `#[cfg(test)]` test module, or create a new `#[cfg(test)] mod workspace_runtime_registry_tests` at the end of the file:

```rust
#[cfg(all(test, feature = "fs"))]
mod workspace_runtime_registry_tests {
    #[test]
    fn default_registry_uses_mcp_filtered_fs_actions() {
        let registry = crate::registry::build_default_registry();
        let fs = registry
            .services()
            .iter()
            .find(|service| service.name == "fs")
            .expect("fs registered");
        let names: Vec<&str> = fs.actions.iter().map(|action| action.name).collect();

        assert!(names.contains(&"fs.list"));
        assert!(!names.contains(&"fs.preview"));
    }
}
```

- [ ] **Step 2: Run the focused test**

Run:

```bash
cargo test -p labby default_registry_uses_mcp_filtered_fs_actions --all-features
```

Expected: pass before the code change is possible, because current behavior already filters `fs.preview`. Keep this test anyway; it locks the invariant while other runtime wiring moves.

- [ ] **Step 3: Replace inline `fs` registration with the constructor**

In `crates/lab/src/registry.rs`, replace this block:

```rust
#[cfg(feature = "fs")]
reg.register(RegisteredService {
    name: "fs",
    description: "Workspace filesystem browser (read-only, deny-listed)",
    category: "bootstrap",
    kind: RegisteredServiceKind::BootstrapOperator,
    status: "available",
    actions: crate::mcp::services::fs::ACTIONS,
    dispatch: dispatch_fn!(crate::mcp::services::fs::dispatch),
});
```

with:

```rust
#[cfg(feature = "fs")]
reg.register(RegisteredService::bootstrap_operator(
    "fs",
    "Workspace filesystem browser (read-only, deny-listed)",
    "bootstrap",
    crate::mcp::services::fs::ACTIONS,
    dispatch_fn!(crate::mcp::services::fs::dispatch),
));
```

Leave the existing security comment above the registration in place. It explains why the registry uses the MCP-filtered action set.

- [ ] **Step 4: Run the focused registry test**

Run:

```bash
cargo test -p labby default_registry_uses_mcp_filtered_fs_actions --all-features
```

Expected: pass.

- [ ] **Step 5: Run registry/catalog smoke tests**

Run:

```bash
cargo test -p labby registry --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/registry.rs
git commit -m "refactor: register fs through bootstrap constructor"
```

---

### Task 4: Wire Workspace Runtime Into Serve Startup

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`

- [ ] **Step 1: Add a focused startup helper test if a test module exists**

If `crates/lab/src/cli/serve.rs` already has a `#[cfg(test)]` module, add this helper test there after introducing the helper in Step 3. If no test module exists, skip this test and rely on the workspace runtime tests from Task 2.

```rust
#[cfg(all(test, feature = "fs"))]
mod workspace_runtime_startup_tests {
    use crate::config::LabConfig;

    #[test]
    fn workspace_runtime_builder_is_used_for_workspace_root_resolution() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut config = LabConfig::default();
        config.workspace.root = Some(temp.path().to_path_buf());

        let runtime = crate::workspace::WorkspaceRuntimeBuilder::new(
            crate::workspace::WorkspaceRuntimeConfig {
                root: config.workspace.root.clone(),
                home: Some(temp.path().to_path_buf()),
            },
        )
        .build();

        assert!(runtime.workspace_root().is_some());
    }
}
```

- [ ] **Step 2: Locate the current `fs.workspace_root` block**

Find this block in `crates/lab/src/cli/serve.rs`:

```rust
#[cfg(feature = "fs")]
match crate::dispatch::fs::resolve_workspace_root(config) {
    Ok(root) => {
        tracing::info!(
            subsystem = "startup",
            phase = "fs.workspace_root",
            path = %root.display(),
            "workspace filesystem browser enabled"
        );
        state = state.with_workspace_root(root);
    }
    Err(e) => {
        tracing::warn!(
            subsystem = "startup",
            phase = "fs.workspace_root",
            error = %e,
            "workspace.root invalid; fs service disabled"
        );
    }
}
```

- [ ] **Step 3: Replace direct resolution with the builder**

Replace the block with:

```rust
#[cfg(feature = "fs")]
{
    let workspace_runtime = crate::workspace::WorkspaceRuntimeBuilder::new(
        crate::workspace::WorkspaceRuntimeConfig {
            root: config.workspace.root.clone(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        },
    )
    .build();
    if let Some(root) = workspace_runtime.workspace_root() {
        tracing::info!(
            subsystem = "startup",
            phase = "fs.workspace_root",
            path = %root.display(),
            "workspace filesystem browser enabled"
        );
        state = state.with_workspace_root(root.to_path_buf());
    } else {
        tracing::warn!(
            subsystem = "startup",
            phase = "fs.workspace_root",
            error = workspace_runtime.workspace_root_error(),
            "workspace.root invalid; fs service disabled"
        );
    }
}
```

This preserves the raw startup error through `workspace_root_error()` while keeping the runtime builder independent from `LabConfig`.

- [ ] **Step 4: Run a compile check**

Run:

```bash
cargo check -p labby --all-features
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/cli/serve.rs
git commit -m "refactor: resolve workspace root through runtime builder"
```

---

### Task 5: Wire Workspace Runtime Into HTTP Route Mounting

**Files:**
- Modify: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Preserve the existing security policy test**

Run the existing router tests that cover disabled-auth behavior:

```bash
cargo test -p labby setup_actions_require_auth_when_web_auth_disabled_without_bearer --all-features
```

Expected: pass. If the exact test name has changed, run:

```bash
cargo test -p labby web_auth_disabled --all-features
```

Expected: pass.

- [ ] **Step 2: Replace inline `/v1/fs` mount policy**

In `crates/lab/src/api/router.rs`, replace the body of the existing `#[cfg(feature = "fs")]` block that mounts `/fs` with:

```rust
#[cfg(feature = "fs")]
if state.registry.services().iter().any(|service| service.name == "fs") {
    if crate::workspace::WorkspaceRuntime::should_mount_http_routes(
        state.web_ui_auth_disabled,
        api_auth_configured,
    ) {
        v1 = v1.nest("/fs", services::fs::routes(state.clone()));
    } else {
        tracing::warn!(
            subsystem = "startup",
            phase = "fs.mount.skipped",
            reason = "web_ui_auth_disabled",
            "fs service is configured but LAB_WEB_UI_AUTH_DISABLED=true would expose workspace files unauthenticated; refusing to mount /v1/fs"
        );
    }
}
```

Keep the surrounding security comment. The runtime owns the boolean decision; the router still owns the `/v1/fs` path and route adapter.

- [ ] **Step 3: Run the fs API tests**

Run:

```bash
cargo test -p labby --test api_fs_headers --all-features
```

Expected: pass.

- [ ] **Step 4: Run router tests**

Run:

```bash
cargo test -p labby router --all-features
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/router.rs
git commit -m "refactor: mount fs routes through workspace runtime"
```

---

### Task 6: Verify The First Runtime Builder Slice

**Files:**
- No source changes unless verification finds a defect.

- [ ] **Step 1: Run focused workspace tests**

Run:

```bash
cargo test -p labby workspace --all-features
```

Expected: pass.

- [ ] **Step 2: Run fs dispatch and API tests**

Run:

```bash
cargo test -p labby fs --all-features
cargo test -p labby --test api_fs_headers --all-features
```

Expected: pass.

- [ ] **Step 3: Run registry and docs projection checks**

Run:

```bash
cargo test -p labby registry --all-features
cargo test -p labby docs --all-features
```

Expected: pass.

- [ ] **Step 4: Run the minimum backend gate for this slice**

Run:

```bash
cargo check --workspace --all-features
```

Expected: pass.

- [ ] **Step 5: Run the broader test gate if time allows**

Run:

```bash
cargo nextest run --workspace --all-features
```

Expected: pass. If failures are unrelated to workspace/fs changes, record the failing test names and error summaries in the final handoff.

- [ ] **Step 6: Commit any verification fixes**

If verification required fixes:

```bash
git add crates/lab/src
git commit -m "fix: stabilize workspace runtime builder wiring"
```

If no fixes were required, do not create an empty commit.

---

## Acceptance Criteria

- `crates/lab/src/workspace/runtime.rs` exists and exposes `WorkspaceRuntimeBuilder`.
- `WorkspaceRuntimeBuilder::new(config).build()` resolves the workspace root from explicit `WorkspaceRuntimeConfig`.
- `WorkspaceRuntimeConfig` preserves `$HOME/.labby/stash` defaulting and `~` expansion behavior.
- `registry.rs` registers `fs` through `RegisteredService::bootstrap_operator(...)` with the MCP-filtered action slice.
- `cli/serve.rs` attaches `AppState.workspace_root` through the workspace runtime builder.
- `api/router.rs` delegates `/v1/fs` mount policy to `WorkspaceRuntime::should_mount_http_routes(...)`.
- MCP still exposes `fs.list` and does not expose `fs.preview`.
- HTTP still exposes `/v1/fs/list` and `/v1/fs/preview` when auth policy allows mounting.
- `/v1/fs` still refuses to mount when web UI auth is disabled and no API auth is configured.
- `cargo check --workspace --all-features` passes.

## Self-Review

- Spec coverage: This plan implements the first product runtime-builder proof from `docs/crate-extract/spec.md` without creating a standalone crate or changing the REST/MCP contract.
- Boundary check: `workspace` is surface-neutral and does not import MCP/API adapters. The current `dispatch::fs` remains the domain/action layer.
- Placeholder scan: No task uses placeholder markers or unspecified validation.
- Type consistency: The plan consistently uses `WorkspaceRuntime`, `WorkspaceRuntimeBuilder`, `WorkspaceRuntimeConfig`, `workspace_root`, `workspace_root_error`, and `should_mount_http_routes`.
- Known caveat: `cargo nextest run --workspace --all-features` may expose unrelated existing failures; record those rather than widening this slice.
