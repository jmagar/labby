# Stash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `stash` as a new always-on first-class capability service for importing, versioning, syncing, deploying, and exporting all Agent Artifacts across CLI, MCP, and HTTP. Stash is the staging buffer between authoring and deployment — reducing configuration drift across machines and Agents.

**Architecture:** `stash` follows the same first-class service shape as `marketplace`, but owns a distinct authored-component lifecycle. Pure types live in `crates/lab-apis`, while the canonical local store, import/save/export logic, provider adapters, and action dispatch live under `crates/lab/src/dispatch/stash/`. The CLI, MCP, and HTTP layers remain thin adapters over the shared dispatch layer.

**Storage model:** Component workspaces live under `stash/workspaces/<id>/`. A workspace is either a directory tree (most kinds) or a single file (LspConfig, McpConfig, Settings, BinFile, Script). Revision snapshots are immutable copies stored at `stash/revisions/<rev_id>/files/` with a `meta.json` sibling. There is no separate content-addressed object store — revision directories are the artifacts. SHA-256 of the entire snapshot content tree is stored as `content_digest` for integrity verification. Deploy targets are stored in `stash/targets/`.

**Tech Stack:** Rust 2024, `serde`, `thiserror`, `tokio`, `tracing`, `fd-lock` (per-component advisory locks), `futures` (buffered concurrent reads), existing `lab` dispatch/catalog/registry patterns, filesystem store, workspace-level tests.

---

## Scope and delivery order

Build this in vertical slices so each phase leaves the repo in a coherent state:

1. Define the `stash` domain types and action catalog.
2. Build a local-only `stash` store with import, workspace inspection, revision save, revisions list, and export.
3. Expose the local-only feature through CLI, MCP, and HTTP.
4. Add provider abstraction and the `filesystem` provider.
5. Finish observability, docs, and end-to-end verification.

**Explicitly deferred (not in v1):**
- Google Drive provider — requires a `crates/lab-apis/src/google_drive/` OAuth client that does not yet exist in this repo.
- Marketplace import integration — cross-service dispatch coupling; use CLI pipeline instead (`lab marketplace plugin.workspace ... | lab stash component.import`).

## File map

### New files

- `crates/lab-apis/src/stash.rs`
  - public `stash` module entrypoint and `META`
- `crates/lab-apis/src/stash/types.rs`
  - pure stash domain types, enums, summaries, provider capability flags, `StashLimits` constants
- `crates/lab/src/dispatch/stash.rs`
  - service entrypoint re-exporting catalog + dispatch
- `crates/lab/src/dispatch/stash/catalog.rs`
  - `ActionSpec[]` definitions for all stash actions
- `crates/lab/src/dispatch/stash/client.rs`
  - `StashClient` — stash root-path resolution, `not_configured_error()`, `StashLimits` enforcement helpers
- `crates/lab/src/dispatch/stash/params.rs`
  - typed request params parsing and validation
- `crates/lab/src/dispatch/stash/dispatch.rs`
  - main action router
- `crates/lab/src/dispatch/stash/service.rs`
  - high-level stash service orchestration methods
- `crates/lab/src/dispatch/stash/store.rs`
  - canonical local state store, path layout helpers, per-component advisory lock
- `crates/lab/src/dispatch/stash/import.rs`
  - import detection and workspace materialization (path safety, kind detection, limits enforcement)
- `crates/lab/src/dispatch/stash/revision.rs`
  - immutable save/snapshot logic, SHA-256 digest, `spawn_blocking` fan-out, symlink rejection
- `crates/lab/src/dispatch/stash/export.rs`
  - export/handoff logic — path containment, overwrite protection, concurrent reads
- `crates/lab/src/dispatch/stash/provider.rs`
  - `StashProvider` trait, capability types, provider registry
- `crates/lab/src/dispatch/stash/providers.rs`
  - provider module wiring — sibling to `providers/` directory; **never** `providers/mod.rs`
- `crates/lab/src/dispatch/stash/providers/filesystem.rs`
  - filesystem provider adapter
- `crates/lab/src/dispatch/path_safety.rs`
  - **extracted shared module** containing `reject_path_traversal`, `reject_symlink`, `ensure_target_within_write_root` — extracted from `crates/lab/src/dispatch/acp/` so stash and marketplace share a single source of truth for path validation; extract as part of Task 2
- `crates/lab/src/cli/stash.rs`
  - CLI shim for stash subcommands
- `crates/lab/src/api/services/stash.rs`
  - HTTP route group + handlers
- `crates/lab/src/mcp/services/stash.rs`
  - MCP adapter
- `docs/STASH.md`
  - product and operator documentation for the new feature
- `docs/coverage/stash.md`
  - coverage/status doc if the repo's service coverage convention requires one

### Existing files to modify

- `crates/lab-apis/src/lib.rs`
  - export the new always-on `stash` module
- `crates/lab/Cargo.toml`
  - add `fd-lock` for per-component advisory locks
- `crates/lab/src/registry.rs`
  - register `stash` as an always-on first-class service with catalog + dispatch
- `crates/lab/src/cli.rs`
  - add `stash` command group
- `crates/lab/src/api/router.rs`
  - mount `/v1/stash`
- `crates/lab/src/catalog.rs`
  - include stash in top-level help/catalog if required by current registry pattern
- `crates/lab/src/tui/metadata.rs`
  - include stash metadata if product-local always-on capabilities are shown there
- `docs/README.md`
  - link `docs/STASH.md`
- `docs/SERVICES.md`
  - add stash to the first-class capability inventory
- `docs/MCP.md`
  - mention `stash` in service examples if the doc enumerates notable always-on services
- `docs/CLI.md`
  - add stash command behavior
- `docs/OBSERVABILITY.md`
  - update only if stash introduces a new owned dispatch pattern or verification example
- `docs/ERRORS.md`
  - add new stable error kinds: `conflict`, `unsupported_provider`, `unsupported_component_kind`, `sync_failed`, `workspace_too_large`, `file_too_large`, `path_traversal`, `symlink_rejected`, `export_target_not_empty`, `ambiguous_kind`

### Likely test files

- `crates/lab/tests/stash_cli.rs`
- `crates/lab/tests/stash_api.rs`
- `crates/lab/tests/stash_mcp.rs`
- `crates/lab/src/dispatch/stash/tests.rs`
- `crates/lab/src/dispatch/stash/store_tests.rs`
- `crates/lab/src/dispatch/stash/import_tests.rs`
- `crates/lab/src/dispatch/stash/revision_tests.rs`
- `crates/lab/src/dispatch/stash/provider_tests.rs`

## Task 1: Define pure stash domain types

**Files:**
- Create: `crates/lab-apis/src/stash.rs`
- Create: `crates/lab-apis/src/stash/types.rs`
- Modify: `crates/lab-apis/src/lib.rs`
- Test: `crates/lab-apis/src/stash/types.rs` unit tests if colocated, or equivalent type serialization tests

- [ ] **Step 1: Write failing type serialization tests for the core model**

```rust
#[test]
fn stash_component_kind_serializes_as_expected() {
    let json = serde_json::to_string(&StashComponentKind::McpConfig).unwrap();
    assert_eq!(json, "\"mcp_config\"");
}

#[test]
fn provider_capabilities_round_trip() {
    let caps = StashProviderCapabilities {
        push_revision: true,
        pull_revision: true,
        list_remote: true,
        supports_metadata: false,
    };
    let json = serde_json::to_string(&caps).unwrap();
    let decoded: StashProviderCapabilities = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, caps);
}
```

- [ ] **Step 2: Run the focused type tests to verify they fail**

Run: `cargo test -p lab-apis stash_component_kind_serializes_as_expected provider_capabilities_round_trip`
Expected: FAIL because `stash` types do not exist yet

- [ ] **Step 3: Implement the pure stash types**

Define the minimum initial model:

```rust
/// All first-class artifact types managed by stash. Do NOT collapse into coarser
/// categories — each has distinct detection heuristics and deploy semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StashComponentKind {
    Agent,
    Skill,
    Command,
    Channel,
    Monitor,
    Hook,
    OutputStyle,
    Theme,
    Script,
    BinFile,
    LspConfig,   // .lsp.json
    McpConfig,   // .mcp.json
    Settings,    // settings.json
}

/// Provider capability flags. Does NOT include `content_addressed` — stash uses
/// snapshot directories, not a hash-addressed object store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StashProviderCapabilities {
    pub push_revision: bool,
    pub pull_revision: bool,
    pub list_remote: bool,
    pub supports_metadata: bool,
}

/// Hard limits enforced at import and snapshot boundaries. Check before walking,
/// not after — return a structured error rather than running for minutes then failing.
pub struct StashLimits;
impl StashLimits {
    pub const MAX_FILES: usize = 10_000;
    pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;      // 50 MB per file
    pub const MAX_SNAPSHOT_BYTES: u64 = 500 * 1024 * 1024; // 500 MB total
    /// Soft cap for components.list — O(n) scan; document in STASH.md as a known limitation.
    pub const MAX_COMPONENTS: usize = 10_000;
    /// Deploy operations time out after this many ms to avoid holding deploy locks indefinitely.
    pub const DEPLOY_TIMEOUT_MS: u64 = 30_000;
    pub const DEFAULT_EXCLUDES: &'static [&'static str] = &[
        ".git", "target", "node_modules", "__pycache__", "dist", ".next",
    ];
}
```

Also add:

```rust
/// Whether a component's workspace is a single file or a directory tree.
/// Single-file kinds: LspConfig, McpConfig, Settings, BinFile, Script.
/// All others are directory trees.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StashWorkspaceShape {
    File,
    Directory,
}

impl StashComponentKind {
    pub fn workspace_shape(&self) -> StashWorkspaceShape {
        match self {
            Self::LspConfig | Self::McpConfig | Self::Settings
            | Self::BinFile | Self::Script => StashWorkspaceShape::File,
            _ => StashWorkspaceShape::Directory,
        }
    }
}

/// A configured deploy target. Each variant carries only the fields meaningful
/// for that target kind — no shared `path: String` tagged-union smell.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StashDeployTarget {
    /// Install to a local filesystem path.
    Local { id: String, name: String, path: PathBuf },
    /// Install via a remote gateway connection.
    Remote { id: String, name: String, gateway_id: String },
}

impl StashDeployTarget {
    pub fn id(&self) -> &str {
        match self { Self::Local { id, .. } | Self::Remote { id, .. } => id }
    }
    pub fn name(&self) -> &str {
        match self { Self::Local { name, .. } | Self::Remote { name, .. } => name }
    }
}

/// Options for the component.export action.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StashExportOptions {
    /// If false (default), exporting Settings or McpConfig kinds returns
    /// `secrets_export_not_allowed`. Set to true and mark the action
    /// `destructive: true` to include credential-containing files in the export.
    pub include_secrets: bool,
    /// Overwrite a non-empty target directory.
    pub force: bool,
}
```

Also add structs for:
- `StashComponent` — `id`, `kind`, `name`, `head_revision_id: Option<String>`, `origin: Option<String>`, `workspace_root: PathBuf`, `workspace_shape: StashWorkspaceShape`
- `StashRevision` — `id`, `component_id`, `label: Option<String>`, `content_digest` (SHA-256 hex), `created_at`, `file_count`, `unix_mode: Option<u32>` (BinFile only — stored as `mode & 0o0755`; setuid/setgid/sticky bits are stripped at save time and never restored)
- `StashProviderRecord` — `id`, `name`, `root` (filesystem path), `capabilities: StashProviderCapabilities`, `remote_head: Option<String>`
- `StashProviderSummary` — display summary for `providers.list`

**Note:** `StashArtifact` and `StashWorkspace` are not separate types. Revision files live at `stash/revisions/<rev_id>/files/` — there is no separate object store.

Expose them via `crates/lab-apis/src/stash.rs`, and add a `META` entry appropriate for an always-on capability service.

- [ ] **Step 4: Run the focused tests again**

Run: `cargo test -p lab-apis stash_component_kind_serializes_as_expected provider_capabilities_round_trip`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/stash.rs crates/lab-apis/src/stash/types.rs crates/lab-apis/src/lib.rs
git commit -m "feat: add stash domain types"
```

## Task 2: Define the stash action catalog and typed params

**Files:**
- Create: `crates/lab/src/dispatch/stash.rs`
- Create: `crates/lab/src/dispatch/stash/catalog.rs`
- Create: `crates/lab/src/dispatch/stash/client.rs`
- Create: `crates/lab/src/dispatch/stash/params.rs`
- Test: `crates/lab/src/dispatch/stash/params.rs` tests

- [ ] **Step 1: Write failing tests for action validation and param parsing**

```rust
#[test]
fn import_requires_exactly_one_source() {
    let err = StashImportParams::try_from_value(json!({})).unwrap_err();
    assert_eq!(err.kind(), "missing_param");
}

#[test]
fn import_rejects_both_path_and_plugin() {
    let err = StashImportParams::try_from_value(json!({
        "path": "/tmp/skill",
        "plugin": "foo@bar"
    })).unwrap_err();
    assert_eq!(err.kind(), "invalid_param");
}
```

- [ ] **Step 2: Run the focused tests to verify they fail**

Run: `cargo test -p lab import_requires_exactly_one_source import_rejects_both_path_and_plugin`
Expected: FAIL because stash dispatch/params do not exist yet

- [ ] **Step 3: Extract `dispatch/path_safety.rs` and implement the action catalog, `StashClient`, and param types**

**First:** extract the three-layer path safety helpers (`reject_path_traversal`, `reject_symlink`, `ensure_target_within_write_root`) from `crates/lab/src/dispatch/acp/` into a new `crates/lab/src/dispatch/path_safety.rs`. Update the existing ACP imports to use the shared module. This ensures stash imports the same helpers rather than copying them.

Create `client.rs` with `StashClient` — stash root-path resolution under lab state, `not_configured_error()`, and `StashLimits` as accessible constants. This file is required by the dispatch layer convention even though stash has no remote API.

Add action specs for the following 16 actions:

*Component lifecycle:*
- `components.list`
- `component.get`
- `component.create` — create a new empty component in the managed workspace (for authoring from scratch); params: `kind` (required, `StashComponentKind`), `name` (required, `String`), `label` (optional, `String`); creates an empty workspace directory or empty file depending on `kind.workspace_shape()`
- `component.import` — **`destructive: true`** (writes to managed workspace from external source)
- `component.workspace`
- `component.save`
- `component.revisions`
- `component.export` — **`destructive: true`** (writes to caller-specified directory)
- `component.deploy` — **`destructive: true`** (installs a revision to a configured deploy target)

*Provider sync:*
- `providers.list`
- `provider.link`
- `provider.push` — **`destructive: true`** (mutates remote provider state)
- `provider.pull` — **`destructive: true`** (overwrites local revision/workspace state)

*Deploy targets:*
- `targets.list`
- `target.add`
- `target.remove` — **`destructive: true`**

**`component.fetch` is not included** — semantics overlap with `component.get`/`provider.pull`. Add in a future iteration if a concrete use case emerges.

**`provider.sync.status` is not included** — implies background polling infrastructure that does not exist in `lab`.

Define typed param structs with validation helpers. Keep `help` and `schema` implicit through the shared service model.

- [ ] **Step 4: Run the focused tests again**

Run: `cargo test -p lab import_requires_exactly_one_source import_rejects_both_path_and_plugin`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash.rs crates/lab/src/dispatch/stash/catalog.rs crates/lab/src/dispatch/stash/client.rs crates/lab/src/dispatch/stash/params.rs
git commit -m "feat: add stash action catalog"
```

## Task 3: Build the canonical local stash store

**Files:**
- Create: `crates/lab/src/dispatch/stash/store.rs`
- Modify: `crates/lab/Cargo.toml`
- Test: `crates/lab/src/dispatch/stash/store_tests.rs`

- [ ] **Step 1: Write failing tests for path layout, atomic metadata persistence, and per-component locking**

```rust
#[tokio::test]
async fn init_creates_required_stash_directories() {
    let root = tempfile::tempdir().unwrap();
    let store = StashStore::new(root.path().join("stash"));
    store.init().await.unwrap();

    assert!(root.path().join("stash/components").exists());
    assert!(root.path().join("stash/revisions").exists());
    assert!(root.path().join("stash/workspaces").exists());
    assert!(root.path().join("stash/providers").exists());
    assert!(root.path().join("stash/targets").exists());
    // No stash/objects — revision content lives at stash/revisions/<id>/files/
}

#[tokio::test]
async fn concurrent_saves_serialize_via_component_lock() {
    let (store, component) = store_with_imported_component().await;
    let (r1, r2) = tokio::join!(
        store.with_component_lock(&component.id, || save_revision_inner(&store, &component)),
        store.with_component_lock(&component.id, || save_revision_inner(&store, &component)),
    );
    assert!(r1.is_ok());
    assert!(r2.is_ok());
    let meta = store.read_component(&component.id).await.unwrap();
    assert!(meta.head_revision_id.is_some());
}
```

- [ ] **Step 2: Run the store tests to verify they fail**

Run: `cargo test -p lab init_creates_required_stash_directories concurrent_saves_serialize_via_component_lock`
Expected: FAIL because `StashStore` does not exist yet

- [ ] **Step 3: Implement `StashStore` with managed-root initialization, metadata helpers, and per-component advisory locks**

Minimum responsibilities:
- resolve managed stash root under lab state
- initialize required subdirectories: `components/`, `revisions/`, `workspaces/`, `providers/`, `targets/`
- read/write component metadata records atomically (temp-file + rename, fsync parent dir)
- read/write revision metadata atomically (same pattern)
- read/write deploy target records atomically under `targets/`
- `revision_files_path(rev_id)` → `stash/revisions/<rev_id>/files/`
- workspace path from component id → `stash/workspaces/<id>/` for directory-shaped components; `stash/workspaces/<id>/<filename>` for single-file components (shape determined by `StashComponentKind::workspace_shape()`)
- `with_component_lock(id, f)` — acquires a per-component advisory lock via `fd-lock` on `components/<id>.lock`, runs `f`, releases; serializes all state mutations for a given component across processes
- `with_deploy_lock(id, f)` — acquires a **separate** advisory lock on `components/<id>.deploy.lock`; used exclusively by `component.deploy` so a slow or remote deploy does not block concurrent workspace reads or saves on the same component

**Per-component lock is mandatory.** All callers that mutate component state (save, import into workspace, head update, provider pull) must use `with_component_lock`. Concurrent readers (`get`, `revisions`) do not need the lock. Deploy operations use `with_deploy_lock` — not `with_component_lock` — so they do not block the workspace path.

Add `fd-lock` to `crates/lab/Cargo.toml`.

- [ ] **Step 4: Run the store tests again**

Run: `cargo test -p lab init_creates_required_stash_directories concurrent_saves_serialize_via_component_lock`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash/store.rs crates/lab/src/dispatch/stash/store_tests.rs crates/lab/Cargo.toml
git commit -m "feat: add stash local store"
```

## Task 4: Implement import detection and workspace materialization

**Files:**
- Create: `crates/lab/src/dispatch/stash/import.rs`
- Modify: `crates/lab/src/dispatch/stash/store.rs`
- Test: `crates/lab/src/dispatch/stash/import_tests.rs`

- [ ] **Step 1: Write failing tests for importing a local skill directory, path safety, and limits**

```rust
#[tokio::test]
async fn import_local_skill_copies_into_managed_workspace() {
    let fixture = make_skill_fixture();
    let imported = service.import_from_path(&fixture, None).await.unwrap();

    assert_eq!(imported.kind, StashComponentKind::Skill);
    assert!(imported.origin.is_some());
    assert!(imported.workspace_root.exists());
}

#[tokio::test]
async fn import_rejects_path_traversal() {
    let err = service.import_from_path(Path::new("/tmp/../../etc"), None).await.unwrap_err();
    assert_eq!(err.kind(), "path_traversal");
}

#[tokio::test]
async fn import_rejects_symlinks_in_source() {
    let fixture = make_fixture_with_symlink();
    let err = service.import_from_path(&fixture, None).await.unwrap_err();
    assert_eq!(err.kind(), "symlink_rejected");
}

#[tokio::test]
async fn import_enforces_file_count_limit() {
    let fixture = make_large_fixture(StashLimits::MAX_FILES + 1);
    let err = service.import_from_path(&fixture, None).await.unwrap_err();
    assert_eq!(err.kind(), "workspace_too_large");
}

#[tokio::test]
async fn import_rejects_ambiguous_kind_without_override() {
    let fixture = make_ambiguous_fixture(); // contains both skill.md and agent.md
    let err = service.import_from_path(&fixture, None).await.unwrap_err();
    assert_eq!(err.kind(), "ambiguous_kind");
}
```

- [ ] **Step 2: Run the focused import tests to verify they fail**

Run: `cargo test -p lab import_local_skill_copies_into_managed_workspace import_rejects_path_traversal import_rejects_symlinks_in_source import_enforces_file_count_limit import_rejects_ambiguous_kind_without_override`
Expected: FAIL because import logic does not exist yet

- [ ] **Step 3: Implement kind detection and managed-copy materialization**

Support:
- path import with **mandatory path safety** (see below)
- source can be a file or a directory — detect via `symlink_metadata` before walking
- single-file sources: LspConfig, McpConfig, Settings, BinFile, Script — copy the file to `workspaces/<id>/<filename>`
- directory sources: Agent, Skill, Command, Channel, Monitor, Hook, OutputStyle, Theme — copy tree to `workspaces/<id>/`
- optional explicit kind override; return `ambiguous_kind` error (not a guess) when detection is ambiguous and no override is provided
- workspace copy via `spawn_blocking` + `tokio::fs::copy`
- `origin` set to the canonical absolute source path as an `Option<String>`
- entire import wrapped in `with_component_lock` on the new component id
- **BinFile:** preserve execute permission bits from source via `std::fs::metadata().permissions().mode()` and store in component metadata

**Kind detection rules (canonical — do not invent alternatives):**

| Source shape | Filename / contents match | Detected kind |
|---|---|---|
| Single file | `*.lsp.json` or filename `.lsp.json` | `LspConfig` |
| Single file | `*.mcp.json` or filename `.mcp.json` | `McpConfig` |
| Single file | `settings.json` | `Settings` |
| Single file | `*.sh`, `*.py`, `*.rb`, `*.js`, `*.ts` | `Script` (extension takes priority over executable bit — a `chmod +x foo.sh` is still `Script`) |
| Single file | no known extension + executable bit set | `BinFile` |
| Single file | anything else | `ambiguous_kind` — require explicit override |
| Directory | contains `SKILL.md` or `skill.md` | `Skill` |
| Directory | contains `AGENT.md` or `agent.md` | `Agent` |
| Directory | contains `command.json` or `COMMAND.md` | `Command` |
| Directory | contains `channel.json` or `CHANNEL.md` | `Channel` |
| Directory | contains `monitor.json` or `MONITOR.md` | `Monitor` |
| Directory | contains `hook.json` or `HOOK.md` | `Hook` |
| Directory | contains `output-style.json` | `OutputStyle` |
| Directory | contains `theme.json` or `theme.css` | `Theme` |
| Directory | multiple markers match | `ambiguous_kind` — require explicit override |
| Directory | no markers match | `ambiguous_kind` — require explicit override |

**Required path safety — import from `crates/lab/src/dispatch/path_safety.rs` (extracted in Task 2):**
1. `reject_path_traversal(path)` — reject any path component that is not `Component::Normal(_)`
2. `reject_symlink(path)` — use `tokio::fs::symlink_metadata` (not `metadata`) to detect symlinks before any read; reject both the source root and every file encountered during the copy walk
3. `ensure_target_within_write_root(workspace_root, target_path)` — canonicalize both the managed workspace root and each target parent; assert `canonical_parent.starts_with(&canonical_root)`

**`StashLimits` enforcement — check before copying, not after:**
- Count files and sum bytes before starting the copy; return `workspace_too_large` (file count) or `file_too_large` (single file) if any limit is exceeded
- Apply `StashLimits::DEFAULT_EXCLUDES` patterns before counting

Return `validation_failed` on unsupported component layouts.

- [ ] **Step 4: Run the focused import tests again**

Run: `cargo test -p lab import_local_skill_copies_into_managed_workspace import_rejects_path_traversal import_rejects_symlinks_in_source import_enforces_file_count_limit import_rejects_ambiguous_kind_without_override`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash/import.rs crates/lab/src/dispatch/stash/store.rs crates/lab/src/dispatch/stash/import_tests.rs
git commit -m "feat: add stash import flow"
```

## Task 5: Implement revision save and revisions list

**Files:**
- Create: `crates/lab/src/dispatch/stash/revision.rs`
- Modify: `crates/lab/src/dispatch/stash/store.rs`
- Test: `crates/lab/src/dispatch/stash/revision_tests.rs`

- [ ] **Step 1: Write failing tests for immutable revision save and digest stability**

```rust
#[tokio::test]
async fn save_creates_immutable_revision_snapshot() {
    let component = imported_component_fixture().await;
    let first = service.save_revision(&component.id, Some("initial")).await.unwrap();
    mutate_workspace_file(&component.workspace_root, "SKILL.md", "changed");
    let second = service.save_revision(&component.id, Some("second")).await.unwrap();

    assert_ne!(first.id, second.id);
    assert_ne!(first.content_digest, second.content_digest);
    // load_revision_file reads from stash/revisions/<id>/files/SKILL.md
    assert_eq!(load_revision_file(&first, "SKILL.md"), original_skill_contents());
}

#[tokio::test]
async fn save_rejects_symlinks_in_workspace() {
    let component = component_with_symlink_fixture().await;
    let err = service.save_revision(&component.id, None).await.unwrap_err();
    assert_eq!(err.kind(), "symlink_rejected");
}
```

- [ ] **Step 2: Run the focused revision tests to verify they fail**

Run: `cargo test -p lab save_creates_immutable_revision_snapshot save_rejects_symlinks_in_workspace`
Expected: FAIL because revision logic does not exist yet

- [ ] **Step 3: Implement revision snapshotting**

**Storage layout for each revision:**
```
stash/revisions/<rev_id>/
├── meta.json       # StashRevision metadata (id, component_id, label, content_digest, created_at, file_count)
└── files/          # immutable snapshot — direct copy of workspace files at save time
    ├── SKILL.md
    └── ...
```

No separate object store. Content deduplication is not a requirement.

**Required behavior:**
- Entire snapshot wrapped in `with_component_lock(component_id, ...)` — lock held through head-pointer update
- Walk managed workspace with `StashLimits` enforcement and `DEFAULT_EXCLUDES` applied
- Use `tokio::fs::symlink_metadata` (not `metadata`) for every file stat; reject symlinks with `symlink_rejected`
- All stored paths normalized to `Component::Normal`-only relative paths — reject traversal attempts
- Compute `content_digest` using **SHA-256** (not SHA-1) over the full content tree
- File hashing and copy are CPU/IO-bound — always use `tokio::task::spawn_blocking`; never block the async executor
- Fan out hashing with a `JoinSet` capped at `num_cpus::get()` workers; collect all results before writing any files
- Write snapshot files to `stash/revisions/<rev_id>/files/` using temp-dir + rename semantics
- **Single-file workspaces** (LspConfig, McpConfig, Settings, BinFile, Script): snapshot the single file directly into `files/<filename>` — no directory walk needed
- **BinFile:** read and store `unix_mode` (execute permission bits) from `symlink_metadata().permissions().mode()` into `meta.json`; this is needed to restore executable bits on export/deploy
- Write `meta.json` atomically (temp-file + rename)
- Update component head pointer only after both writes succeed (inside the component lock)

- [ ] **Step 4: Run the focused revision tests again**

Run: `cargo test -p lab save_creates_immutable_revision_snapshot save_rejects_symlinks_in_workspace`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash/revision.rs crates/lab/src/dispatch/stash/store.rs crates/lab/src/dispatch/stash/revision_tests.rs
git commit -m "feat: add stash revision snapshots"
```

## Task 6: Implement export/handoff flow

**Files:**
- Create: `crates/lab/src/dispatch/stash/export.rs`
- Test: `crates/lab/src/dispatch/stash/revision_tests.rs` or dedicated export tests

- [ ] **Step 1: Write failing tests for exporting a chosen revision to a handoff directory**

```rust
#[tokio::test]
async fn export_materializes_selected_revision() {
    let revision = saved_revision_fixture().await;
    let export_dir = tempfile::tempdir().unwrap();

    let result = service.export_revision(&revision.component_id, &revision.id, export_dir.path()).await.unwrap();
    assert!(result.output_root.join("SKILL.md").exists());
}

#[tokio::test]
async fn export_rejects_nonempty_target_without_force() {
    let revision = saved_revision_fixture().await;
    let export_dir = tempfile::tempdir().unwrap();
    std::fs::write(export_dir.path().join("existing.txt"), b"data").unwrap();

    let err = service.export_revision(&revision.component_id, &revision.id, export_dir.path()).await.unwrap_err();
    assert_eq!(err.kind(), "export_target_not_empty");
}

#[tokio::test]
async fn export_path_stays_within_target_root() {
    let revision = saved_revision_with_traversal_path().await;
    let export_dir = tempfile::tempdir().unwrap();
    let err = service.export_revision(&revision.component_id, &revision.id, export_dir.path()).await.unwrap_err();
    assert_eq!(err.kind(), "path_traversal");
}
```

- [ ] **Step 2: Run the focused export test to verify it fails**

Run: `cargo test -p lab export_materializes_selected_revision export_rejects_nonempty_target_without_force export_path_stays_within_target_root`
Expected: FAIL because export logic does not exist yet

- [ ] **Step 3: Implement minimal export logic**

V1 must:
- Parse `StashExportOptions` from params: `include_secrets` (default `false`), `force` (default `false`)
- **Credential-kind guard:** if the component kind is `Settings` or `McpConfig` and `include_secrets` is `false`, return `secrets_export_not_allowed` immediately — do not read any files; `component.export` with `include_secrets: true` is marked `destructive: true` in the catalog
- Fail with `export_target_not_empty` if the target directory exists and is non-empty, unless `options.force` is `true`
- For each file in `stash/revisions/<rev_id>/files/`, assert containment using `ensure_target_within_write_root` (canonicalize + `starts_with`); reject any path that is absolute or resolves outside the target root with `path_traversal`
- Read revision files concurrently using `futures::stream::iter(...).buffer_unordered(8)` to avoid sequential bottleneck on slow or network-mounted storage
- Materialize files with `tokio::fs::write` after containment check
- **BinFile:** after writing, restore execute permission bits as `unix_mode & 0o0755` via `std::fs::set_permissions` — setuid/setgid/sticky bits (04000/02000/01000) are unconditionally stripped before the `set_permissions` call; never pass raw stored bits directly
- **Single-file workspaces:** export materializes the single file directly at `output_root/<filename>`; the caller-specified directory is still the root
- Return a structured result with `output_root` and `revision_id`
- Does not own deployment; callers decide what to do with the exported directory

- [ ] **Step 4: Run the focused export test again**

Run: `cargo test -p lab export_materializes_selected_revision export_rejects_nonempty_target_without_force export_path_stays_within_target_root`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash/export.rs crates/lab/src/dispatch/stash/revision_tests.rs
git commit -m "feat: add stash export flow"
```

## Task 7: Wire the stash service orchestration and dispatch

**Files:**
- Create: `crates/lab/src/dispatch/stash/service.rs`
- Create: `crates/lab/src/dispatch/stash/dispatch.rs`
- Modify: `crates/lab/src/dispatch/stash.rs`
- Test: `crates/lab/src/dispatch/stash/tests.rs`

- [ ] **Step 1: Write failing tests for action dispatch across the local-only flows**

```rust
#[tokio::test]
async fn component_save_dispatch_returns_success_envelope_data() {
    let result = dispatch("component.save", json!({ "id": fixture_component_id() })).await.unwrap();
    assert!(result.get("id").is_some());
}
```

- [ ] **Step 2: Run the focused dispatch tests to verify they fail**

Run: `cargo test -p lab component_save_dispatch_returns_success_envelope_data`
Expected: FAIL because stash dispatch does not exist yet

- [ ] **Step 3: Implement `StashService` and action routing**

Route each action to the local service methods:
- list/get/create/import/workspace/save/revisions/export
- `component.deploy` — resolve the target from `stash/targets/` using `StashDeployTarget` enum match; then:
  - **Local:** call `ensure_target_within_write_root` on the deploy target `path` before writing any file; reject any target path that resolves to a system root (`/etc`, `/usr`, `/bin`, `/sbin`, etc.) with `deploy_failed`; copy revision files using the same three-layer path safety as export; wrap the entire copy in `with_deploy_lock` with a `STASH_DEPLOY_TIMEOUT_MS` timeout
  - **Remote:** stub for v1 — return `unsupported_provider` with a `"remote gateway deploy not yet implemented"` message
  - Return a structured deploy result with `target_id`, `revision_id`, and `files_written` count
- `targets.list` / `target.add` / `target.remove` — CRUD over `stash/targets/` records
- provider actions return `unsupported_provider` until Task 9 lands

Use the shared `ToolError` envelope contract and keep the dispatch layer surface-neutral.

- [ ] **Step 4: Run the focused dispatch tests again**

Run: `cargo test -p lab component_save_dispatch_returns_success_envelope_data`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash.rs crates/lab/src/dispatch/stash/service.rs crates/lab/src/dispatch/stash/dispatch.rs crates/lab/src/dispatch/stash/tests.rs
git commit -m "feat: add stash dispatch service"
```

## Task 8: Register stash in the product surfaces

**Files:**
- Create: `crates/lab/src/cli/stash.rs`
- Create: `crates/lab/src/mcp/services/stash.rs`
- Create: `crates/lab/src/api/services/stash.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/src/api/router.rs`
- Test: `crates/lab/tests/stash_cli.rs`
- Test: `crates/lab/tests/stash_api.rs`
- Test: `crates/lab/tests/stash_mcp.rs`

- [ ] **Step 1: Write failing surface tests for one CLI action, one HTTP action, and one MCP action**

```rust
#[test]
fn stash_cli_help_lists_component_import() {
    let out = run_lab(["stash", "help"]);
    assert!(out.contains("component.import"));
}

#[tokio::test]
async fn stash_api_routes_component_get() {
    let response = app().oneshot(get("/v1/stash/component/get?id=test")).await.unwrap();
    assert_ne!(response.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run the focused surface tests to verify they fail**

Run: `cargo test -p lab stash_cli_help_lists_component_import stash_api_routes_component_get`
Expected: FAIL because stash is not registered on any surface yet

- [ ] **Step 3: Implement thin adapters and registration**

Requirements:
- CLI subcommand group mirrors stash actions cleanly
- MCP registers `stash` as one tool with standard `action` + `params`
- HTTP mounts `/v1/stash` under the shared router pattern
- **Write operations (`import`, `export`, `provider.push`, `provider.pull`) require `lab:admin` scope** — extract `AuthContext` from request extensions and assert the required scope before dispatching, same pattern as other write-capable service handlers
- `ActionSpec.destructive: true` gates (MCP elicitation, CLI `-y`, HTTP `"confirm": true` body) apply automatically for actions marked destructive in Task 2 — no extra per-handler work required
- registry and top-level catalog include the service

- [ ] **Step 4: Run the focused surface tests again**

Run: `cargo test -p lab stash_cli_help_lists_component_import stash_api_routes_component_get`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/cli/stash.rs crates/lab/src/mcp/services/stash.rs crates/lab/src/api/services/stash.rs crates/lab/src/cli.rs crates/lab/src/registry.rs crates/lab/src/api/router.rs crates/lab/tests/stash_cli.rs crates/lab/tests/stash_api.rs crates/lab/tests/stash_mcp.rs
git commit -m "feat: expose stash via cli mcp and api"
```

## Task 9: Add provider abstraction and the filesystem provider

**Files:**
- Create: `crates/lab/src/dispatch/stash/provider.rs`
- Create: `crates/lab/src/dispatch/stash/providers.rs` (**not** `providers/mod.rs` — sibling to `providers/` directory)
- Create: `crates/lab/src/dispatch/stash/providers/filesystem.rs`
- Modify: `crates/lab/src/dispatch/stash/service.rs`
- Modify: `crates/lab/src/dispatch/stash/dispatch.rs`
- Test: `crates/lab/src/dispatch/stash/provider_tests.rs`

- [ ] **Step 1: Write failing tests for filesystem provider push/pull round-trip**

```rust
#[tokio::test]
async fn filesystem_provider_round_trips_revision() {
    let revision = saved_revision_fixture().await;
    let provider_root = tempfile::tempdir().unwrap();

    service.link_provider(&revision.component_id, "filesystem", provider_root.path()).await.unwrap();
    service.push_revision(&revision.component_id, "filesystem", Some(&revision.id)).await.unwrap();

    let fetched = service.pull_latest(&revision.component_id, "filesystem").await.unwrap();
    assert_eq!(fetched.content_digest, revision.content_digest);
}

#[tokio::test]
async fn unknown_provider_push_returns_unsupported_provider() {
    let err = service.push_revision("cmp_123", "unknown_provider", None).await.unwrap_err();
    assert_eq!(err.kind(), "unsupported_provider");
}
```

- [ ] **Step 2: Run the focused provider tests to verify they fail**

Run: `cargo test -p lab filesystem_provider_round_trips_revision unknown_provider_push_returns_unsupported_provider`
Expected: FAIL because provider abstractions do not exist yet

- [ ] **Step 3: Implement the `StashProvider` trait and filesystem adapter**

**Provider trait:**
```rust
pub trait StashProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> StashProviderCapabilities;
    async fn push_revision(&self, component: &StashComponent, revision: &StashRevision, files_root: &Path) -> Result<(), ToolError>;
    async fn pull_latest(&self, component_id: &str) -> Result<(StashRevision, PathBuf), ToolError>;
    async fn list_remote(&self, component_id: &str) -> Result<Vec<StashRevision>, ToolError>;
}
```

**Credential types:** Any type holding provider authentication must implement `Debug` manually with redacted secret fields, matching the pattern in `lab_apis::core::Auth`. Even for filesystem provider (which has no secrets), establish this pattern now so future adapters follow it automatically.

**Filesystem provider behavior:**
- Provider linking records the target root path in `stash/providers/<component_id>-<name>.json` using `with_component_lock`
- Push: copy revision `files/` directory to `<provider_root>/<component_id>/<rev_id>/` using `spawn_blocking` + path safety (`ensure_target_within_write_root` on each output path)
- Pull: read the latest revision directory from provider root, copy back to local stash workspace, update local metadata (inside `with_component_lock`)
- Provider link metadata writes use `with_component_lock` to avoid races

**No Google Drive provider in v1.** The trait is designed for future adapters. A Google Drive provider will be added when `crates/lab-apis/src/google_drive/` OAuth client is built. Do not add stubs or placeholders for it here.

- [ ] **Step 4: Run the focused provider tests again**

Run: `cargo test -p lab filesystem_provider_round_trips_revision unknown_provider_push_returns_unsupported_provider`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash/provider.rs crates/lab/src/dispatch/stash/providers.rs crates/lab/src/dispatch/stash/providers/filesystem.rs crates/lab/src/dispatch/stash/service.rs crates/lab/src/dispatch/stash/dispatch.rs crates/lab/src/dispatch/stash/provider_tests.rs
git commit -m "feat: add stash filesystem provider"
```

## Task 10: Add observability and stable error coverage

**Files:**
- Modify: `crates/lab/src/dispatch/stash/dispatch.rs`
- Modify: `crates/lab/src/dispatch/stash/service.rs`
- Modify: `docs/OBSERVABILITY.md` only if needed
- Modify: `docs/ERRORS.md`
- Test: existing stash tests plus any focused logging/error tests

- [ ] **Step 1: Write failing tests for stable error mapping**

```rust
#[tokio::test]
async fn unsupported_provider_returns_stable_error_kind() {
    let err = service.push_revision("cmp_123", "unknown", None).await.unwrap_err();
    assert_eq!(err.kind(), "unsupported_provider");
}

#[tokio::test]
async fn workspace_too_large_returns_stable_error_kind() {
    let err = service.import_from_path(&oversized_fixture(), None).await.unwrap_err();
    assert_eq!(err.kind(), "workspace_too_large");
}
```

- [ ] **Step 2: Run the focused error tests to verify they fail**

Run: `cargo test -p lab unsupported_provider_returns_stable_error_kind workspace_too_large_returns_stable_error_kind`
Expected: FAIL if any stash operation still reports a generic error kind

- [ ] **Step 3: Implement final error mapping, dispatch logging, and credential redaction**

Requirements:
- Add all net-new stable kinds to `docs/ERRORS.md` and code: `conflict`, `unsupported_provider`, `unsupported_component_kind`, `sync_failed`, `workspace_too_large`, `file_too_large`, `path_traversal`, `symlink_rejected`, `export_target_not_empty`, `ambiguous_kind`, `deploy_failed`, `unknown_target`, `secrets_export_not_allowed`
- All save/import/export/push/pull operations log at the dispatch boundary with standard fields (`surface`, `service`, `action`, `elapsed_ms`)
- **Provider credential redaction:**
  - All provider credential-holding types implement `Debug` manually with redacted secret fields (same pattern as `lab_apis::core::Auth`)
  - All outbound provider URLs must pass through `redact_url` from `crates/lab/src/dispatch/redact.rs` before being included in log events
  - Token values must never appear in log events, error messages, or structured error envelopes
- **Provider token storage:** Document in `docs/STASH.md` where provider link records are persisted. For v1 filesystem provider this is `stash/providers/`, a local file readable only by the process user. When a token-bearing provider is added, evaluate encryption-at-rest against the `~/.labby/.env` precedent before storing credentials there.

- [ ] **Step 4: Run the focused error tests again**

Run: `cargo test -p lab unsupported_provider_returns_stable_error_kind workspace_too_large_returns_stable_error_kind`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/stash/dispatch.rs crates/lab/src/dispatch/stash/service.rs docs/ERRORS.md docs/OBSERVABILITY.md
git commit -m "feat: finalize stash errors and observability"
```

## Task 11: Write product docs and integrate the docs index

**Files:**
- Create: `docs/STASH.md`
- Modify: `docs/README.md`
- Modify: `docs/SERVICES.md`
- Modify: `docs/CLI.md`
- Modify: `docs/MCP.md`
- Modify: `docs/coverage/stash.md` if the coverage pattern is used

- [ ] **Step 1: Write a failing docs checklist from the spec**

Add a checklist in the plan execution branch covering:
- stash boundary and what it owns vs does not own
- action list (11 actions)
- storage model (workspaces + revision snapshot dirs, no object store)
- provider model (trait + filesystem only in v1; Google Drive deferred)
- export/deploy handoff
- security model (path safety, symlink rejection, export containment, `lab:admin` scope)
- deferred items (Google Drive, marketplace import)
- non-goals

- [ ] **Step 2: Validate current docs do not mention stash yet**

Run: `rg -n "\bstash\b" docs`
Expected: only the new spec/plan files, not product docs

- [ ] **Step 3: Write `docs/STASH.md` and update the owning docs**

Document:
- what stash is and what it owns vs does not own
- storage model (snapshot directories at `stash/revisions/<id>/files/`, SHA-256 digest, no object store)
- action model (11 actions)
- provider model (trait, filesystem v1, Google Drive deferred until OAuth client exists)
- how to import a component from a local path
- how to export a revision for deployment
- security model: path safety, symlink rejection, export containment, `lab:admin` scope requirement for write operations
- provider token storage location decision
- examples for CLI, MCP, and HTTP

- [ ] **Step 4: Re-run the docs grep to confirm the new docs are wired in**

Run: `rg -n "\bstash\b" docs/README.md docs/SERVICES.md docs/CLI.md docs/MCP.md docs/STASH.md`
Expected: matches in each updated doc

- [ ] **Step 5: Commit**

```bash
git add docs/STASH.md docs/README.md docs/SERVICES.md docs/CLI.md docs/MCP.md docs/coverage/stash.md
git commit -m "docs: add stash documentation"
```

## Task 12: Run the final all-features verification slice for stash

**Files:**
- No code changes expected
- Test: all relevant stash test files and all-features workspace checks

- [ ] **Step 1: Run focused stash tests**

Run: `cargo test -p lab stash --all-features`
Expected: PASS

- [ ] **Step 2: Run all-features workspace tests for the touched areas**

Run: `cargo test --workspace --all-features --tests --no-fail-fast`
Expected: PASS

- [ ] **Step 3: Run all-features build**

Run: `cargo build --workspace --all-features`
Expected: PASS

- [ ] **Step 4: Manually exercise one CLI flow and one MCP/HTTP flow**

Run examples:
- `cargo run --all-features -- stash component import --path /tmp/my-skill`
- invoke `stash({"action":"components.list"})` in an MCP harness or hit `/v1/stash/...` through the HTTP surface
Expected: successful end-to-end result envelopes

- [ ] **Step 5: Commit the final verification and integration fixes**

```bash
git add .
git commit -m "feat: finalize stash service"
```

## Notes for the implementing engineer

- Follow existing first-class service patterns rather than inventing a new service shape.
- Keep `stash` local-first even when adding providers.
- **Path safety is mandatory.** Import (`import.rs`), export (`export.rs`), and deploy (`service.rs`) all use the three-layer pattern from `crates/lab/src/dispatch/path_safety.rs`: `reject_path_traversal`, `reject_symlink`, `ensure_target_within_write_root`. **Import from `path_safety.rs`; do not copy.** The shared module is extracted from ACP in Task 2.
- **Per-component locking is mandatory.** All state mutations use `StashStore::with_component_lock`. The lock must be held through the head-pointer update — never release before the final atomic rename.
- **No `spawn_blocking` omissions.** Every file hash and file copy is CPU/IO-bound. Always use `spawn_blocking`; never block the async executor with synchronous filesystem operations.
- **No `providers/mod.rs`.** The module is declared in `providers.rs` (sibling to `providers/` directory). This is a hard project rule.
- `client.rs` is required in the dispatch layout even for synthetic services with no remote API.
- Google Drive provider is explicitly deferred. Do not add it until `crates/lab-apis/src/google_drive/` OAuth client exists. Do not add stubs.
- Do not collapse `marketplace` and `stash` dispatch layers. Marketplace → stash import is a CLI composition concern in v1.
- Keep deploy ownership separate. `export` hands off; it does not write to deployment targets.
- If a stable error kind is added, update `docs/ERRORS.md` and the transport surface together.
- Verify decisions against all-features builds before removing or restructuring any shared helpers.
