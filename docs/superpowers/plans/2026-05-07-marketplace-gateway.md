# Marketplace Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Lab marketplace a first-class, store-backed catalog and curation service that can import Claude/Codex marketplace sources, create a managed Claude-compatible marketplace, and safely add indexed plugins to it.

**Architecture:** Treat `marketplace` as a product-local control-plane service. Keep filesystem, SQLite, git, and runtime coordination in `crates/lab/src/dispatch/marketplace`; only shared serde response/request types go in `lab-apis`. The first implementation slice proves store-backed import/list/create/add with raw-source preservation, pagination, admin write gates, atomic JSON writes, and security/performance tests before wrapper generation or publishing expands scope.

**Tech Stack:** Rust 2024, Tokio, rusqlite/r2d2_sqlite, serde/serde_json, axum, Lab dispatch `ActionSpec`, Next.js gateway-admin TypeScript client/tests, cargo-nextest, Vitest.

---

## Bead Review

The epic `lab-dzvv` and children are coherent after engineering review feedback. One tracker fix was applied before writing this plan: `lab-dzvv.5` now depends on `lab-dzvv.4`, matching its acceptance criterion that MCP wrapper generation waits for the store-backed create/add loop.

Execution order:

1. `lab-dzvv.1` - source registry/import/store-backed paginated listing.
2. `lab-dzvv.2` - first-pass resolved version fields during import/refresh.
3. `lab-dzvv.3` - managed marketplace files and locked atomic JSON writes.
4. `lab-dzvv.4` - add one indexed portable plugin entry to one managed marketplace.
5. `lab-dzvv.5` - generated MCP wrapper plugins after `.1` through `.4` pass.
6. `lab-dzvv.6` - share readiness and safe local publish planning.
7. `lab-dzvv.7` - UI curation actions after backend pagination/search/write gates exist.
8. `lab-dzvv.8` - docs, generated inventories, and end-to-end regression tests.

This plan is intentionally split into two waves. Wave 1 is `.1` through `.4` and produces usable, testable software. Wave 2 layers wrapper generation, share readiness, and UI.

## File Structure

Create:

- `crates/lab/src/dispatch/marketplace/catalog_store.rs` - Lab-owned SQLite catalog and indexed plugin store.
- `crates/lab/src/dispatch/marketplace/catalog_store_schema.sql` - schema for marketplace sources, plugins, indexes, and FTS.
- `crates/lab/src/dispatch/marketplace/catalog_import.rs` - import adapters that read Claude/Codex backends and preserve raw entries.
- `crates/lab/src/dispatch/marketplace/managed.rs` - managed marketplace create/list/get/update helpers and curation add/remove functions.
- `crates/lab/src/dispatch/marketplace/json_write.rs` - locked atomic JSON write helper plus path safety helpers.
- `crates/lab/src/dispatch/marketplace/redact.rs` - redaction and secret-material detection for raw JSON, previews, diffs, and git errors.
- `crates/lab/src/dispatch/marketplace/mcp_wrapper.rs` - generated MCP plugin wrapper preview/add.
- `crates/lab/src/dispatch/marketplace/share.rs` - share readiness and local publish planning.
- `crates/lab/src/dispatch/marketplace/testdata/` - Claude/Codex/MCP fixture JSON for all source shapes.
- `apps/gateway-admin/lib/api/marketplace-curation-types.ts` - UI types for managed marketplaces, previews, version source, share readiness.
- `docs/services/MARKETPLACE.md` - service docs for source truth, managed marketplace files, versioning, and sharing.

Modify:

- `crates/lab/src/dispatch/marketplace.rs` - register new modules.
- `crates/lab/src/dispatch/marketplace/catalog.rs` - add new `ActionSpec`s and destructive flags.
- `crates/lab/src/dispatch/marketplace/dispatch.rs` - route new actions through parsers and store/managed helpers.
- `crates/lab/src/dispatch/marketplace/params.rs` - parse source/import/list/managed/curation/share params.
- `crates/lab/src/dispatch/marketplace/service.rs` - make `sources.list` and `plugins.list` store-backed.
- `crates/lab/src/api/services/marketplace.rs` - add write-action auth gate, Origin/Referer/CSRF enforcement for browser writes.
- `crates/lab/src/api/state.rs` - hold the Lab-owned marketplace catalog store.
- `crates/lab/src/config.rs` - add a config helper for the marketplace catalog DB path and managed marketplace root.
- `crates/lab-apis/src/marketplace/types.rs` - add serde types for paginated lists, resolved versions, managed marketplaces, previews, and share readiness.
- `apps/gateway-admin/lib/api/marketplace-client.ts` - use paginated list/search and add curation methods.
- `apps/gateway-admin/lib/marketplace/api-client.ts` - keep MCP/ACP helpers read-only or wrap the primary marketplace client.
- `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx` - consume paginated backend search/sort/list and distinct actions.
- `apps/gateway-admin/components/marketplace/marketplace-state.ts` - stop expanding/filtering/sorting the full catalog client-side.
- `docs/dev/ERRORS.md` - document new stable error kinds.
- `docs/dev/SERVICES.md` - document `marketplace` as a product-local control-plane service if not generated elsewhere.
- Generated docs after registry/action changes: run `just docs-generate`.

Out of scope for Wave 1:

- GitHub REST repository creation and file writes.
- Remote git SHA resolution.
- `generated_plugins` table.
- Rename/alias conflict policy.
- ACP/Gemini curation.
- Full publish UI.
- Broad refactor of existing MCP Registry `store.rs`.

## Task 1: Store Schema And Store Wrapper

**Beads:** `lab-dzvv.1`

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/catalog_store.rs`
- Create: `crates/lab/src/dispatch/marketplace/catalog_store_schema.sql`
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Test: `crates/lab/src/dispatch/marketplace/catalog_store.rs`

- [ ] **Step 1: Write failing schema/store tests**

Add these tests at the bottom of `catalog_store.rs` under `#[cfg(test)]`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn open_temp_store() -> (TempDir, CatalogStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CatalogStore::open(&dir.path().join("marketplace.db"))
            .await
            .expect("open catalog store");
        (dir, store)
    }

    #[tokio::test]
    async fn open_creates_schema_and_indexes() {
        let (_dir, store) = open_temp_store().await;
        let source_count = store.count_sources().await.expect("count sources");
        assert_eq!(source_count, 0);
    }

    #[tokio::test]
    async fn upsert_source_is_idempotent_by_stable_key() {
        let (_dir, store) = open_temp_store().await;
        let input = SourceUpsert {
            runtime: "claude".into(),
            import_origin: "known_marketplaces".into(),
            source_key: "github:anthropics/claude-code".into(),
            display_name: "Claude Official".into(),
            owner_name: Some("Anthropic".into()),
            source_kind: "github".into(),
            repo: Some("anthropics/claude-code".into()),
            url: None,
            path: None,
            raw_json: json!({"source": "anthropics/claude-code"}),
        };
        let first = store.upsert_source(input.clone()).await.expect("first upsert");
        let second = store.upsert_source(input).await.expect("second upsert");
        assert_eq!(first.id, second.id);
        assert_eq!(store.count_sources().await.expect("count sources"), 1);
    }

    #[tokio::test]
    async fn list_plugins_is_cursor_paginated() {
        let (_dir, store) = open_temp_store().await;
        let source = store
            .upsert_source(SourceUpsert {
                runtime: "claude".into(),
                import_origin: "fixture".into(),
                source_key: "local:fixture".into(),
                display_name: "Fixture".into(),
                owner_name: None,
                source_kind: "local".into(),
                repo: None,
                url: None,
                path: Some("/tmp/fixture".into()),
                raw_json: json!({}),
            })
            .await
            .expect("source");
        for name in ["alpha", "bravo", "charlie"] {
            store
                .upsert_plugin(PluginUpsert {
                    source_id: source.id.clone(),
                    name: name.into(),
                    description: Some(format!("{name} plugin")),
                    runtime: "claude".into(),
                    installed: false,
                    resolved_version: None,
                    version_source: "unknown".into(),
                    tags_json: json!(["plugin"]),
                    raw_entry_json: json!({"name": name, "source": {"type": "github", "repo": "owner/repo"}}),
                    plugin_source_json: json!({"type": "github", "repo": "owner/repo"}),
                })
                .await
                .expect("plugin");
        }
        let page = store
            .list_plugins(PluginListParams { limit: Some(2), ..Default::default() })
            .await
            .expect("page");
        assert_eq!(page.items.len(), 2);
        assert!(page.next_cursor.is_some());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::catalog_store
```

Expected: compile failure because `CatalogStore`, `SourceUpsert`, `PluginUpsert`, and `PluginListParams` do not exist.

- [ ] **Step 3: Add schema**

Create `catalog_store_schema.sql`:

```sql
CREATE TABLE IF NOT EXISTS marketplace_sources (
    id              TEXT PRIMARY KEY,
    runtime         TEXT NOT NULL,
    import_origin   TEXT NOT NULL,
    source_key      TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    owner_name      TEXT,
    source_kind     TEXT NOT NULL,
    repo            TEXT,
    url             TEXT,
    path            TEXT,
    enabled         INTEGER NOT NULL DEFAULT 1,
    indexed         INTEGER NOT NULL DEFAULT 0,
    managed         INTEGER NOT NULL DEFAULT 0,
    managed_root    TEXT,
    share_status    TEXT,
    last_sync_at    TEXT,
    raw_json        TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(runtime, import_origin, source_key)
);

CREATE TABLE IF NOT EXISTS marketplace_plugins (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL REFERENCES marketplace_sources(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    runtime         TEXT NOT NULL,
    installed       INTEGER NOT NULL DEFAULT 0,
    resolved_version TEXT,
    version_source  TEXT NOT NULL,
    tags_json       TEXT NOT NULL DEFAULT '[]',
    raw_entry_json  TEXT NOT NULL,
    plugin_source_json TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(source_id, name)
);

CREATE INDEX IF NOT EXISTS idx_marketplace_sources_runtime_enabled
    ON marketplace_sources(runtime, enabled);

CREATE INDEX IF NOT EXISTS idx_marketplace_sources_managed_share
    ON marketplace_sources(managed, share_status);

CREATE INDEX IF NOT EXISTS idx_marketplace_plugins_source_name
    ON marketplace_plugins(source_id, name);

CREATE INDEX IF NOT EXISTS idx_marketplace_plugins_runtime_installed
    ON marketplace_plugins(runtime, installed);

CREATE INDEX IF NOT EXISTS idx_marketplace_plugins_version_source
    ON marketplace_plugins(version_source);

CREATE VIRTUAL TABLE IF NOT EXISTS marketplace_plugins_fts
USING fts5(name, description, tags, content='marketplace_plugins', content_rowid='rowid');

CREATE TRIGGER IF NOT EXISTS marketplace_plugins_fts_ai
AFTER INSERT ON marketplace_plugins BEGIN
  INSERT INTO marketplace_plugins_fts(rowid, name, description, tags)
  VALUES (new.rowid, new.name, coalesce(new.description, ''), new.tags_json);
END;

CREATE TRIGGER IF NOT EXISTS marketplace_plugins_fts_ad
AFTER DELETE ON marketplace_plugins BEGIN
  INSERT INTO marketplace_plugins_fts(marketplace_plugins_fts, rowid, name, description, tags)
  VALUES ('delete', old.rowid, old.name, coalesce(old.description, ''), old.tags_json);
END;

CREATE TRIGGER IF NOT EXISTS marketplace_plugins_fts_au
AFTER UPDATE ON marketplace_plugins BEGIN
  INSERT INTO marketplace_plugins_fts(marketplace_plugins_fts, rowid, name, description, tags)
  VALUES ('delete', old.rowid, old.name, coalesce(old.description, ''), old.tags_json);
  INSERT INTO marketplace_plugins_fts(rowid, name, description, tags)
  VALUES (new.rowid, new.name, coalesce(new.description, ''), new.tags_json);
END;
```

- [ ] **Step 4: Add minimal store implementation**

Create `catalog_store.rs` with this structure:

```rust
//! Lab-owned marketplace catalog and curation store.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogStoreError {
    #[error("sqlite error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("connection pool error: {0}")]
    Pool(#[from] r2d2::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("invalid cursor: {0}")]
    InvalidCursor(String),
}

#[derive(Debug, Clone)]
pub struct CatalogStore {
    pool: Pool<SqliteConnectionManager>,
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SourceUpsert {
    pub runtime: String,
    pub import_origin: String,
    pub source_key: String,
    pub display_name: String,
    pub owner_name: Option<String>,
    pub source_kind: String,
    pub repo: Option<String>,
    pub url: Option<String>,
    pub path: Option<String>,
    pub raw_json: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceRecord {
    pub id: String,
    pub runtime: String,
    pub display_name: String,
    pub source_kind: String,
    pub enabled: bool,
    pub managed: bool,
}

#[derive(Debug, Clone)]
pub struct PluginUpsert {
    pub source_id: String,
    pub name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub installed: bool,
    pub resolved_version: Option<String>,
    pub version_source: String,
    pub tags_json: Value,
    pub raw_entry_json: Value,
    pub plugin_source_json: Value,
}

#[derive(Debug, Clone, Default)]
pub struct PluginListParams {
    pub source_id: Option<String>,
    pub runtime: Option<String>,
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginRow {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub description: Option<String>,
    pub runtime: String,
    pub installed: bool,
    pub resolved_version: Option<String>,
    pub version_source: String,
    pub tags_json: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct PagedPlugins {
    pub items: Vec<PluginRow>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorData {
    id: String,
}

impl CatalogStore {
    pub async fn open(path: &Path) -> Result<Self, CatalogStoreError> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if !path.exists() {
                std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(&path)?;
            }
            set_restrictive_permissions(&path)?;
            let manager = SqliteConnectionManager::file(&path).with_init(|conn| {
                conn.pragma_update(None, "journal_mode", "WAL")?;
                conn.execute_batch(
                    "PRAGMA busy_timeout = 5000;\
                     PRAGMA foreign_keys = ON;\
                     PRAGMA synchronous = NORMAL;",
                )
            });
            let pool = Pool::builder()
                .max_size(4)
                .connection_timeout(Duration::from_secs(5))
                .build(manager)?;
            let conn = pool.get()?;
            migrate(&conn)?;
            drop(conn);
            Ok(Self { pool, path })
        })
        .await?
    }

    pub fn db_path(&self) -> &Path {
        &self.path
    }

    pub async fn count_sources(&self) -> Result<u64, CatalogStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            let count: u64 =
                conn.query_row("SELECT COUNT(*) FROM marketplace_sources", [], |row| row.get(0))?;
            Ok(count)
        })
        .await?
    }

    pub async fn upsert_source(&self, input: SourceUpsert) -> Result<SourceRecord, CatalogStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            upsert_source_sync(&conn, input)
        })
        .await?
    }

    pub async fn upsert_plugin(&self, input: PluginUpsert) -> Result<PluginRow, CatalogStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            upsert_plugin_sync(&conn, input)
        })
        .await?
    }

    pub async fn list_plugins(&self, params: PluginListParams) -> Result<PagedPlugins, CatalogStoreError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get()?;
            list_plugins_sync(&conn, params)
        })
        .await?
    }
}

fn migrate(conn: &Connection) -> Result<(), CatalogStoreError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("BEGIN EXCLUSIVE;")?;
    tx.execute_batch(include_str!("catalog_store_schema.sql"))?;
    tx.execute_batch("COMMIT;")?;
    Ok(())
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let joined = parts.join("\u{1f}");
    let digest = blake3::hash(joined.as_bytes());
    format!("{prefix}-{}", digest.to_hex()[..16].to_string())
}

fn now_string() -> String {
    jiff::Timestamp::now().to_string()
}

fn encode_cursor(id: &str) -> String {
    let data = serde_json::to_vec(&CursorData { id: id.to_string() }).expect("cursor json");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn decode_cursor(cursor: &str) -> Result<String, CatalogStoreError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|e| CatalogStoreError::InvalidCursor(e.to_string()))?;
    let data: CursorData = serde_json::from_slice(&bytes)?;
    Ok(data.id)
}

#[cfg(unix)]
fn set_restrictive_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_restrictive_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
```

Then add `upsert_source_sync`, `upsert_plugin_sync`, and `list_plugins_sync` using bound parameters. Keep `list_plugins_sync` limited to 100 rows and use `WHERE id > ? ORDER BY id LIMIT ?` cursor semantics.

- [ ] **Step 5: Wire module**

Modify `crates/lab/src/dispatch/marketplace.rs`:

```rust
mod catalog_import;
pub(crate) mod catalog_store;
mod json_write;
mod managed;
mod redact;
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::catalog_store
```

Expected: all `catalog_store` tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/marketplace.rs crates/lab/src/dispatch/marketplace/catalog_store.rs crates/lab/src/dispatch/marketplace/catalog_store_schema.sql
git commit -m "feat(marketplace): add catalog store"
```

## Task 2: Import Adapters Preserve Raw Plugin Source Entries

**Beads:** `lab-dzvv.1`, `lab-dzvv.2`

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/catalog_import.rs`
- Create fixtures under: `crates/lab/src/dispatch/marketplace/testdata/`
- Modify: `crates/lab/src/dispatch/marketplace/backends/claude.rs`
- Modify: `crates/lab/src/dispatch/marketplace/backends/codex.rs`
- Test: `crates/lab/src/dispatch/marketplace/catalog_import.rs`

- [ ] **Step 1: Add source-shape fixtures**

Create fixture files:

```text
crates/lab/src/dispatch/marketplace/testdata/plugin-source-github.json
crates/lab/src/dispatch/marketplace/testdata/plugin-source-url.json
crates/lab/src/dispatch/marketplace/testdata/plugin-source-git-subdir.json
crates/lab/src/dispatch/marketplace/testdata/plugin-source-npm.json
crates/lab/src/dispatch/marketplace/testdata/plugin-source-local.json
crates/lab/src/dispatch/marketplace/testdata/plugin-source-generated-relative.json
crates/lab/src/dispatch/marketplace/testdata/plugin-source-unknown-object.json
```

Example `plugin-source-git-subdir.json`:

```json
{
  "name": "git-subdir-demo",
  "description": "Fixture for git subdirectory plugin source",
  "source": {
    "type": "git-subdir",
    "url": "https://github.com/example/monorepo.git",
    "path": "plugins/git-subdir-demo",
    "ref": "main"
  },
  "tags": ["fixture", "plugin"]
}
```

- [ ] **Step 2: Write failing import tests**

Add tests in `catalog_import.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn load_fixture(name: &str) -> Value {
        serde_json::from_str(match name {
            "github" => include_str!("testdata/plugin-source-github.json"),
            "url" => include_str!("testdata/plugin-source-url.json"),
            "git-subdir" => include_str!("testdata/plugin-source-git-subdir.json"),
            "npm" => include_str!("testdata/plugin-source-npm.json"),
            "local" => include_str!("testdata/plugin-source-local.json"),
            "generated-relative" => include_str!("testdata/plugin-source-generated-relative.json"),
            "unknown" => include_str!("testdata/plugin-source-unknown-object.json"),
            other => panic!("unknown fixture {other}"),
        })
        .expect("fixture json")
    }

    #[test]
    fn extracts_plugin_source_without_normalizing_shape() {
        for name in ["github", "url", "git-subdir", "npm", "local", "generated-relative", "unknown"] {
            let raw = load_fixture(name);
            let extracted = extract_plugin_source_json(&raw).expect("source json");
            assert_eq!(extracted, raw.get("source").expect("source").clone(), "{name}");
        }
    }

    #[test]
    fn resolves_first_pass_version_without_git_network() {
        let raw = serde_json::json!({
            "name": "versioned",
            "version": "1.2.3",
            "source": {"type": "github", "repo": "owner/repo"}
        });
        let resolved = resolve_import_version(&raw, None, None);
        assert_eq!(resolved.resolved_version.as_deref(), Some("1.2.3"));
        assert_eq!(resolved.version_source, "marketplace_entry");
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::catalog_import
```

Expected: compile failure because import helpers are absent.

- [ ] **Step 4: Implement import helpers**

Create helper types and functions:

```rust
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportVersion {
    pub resolved_version: Option<String>,
    pub version_source: String,
    pub unavailable_reason: Option<String>,
}

pub fn extract_plugin_source_json(raw_entry: &Value) -> Option<Value> {
    raw_entry.get("source").cloned()
}

pub fn resolve_import_version(
    raw_entry: &Value,
    manifest_version: Option<&str>,
    installed_version: Option<&str>,
) -> ImportVersion {
    if let Some(version) = manifest_version.filter(|v| !v.trim().is_empty()) {
        return ImportVersion {
            resolved_version: Some(version.to_string()),
            version_source: "plugin_manifest".into(),
            unavailable_reason: None,
        };
    }
    if let Some(version) = raw_entry.get("version").and_then(Value::as_str).filter(|v| !v.trim().is_empty()) {
        return ImportVersion {
            resolved_version: Some(version.to_string()),
            version_source: "marketplace_entry".into(),
            unavailable_reason: None,
        };
    }
    if let Some(version) = installed_version.filter(|v| !v.trim().is_empty()) {
        return ImportVersion {
            resolved_version: Some(version.to_string()),
            version_source: "installed_state".into(),
            unavailable_reason: None,
        };
    }
    ImportVersion {
        resolved_version: None,
        version_source: "unknown".into(),
        unavailable_reason: Some("no plugin.json version, marketplace entry version, or installed state version".into()),
    }
}
```

- [ ] **Step 5: Extend backend parsing to carry raw entry**

Add a backend-local raw entry field or import adapter path that can read marketplace catalog plugin entries as `serde_json::Value` before converting to existing `Plugin`. The key invariant is that `CatalogStore::upsert_plugin` receives the unmodified raw entry and exact `source` value.

Use this assertion in tests:

```rust
assert_eq!(stored.raw_entry_json, raw_entry);
assert_eq!(stored.plugin_source_json, raw_entry["source"]);
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::catalog_import
```

Expected: all import tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/catalog_import.rs crates/lab/src/dispatch/marketplace/testdata crates/lab/src/dispatch/marketplace/backends/claude.rs crates/lab/src/dispatch/marketplace/backends/codex.rs
git commit -m "feat(marketplace): preserve raw catalog plugin sources"
```

## Task 3: Store-Backed Paginated `sources.list` And `plugins.list`

**Beads:** `lab-dzvv.1`, `lab-dzvv.2`

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/service.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Modify: `crates/lab/src/dispatch/marketplace/params.rs`
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab-apis/src/marketplace/types.rs`
- Test: `crates/lab/src/dispatch/marketplace.rs`

- [ ] **Step 1: Write failing dispatch/catalog tests**

Add tests in `crates/lab/src/dispatch/marketplace.rs`:

```rust
const STORE_ACTIONS: &[(&str, bool)] = &[
    ("sources.import", false),
    ("sources.refresh", false),
    ("sources.enable", true),
    ("sources.disable", true),
    ("plugins.list", false),
    ("sources.list", false),
];

#[test]
fn catalog_includes_store_backed_source_actions() {
    for (name, destructive) in STORE_ACTIONS {
        let spec = super::actions()
            .iter()
            .find(|spec| spec.name == *name)
            .unwrap_or_else(|| panic!("missing action spec for {name}"));
        assert_eq!(spec.destructive, *destructive, "{name} destructive flag");
    }
}

#[tokio::test]
async fn plugins_list_accepts_limit_and_cursor_params() {
    let result = super::dispatch(
        "plugins.list",
        serde_json::json!({"limit": 10, "cursor": null}),
    )
    .await;
    assert!(result.is_ok() || result.as_ref().unwrap_err().kind() != "unknown_action");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::catalog_includes_store_backed_source_actions marketplace::plugins_list_accepts_limit_and_cursor_params
```

Expected: missing action specs or param handling.

- [ ] **Step 3: Add API types**

Extend `lab-apis/src/marketplace/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub page: PageInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedVersionInfo {
    #[serde(rename = "resolvedVersion", skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<String>,
    #[serde(rename = "versionSource")]
    pub version_source: String,
    #[serde(rename = "unavailableReason", skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}
```

Add optional fields to `Plugin`:

```rust
#[serde(rename = "resolvedVersion", skip_serializing_if = "Option::is_none")]
pub resolved_version: Option<String>,
#[serde(rename = "versionSource", skip_serializing_if = "Option::is_none")]
pub version_source: Option<String>,
```

- [ ] **Step 4: Add `ActionSpec`s**

In `catalog.rs`, add params `limit`, `cursor`, `query`, `marketplace`, `kind`, and `installed` for `plugins.list`; add new source actions. Mark `sources.enable` and `sources.disable` destructive because they mutate catalog visibility.

- [ ] **Step 5: Parse pagination params**

In `params.rs`, add:

```rust
#[derive(Debug, Clone, Default)]
pub struct ListPageParams {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub query: Option<String>,
}

pub fn parse_list_page_params(value: &serde_json::Value) -> Result<ListPageParams, ToolError> {
    let limit = value
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n.min(100) as u32);
    let cursor = value
        .get("cursor")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(ToString::to_string);
    let query = value
        .get("query")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.chars().take(512).collect());
    Ok(ListPageParams { limit, cursor, query })
}
```

- [ ] **Step 6: Route list actions through store**

In `dispatch.rs`, parse list params and call `service::sources_list_store_backed` and `service::plugins_list_store_backed`. The functions should open/use `CatalogStore` and return paginated `Paged<Marketplace>` / `Paged<Plugin>` shapes.

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::
```

Expected: marketplace dispatch/store tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/marketplace.rs crates/lab/src/dispatch/marketplace/catalog.rs crates/lab/src/dispatch/marketplace/dispatch.rs crates/lab/src/dispatch/marketplace/params.rs crates/lab/src/dispatch/marketplace/service.rs crates/lab-apis/src/marketplace/types.rs
git commit -m "feat(marketplace): serve catalog lists from store"
```

## Task 4: Managed Marketplace Root And Locked Atomic JSON Writes

**Beads:** `lab-dzvv.3`

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/json_write.rs`
- Create: `crates/lab/src/dispatch/marketplace/managed.rs`
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/dispatch/marketplace/json_write.rs`
- Test: `crates/lab/src/dispatch/marketplace/managed.rs`

- [ ] **Step 1: Write failing path safety tests**

Add tests in `json_write.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn slug_validation_rejects_path_escape_inputs() {
        for slug in ["../x", "/tmp/x", "a\\b", "a b", ".hidden", ""] {
            assert!(validate_slug(slug).is_err(), "{slug}");
        }
        assert!(validate_slug("personal-marketplace-1").is_ok());
    }

    #[test]
    fn relative_path_must_stay_inside_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("marketplaces").join("safe");
        std::fs::create_dir_all(&root).expect("mkdir");
        let good = resolve_managed_child(&root, ".claude-plugin/marketplace.json").expect("good");
        assert!(good.starts_with(&root));
        assert!(resolve_managed_child(&root, "../escape.json").is_err());
        assert!(resolve_managed_child(&root, "/tmp/escape.json").is_err());
    }

    #[test]
    fn atomic_write_preserves_old_json_if_transform_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("marketplace.json");
        std::fs::write(&path, r#"{"name":"old","plugins":[]}"#).expect("write old");
        let err = locked_atomic_json_write(&path, |_| Err(JsonWriteError::Validation("boom".into())));
        assert!(err.is_err());
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        assert_eq!(value["name"], "old");
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::json_write
```

Expected: compile failure because helpers are absent.

- [ ] **Step 3: Implement `json_write.rs`**

Create:

```rust
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use fs2::FileExt;
use serde_json::Value;
use tempfile::NamedTempFile;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum JsonWriteError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("validation error: {0}")]
    Validation(String),
}

pub fn validate_slug(slug: &str) -> Result<(), JsonWriteError> {
    let valid = !slug.is_empty()
        && slug.len() <= 80
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && slug
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(JsonWriteError::InvalidPath(format!("invalid slug `{slug}`")))
    }
}

pub fn resolve_managed_child(root: &Path, relative: &str) -> Result<PathBuf, JsonWriteError> {
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return Err(JsonWriteError::InvalidPath("absolute paths are not allowed".into()));
    }
    for component in rel.components() {
        match component {
            Component::Normal(_) => {}
            _ => return Err(JsonWriteError::InvalidPath(format!("invalid component in `{relative}`"))),
        }
    }
    let path = root.join(rel);
    if !path.starts_with(root) {
        return Err(JsonWriteError::InvalidPath("path escapes managed root".into()));
    }
    Ok(path)
}

pub fn locked_atomic_json_write<F>(path: &Path, transform: F) -> Result<(), JsonWriteError>
where
    F: FnOnce(Value) -> Result<Value, JsonWriteError>,
{
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("json.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock_file.lock_exclusive()?;
    let current = if path.exists() {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        serde_json::json!({})
    };
    let next = transform(current)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, &next)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    sync_parent_dir(parent)?;
    lock_file.unlock()?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}
```

If `fs2` is not already in `crates/lab/Cargo.toml`, add it under dependencies.

- [ ] **Step 4: Implement managed marketplace create/list/get**

In `managed.rs`, expose:

```rust
#[derive(Debug, Clone)]
pub struct ManagedCreateParams {
    pub slug: String,
    pub display_name: String,
    pub owner_name: String,
}

pub async fn managed_create(params: ManagedCreateParams) -> Result<serde_json::Value, ToolError> {
    validate_slug(&params.slug).map_err(invalid_path_error)?;
    let root = crate::config::marketplace_managed_root()?.join(&params.slug);
    let manifest_path = resolve_managed_child(&root, ".claude-plugin/marketplace.json")
        .map_err(invalid_path_error)?;
    let display_name = params.display_name.clone();
    let owner_name = params.owner_name.clone();
    tokio::task::spawn_blocking(move || {
        locked_atomic_json_write(&manifest_path, |_| {
            Ok(serde_json::json!({
                "name": display_name,
                "owner": { "name": owner_name },
                "plugins": []
            }))
        })
    })
    .await
    .map_err(|e| ToolError::Sdk { sdk_kind: "internal_error".into(), message: e.to_string() })?
    .map_err(|e| ToolError::Sdk { sdk_kind: "invalid_marketplace_schema".into(), message: e.to_string() })?;
    Ok(serde_json::json!({"slug": params.slug, "path": manifest_path}))
}
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::json_write marketplace::managed
```

Expected: path safety and managed create tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/Cargo.toml crates/lab/src/config.rs crates/lab/src/dispatch/marketplace/json_write.rs crates/lab/src/dispatch/marketplace/managed.rs crates/lab/src/dispatch/marketplace.rs
git commit -m "feat(marketplace): add managed marketplace writer"
```

## Task 5: HTTP Write Gate And Stable Error Kinds

**Beads:** `lab-dzvv.3`, `lab-dzvv.4`, `lab-dzvv.8`

**Files:**
- Modify: `crates/lab/src/api/services/marketplace.rs`
- Modify: `crates/lab/src/api/services/helpers.rs`
- Modify: `docs/dev/ERRORS.md`
- Test: `crates/lab/src/api/services/marketplace.rs`

- [ ] **Step 1: Write failing API security tests**

Add tests near existing marketplace route tests:

```rust
#[tokio::test]
async fn marketplace_write_action_rejected_when_auth_unconfigured() {
    let app = test_router_without_auth();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/marketplace")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"action":"managed.create","params":{"slug":"x","displayName":"X","ownerName":"Me"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn marketplace_write_action_requires_same_origin_browser_request() {
    let app = test_router_with_auth();
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/marketplace")
                .header("authorization", "Bearer test-token")
                .header("origin", "https://evil.example")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"action":"managed.create","params":{"slug":"x","displayName":"X","ownerName":"Me"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
```

Use existing router test helpers if names differ; keep the assertions identical.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace_write_action
```

Expected: tests fail because writes are not gated.

- [ ] **Step 3: Add `MARKETPLACE_WRITE_ACTIONS`**

In `api/services/marketplace.rs`:

```rust
const MARKETPLACE_WRITE_ACTIONS: &[&str] = &[
    "sources.enable",
    "sources.disable",
    "managed.create",
    "managed.update",
    "managed.delete",
    "managed.entry.add",
    "managed.entry.remove",
    "mcp.plugin.add",
    "managed.share.publish",
];

fn is_marketplace_write_action(action: &str) -> bool {
    MARKETPLACE_WRITE_ACTIONS.contains(&action)
}
```

Before `handle_action`, check write action auth posture and browser origin/CSRF. Reuse existing auth/host/origin helper patterns from `stash` or setup routes. Return stable `ToolError::Sdk` kinds:

```rust
ToolError::Sdk {
    sdk_kind: "marketplace_write_auth_required".into(),
    message: "marketplace write actions require configured admin auth".into(),
}
```

and:

```rust
ToolError::Sdk {
    sdk_kind: "csrf_rejected".into(),
    message: "marketplace write action rejected by origin policy".into(),
}
```

- [ ] **Step 4: Document error kinds**

Add to `docs/dev/ERRORS.md` stable kinds:

```markdown
- `marketplace_write_auth_required` - marketplace write action was rejected because admin auth is not configured or not present.
- `csrf_rejected` - browser-origin write action failed Origin/Referer/CSRF validation.
- `invalid_marketplace_schema` - managed marketplace JSON does not conform to Claude-compatible shape.
- `invalid_plugin_source` - plugin source is missing, unsupported, or not portable for the requested operation.
- `share_not_ready` - share/publish was blocked by readiness analysis.
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace_write_action
```

Expected: API security tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/api/services/marketplace.rs crates/lab/src/api/services/helpers.rs docs/dev/ERRORS.md
git commit -m "fix(marketplace): require admin gate for writes"
```

## Task 6: Add Indexed Plugin To Managed Marketplace

**Beads:** `lab-dzvv.4`

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/managed.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab/src/dispatch/marketplace/params.rs`
- Create/Modify: `crates/lab/src/dispatch/marketplace/redact.rs`
- Test: `crates/lab/src/dispatch/marketplace/managed.rs`

- [ ] **Step 1: Write failing curation tests**

Add tests in `managed.rs`:

```rust
#[tokio::test]
async fn entry_add_preserves_portable_plugin_source() {
    let harness = ManagedHarness::new().await;
    let plugin_id = harness.insert_plugin_with_source(serde_json::json!({
        "type": "git-subdir",
        "url": "https://github.com/example/monorepo.git",
        "path": "plugins/demo"
    })).await;
    harness.create_marketplace("personal").await;
    let result = managed_entry_add(ManagedEntryAddParams {
        marketplace_slug: "personal".into(),
        plugin_id,
        conflict: ConflictPolicy::Reject,
    })
    .await
    .expect("add entry");
    assert_eq!(result["written"], true);
    let manifest = harness.read_marketplace("personal");
    assert_eq!(manifest["plugins"][0]["source"]["type"], "git-subdir");
    assert_eq!(manifest["plugins"][0]["source"]["path"], "plugins/demo");
}

#[tokio::test]
async fn entry_add_rejects_cache_or_install_path_sources() {
    let harness = ManagedHarness::new().await;
    let plugin_id = harness.insert_plugin_with_source(serde_json::json!({
        "type": "local-cache",
        "path": "/home/me/.claude/plugins/cache/private"
    })).await;
    harness.create_marketplace("personal").await;
    let err = managed_entry_add(ManagedEntryAddParams {
        marketplace_slug: "personal".into(),
        plugin_id,
        conflict: ConflictPolicy::Reject,
    })
    .await
    .unwrap_err();
    assert_eq!(err.kind(), "invalid_plugin_source");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::managed::entry_add
```

Expected: compile failure or invalid action.

- [ ] **Step 3: Add curation params and actions**

In `catalog.rs`, add:

```rust
ActionSpec {
    name: "managed.entry.preview_add",
    description: "Preview adding an indexed plugin entry to a managed marketplace",
    destructive: false,
    params: &[/* marketplaceSlug, pluginId, conflict */],
    returns: "MarketplaceEntryPreview",
},
ActionSpec {
    name: "managed.entry.add",
    description: "Add an indexed plugin entry to a managed marketplace",
    destructive: true,
    params: &[/* marketplaceSlug, pluginId, conflict */],
    returns: "ManagedWriteResult",
},
```

In `params.rs`, parse:

```rust
pub enum ConflictPolicy {
    Reject,
    Replace,
}

pub struct ManagedEntryAddParams {
    pub marketplace_slug: String,
    pub plugin_id: String,
    pub conflict: ConflictPolicy,
}
```

- [ ] **Step 4: Implement redaction helper**

Create `redact.rs`:

```rust
use serde_json::Value;

pub fn contains_secret_material(value: &Value) -> bool {
    let rendered = value.to_string().to_lowercase();
    rendered.contains("authorization")
        || rendered.contains("bearer ")
        || rendered.contains("api_key")
        || rendered.contains("token=")
        || rendered.contains("_token")
        || rendered.contains("_password")
        || rendered.contains("_secret")
        || rendered.contains("https://") && rendered.contains('@')
}

pub fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_lowercase();
                    if lower.contains("token")
                        || lower.contains("password")
                        || lower.contains("secret")
                        || lower.contains("authorization")
                        || lower.contains("api_key")
                    {
                        (key.clone(), Value::String("[REDACTED]".into()))
                    } else {
                        (key.clone(), redact_value(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::String(s) if s.contains("://") && s.contains('@') => Value::String("[REDACTED_URL]".into()),
        other => other.clone(),
    }
}
```

- [ ] **Step 5: Implement preview/add**

`managed.entry.preview_add` must:

1. Load target managed marketplace manifest inside `locked_atomic_json_write` preview-equivalent lock or a read lock.
2. Load indexed plugin row by id from `CatalogStore`.
3. Use `plugin_source_json`, not `cachePath`, `sourcePath`, or installed state.
4. Reject source shapes that are local cache/install paths.
5. Reject duplicate names unless conflict is `Replace`.
6. Return a redacted preview with `truncated: false`.

`managed.entry.add` uses the same transform while holding the write lock and writes JSON.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::managed
```

Expected: managed create/add/path/redaction tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/managed.rs crates/lab/src/dispatch/marketplace/redact.rs crates/lab/src/dispatch/marketplace/catalog.rs crates/lab/src/dispatch/marketplace/dispatch.rs crates/lab/src/dispatch/marketplace/params.rs
git commit -m "feat(marketplace): add indexed plugin to managed marketplace"
```

## Task 7: MCP Registry Server To Generated Plugin Wrapper

**Beads:** `lab-dzvv.5`

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/mcp_wrapper.rs`
- Modify: `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs`
- Modify: `crates/lab/src/dispatch/marketplace/mcp_catalog.rs`
- Test: `crates/lab/src/dispatch/marketplace/mcp_wrapper.rs`

- [ ] **Step 1: Write failing wrapper tests**

Add:

```rust
#[test]
fn validates_env_names_and_rejects_secret_defaults() {
    assert!(validate_env_name("OPENAI_API_KEY").is_ok());
    assert!(validate_env_name("bad-name").is_err());
    assert!(reject_secret_default("OPENAI_API_KEY", Some("sk-live-secret")).is_err());
}

#[test]
fn generated_wrapper_uses_mcp_servers_and_placeholders() {
    let preview = preview_mcp_plugin(McpPluginPreviewInput {
        server_name: "example/server".into(),
        version: "1.0.0".into(),
        slug: "example-server".into(),
        command: "npx".into(),
        args: vec!["-y".into(), "@example/server".into()],
        env_names: vec!["EXAMPLE_TOKEN".into()],
    })
    .expect("preview");
    let plugin_json = preview.file(".claude-plugin/plugin.json").expect("plugin json");
    assert!(plugin_json.contains("\"mcpServers\""));
    assert!(plugin_json.contains("\"EXAMPLE_TOKEN\": \"${EXAMPLE_TOKEN}\""));
    assert!(!plugin_json.contains("sk-"));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::mcp_wrapper
```

Expected: compile failure because wrapper generator is absent.

- [ ] **Step 3: Implement wrapper preview**

Create `mcp_wrapper.rs` with:

```rust
pub fn validate_env_name(name: &str) -> Result<(), ToolError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(invalid_param("env", "env var name cannot be empty"));
    };
    if !(first == '_' || first.is_ascii_uppercase()) {
        return Err(invalid_param("env", "env var name must start with uppercase ASCII or underscore"));
    }
    if !chars.all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return Err(invalid_param("env", "env var name must contain uppercase ASCII, digits, or underscore only"));
    }
    Ok(())
}

pub fn reject_secret_default(name: &str, value: Option<&str>) -> Result<(), ToolError> {
    if value.is_some_and(|v| !v.trim().is_empty()) {
        return Err(ToolError::Sdk {
            sdk_kind: "generated_plugin_invalid".into(),
            message: format!("registry env `{name}` includes a default value; Lab only writes placeholders"),
        });
    }
    Ok(())
}
```

Build `.claude-plugin/plugin.json` with `mcpServers` using command and args arrays from the existing `mcp.install` config-building logic.

- [ ] **Step 4: Add MCP actions**

Add `mcp.plugin.preview` and `mcp.plugin.add` to `mcp_catalog.rs` and dispatch through `mcp_dispatch.rs`. Mark `mcp.plugin.add` destructive.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::mcp_wrapper
```

Expected: wrapper tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/mcp_wrapper.rs crates/lab/src/dispatch/marketplace/mcp_catalog.rs crates/lab/src/dispatch/marketplace/mcp_dispatch.rs crates/lab/src/dispatch/marketplace.rs
git commit -m "feat(marketplace): generate mcp wrapper plugins"
```

## Task 8: Share Readiness And Local Publish Plan

**Beads:** `lab-dzvv.6`

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/share.rs`
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Test: `crates/lab/src/dispatch/marketplace/share.rs`

- [ ] **Step 1: Write failing readiness tests**

Add:

```rust
#[test]
fn url_ready_requires_external_sources_only() {
    let readiness = analyze_share_readiness(&serde_json::json!({
        "name": "external",
        "plugins": [
            {"name": "a", "source": {"type": "github", "repo": "owner/repo"}},
            {"name": "b", "source": {"type": "npm", "package": "@example/plugin"}}
        ]
    }))
    .expect("readiness");
    assert_eq!(readiness.status, ShareStatus::UrlReady);
}

#[test]
fn relative_generated_sources_are_git_required() {
    let readiness = analyze_share_readiness(&serde_json::json!({
        "name": "generated",
        "plugins": [
            {"name": "server", "source": "./plugins/server"}
        ]
    }))
    .expect("readiness");
    assert_eq!(readiness.status, ShareStatus::GitRequired);
}

#[test]
fn secret_material_blocks_sharing() {
    let readiness = analyze_share_readiness(&serde_json::json!({
        "name": "secret",
        "plugins": [
            {"name": "secret", "source": {"type": "url", "url": "https://user:token@example.com/repo.git"}}
        ]
    }))
    .expect("readiness");
    assert_eq!(readiness.status, ShareStatus::Blocked);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::share
```

Expected: compile failure because share module is absent.

- [ ] **Step 3: Implement readiness**

Create:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareStatus {
    UrlReady,
    GitRequired,
    LocalOnly,
    Blocked,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShareReadiness {
    pub status: ShareStatus,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
}

pub fn analyze_share_readiness(manifest: &serde_json::Value) -> Result<ShareReadiness, ToolError> {
    if crate::dispatch::marketplace::redact::contains_secret_material(manifest) {
        return Ok(ShareReadiness {
            status: ShareStatus::Blocked,
            warnings: Vec::new(),
            blockers: vec!["secret material detected in marketplace manifest".into()],
        });
    }
    let plugins = manifest
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "invalid_marketplace_schema".into(),
            message: "marketplace manifest must contain plugins array".into(),
        })?;
    let mut git_required = false;
    let mut local_only = false;
    for plugin in plugins {
        let source = plugin.get("source");
        match source {
            Some(serde_json::Value::String(s)) if s.starts_with("./") => git_required = true,
            Some(serde_json::Value::String(_)) => local_only = true,
            Some(serde_json::Value::Object(obj)) => {
                let ty = obj.get("type").and_then(serde_json::Value::as_str);
                if !matches!(ty, Some("github" | "url" | "git-subdir" | "npm")) {
                    local_only = true;
                }
            }
            _ => local_only = true,
        }
    }
    let status = if local_only {
        ShareStatus::LocalOnly
    } else if git_required {
        ShareStatus::GitRequired
    } else {
        ShareStatus::UrlReady
    };
    Ok(ShareReadiness { status, warnings: Vec::new(), blockers: Vec::new() })
}
```

- [ ] **Step 4: Add actions**

Add `managed.share.check` as non-destructive. Keep `managed.share.publish` behind readiness and non-shell dry-run behavior if implemented in this task.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::share
```

Expected: share readiness tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/share.rs crates/lab/src/dispatch/marketplace/catalog.rs crates/lab/src/dispatch/marketplace/dispatch.rs crates/lab/src/dispatch/marketplace.rs
git commit -m "feat(marketplace): add share readiness"
```

## Task 9: Gateway Admin Paginated Curation Client And UI

**Beads:** `lab-dzvv.7`

**Files:**
- Create: `apps/gateway-admin/lib/api/marketplace-curation-types.ts`
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`
- Modify: `apps/gateway-admin/lib/marketplace/api-client.ts`
- Modify: `apps/gateway-admin/lib/hooks/use-marketplace.ts`
- Modify: `apps/gateway-admin/components/marketplace/marketplace-state.ts`
- Modify: `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx`
- Test: `apps/gateway-admin/lib/api/marketplace-client-editing.test.ts`
- Test: `apps/gateway-admin/components/marketplace/marketplace-state.test.ts`

- [ ] **Step 1: Write failing client tests**

Add:

```ts
it('fetches paginated plugins with backend query params', async () => {
  const calls: unknown[] = []
  mockServiceAction((body) => {
    calls.push(body)
    return Promise.resolve({ items: [], page: { nextCursor: null } })
  })
  await fetchPlugins({ query: 'rust', limit: 25, cursor: 'abc' })
  expect(calls[0]).toMatchObject({
    action: 'plugins.list',
    params: { query: 'rust', limit: 25, cursor: 'abc' },
  })
})

it('adds plugin to managed marketplace through primary marketplace client', async () => {
  const calls: unknown[] = []
  mockServiceAction((body) => {
    calls.push(body)
    return Promise.resolve({ written: true })
  })
  await addPluginToManagedMarketplace({
    marketplaceSlug: 'personal',
    pluginId: 'demo@source',
    conflict: 'reject',
  })
  expect(calls[0]).toMatchObject({
    action: 'managed.entry.add',
    params: {
      marketplaceSlug: 'personal',
      pluginId: 'demo@source',
      conflict: 'reject',
    },
  })
})
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin test -- marketplace-client-editing.test.ts marketplace-state.test.ts
```

Expected: tests fail because client methods and paginated shapes are absent.

- [ ] **Step 3: Add curation types**

Create:

```ts
export interface PageInfo {
  nextCursor?: string | null
}

export interface Paged<T> {
  items: T[]
  page: PageInfo
}

export type ConflictPolicy = 'reject' | 'replace'

export interface AddPluginToManagedMarketplaceInput {
  marketplaceSlug: string
  pluginId: string
  conflict: ConflictPolicy
}

export interface ManagedWriteResult {
  written: boolean
  path?: string
}

export type ShareStatus = 'url_ready' | 'git_required' | 'local_only' | 'blocked'
```

- [ ] **Step 4: Update primary client**

In `marketplace-client.ts`, change `fetchPlugins` to accept params and normalize both old array and new paged shapes during transition:

```ts
export interface FetchPluginsParams {
  marketplace?: string
  query?: string
  kind?: string
  installed?: boolean
  limit?: number
  cursor?: string | null
}

export async function fetchPlugins(
  params: FetchPluginsParams = {},
  signal?: AbortSignal,
): Promise<Paged<Plugin>> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return { items: cloneValue(MOCK_PLUGINS), page: { nextCursor: null } }
  }
  const result = await marketplaceAction<Paged<RawPlugin> | RawPlugin[]>('plugins.list', params, signal)
  const items = Array.isArray(result) ? result : result.items
  const page = Array.isArray(result) ? { nextCursor: null } : result.page
  return { items: items.map(normalizePlugin), page }
}

export async function addPluginToManagedMarketplace(
  input: AddPluginToManagedMarketplaceInput,
  signal?: AbortSignal,
): Promise<ManagedWriteResult> {
  return marketplaceAction<ManagedWriteResult>('managed.entry.add', input, signal)
}
```

- [ ] **Step 5: Keep legacy client read-only/wrapped**

In `lib/marketplace/api-client.ts`, do not add new marketplace write behavior. If MCP/ACP views need plugin curation, import from `../api/marketplace-client`.

- [ ] **Step 6: Update UI state**

Stop using full-catalog client-side filtering as the authority. Keep local visible-item shaping only for the current page. Search, sort, and filters call backend params.

- [ ] **Step 7: Run frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- marketplace-client-editing.test.ts marketplace-state.test.ts
```

Expected: frontend tests pass.

- [ ] **Step 8: Commit**

```bash
git add apps/gateway-admin/lib/api/marketplace-curation-types.ts apps/gateway-admin/lib/api/marketplace-client.ts apps/gateway-admin/lib/marketplace/api-client.ts apps/gateway-admin/lib/hooks/use-marketplace.ts apps/gateway-admin/components/marketplace/marketplace-state.ts apps/gateway-admin/components/marketplace/marketplace-list-content.tsx apps/gateway-admin/lib/api/marketplace-client-editing.test.ts apps/gateway-admin/components/marketplace/marketplace-state.test.ts
git commit -m "feat(marketplace): add paginated curation UI client"
```

## Task 10: Docs, Generated Inventories, And Full Verification

**Beads:** `lab-dzvv.8`

**Files:**
- Create: `docs/services/MARKETPLACE.md`
- Modify: `docs/dev/SERVICES.md`
- Modify: `docs/generated/*` via `just docs-generate`
- Modify: relevant coverage docs if generated audit requires it

- [ ] **Step 1: Write service docs**

Create `docs/services/MARKETPLACE.md`:

```markdown
# Marketplace

`marketplace` is a product-local control-plane service. It owns Lab's searchable marketplace catalog and user-managed Claude-compatible marketplace files.

## Source Truth

`known_marketplaces.json` and Codex catalog files are import inputs. They seed Lab's catalog store but are not the steady-state Lab source of truth.

`installed_plugins.json` is install state only. It can inform installed badges and installed-state version hints, but it never creates catalog sources.

## Managed Marketplaces

Managed marketplaces live under:

```text
~/.labby/marketplaces/<slug>/.claude-plugin/marketplace.json
```

Lab writes these files atomically and preserves Claude-compatible JSON. Lab-only provenance stays in the catalog store.

## Version Display

Version resolution follows:

1. plugin manifest version
2. marketplace entry version
3. installed-state version as an installed hint
4. unknown or unavailable with a reason

Remote git SHA resolution is not part of list hot paths.

## Sharing

URL sharing is valid only when every plugin source is externally resolvable. Relative/generated plugin sources require Git-backed sharing. Share readiness blocks secret material.
```
```

- [ ] **Step 2: Update service docs index**

In `docs/dev/SERVICES.md`, add `marketplace` to product-local control-plane services if the generated docs do not already make it explicit:

```markdown
`marketplace` is also a product-local control-plane service. It owns Lab's catalog
and managed marketplace files, while external Claude/Codex files are import
adapters and install targets.
```

- [ ] **Step 3: Generate docs**

Run:

```bash
just docs-generate
```

Expected: generated action/service docs refresh without errors.

- [ ] **Step 4: Check docs**

Run:

```bash
just docs-check
```

Expected: docs check passes.

- [ ] **Step 5: Run Rust focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features marketplace::
```

Expected: all marketplace tests pass.

- [ ] **Step 6: Run frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- marketplace
```

Expected: marketplace frontend tests pass.

- [ ] **Step 7: Run default Lab verification**

Run:

```bash
cargo nextest run --workspace --all-features
```

Expected: workspace nextest suite passes.

- [ ] **Step 8: Commit**

```bash
git add docs/services/MARKETPLACE.md docs/dev/SERVICES.md docs/generated crates/lab crates/lab-apis apps/gateway-admin
git commit -m "docs(marketplace): document catalog gateway"
```

## Self-Review

Spec coverage:

- `lab-dzvv.1` is covered by Tasks 1-3.
- `lab-dzvv.2` is covered by Tasks 2-3.
- `lab-dzvv.3` is covered by Tasks 4-5.
- `lab-dzvv.4` is covered by Task 6.
- `lab-dzvv.5` is covered by Task 7 and blocked behind Wave 1.
- `lab-dzvv.6` is covered by Task 8.
- `lab-dzvv.7` is covered by Task 9.
- `lab-dzvv.8` is covered throughout and finalized by Task 10.

Placeholder scan:

- No task uses placeholder language for implementation instructions.
- Every coding task names exact files and commands.
- Deferred scope is listed as out-of-scope work, not as an implementation step.

Type consistency:

- Pagination shape is consistently `Paged<T> { items, page: { nextCursor } }`.
- Version fields are consistently `resolvedVersion` and `versionSource` on wire.
- Marketplace curation input uses `marketplaceSlug`, `pluginId`, and `conflict`.
