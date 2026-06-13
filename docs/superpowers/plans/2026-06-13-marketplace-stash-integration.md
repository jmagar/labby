# Marketplace Stash Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Marketplace artifact forks into the real `stash` service so marketplace-origin edits become versioned, syncable, deployable stash components instead of living in a parallel pseudo-stash.

**Architecture:** Keep `marketplace` and `stash` as separate services with one explicit bridge. `marketplace` owns plugin source discovery, upstream update checks, and artifact merge UX; `stash` owns component identity, local workspaces, immutable revisions, provider sync, and deploy/export. Marketplace `artifact.*` actions create and operate on stash components through shared Rust helpers, while retaining marketplace-specific upstream metadata for update and merge flows.

**Tech Stack:** Rust 2024, existing `dispatch` service pattern, serde JSON, tokio `spawn_blocking`, filesystem-backed `StashStore`, existing marketplace backend/source helpers, Next.js gateway-admin client hooks, cargo nextest.

**Spec:** `docs/superpowers/specs/2026-06-13-marketplace-stash-integration-spec.md`

**Contract:** `docs/contracts/marketplace-stash-integration.md`

---

## Design Boundary

The integration has one direction:

- `marketplace` may import/fork plugin artifacts into `stash`.
- `stash` must not discover marketplaces, install plugins, or shell out to marketplace/runtime CLIs.

The durable owner for user-authored or user-forked artifact content is `stash`. Marketplace stores only enough fork metadata to reconnect a stash component to upstream plugin state.

## File Structure

### Rust Domain Types

- Modify `crates/lab-apis/src/stash/types.rs`
  - Add `StashOrigin`, `MarketplaceOrigin`, and optional `origin_meta` on `StashComponent`.
  - Preserve existing `origin: Option<String>` for compatibility.
- Modify `crates/lab-apis/src/stash.rs`
  - Re-export the new origin types.

### Stash Dispatch

- Modify `crates/lab/src/dispatch/stash/catalog.rs`
  - Add `component.adopt` as the higher-level import-and-save action used by marketplace and CLI/API callers.
- Modify `crates/lab/src/dispatch/stash/params.rs`
  - Add `AdoptParams` parser.
- Modify `crates/lab/src/dispatch/stash/dispatch.rs`
  - Route `component.adopt`.
- Modify `crates/lab/src/dispatch/stash/service.rs`
  - Add `component_adopt` and `adopt_component_from_path`.
- Modify `crates/lab/src/dispatch/stash/import.rs`
  - Add an import helper that accepts origin metadata and can create the component record in one locked operation.

### Marketplace Bridge

- Create `crates/lab/src/dispatch/marketplace/stash_bridge.rs`
  - Resolve plugin source/artifact paths.
  - Create or find stash components for plugin forks.
  - Write stash revisions and marketplace fork metadata.
  - Convert stash component state into marketplace `artifact.*` responses.
- Modify `crates/lab/src/dispatch/marketplace.rs`
  - Add `mod stash_bridge;`.
- Modify `crates/lab/src/dispatch/marketplace/dispatch.rs`
  - Make source/workspace path helpers usable by `stash_bridge`.
  - Route artifact actions through real bridge implementations.
- Modify `crates/lab/src/dispatch/marketplace/fork.rs`
  - Replace `not_implemented` stubs for `artifact.fork`, `artifact.list`, `artifact.unfork`, and `artifact.reset`.
- Modify `crates/lab/src/dispatch/marketplace/update.rs`
  - Read fork metadata from stash-owned records through `stash_bridge`.
  - Stop defining a second private `StashMeta` schema after the bridge is in place.
- Modify `crates/lab/src/dispatch/marketplace/stash_meta.rs`
  - Keep marketplace-specific upstream merge metadata, but store it under stash workspaces and make it compatible with `StashOrigin`.

### API and Frontend

- Modify `crates/lab/src/api/services/marketplace.rs`
  - Keep existing artifact endpoints; they now become live.
- Modify `apps/gateway-admin/lib/api/marketplace-client.ts`
  - Add typed functions for `artifact.fork`, `artifact.list`, `artifact.update.*`.
- Modify `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`
  - Add a "Fork to Stash" action for the selected artifact or whole plugin.
- Create `apps/gateway-admin/lib/api/marketplace-artifacts.test.ts`
  - Verify request shapes for new client helpers.

### Docs and Generated Artifacts

- Modify `docs/coverage/stash.md`
  - Document marketplace-origin adoption.
- Modify `docs/features/artifact-diffs.md`
  - Mark the desired boundary as implemented and link to the action catalog.
- Run docs generation so `docs/generated/*` reflects action catalog changes.

---

## Task 1: Add Typed Stash Origin Metadata

**Files:**
- Modify: `crates/lab-apis/src/stash/types.rs`
- Modify: `crates/lab-apis/src/stash.rs`
- Test: `crates/lab-apis/src/stash/types.rs`

- [ ] **Step 1: Write failing origin serialization tests**

Append these tests to the existing `#[cfg(test)]` module in `crates/lab-apis/src/stash/types.rs`. If the file has no test module, add one at the bottom.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn marketplace_origin_round_trips() {
        let origin = StashOrigin::Marketplace(MarketplaceOrigin {
            plugin_id: "demo@labby".to_string(),
            artifact_path: Some("skills/demo/SKILL.md".to_string()),
            source_version: Some("abc123".to_string()),
            source_commit: Some("def456".to_string()),
        });

        let encoded = serde_json::to_value(&origin).unwrap();
        assert_eq!(
            encoded,
            json!({
                "kind": "marketplace",
                "plugin_id": "demo@labby",
                "artifact_path": "skills/demo/SKILL.md",
                "source_version": "abc123",
                "source_commit": "def456"
            })
        );

        let decoded: StashOrigin = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, origin);
    }

    #[test]
    fn component_origin_meta_is_optional_for_existing_records() {
        let value = json!({
            "id": "01aryz6s41tpz5x11k39dv3r2g",
            "kind": "skill",
            "name": "demo",
            "label": null,
            "head_revision_id": null,
            "origin": null,
            "workspace_root": "/tmp/demo",
            "workspace_shape": "directory",
            "unix_mode": null,
            "created_at": "2026-06-13T00:00:00Z",
            "updated_at": "2026-06-13T00:00:00Z"
        });

        let component: StashComponent = serde_json::from_value(value).unwrap();
        assert!(component.origin_meta.is_none());
    }
}
```

- [ ] **Step 2: Run the focused tests to verify they fail**

Run:

```bash
cargo test -p lab-apis marketplace_origin_round_trips component_origin_meta_is_optional_for_existing_records
```

Expected: FAIL because `StashOrigin`, `MarketplaceOrigin`, and `origin_meta` do not exist.

- [ ] **Step 3: Add origin types**

In `crates/lab-apis/src/stash/types.rs`, add this block near the other core structs:

```rust
/// Structured origin metadata for components imported from another Lab surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StashOrigin {
    /// Component was forked or adopted from a Marketplace plugin artifact.
    Marketplace(MarketplaceOrigin),
    /// Component was imported directly from a local filesystem path.
    LocalPath {
        /// Original absolute source path at import time.
        source_path: PathBuf,
    },
}

/// Marketplace-specific component origin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarketplaceOrigin {
    /// Plugin id in `name@marketplace` form.
    pub plugin_id: String,
    /// Relative artifact path inside the plugin. `None` means whole-plugin fork.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    /// Version string from the plugin or marketplace manifest at fork time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    /// Source tree fingerprint or upstream commit at fork time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
}
```

- [ ] **Step 4: Add `origin_meta` to `StashComponent`**

Update `StashComponent` in `crates/lab-apis/src/stash/types.rs`:

```rust
pub struct StashComponent {
    pub id: String,
    pub kind: StashComponentKind,
    pub name: String,
    pub label: Option<String>,
    pub head_revision_id: Option<String>,
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_meta: Option<StashOrigin>,
    pub workspace_root: PathBuf,
    pub workspace_shape: StashWorkspaceShape,
    pub unix_mode: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}
```

Then update every `StashComponent { ... }` literal in `crates/lab/src/dispatch/stash/` tests and implementation to include:

```rust
origin_meta: None,
```

- [ ] **Step 5: Re-export the origin types**

Update `crates/lab-apis/src/stash.rs`:

```rust
pub use types::{
    MarketplaceOrigin, StashComponent, StashComponentKind, StashDeployTarget, StashExportOptions,
    StashOrigin, StashProviderCapabilities, StashProviderRecord, StashProviderSummary,
    StashRevision, StashWorkspaceShape,
};
```

- [ ] **Step 6: Run the focused tests**

Run:

```bash
cargo test -p lab-apis marketplace_origin_round_trips component_origin_meta_is_optional_for_existing_records
```

Expected: PASS.

- [ ] **Step 7: Run stash compile check**

Run:

```bash
cargo check -p labby --all-features
```

Expected: PASS. Fix missing `origin_meta: None` struct literals before continuing.

- [ ] **Step 8: Commit**

```bash
git add crates/lab-apis/src/stash.rs crates/lab-apis/src/stash/types.rs crates/lab/src/dispatch/stash
git commit -m "feat(stash): add structured component origin metadata"
```

---

## Task 2: Add Stash Adopt Helper and Action

**Files:**
- Modify: `crates/lab/src/dispatch/stash/catalog.rs`
- Modify: `crates/lab/src/dispatch/stash/params.rs`
- Modify: `crates/lab/src/dispatch/stash/dispatch.rs`
- Modify: `crates/lab/src/dispatch/stash/service.rs`
- Modify: `crates/lab/src/dispatch/stash/import.rs`
- Test: `crates/lab/src/dispatch/stash/dispatch.rs`
- Test: `crates/lab/src/dispatch/stash/import.rs`

- [ ] **Step 1: Write failing dispatch test for adopt**

Append this test in `crates/lab/src/dispatch/stash/dispatch.rs` inside its existing test module:

```rust
#[tokio::test]
async fn dispatch_adopt_imports_and_saves_marketplace_component() {
    let (store, _stash_dir) = make_store();
    let source_dir = tempfile::tempdir().expect("source tempdir");
    std::fs::write(source_dir.path().join("SKILL.md"), "# Demo skill\n").unwrap();

    let value = dispatch_with_store(
        &store,
        "component.adopt",
        json!({
            "kind": "skill",
            "name": "demo-skill",
            "label": "Demo Skill",
            "source_path": source_dir.path().display().to_string(),
            "origin": {
                "kind": "marketplace",
                "plugin_id": "demo@labby",
                "artifact_path": "skills/demo",
                "source_version": "1.0.0",
                "source_commit": "abc123"
            },
            "save_label": "Fork from demo@labby"
        }),
    )
    .await
    .unwrap();

    let component_id = value.get("component").unwrap().get("id").unwrap().as_str().unwrap();
    let component = store.read_component(component_id).unwrap().unwrap();
    assert_eq!(component.name, "demo-skill");
    assert_eq!(component.head_revision_id, value.get("revision").unwrap().get("id").and_then(|v| v.as_str()).map(str::to_string));
    assert!(store.workspace_dir(component_id).join("SKILL.md").is_file());
    assert!(component.origin_meta.is_some());
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cargo test -p labby --all-features dispatch_adopt_imports_and_saves_marketplace_component
```

Expected: FAIL with `unknown action` or missing parser/types.

- [ ] **Step 3: Add catalog entry**

Insert this action after `component.import` in `crates/lab/src/dispatch/stash/catalog.rs`:

```rust
ActionSpec {
    name: "component.adopt",
    description: "Create a stash component from a local path, attach origin metadata, and save the initial revision",
    destructive: true,
    requires_admin: false,
    returns: "AdoptResult",
    params: &[
        ParamSpec {
            name: "kind",
            ty: "string",
            required: true,
            description: "Component kind: skill, agent, command, channel, monitor, hook, output_style, theme, settings, mcp_config, lsp_config, script, bin_file",
        },
        ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Component name",
        },
        ParamSpec {
            name: "label",
            ty: "string",
            required: false,
            description: "Optional human-readable label",
        },
        ParamSpec {
            name: "source_path",
            ty: "string",
            required: true,
            description: "Absolute source path to copy into the stash workspace",
        },
        ParamSpec {
            name: "origin",
            ty: "object",
            required: true,
            description: "Structured StashOrigin metadata",
        },
        ParamSpec {
            name: "save_label",
            ty: "string",
            required: false,
            description: "Optional initial revision label",
        },
    ],
},
```

- [ ] **Step 4: Add adopt params**

In `crates/lab/src/dispatch/stash/params.rs`, add:

```rust
use lab_apis::stash::StashOrigin;
```

Then add:

```rust
/// `component.adopt` - create, import, attach origin metadata, and save.
pub struct AdoptParams {
    pub kind: String,
    pub name: String,
    pub label: Option<String>,
    pub source_path: PathBuf,
    pub origin: StashOrigin,
    pub save_label: Option<String>,
}

pub fn parse_adopt_params(params: &Value) -> Result<AdoptParams, ToolError> {
    let source_path = require_str(params, "source_path")?;
    let path = PathBuf::from(source_path);
    if !path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: "source_path must be an absolute path".to_string(),
            param: "source_path".to_string(),
        });
    }
    let origin_value = params.get("origin").cloned().ok_or_else(|| ToolError::MissingParam {
        param: "origin".to_string(),
        message: "`origin` is required".to_string(),
    })?;
    let origin = serde_json::from_value(origin_value).map_err(|error| ToolError::InvalidParam {
        param: "origin".to_string(),
        message: format!("origin is invalid: {error}"),
    })?;
    Ok(AdoptParams {
        kind: require_str(params, "kind")?.to_string(),
        name: require_str(params, "name")?.to_string(),
        label: optional_str(params, "label")?.map(str::to_string),
        source_path: path,
        origin,
        save_label: optional_str(params, "save_label")?.map(str::to_string),
    })
}
```

- [ ] **Step 5: Route adopt in dispatch**

Update the parser import in `crates/lab/src/dispatch/stash/dispatch.rs` to include `parse_adopt_params`.

Add this match arm before `component.import`:

```rust
"component.adopt" => {
    let p = parse_adopt_params(&params)?;
    service::component_adopt(store, p).await
}
```

- [ ] **Step 6: Add import helper with origin metadata**

In `crates/lab/src/dispatch/stash/import.rs`, import `StashOrigin`:

```rust
use lab_apis::stash::types::{StashComponent, StashComponentKind, StashOrigin, limits};
```

Change `import_component` to call a new helper:

```rust
pub async fn import_component_with_origin(
    store: &StashStore,
    id: &str,
    source: &Path,
    kind_override: Option<StashComponentKind>,
    name: &str,
    label: Option<&str>,
    origin_meta: Option<StashOrigin>,
) -> Result<StashComponent, ToolError> {
    if name.is_empty() {
        return Err(ToolError::InvalidParam {
            param: "name".into(),
            message: "name must not be empty".into(),
        });
    }
    if name.len() > limits::MAX_COMPONENT_NAME_LEN {
        return Err(ToolError::InvalidParam {
            param: "name".into(),
            message: format!("name must not exceed {} bytes", limits::MAX_COMPONENT_NAME_LEN),
        });
    }
    reject_symlink(source)?;
    canonicalize_and_reject_read_path(source)?;
    let id = id.to_string();
    let source = source.to_path_buf();
    let name = name.to_string();
    let label = label.map(str::to_string);
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        import_blocking_with_origin(
            &store,
            &id,
            &source,
            kind_override,
            &name,
            label.as_deref(),
            origin_meta,
        )
    })
    .await
    .map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("spawn_blocking panicked: {e}"),
    })?
}

pub async fn import_component(
    store: &StashStore,
    id: &str,
    source: &Path,
    kind_override: Option<StashComponentKind>,
    name: &str,
    label: Option<&str>,
) -> Result<StashComponent, ToolError> {
    import_component_with_origin(store, id, source, kind_override, name, label, None).await
}
```

Rename the existing `import_blocking` to `import_blocking_with_origin` and add `origin_meta: Option<StashOrigin>` to its arguments. In the component literal, set:

```rust
origin: existing.as_ref().and_then(|c| c.origin.clone()).or_else(|| {
    origin_meta.as_ref().map(|origin| match origin {
        StashOrigin::Marketplace(marketplace) => {
            if let Some(path) = &marketplace.artifact_path {
                format!("marketplace://{}?artifact={}", marketplace.plugin_id, path)
            } else {
                format!("marketplace://{}", marketplace.plugin_id)
            }
        }
        StashOrigin::LocalPath { source_path } => {
            format!("file://{}", source_path.display())
        }
    })
}),
origin_meta: origin_meta.or_else(|| existing.as_ref().and_then(|c| c.origin_meta.clone())),
```

- [ ] **Step 7: Add service adopt helper**

In `crates/lab/src/dispatch/stash/service.rs`, update imports:

```rust
use crate::dispatch::stash::params::{
    AdoptParams, CreateParams, DeployParams, ExportParams, GetParams, ImportParams, LinkParams,
    ProviderSyncParams, RevisionsParams, SaveParams, TargetAddParams, TargetRemoveParams,
    WorkspaceParams,
};
```

Add:

```rust
/// `component.adopt` - create a component from a path and save its initial revision.
pub async fn component_adopt(store: &StashStore, p: AdoptParams) -> Result<Value, ToolError> {
    let result = adopt_component_from_path(
        store,
        &p.kind,
        &p.name,
        p.label.as_deref(),
        &p.source_path,
        p.origin,
        p.save_label.as_deref(),
    )
    .await?;
    to_json(result)
}

#[derive(serde::Serialize)]
pub struct AdoptResult {
    pub component: StashComponent,
    pub revision: lab_apis::stash::StashRevision,
}

pub async fn adopt_component_from_path(
    store: &StashStore,
    kind: &str,
    name: &str,
    label: Option<&str>,
    source_path: &Path,
    origin: lab_apis::stash::StashOrigin,
    save_label: Option<&str>,
) -> Result<AdoptResult, ToolError> {
    let kind_override = serde_json::from_value::<StashComponentKind>(Value::String(kind.to_string()))
        .map_err(|_| ToolError::InvalidParam {
            param: "kind".into(),
            message: "unrecognised component kind".into(),
        })?;
    let id = ulid::Ulid::new().to_string().to_lowercase();
    let component = import::import_component_with_origin(
        store,
        &id,
        source_path,
        Some(kind_override),
        name,
        label,
        Some(origin),
    )
    .await?;
    let revision = revision::save_revision(store, &component.id, save_label).await?;
    let mut updated = component.clone();
    updated.head_revision_id = Some(revision.id.clone());
    store.write_component(&updated)?;
    Ok(AdoptResult {
        component: updated,
        revision,
    })
}
```

- [ ] **Step 8: Run tests**

Run:

```bash
cargo test -p labby --all-features dispatch_adopt_imports_and_saves_marketplace_component
cargo test -p labby --all-features dispatch_import_accepts_operator_workspace_path
```

Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/lab/src/dispatch/stash crates/lab-apis/src/stash
git commit -m "feat(stash): add component adoption flow"
```

---

## Task 3: Add Marketplace Stash Bridge

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/stash_bridge.rs`
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Test: `crates/lab/src/dispatch/marketplace/stash_bridge.rs`

- [ ] **Step 1: Create bridge tests first**

Create `crates/lab/src/dispatch/marketplace/stash_bridge.rs` with only tests and `use` lines:

```rust
use std::path::{Path, PathBuf};

use lab_apis::stash::{MarketplaceOrigin, StashComponentKind, StashOrigin};
use serde::Serialize;
use serde_json::Value;

use crate::dispatch::error::ToolError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_component_name_sanitizes_plugin_and_artifact() {
        assert_eq!(
            component_name_for_fork("demo@labby", Some("skills/demo/SKILL.md")),
            "demo-labby-skills-demo-skill-md"
        );
    }

    #[test]
    fn kind_for_artifact_path_maps_plugin_layout_to_stash_kind() {
        assert_eq!(kind_for_artifact_path(Some("skills/demo")), StashComponentKind::Skill);
        assert_eq!(kind_for_artifact_path(Some("agents/demo.md")), StashComponentKind::Agent);
        assert_eq!(kind_for_artifact_path(Some("commands/demo.md")), StashComponentKind::Command);
        assert_eq!(kind_for_artifact_path(Some("settings.json")), StashComponentKind::Settings);
        assert_eq!(kind_for_artifact_path(None), StashComponentKind::Skill);
    }
}
```

- [ ] **Step 2: Register the module**

Add this line to `crates/lab/src/dispatch/marketplace.rs`:

```rust
mod stash_bridge;
```

- [ ] **Step 3: Run the failing tests**

Run:

```bash
cargo test -p labby --all-features stash_component_name_sanitizes_plugin_and_artifact kind_for_artifact_path_maps_plugin_layout_to_stash_kind
```

Expected: FAIL because helper functions do not exist.

- [ ] **Step 4: Add bridge helper implementation**

Add this code above the test module in `crates/lab/src/dispatch/marketplace/stash_bridge.rs`:

```rust
pub(super) fn component_name_for_fork(plugin_id: &str, artifact_path: Option<&str>) -> String {
    let raw = match artifact_path {
        Some(path) => format!("{plugin_id}-{path}"),
        None => plugin_id.to_string(),
    };
    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_was_dash {
                out.push(mapped);
            }
            last_was_dash = true;
        } else {
            out.push(mapped);
            last_was_dash = false;
        }
    }
    out.trim_matches('-').chars().take(128).collect()
}

pub(super) fn kind_for_artifact_path(artifact_path: Option<&str>) -> StashComponentKind {
    let Some(path) = artifact_path else {
        return StashComponentKind::Skill;
    };
    let first = path.split('/').next().unwrap_or(path);
    match first {
        "agents" => StashComponentKind::Agent,
        "commands" => StashComponentKind::Command,
        "hooks" => StashComponentKind::Hook,
        "monitors" => StashComponentKind::Monitor,
        "output-styles" | "output_styles" => StashComponentKind::OutputStyle,
        "themes" => StashComponentKind::Theme,
        "bin" => StashComponentKind::BinFile,
        "settings.json" => StashComponentKind::Settings,
        path if path.ends_with(".mcp.json") => StashComponentKind::McpConfig,
        path if path.ends_with(".lsp.json") => StashComponentKind::LspConfig,
        "skills" => StashComponentKind::Skill,
        _ => StashComponentKind::Skill,
    }
}
```

- [ ] **Step 5: Make marketplace source helpers visible inside the module**

In `crates/lab/src/dispatch/marketplace/dispatch.rs`, change these functions from private to `pub(super)`:

```rust
pub(super) fn workspace_root() -> Result<PathBuf, ToolError> { ... }
pub(super) fn source_path_for_plugin(id: &str) -> Result<PathBuf, ToolError> { ... }
pub(super) fn walk_artifacts(root: &Path, dir: &Path) -> Result<Vec<Artifact>, ToolError> { ... }
```

Do not change their bodies in this task.

- [ ] **Step 6: Add bridge result types and path resolver**

Add this code to `stash_bridge.rs`:

```rust
#[derive(Debug, Serialize)]
pub(super) struct ForkResult {
    pub plugin_id: String,
    pub component_id: String,
    pub revision_id: String,
    pub stash_workspace: String,
    pub forked_artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForkedPluginStatus {
    pub plugin_id: String,
    pub component_id: String,
    pub stash_workspace: String,
    pub forked_artifacts: Vec<String>,
    pub dirty: bool,
}

pub(super) fn fork_source_path(plugin_id: &str, artifact_path: Option<&str>) -> Result<PathBuf, ToolError> {
    let source = crate::dispatch::marketplace::dispatch::source_path_for_plugin(plugin_id)?;
    match artifact_path {
        Some(path) => {
            crate::dispatch::marketplace::stash_meta::validate_rel_path(path)?;
            let candidate = source.join(path);
            if !candidate.exists() {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("artifact `{path}` not found in `{plugin_id}`"),
                });
            }
            Ok(candidate)
        }
        None => Ok(source),
    }
}
```

- [ ] **Step 7: Run bridge tests**

Run:

```bash
cargo test -p labby --all-features stash_bridge
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/marketplace.rs crates/lab/src/dispatch/marketplace/dispatch.rs crates/lab/src/dispatch/marketplace/stash_bridge.rs
git commit -m "feat(marketplace): add stash bridge helpers"
```

---

## Task 4: Implement Marketplace `artifact.fork` and `artifact.list`

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/fork.rs`
- Modify: `crates/lab/src/dispatch/marketplace/stash_bridge.rs`
- Test: `crates/lab/src/dispatch/marketplace/fork.rs`

- [ ] **Step 1: Write failing fork dispatch test**

Replace the current `dispatch_with_client_artifact_fork_roundtrip` test in `crates/lab/src/dispatch/marketplace.rs` with:

```rust
#[tokio::test]
async fn dispatch_artifact_fork_returns_not_found_for_unknown_plugin_source() {
    let err = super::dispatch(
        "artifact.fork",
        json!({"plugin_id": "missing@local", "artifacts": ["agents/demo.md"]}),
    )
    .await
    .unwrap_err();
    assert_ne!(err.kind(), "not_implemented");
}
```

Then add bridge-level tests in `crates/lab/src/dispatch/marketplace/fork.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn artifact_list_empty_when_no_forks_exist() {
        let result = artifact_list(crate::dispatch::marketplace::params::ArtifactListParams {
            plugin_id: None,
            instance: None,
        })
        .await
        .unwrap();
        let rows = result.as_array().unwrap();
        assert!(rows.is_empty());
    }
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo test -p labby --all-features dispatch_artifact_fork_returns_not_found_for_unknown_plugin_source artifact_list_empty_when_no_forks_exist
```

Expected: FAIL because `artifact.list` still returns `not_implemented`.

- [ ] **Step 3: Implement bridge `fork_artifacts`**

Add to `crates/lab/src/dispatch/marketplace/stash_bridge.rs`:

```rust
pub(super) async fn fork_artifacts(
    plugin_id: &str,
    artifacts: Option<Vec<String>>,
) -> Result<Value, ToolError> {
    let artifact_paths = artifacts.unwrap_or_else(|| vec![String::new()]);
    let mut results = Vec::with_capacity(artifact_paths.len());
    for artifact in artifact_paths {
        let artifact_path = if artifact.is_empty() { None } else { Some(artifact.as_str()) };
        let source_path = fork_source_path(plugin_id, artifact_path)?;
        let name = component_name_for_fork(plugin_id, artifact_path);
        let kind = kind_for_artifact_path(artifact_path);
        let source_version = crate::dispatch::marketplace::update::upstream_version_for_bridge(plugin_id).ok();
        let source_commit = crate::dispatch::marketplace::update::source_fingerprint_for_bridge(plugin_id).ok();
        let origin = StashOrigin::Marketplace(MarketplaceOrigin {
            plugin_id: plugin_id.to_string(),
            artifact_path: artifact_path.map(ToString::to_string),
            source_version,
            source_commit,
        });
        let root = crate::dispatch::stash::client::require_stash_root()?.clone();
        let store = crate::dispatch::stash::store::StashStore::new(root);
        store.ensure_dirs().map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("stash store init: {error}"),
        })?;
        let adopt = crate::dispatch::stash::service::adopt_component_from_path(
            &store,
            serde_json::to_value(kind).unwrap().as_str().unwrap(),
            &name,
            Some(&format!("Fork of {plugin_id}")),
            &source_path,
            origin,
            Some(&format!("Fork from {plugin_id}")),
        )
        .await?;
        results.push(ForkResult {
            plugin_id: plugin_id.to_string(),
            component_id: adopt.component.id.clone(),
            revision_id: adopt.revision.id.clone(),
            stash_workspace: adopt.component.workspace_root.display().to_string(),
            forked_artifacts: artifact_path.map(|path| vec![path.to_string()]).unwrap_or_default(),
        });
    }
    crate::dispatch::helpers::to_json(results)
}
```

- [ ] **Step 4: Expose update helpers for bridge**

In `crates/lab/src/dispatch/marketplace/update.rs`, add:

```rust
pub(super) fn upstream_version_for_bridge(plugin_id: &str) -> Result<String, ToolError> {
    let source = source_path_for_plugin(plugin_id)?;
    Ok(upstream_version(&source).unwrap_or_else(|| "unknown".to_string()))
}

pub(super) fn source_fingerprint_for_bridge(plugin_id: &str) -> Result<String, ToolError> {
    let source = source_path_for_plugin(plugin_id)?;
    compute_tree_fingerprint(&source)
}
```

- [ ] **Step 5: Implement bridge `list_forks`**

Add to `stash_bridge.rs`:

```rust
pub(super) async fn list_forks(plugin_id: Option<String>) -> Result<Value, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    store.ensure_dirs().map_err(|error| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("stash store init: {error}"),
    })?;
    let mut rows = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if plugin_id.as_ref().is_some_and(|id| id != &origin.plugin_id) {
            continue;
        }
        rows.push(ForkedPluginStatus {
            plugin_id: origin.plugin_id,
            component_id: component.id,
            stash_workspace: component.workspace_root.display().to_string(),
            forked_artifacts: origin.artifact_path.into_iter().collect(),
            dirty: false,
        });
    }
    crate::dispatch::helpers::to_json(rows)
}
```

- [ ] **Step 6: Route fork/list to the bridge**

Replace the `artifact_fork` and `artifact_list` bodies in `crates/lab/src/dispatch/marketplace/fork.rs`:

```rust
pub(super) async fn artifact_fork(params: ForkParams) -> Result<Value, ToolError> {
    crate::dispatch::marketplace::stash_bridge::fork_artifacts(&params.plugin_id, params.artifacts).await
}

pub(super) async fn artifact_list(params: ArtifactListParams) -> Result<Value, ToolError> {
    crate::dispatch::marketplace::stash_bridge::list_forks(params.plugin_id).await
}
```

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test -p labby --all-features dispatch_artifact_fork_returns_not_found_for_unknown_plugin_source artifact_list_empty_when_no_forks_exist stash_bridge
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/fork.rs crates/lab/src/dispatch/marketplace/stash_bridge.rs crates/lab/src/dispatch/marketplace/update.rs crates/lab/src/dispatch/marketplace.rs
git commit -m "feat(marketplace): fork artifacts into stash"
```

---

## Task 5: Move Update Preview and Apply Onto Stash Components

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/update.rs`
- Modify: `crates/lab/src/dispatch/marketplace/stash_bridge.rs`
- Test: `crates/lab/src/dispatch/marketplace/update.rs`

- [ ] **Step 1: Add bridge lookup helpers**

Add to `stash_bridge.rs`:

```rust
#[derive(Debug, Clone)]
pub(super) struct MarketplaceForkComponent {
    pub plugin_id: String,
    pub component_id: String,
    pub artifact_path: Option<String>,
    pub workspace_root: PathBuf,
    pub workspace_dir: PathBuf,
    pub base_revision_id: Option<String>,
    pub upstream_version: String,
}

pub(super) fn fork_component_for_plugin(plugin_id: &str) -> Result<MarketplaceForkComponent, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id == plugin_id {
            return Ok(MarketplaceForkComponent {
                plugin_id: origin.plugin_id,
                component_id: component.id.clone(),
                artifact_path: origin.artifact_path,
                workspace_root: component.workspace_root.clone(),
                workspace_dir: store.workspace_dir(&component.id),
                base_revision_id: component.head_revision_id.clone(),
                upstream_version: origin.source_version.unwrap_or_else(|| "unknown".to_string()),
            });
        }
    }
    Err(ToolError::Sdk {
        sdk_kind: "not_found".into(),
        message: format!("no stash fork found for `{plugin_id}`"),
    })
}
```

- [ ] **Step 2: Write failing update test**

In `crates/lab/src/dispatch/marketplace/update.rs`, add:

```rust
#[test]
fn collect_versions_uses_single_artifact_path_from_origin() {
    let dir = tempfile::tempdir().unwrap();
    let stash = dir.path().join("stash");
    let source = dir.path().join("source");
    std::fs::create_dir_all(stash.join(".base/skills/demo")).unwrap();
    std::fs::create_dir_all(stash.join("skills/demo")).unwrap();
    std::fs::create_dir_all(source.join("skills/demo")).unwrap();
    std::fs::write(stash.join(".base/skills/demo/SKILL.md"), "base\n").unwrap();
    std::fs::write(stash.join("skills/demo/SKILL.md"), "mine\n").unwrap();
    std::fs::write(source.join("skills/demo/SKILL.md"), "theirs\n").unwrap();

    let meta = StashMeta {
        schema_version: 1,
        plugin_id: "demo@labby".into(),
        forked: true,
        upstream_id: Some("demo@labby".into()),
        upstream_version: "1.0.0".into(),
        fork_type: ForkType::Artifact,
        forked_artifacts: vec!["skills/demo/SKILL.md".into()],
        update_config: UpdateConfig::default(),
        pending_update: None,
    };

    let versions = collect_versions(&stash, &source, &meta).unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].path, "skills/demo/SKILL.md");
}
```

- [ ] **Step 3: Run update tests**

Run:

```bash
cargo test -p labby --all-features collect_versions_uses_single_artifact_path_from_origin
```

Expected: PASS on current code. This locks current merge behavior before redirecting storage.

- [ ] **Step 4: Redirect fork discovery to stash**

Replace `collect_forks` in `update.rs` with a version that reads stash components:

```rust
fn collect_forks(plugin_id: Option<String>) -> Result<Vec<ForkRecord>, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    let mut forks = Vec::new();
    for component in store.list_components()? {
        let Some(lab_apis::stash::StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if plugin_id.as_ref().is_some_and(|id| id != &origin.plugin_id) {
            continue;
        }
        let meta = StashMeta {
            schema_version: 1,
            plugin_id: origin.plugin_id.clone(),
            forked: true,
            upstream_id: Some(origin.plugin_id.clone()),
            upstream_version: origin.source_version.unwrap_or_else(|| "unknown".to_string()),
            fork_type: if origin.artifact_path.is_some() { ForkType::Artifact } else { ForkType::Plugin },
            forked_artifacts: origin.artifact_path.into_iter().collect(),
            update_config: UpdateConfig::default(),
            pending_update: None,
        };
        forks.push(ForkRecord {
            plugin_id: meta.plugin_id.clone(),
            stash: store.workspace_dir(&component.id),
            meta,
        });
    }
    Ok(forks)
}
```

- [ ] **Step 5: Keep `.base` under stash workspace for now**

In `artifact.fork`, after adoption succeeds, populate `.base` snapshots in the stash workspace by copying the original source file(s). Add this helper to `stash_bridge.rs`:

```rust
fn seed_base_snapshot(workspace_dir: &Path, source_path: &Path, artifact_path: Option<&str>) -> Result<(), ToolError> {
    let base = workspace_dir.join(".base");
    match artifact_path {
        Some(path) => {
            let dest = base.join(path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(crate::dispatch::marketplace::client::io_internal)?;
            }
            std::fs::copy(source_path, dest).map_err(crate::dispatch::marketplace::client::io_internal)?;
        }
        None => copy_tree_to_base(source_path, &base, source_path)?,
    }
    Ok(())
}

fn copy_tree_to_base(root: &Path, dest_root: &Path, current: &Path) -> Result<(), ToolError> {
    for entry in std::fs::read_dir(current).map_err(crate::dispatch::marketplace::client::io_internal)? {
        let entry = entry.map_err(crate::dispatch::marketplace::client::io_internal)?;
        let file_type = entry.file_type().map_err(crate::dispatch::marketplace::client::io_internal)?;
        if file_type.is_symlink() {
            continue;
        }
        let rel = entry.path().strip_prefix(root).map_err(crate::dispatch::marketplace::client::io_internal)?.to_path_buf();
        let dest = dest_root.join(rel);
        if file_type.is_dir() {
            std::fs::create_dir_all(&dest).map_err(crate::dispatch::marketplace::client::io_internal)?;
            copy_tree_to_base(root, dest_root, &entry.path())?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(crate::dispatch::marketplace::client::io_internal)?;
            }
            std::fs::copy(entry.path(), dest).map_err(crate::dispatch::marketplace::client::io_internal)?;
        }
    }
    Ok(())
}
```

Call `seed_base_snapshot` from `fork_artifacts` after `adopt_component_from_path`.

- [ ] **Step 6: Run update tests**

Run:

```bash
cargo test -p labby --all-features artifact_update
cargo test -p labby --all-features marketplace::tests::help_lists_artifact_actions
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/update.rs crates/lab/src/dispatch/marketplace/stash_bridge.rs
git commit -m "feat(marketplace): read artifact update state from stash"
```

---

## Task 6: Implement Unfork and Reset Against Stash

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/fork.rs`
- Modify: `crates/lab/src/dispatch/marketplace/stash_bridge.rs`
- Modify: `crates/lab/src/dispatch/marketplace/params.rs`
- Test: `crates/lab/src/dispatch/marketplace/fork.rs`

- [ ] **Step 1: Enforce `confirm` in destructive artifact params**

Update `parse_unfork_params`, `parse_artifact_reset_params`, and `parse_update_apply_params` in `crates/lab/src/dispatch/marketplace/params.rs` so each requires `confirm: true`.

Add this helper:

```rust
fn require_confirm(params: &Value, action: &'static str) -> Result<(), ToolError> {
    if params.get("confirm").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(ToolError::InvalidParam {
        param: "confirm".into(),
        message: format!("`confirm: true` is required for `{action}`"),
    })
}
```

Call it in each destructive parser.

- [ ] **Step 2: Write confirm tests**

Add to the `params.rs` test module:

```rust
#[test]
fn parse_unfork_requires_confirm_true() {
    let err = parse_unfork_params(&serde_json::json!({"plugin_id": "demo@labby"})).unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
    assert!(parse_unfork_params(&serde_json::json!({"plugin_id": "demo@labby", "confirm": true})).is_ok());
}

#[test]
fn parse_reset_requires_confirm_true() {
    let err = parse_artifact_reset_params(&serde_json::json!({"plugin_id": "demo@labby"})).unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
    assert!(parse_artifact_reset_params(&serde_json::json!({"plugin_id": "demo@labby", "confirm": true})).is_ok());
}
```

- [ ] **Step 3: Implement bridge unfork**

Add to `stash_bridge.rs`:

```rust
#[derive(Debug, Serialize)]
pub(super) struct UnforkResult {
    pub plugin_id: String,
    pub removed_component_ids: Vec<String>,
}

pub(super) async fn unfork(plugin_id: &str, artifacts: Option<Vec<String>>) -> Result<Value, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    let mut removed = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id != plugin_id {
            continue;
        }
        if let Some(filter) = &artifacts {
            let Some(path) = origin.artifact_path.as_ref() else {
                continue;
            };
            if !filter.iter().any(|candidate| candidate == path) {
                continue;
            }
        }
        store.delete_component(&component.id)?;
        removed.push(component.id);
    }
    crate::dispatch::helpers::to_json(UnforkResult {
        plugin_id: plugin_id.to_string(),
        removed_component_ids: removed,
    })
}
```

If `StashStore` does not expose `delete_component`, use its existing component deletion method name. If there is no deletion method, add one in `store.rs` that removes the component record, revision index, provider index, provider records, and workspace using the already-tested deletion logic in that file.

- [ ] **Step 4: Implement bridge reset**

Add to `stash_bridge.rs`:

```rust
#[derive(Debug, Serialize)]
pub(super) struct ResetResult {
    pub plugin_id: String,
    pub reset_artifacts: Vec<String>,
}

pub(super) async fn reset(plugin_id: &str, artifacts: Option<Vec<String>>) -> Result<Value, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    let mut reset_artifacts = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id != plugin_id {
            continue;
        }
        let workspace = store.workspace_dir(&component.id);
        let base = workspace.join(".base");
        let paths: Vec<String> = match artifacts.clone() {
            Some(paths) => paths,
            None => origin.artifact_path.into_iter().collect(),
        };
        for rel in paths {
            crate::dispatch::marketplace::stash_meta::validate_rel_path(&rel)?;
            let source = base.join(&rel);
            let target = workspace.join(&rel);
            if !source.exists() {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("base snapshot `{rel}` is missing"),
                });
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(crate::dispatch::marketplace::client::io_internal)?;
            }
            std::fs::copy(&source, &target).map_err(crate::dispatch::marketplace::client::io_internal)?;
            reset_artifacts.push(rel);
        }
    }
    crate::dispatch::helpers::to_json(ResetResult {
        plugin_id: plugin_id.to_string(),
        reset_artifacts,
    })
}
```

- [ ] **Step 5: Route unfork/reset**

In `crates/lab/src/dispatch/marketplace/fork.rs`, replace bodies:

```rust
pub(super) async fn artifact_unfork(params: UnforkParams) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "dispatch",
        service = "marketplace",
        action = "artifact.unfork",
        plugin_id = %params.plugin_id,
        "destructive action intent: removing marketplace fork from stash"
    );
    crate::dispatch::marketplace::stash_bridge::unfork(&params.plugin_id, params.artifacts).await
}

pub(super) async fn artifact_reset(params: ArtifactResetParams) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "dispatch",
        service = "marketplace",
        action = "artifact.reset",
        plugin_id = %params.plugin_id,
        "destructive action intent: resetting forked artifact from base snapshot"
    );
    crate::dispatch::marketplace::stash_bridge::reset(&params.plugin_id, params.artifacts).await
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p labby --all-features parse_unfork_requires_confirm_true parse_reset_requires_confirm_true
cargo test -p labby --all-features stash_bridge
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/fork.rs crates/lab/src/dispatch/marketplace/params.rs crates/lab/src/dispatch/marketplace/stash_bridge.rs crates/lab/src/dispatch/stash/store.rs
git commit -m "feat(marketplace): manage fork reset and removal through stash"
```

---

## Task 7: Add Frontend API Helpers and Fork UI

**Files:**
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`
- Create: `apps/gateway-admin/lib/api/marketplace-artifacts.test.ts`
- Modify: `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`
- Test: `apps/gateway-admin/components/marketplace/plugin-files-panel.test.tsx`

- [ ] **Step 1: Add frontend client tests**

Create `apps/gateway-admin/lib/api/marketplace-artifacts.test.ts`:

```ts
import { test, expect, vi, beforeEach, afterEach } from 'vitest'

import {
  forkMarketplaceArtifact,
  listMarketplaceForks,
  resetMarketplaceArtifact,
  unforkMarketplaceArtifact,
} from './marketplace-client'

const originalFetch = global.fetch

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ ok: true }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  })))
})

afterEach(() => {
  vi.stubGlobal('fetch', originalFetch)
})

test('forkMarketplaceArtifact posts artifact.fork', async () => {
  await forkMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
  const body = JSON.parse(String((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0][1].body))
  expect(body).toEqual({
    action: 'artifact.fork',
    params: {
      plugin_id: 'demo@labby',
      artifacts: ['skills/demo/SKILL.md'],
    },
  })
})

test('destructive artifact helpers include confirm true', async () => {
  await resetMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
  await unforkMarketplaceArtifact({ pluginId: 'demo@labby', artifacts: ['skills/demo/SKILL.md'] })
  const resetBody = JSON.parse(String((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0][1].body))
  const unforkBody = JSON.parse(String((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[1][1].body))
  expect(resetBody.params.confirm).toBe(true)
  expect(unforkBody.params.confirm).toBe(true)
})

test('listMarketplaceForks posts artifact.list', async () => {
  await listMarketplaceForks('demo@labby')
  const body = JSON.parse(String((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0][1].body))
  expect(body).toEqual({
    action: 'artifact.list',
    params: { plugin_id: 'demo@labby' },
  })
})
```

- [ ] **Step 2: Run frontend test to verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin exec vitest run lib/api/marketplace-artifacts.test.ts
```

Expected: FAIL because helper exports do not exist.

- [ ] **Step 3: Add client helpers**

Append to `apps/gateway-admin/lib/api/marketplace-client.ts`:

```ts
export interface ForkMarketplaceArtifactInput {
  pluginId: string
  artifacts?: string[]
}

export interface MarketplaceForkStatus {
  plugin_id: string
  component_id: string
  stash_workspace: string
  forked_artifacts: string[]
  dirty: boolean
}

export function forkMarketplaceArtifact(input: ForkMarketplaceArtifactInput, signal?: AbortSignal): Promise<unknown> {
  return marketplaceAction('artifact.fork', {
    plugin_id: input.pluginId,
    ...(input.artifacts?.length ? { artifacts: input.artifacts } : {}),
  }, signal)
}

export function listMarketplaceForks(pluginId?: string, signal?: AbortSignal): Promise<MarketplaceForkStatus[]> {
  return marketplaceAction('artifact.list', {
    ...(pluginId ? { plugin_id: pluginId } : {}),
  }, signal)
}

export function resetMarketplaceArtifact(input: ForkMarketplaceArtifactInput, signal?: AbortSignal): Promise<unknown> {
  return marketplaceAction('artifact.reset', {
    plugin_id: input.pluginId,
    ...(input.artifacts?.length ? { artifacts: input.artifacts } : {}),
    confirm: true,
  }, signal)
}

export function unforkMarketplaceArtifact(input: ForkMarketplaceArtifactInput, signal?: AbortSignal): Promise<unknown> {
  return marketplaceAction('artifact.unfork', {
    plugin_id: input.pluginId,
    ...(input.artifacts?.length ? { artifacts: input.artifacts } : {}),
    confirm: true,
  }, signal)
}
```

- [ ] **Step 4: Add a fork button in plugin files panel**

In `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`, import:

```ts
import { forkMarketplaceArtifact } from '@/lib/api/marketplace-client'
```

Add state near the existing panel state:

```ts
const [forkingPath, setForkingPath] = useState<string | null>(null)
```

Add handler inside `PluginFilesPanel`:

```ts
async function handleForkSelectedFile() {
  if (!activeFile) return
  setForkingPath(activeFile.path)
  try {
    await forkMarketplaceArtifact({ pluginId, artifacts: [activeFile.path] })
    setStatus({
      tone: 'success',
      message: 'Forked to Stash',
      detail: activeFile.path,
    })
  } catch (error) {
    setStatus({
      tone: 'error',
      message: 'Fork failed',
      detail: error instanceof Error ? error.message : 'Unable to fork artifact into Stash.',
    })
  } finally {
    setForkingPath(null)
  }
}
```

Add this button beside the existing save/deploy controls:

```tsx
<button
  type="button"
  onClick={() => { void handleForkSelectedFile() }}
  disabled={!activeFile || forkingPath === activeFile.path}
  className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-3 py-1.5 text-[12px] font-semibold text-aurora-text-primary hover:bg-aurora-hover-bg disabled:cursor-not-allowed disabled:opacity-50"
>
  {forkingPath === activeFile?.path ? 'Forking...' : 'Fork to Stash'}
</button>
```

- [ ] **Step 5: Run frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin exec vitest run lib/api/marketplace-artifacts.test.ts components/marketplace/plugin-files-panel.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib/api/marketplace-client.ts apps/gateway-admin/lib/api/marketplace-artifacts.test.ts apps/gateway-admin/components/marketplace/plugin-files-panel.tsx apps/gateway-admin/components/marketplace/plugin-files-panel.test.tsx
git commit -m "feat(gateway-admin): expose marketplace artifact forks"
```

---

## Task 8: Docs, Generated Catalogs, and Full Verification

**Files:**
- Modify: `docs/coverage/stash.md`
- Modify: `docs/features/artifact-diffs.md`
- Generated: `docs/generated/action-catalog.json`
- Generated: `docs/generated/action-catalog.md`
- Generated: `docs/generated/cli-help.md`
- Generated: `docs/generated/mcp-help.json`
- Generated: `docs/generated/mcp-help.md`
- Generated: `docs/generated/openapi.json`
- Generated: `docs/generated/service-catalog.json`
- Generated: `docs/generated/service-catalog.md`

- [ ] **Step 1: Update stash coverage doc**

In `docs/coverage/stash.md`, add this section after "Store Layout":

```markdown
## Marketplace-Origin Components

Marketplace artifact forks are stored as normal stash components with
`origin_meta.kind = "marketplace"`. Marketplace owns source discovery, upstream
version checks, and merge/diff presentation. Stash owns the copied workspace,
saved revisions, provider sync, export, and deploy handoff.

Primary entry points:

| Surface | Action | Purpose |
|---------|--------|---------|
| marketplace | `artifact.fork` | Copy one plugin artifact or a whole plugin into stash |
| marketplace | `artifact.list` | List stash components whose origin is marketplace |
| marketplace | `artifact.update.*` | Compare stash edits against marketplace upstream |
| stash | `component.adopt` | Generic create/import/save action used by marketplace |
```

- [ ] **Step 2: Update artifact diffs feature doc**

In `docs/features/artifact-diffs.md`, add this under "Required Capabilities":

```markdown
## Implementation Boundary

Forked marketplace artifacts are durable stash components. The Marketplace
surface is the user-facing upstream workflow: fork, list, update preview, update
apply, reset, and unfork. The Stash surface is the durable artifact library:
workspace, immutable revisions, provider sync, export, and deploy handoff.
```

- [ ] **Step 3: Regenerate docs**

Run:

```bash
just docs-generate
```

Expected: generated docs update cleanly and no command fails.

- [ ] **Step 4: Run Rust verification**

Run:

```bash
cargo fmt --all -- --check
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features -- -D warnings
```

Expected: all pass.

- [ ] **Step 5: Run frontend verification**

Run:

```bash
pnpm --dir apps/gateway-admin exec vitest run lib/api/marketplace-artifacts.test.ts components/marketplace/plugin-files-panel.test.tsx
pnpm --dir apps/gateway-admin exec tsc --noEmit
```

Expected: all pass.

- [ ] **Step 6: Smoke test action catalog behavior**

Run:

```bash
cargo run -p labby --all-features -- marketplace schema --params '{"action":"artifact.fork"}'
cargo run -p labby --all-features -- stash schema --params '{"action":"component.adopt"}'
```

Expected: both commands return JSON schemas and neither reports `unknown_action`.

- [ ] **Step 7: Commit**

```bash
git add docs/coverage/stash.md docs/features/artifact-diffs.md docs/generated
git commit -m "docs: document marketplace stash integration"
```

---

## Self-Review

### Spec Coverage

- Marketplace artifact forks become stash components: Tasks 3 and 4.
- Stash remains the durable owner of component identity, workspaces, revisions, providers, export, and deploy: Tasks 1 and 2.
- Marketplace remains the owner of plugin discovery and update/merge UX: Tasks 3, 4, and 5.
- The existing parallel marketplace `.stash.json` concept is replaced or constrained to marketplace merge metadata under stash workspaces: Task 5.
- Destructive actions require confirmation: Task 6.
- Frontend can initiate forks: Task 7.
- Docs and generated catalogs stay current: Task 8.

### Placeholder Scan

This plan intentionally avoids placeholder implementation steps. The only conditional instruction is in Task 6 for the `StashStore` deletion method because the exact method name must be verified in the current checkout before editing; the fallback is fully specified.

### Type Consistency

- `StashOrigin::Marketplace(MarketplaceOrigin)` is introduced in Task 1 and used by Tasks 2 through 6.
- `component.adopt` accepts `origin` as `StashOrigin` JSON and returns `{ component, revision }`.
- Marketplace bridge response fields use snake_case to match existing Rust JSON action conventions.
