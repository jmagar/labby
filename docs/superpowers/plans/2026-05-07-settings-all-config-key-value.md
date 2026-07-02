# Settings All Config Key/Value Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the existing Settings rail so operators can view and edit all Lab `.env` keys and structured `config.toml` values from the Web UI without adding a raw file editor.

**Architecture:** Keep following the current Bootstrap settings pattern: the frontend calls `/v1/setup`, `setup` dispatch owns persistence, `.env` writes continue through `env_merge`, and Settings keeps its rail/page layout. Add a structured config inventory model that combines known schema-backed fields with unknown/custom `.env` keys and typed `config.toml` paths, then render that model as grouped key/value rows in the existing Advanced settings route renamed to All Config.

**Tech Stack:** Rust 2024, axum dispatch actions, serde/toml, existing `env_merge`, Next.js 16 static export, React 19, TypeScript, existing `setup-client`, shadcn/Radix controls with Aurora tokens.

---

## Current Repo Facts

- Current Settings is not a general key/value editor. It edits hardcoded Core keys, schema-backed service env vars, Extract URL discoveries, and gateway posture/tool-search settings.
- Existing write path for Settings is `setup.draft.set` followed by `setup.draft.commit`.
- `.env` preservation, backup, stale detection, and atomic writes already live in `crates/lab/src/config/env_merge.rs`.
- `config.toml` is loaded structurally in `crates/lab/src/config.rs` as `LabConfig`; service credentials are expected in `.env`, while non-secret operator preferences live in `config.toml`.
- `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx` is currently a v2 raw-editor stub. Replace this with a structured All Config key/value manager.

## File Map

### Backend

- Modify: `crates/lab/src/dispatch/setup/catalog.rs`
  - Add `config.list`, `config.set`, `config.delete`, and `config.validate`.
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`
  - Route new actions and redact secret-bearing config action params in logs.
- Modify: `crates/lab/src/dispatch/setup/client.rs`
  - Add `config_toml_path()` helper that mirrors `crate::config::config_toml_path()` but honors `LABBY_HOME` for tests.
- Create: `crates/lab/src/dispatch/setup/config_inventory.rs`
  - Build the Web UI inventory for `.env`, `.env.draft`, known `PluginMeta` env vars, core fields, and editable `config.toml` paths.
- Create: `crates/lab/src/dispatch/setup/toml_edit.rs`
  - Read, update, delete, validate, backup, and atomically write `config.toml` values using `toml::Value`.
- Modify: `crates/lab/src/dispatch/setup/params.rs`
  - Parse config action params.

### Frontend

- Modify: `apps/gateway-admin/lib/api/setup-client.ts`
  - Add TypeScript models and client methods for config inventory actions.
- Create: `apps/gateway-admin/lib/setup/configInventory.ts`
  - Client-side grouping, filtering, dirty-state helpers, and display labels.
- Modify: `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx`
  - Replace raw-editor stub with All Config page.
- Modify: `apps/gateway-admin/components/settings/SettingsRail.tsx`
  - Rename Advanced label to All Config and remove raw-editor/v2 implication.
- Create: `apps/gateway-admin/components/settings/ConfigKeyValueTable.tsx`
  - Shared grouped key/value table with add/edit/delete row controls.
- Create: `apps/gateway-admin/components/settings/ConfigValueInput.tsx`
  - Type-aware input for string, number, bool, enum, secret, and JSON/TOML array/object fallback.
- Test: `apps/gateway-admin/lib/api/setup-client.test.ts`
- Test: `apps/gateway-admin/lib/setup/configInventory.test.ts`
- Test: `apps/gateway-admin/components/settings/ConfigKeyValueTable.test.tsx`

---

## Task 1: Backend Config Inventory Types And Catalog Actions

**Files:**
- Create: `crates/lab/src/dispatch/setup/config_inventory.rs`
- Modify: `crates/lab/src/dispatch/setup.rs`
- Modify: `crates/lab/src/dispatch/setup/catalog.rs`

- [ ] **Step 1: Add failing action catalog test**

Add this test to `crates/lab/src/dispatch/setup.rs`:

```rust
#[cfg(test)]
mod config_inventory_catalog_tests {
    use super::catalog::ACTIONS;

    #[test]
    fn setup_catalog_includes_config_inventory_actions() {
        let names: Vec<&str> = ACTIONS.iter().map(|action| action.name).collect();

        assert!(names.contains(&"config.list"));
        assert!(names.contains(&"config.set"));
        assert!(names.contains(&"config.delete"));
        assert!(names.contains(&"config.validate"));
    }
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --manifest-path crates/lab/Cargo.toml config_inventory_catalog_tests --all-features`

Expected: FAIL because `config.list`, `config.set`, `config.delete`, and `config.validate` are absent.

- [ ] **Step 3: Add inventory data types**

Create `crates/lab/src/dispatch/setup/config_inventory.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Env,
    Toml,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueKind {
    String,
    Secret,
    Bool,
    Number,
    Array,
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntryView {
    pub id: String,
    pub source: ConfigSource,
    pub key: String,
    pub path: Vec<String>,
    pub group: String,
    pub label: String,
    pub description: String,
    pub value: serde_json::Value,
    pub value_kind: ConfigValueKind,
    pub secret: bool,
    pub known: bool,
    pub required: bool,
    pub editable: bool,
    pub deletable: bool,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigInventoryResponse {
    pub env_path: String,
    pub draft_path: String,
    pub toml_path: String,
    pub draft_stale: bool,
    pub entries: Vec<ConfigEntryView>,
}

pub fn empty_inventory(env_path: String, draft_path: String, toml_path: String) -> ConfigInventoryResponse {
    ConfigInventoryResponse {
        env_path,
        draft_path,
        toml_path,
        draft_stale: false,
        entries: Vec::new(),
    }
}
```

- [ ] **Step 4: Export the module**

In `crates/lab/src/dispatch/setup.rs`, add:

```rust
pub mod config_inventory;
```

- [ ] **Step 5: Add catalog entries**

In `crates/lab/src/dispatch/setup/catalog.rs`, add these `ActionSpec` entries before `finalize`:

```rust
ActionSpec {
    name: "config.list",
    description: "List all editable .env and config.toml key/value settings for the Settings UI",
    destructive: false,
    returns: "ConfigInventoryResponse",
    params: &[],
},
ActionSpec {
    name: "config.validate",
    description: "Validate one .env key or config.toml path/value without writing it",
    destructive: false,
    returns: "ConfigValidationOutcome",
    params: &[
        ParamSpec {
            name: "source",
            ty: "string",
            required: true,
            description: "env or toml",
        },
        ParamSpec {
            name: "key",
            ty: "string",
            required: true,
            description: ".env key or dot-separated config.toml path",
        },
        ParamSpec {
            name: "value",
            ty: "json",
            required: true,
            description: "Candidate value",
        },
    ],
},
ActionSpec {
    name: "config.set",
    description: "Set one .env key or config.toml path using the structured settings write path",
    destructive: true,
    returns: "ConfigMutationOutcome",
    params: &[
        ParamSpec {
            name: "source",
            ty: "string",
            required: true,
            description: "env or toml",
        },
        ParamSpec {
            name: "key",
            ty: "string",
            required: true,
            description: ".env key or dot-separated config.toml path",
        },
        ParamSpec {
            name: "value",
            ty: "json",
            required: true,
            description: "Value to write",
        },
        ParamSpec {
            name: "force",
            ty: "bool",
            required: false,
            description: "Overwrite conflicting existing values",
        },
    ],
},
ActionSpec {
    name: "config.delete",
    description: "Delete one optional/custom .env key or config.toml path",
    destructive: true,
    returns: "ConfigMutationOutcome",
    params: &[
        ParamSpec {
            name: "source",
            ty: "string",
            required: true,
            description: "env or toml",
        },
        ParamSpec {
            name: "key",
            ty: "string",
            required: true,
            description: ".env key or dot-separated config.toml path",
        },
        ParamSpec {
            name: "force",
            ty: "bool",
            required: false,
            description: "Confirm deletion",
        },
    ],
},
```

- [ ] **Step 6: Run catalog test**

Run: `cargo test --manifest-path crates/lab/Cargo.toml config_inventory_catalog_tests --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/setup.rs crates/lab/src/dispatch/setup/catalog.rs crates/lab/src/dispatch/setup/config_inventory.rs
git commit -m "feat(setup): add config inventory action catalog"
```

---

## Task 2: Backend .env Inventory Using Existing Setup Patterns

**Files:**
- Modify: `crates/lab/src/dispatch/setup/config_inventory.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`

- [ ] **Step 1: Add failing inventory test**

Add to `crates/lab/src/dispatch/setup/config_inventory.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::setup::draft::EnvDraftEntry;

    #[test]
    fn env_inventory_includes_known_and_unknown_env_keys() {
        let entries = vec![
            EnvDraftEntry { key: "RADARR_URL".into(), value: "http://radarr:7878".into() },
            EnvDraftEntry { key: "CUSTOM_LAB_FLAG".into(), value: "enabled".into() },
            EnvDraftEntry { key: "RADARR_API_KEY".into(), value: "secret".into() },
        ];

        let views = env_entries_to_config_views(&entries, "/tmp/.env");

        assert!(views.iter().any(|entry| entry.key == "RADARR_URL" && entry.known));
        assert!(views.iter().any(|entry| entry.key == "CUSTOM_LAB_FLAG" && !entry.known));
        let secret = views.iter().find(|entry| entry.key == "RADARR_API_KEY").unwrap();
        assert!(secret.secret);
        assert_eq!(secret.value, serde_json::json!("***"));
    }
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --manifest-path crates/lab/Cargo.toml env_inventory_includes_known_and_unknown_env_keys --all-features`

Expected: FAIL because `EnvDraftEntry` is not public and `env_entries_to_config_views` does not exist.

- [ ] **Step 3: Expose a reusable draft entry type**

In `crates/lab/src/dispatch/setup/draft.rs`, rename or add a public entry type used by existing reads:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvDraftEntry {
    pub key: String,
    pub value: String,
}
```

Update `read_entries()` to return `Vec<EnvDraftEntry>` and keep call sites unchanged by using the same `key` and `value` fields.

- [ ] **Step 4: Implement env inventory projection**

Add to `config_inventory.rs`:

```rust
use std::collections::BTreeSet;

use serde_json::json;

use crate::dispatch::setup::{draft::EnvDraftEntry, secret_mask};
use crate::registry::service_meta;

pub fn known_env_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for service in crate::registry::build_default_registry().services() {
        if let Some(meta) = service_meta(service.name) {
            for env in meta.required_env.iter().chain(meta.optional_env.iter()) {
                keys.insert(env.name.to_string());
            }
        }
    }
    for key in ["LAB_MCP_HTTP_HOST", "LAB_MCP_HTTP_PORT", "LAB_LOG", "LAB_LOG_FORMAT"] {
        keys.insert(key.to_string());
    }
    keys
}

pub fn env_entries_to_config_views(entries: &[EnvDraftEntry], file_path: &str) -> Vec<ConfigEntryView> {
    let known = known_env_keys();
    entries
        .iter()
        .map(|entry| {
            let secret = secret_mask::is_secret_key(&entry.key);
            ConfigEntryView {
                id: format!("env:{}", entry.key),
                source: ConfigSource::Env,
                key: entry.key.clone(),
                path: vec![entry.key.clone()],
                group: env_group(&entry.key),
                label: entry.key.clone(),
                description: env_description(&entry.key),
                value: json!(if secret { "***" } else { entry.value.as_str() }),
                value_kind: if secret { ConfigValueKind::Secret } else { ConfigValueKind::String },
                secret,
                known: known.contains(&entry.key),
                required: false,
                editable: true,
                deletable: !known.contains(&entry.key),
                file_path: file_path.to_string(),
            }
        })
        .collect()
}

fn env_group(key: &str) -> String {
    if key.starts_with("LAB_AUTH_") || key == "LAB_AUTH_MODE" || key.starts_with("LAB_GOOGLE_") {
        "Auth".into()
    } else if key == "LAB_LOG" || key == "LAB_LOG_FORMAT" {
        "Logging".into()
    } else if key.starts_with("LAB_") {
        "Core".into()
    } else if key.contains("_URL") || key.contains("_TOKEN") || key.contains("_API_KEY") || key.contains("_PASSWORD") {
        "Services".into()
    } else {
        "Custom".into()
    }
}

fn env_description(key: &str) -> String {
    match key {
        "LAB_MCP_HTTP_HOST" => "Host the lab HTTP server binds to.".into(),
        "LAB_MCP_HTTP_PORT" => "TCP port for the lab HTTP server.".into(),
        "LAB_LOG" => "tracing-subscriber filter directive.".into(),
        "LAB_LOG_FORMAT" => "Log format, usually text or json.".into(),
        _ => String::new(),
    }
}
```

- [ ] **Step 5: Add dispatch action stub for config.list**

In `dispatch.rs`, import `config_inventory` and add route:

```rust
"config.list" => config_list_action(),
```

Add:

```rust
fn config_list_action() -> Result<Value, ToolError> {
    let env = env_path();
    let draft = draft_path();
    let env_entries = draft::read_entries(&env);
    let entries = config_inventory::env_entries_to_config_views(
        &env_entries,
        &env.to_string_lossy(),
    );
    Ok(json!({
        "env_path": env,
        "draft_path": draft,
        "toml_path": crate::config::config_toml_path().unwrap_or_else(|| std::path::PathBuf::from("config.toml")),
        "draft_stale": false,
        "entries": entries,
    }))
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml env_inventory_includes_known_and_unknown_env_keys --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/setup/draft.rs crates/lab/src/dispatch/setup/config_inventory.rs crates/lab/src/dispatch/setup/dispatch.rs
git commit -m "feat(setup): list env config entries"
```

---

## Task 3: Backend Structured config.toml Inventory

**Files:**
- Create: `crates/lab/src/dispatch/setup/toml_edit.rs`
- Modify: `crates/lab/src/dispatch/setup/config_inventory.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`

- [ ] **Step 1: Add failing TOML flattening test**

Create `crates/lab/src/dispatch/setup/toml_edit.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_toml_lists_leaf_paths() {
        let raw = r#"
[log]
filter = "lab=info"
format = "json"

[tool_search]
enabled = true
top_k_default = 8
"#;
        let value: toml::Value = toml::from_str(raw).unwrap();
        let leaves = flatten_toml(&value);

        assert!(leaves.iter().any(|leaf| leaf.path == vec!["log", "filter"] && leaf.value == toml::Value::String("lab=info".into())));
        assert!(leaves.iter().any(|leaf| leaf.path == vec!["tool_search", "enabled"] && leaf.value == toml::Value::Boolean(true)));
    }
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --manifest-path crates/lab/Cargo.toml flatten_toml_lists_leaf_paths --all-features`

Expected: FAIL because `flatten_toml` and `TomlLeaf` do not exist.

- [ ] **Step 3: Implement TOML flattening**

Add to `toml_edit.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct TomlLeaf {
    pub path: Vec<String>,
    pub value: toml::Value,
}

pub fn flatten_toml(value: &toml::Value) -> Vec<TomlLeaf> {
    let mut leaves = Vec::new();
    flatten_into(value, Vec::new(), &mut leaves);
    leaves
}

fn flatten_into(value: &toml::Value, path: Vec<String>, leaves: &mut Vec<TomlLeaf>) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let mut next = path.clone();
                next.push(key.clone());
                flatten_into(child, next, leaves);
            }
        }
        _ => leaves.push(TomlLeaf { path, value: value.clone() }),
    }
}
```

- [ ] **Step 4: Add TOML inventory projection**

Add to `config_inventory.rs`:

```rust
pub fn toml_leaves_to_config_views(leaves: &[crate::dispatch::setup::toml_edit::TomlLeaf], file_path: &str) -> Vec<ConfigEntryView> {
    leaves
        .iter()
        .map(|leaf| {
            let key = leaf.path.join(".");
            ConfigEntryView {
                id: format!("toml:{key}"),
                source: ConfigSource::Toml,
                key: key.clone(),
                path: leaf.path.clone(),
                group: toml_group(&leaf.path),
                label: key,
                description: String::new(),
                value: toml_value_to_json(&leaf.value),
                value_kind: toml_value_kind(&leaf.value),
                secret: false,
                known: true,
                required: false,
                editable: true,
                deletable: true,
                file_path: file_path.to_string(),
            }
        })
        .collect()
}

fn toml_group(path: &[String]) -> String {
    match path.first().map(String::as_str) {
        Some("auth") => "Auth".into(),
        Some("log") | Some("local_logs") => "Logging".into(),
        Some("mcp") | Some("api") | Some("web") => "Surfaces".into(),
        Some("tool_search") | Some("marketplace") | Some("mcpregistry") => "Features".into(),
        Some("services") => "Services".into(),
        _ => "Config TOML".into(),
    }
}

fn toml_value_kind(value: &toml::Value) -> ConfigValueKind {
    match value {
        toml::Value::Boolean(_) => ConfigValueKind::Bool,
        toml::Value::Integer(_) | toml::Value::Float(_) => ConfigValueKind::Number,
        toml::Value::Array(_) => ConfigValueKind::Array,
        toml::Value::Table(_) => ConfigValueKind::Object,
        _ => ConfigValueKind::String,
    }
}

fn toml_value_to_json(value: &toml::Value) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
```

- [ ] **Step 5: Include TOML entries in config.list**

In `dispatch.rs`, update `config_list_action()`:

```rust
let toml_path = crate::config::config_toml_path()
    .unwrap_or_else(|| crate::config::toml_candidates().into_iter().nth(1).unwrap_or_else(|| std::path::PathBuf::from("config.toml")));
let mut entries = config_inventory::env_entries_to_config_views(&env_entries, &env.to_string_lossy());
if toml_path.exists() {
    let raw = std::fs::read_to_string(&toml_path).map_err(|err| ToolError::Sdk {
        sdk_kind: "config_read_failed".into(),
        message: err.to_string(),
    })?;
    let parsed = toml::from_str::<toml::Value>(&raw).map_err(|err| ToolError::InvalidParam {
        message: format!("invalid config.toml: {err}"),
        param: "config.toml".into(),
    })?;
    let leaves = toml_edit::flatten_toml(&parsed);
    entries.extend(config_inventory::toml_leaves_to_config_views(&leaves, &toml_path.to_string_lossy()));
}
```

- [ ] **Step 6: Export toml_edit module**

In `crates/lab/src/dispatch/setup.rs`, add:

```rust
pub mod toml_edit;
```

- [ ] **Step 7: Run tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml flatten_toml_lists_leaf_paths --all-features`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/setup.rs crates/lab/src/dispatch/setup/toml_edit.rs crates/lab/src/dispatch/setup/config_inventory.rs crates/lab/src/dispatch/setup/dispatch.rs
git commit -m "feat(setup): list config toml entries"
```

---

## Task 4: Backend config.set And config.delete

**Files:**
- Modify: `crates/lab/src/dispatch/setup/toml_edit.rs`
- Modify: `crates/lab/src/dispatch/setup/params.rs`
- Modify: `crates/lab/src/dispatch/setup/dispatch.rs`
- Modify: `crates/lab/src/config.rs`

- [ ] **Step 1: Add failing TOML mutation tests**

Add to `toml_edit.rs` tests:

```rust
#[test]
fn set_toml_path_updates_nested_value() {
    let mut value: toml::Value = toml::from_str("[tool_search]\nenabled = false\n").unwrap();

    set_path(&mut value, &["tool_search".into(), "enabled".into()], toml::Value::Boolean(true)).unwrap();

    assert_eq!(value["tool_search"]["enabled"], toml::Value::Boolean(true));
}

#[test]
fn delete_toml_path_removes_leaf() {
    let mut value: toml::Value = toml::from_str("[tool_search]\nenabled = true\ntop_k_default = 8\n").unwrap();

    delete_path(&mut value, &["tool_search".into(), "top_k_default".into()]).unwrap();

    assert!(value.get("tool_search").unwrap().get("top_k_default").is_none());
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --manifest-path crates/lab/Cargo.toml 'toml_path' --all-features`

Expected: FAIL because `set_path` and `delete_path` do not exist.

- [ ] **Step 3: Implement path mutations**

Add to `toml_edit.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum TomlEditError {
    #[error("empty path")]
    EmptyPath,
    #[error("path segment `{0}` is not a table")]
    NotATable(String),
    #[error("missing path `{0}`")]
    MissingPath(String),
}

pub fn set_path(root: &mut toml::Value, path: &[String], value: toml::Value) -> Result<(), TomlEditError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(TomlEditError::EmptyPath);
    };
    let mut cursor = root;
    for segment in parents {
        let table = cursor
            .as_table_mut()
            .ok_or_else(|| TomlEditError::NotATable(segment.clone()))?;
        cursor = table
            .entry(segment.clone())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    }
    let table = cursor
        .as_table_mut()
        .ok_or_else(|| TomlEditError::NotATable(last.clone()))?;
    table.insert(last.clone(), value);
    Ok(())
}

pub fn delete_path(root: &mut toml::Value, path: &[String]) -> Result<(), TomlEditError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(TomlEditError::EmptyPath);
    };
    let mut cursor = root;
    for segment in parents {
        cursor = cursor
            .as_table_mut()
            .and_then(|table| table.get_mut(segment))
            .ok_or_else(|| TomlEditError::MissingPath(path.join(".")))?;
    }
    let table = cursor
        .as_table_mut()
        .ok_or_else(|| TomlEditError::NotATable(last.clone()))?;
    table.remove(last);
    Ok(())
}
```

- [ ] **Step 4: Add config action param parsing**

In `params.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSourceParam {
    Env,
    Toml,
}

pub fn parse_config_source(params: &serde_json::Value) -> Result<ConfigSourceParam, ToolError> {
    match crate::dispatch::helpers::require_str(params, "source")? {
        "env" => Ok(ConfigSourceParam::Env),
        "toml" => Ok(ConfigSourceParam::Toml),
        other => Err(ToolError::InvalidParam {
            message: format!("source must be env or toml, got {other}"),
            param: "source".into(),
        }),
    }
}

pub fn parse_config_key(params: &serde_json::Value) -> Result<String, ToolError> {
    let key = crate::dispatch::helpers::require_str(params, "key")?;
    if key.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "key must not be blank".into(),
            param: "key".into(),
        });
    }
    Ok(key.to_string())
}
```

- [ ] **Step 5: Implement env config.set through existing draft path**

In `dispatch.rs`, add routes:

```rust
"config.validate" => config_validate_action(params),
"config.set" => config_set_action(params).await,
"config.delete" => config_delete_action(params).await,
```

Add env branch implementation:

```rust
async fn config_set_action(params: &Value) -> Result<Value, ToolError> {
    let source = parse_config_source(params)?;
    let key = parse_config_key(params)?;
    let force = parse_force(params);
    let value = params.get("value").cloned().ok_or_else(|| ToolError::MissingParam {
        message: "missing value".into(),
        param: "value".into(),
    })?;

    match source {
        ConfigSourceParam::Env => {
            let string_value = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
            let set_params = json!({
                "entries": [{ "key": key, "value": string_value }],
                "force": force,
            });
            draft_set_action(&set_params).await?;
            draft_commit_action(&json!({ "force": true })).await
        }
        ConfigSourceParam::Toml => config_set_toml_action(&key, value),
    }
}
```

- [ ] **Step 6: Implement TOML write helper**

Add this public helper to `crates/lab/src/config.rs` near the existing `backup_env()` helper:

```rust
/// Write a non-.env config file atomically after creating a timestamped backup.
///
/// This is used by setup.config.* for structured config.toml writes. `.env`
/// writes must continue to use env_merge so comments/order/quoting semantics
/// stay centralized.
pub fn write_config_file_with_backup(path: &Path, contents: &[u8]) -> Result<Option<PathBuf>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }

    let backup = if path.exists() {
        Some(backup_env(path)?)
    } else {
        None
    };

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("create temp file in {}", parent.display()))?;
    tmp.write_all(contents)
        .with_context(|| format!("write temp config for {}", path.display()))?;
    tmp.flush()
        .with_context(|| format!("flush temp config for {}", path.display()))?;
    tmp.persist(path)
        .map_err(|err| err.error)
        .with_context(|| format!("persist config {}", path.display()))?;

    Ok(backup)
}
```

Then add to `dispatch.rs`:

```rust
fn config_set_toml_action(key: &str, value: Value) -> Result<Value, ToolError> {
    let path = crate::config::config_toml_path()
        .unwrap_or_else(|| crate::config::toml_candidates().into_iter().nth(1).unwrap_or_else(|| std::path::PathBuf::from("config.toml")));
    let raw = if path.exists() {
        std::fs::read_to_string(&path).map_err(|err| ToolError::Sdk {
            sdk_kind: "config_read_failed".into(),
            message: err.to_string(),
        })?
    } else {
        String::new()
    };
    let mut parsed = toml::from_str::<toml::Value>(&raw).unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()));
    let toml_value = serde_json::from_value::<toml::Value>(value).map_err(|err| ToolError::InvalidParam {
        message: format!("value cannot be represented as TOML: {err}"),
        param: "value".into(),
    })?;
    let segments: Vec<String> = key.split('.').map(str::to_string).collect();
    toml_edit::set_path(&mut parsed, &segments, toml_value).map_err(|err| ToolError::InvalidParam {
        message: err.to_string(),
        param: "key".into(),
    })?;
    let serialized = toml::to_string_pretty(&parsed).map_err(|err| ToolError::Sdk {
        sdk_kind: "config_serialize_failed".into(),
        message: err.to_string(),
    })?;
    let backup_path = crate::config::write_config_file_with_backup(&path, serialized.as_bytes()).map_err(|err| ToolError::Sdk {
        sdk_kind: "config_write_failed".into(),
        message: err.to_string(),
    })?;
    Ok(json!({ "written": 1, "skipped": [], "backup_path": backup_path }))
}
```

- [ ] **Step 7: Implement delete**

Use `env_merge` for env deletion only after adding a delete primitive, or implement delete as writing an empty value only if product accepts blank values. For this plan, make deletion strict:

```rust
async fn config_delete_action(params: &Value) -> Result<Value, ToolError> {
    let source = parse_config_source(params)?;
    let key = parse_config_key(params)?;
    let force = parse_force(params);
    if !force {
        return Err(ToolError::InvalidParam {
            message: "delete requires force=true".into(),
            param: "force".into(),
        });
    }
    match source {
        ConfigSourceParam::Env => Err(ToolError::InvalidParam {
            message: "env deletion is not available until env_merge exposes delete semantics".into(),
            param: "source".into(),
        }),
        ConfigSourceParam::Toml => config_delete_toml_action(&key),
    }
}
```

Add `config_delete_toml_action()` using `toml_edit::delete_path`, serialization, and the same atomic write helper as `config_set_toml_action`.

- [ ] **Step 8: Redact config action logs**

In `dispatch.rs`, extend `REDACTED_LOG_ACTIONS`:

```rust
const REDACTED_LOG_ACTIONS: &[&str] = &[
    "draft.set",
    "draft.commit",
    "finalize",
    "config.set",
    "config.delete",
    "config.validate",
];
```

- [ ] **Step 9: Run tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml 'toml_path|config_inventory' --all-features`

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/lab/src/dispatch/setup/toml_edit.rs crates/lab/src/dispatch/setup/params.rs crates/lab/src/dispatch/setup/dispatch.rs crates/lab/src/config.rs
git commit -m "feat(setup): mutate structured config values"
```

---

## Task 5: Frontend setup-client API

**Files:**
- Modify: `apps/gateway-admin/lib/api/setup-client.ts`
- Test: `apps/gateway-admin/lib/api/setup-client.test.ts`

- [ ] **Step 1: Add failing setup-client tests**

Append to `setup-client.test.ts`:

```ts
import { setupApi } from './setup-client.ts'
import { installFetchMock } from './test-utils.ts'

test('setupApi.configList calls setup config.list', async () => {
  const requests = installFetchMock([{ entries: [], env_path: '/tmp/.env', draft_path: '/tmp/.env.draft', toml_path: '/tmp/config.toml', draft_stale: false }])

  await setupApi.configList()

  assert.equal(requests[0]?.url, '/v1/setup')
  assert.deepEqual(JSON.parse(requests[0]?.body ?? '{}'), { action: 'config.list', params: {} })
})

test('setupApi.configSet sends source key and value', async () => {
  const requests = installFetchMock([{ written: 1, skipped: [], backup_path: '/tmp/config.toml.bak' }])

  await setupApi.configSet({ source: 'toml', key: 'tool_search.enabled', value: true, force: true })

  assert.deepEqual(JSON.parse(requests[0]?.body ?? '{}'), {
    action: 'config.set',
    params: { source: 'toml', key: 'tool_search.enabled', value: true, force: true, confirm: true },
  })
})
```

If `installFetchMock` does not exist in this file, copy the local mock pattern already used in `extract-client.test.ts` or `service-action-client.test.ts`.

- [ ] **Step 2: Run tests to verify failure**

Run: `pnpm --dir apps/gateway-admin test:unit -- lib/api/setup-client.test.ts`

Expected: FAIL because config methods and types are absent.

- [ ] **Step 3: Add TypeScript models**

In `setup-client.ts`, add:

```ts
export type ConfigSource = 'env' | 'toml'
export type ConfigValueKind = 'string' | 'secret' | 'bool' | 'number' | 'array' | 'object'

export interface ConfigEntryView {
  id: string
  source: ConfigSource
  key: string
  path: string[]
  group: string
  label: string
  description: string
  value: unknown
  value_kind: ConfigValueKind
  secret: boolean
  known: boolean
  required: boolean
  editable: boolean
  deletable: boolean
  file_path: string
}

export interface ConfigInventoryResponse {
  env_path: string
  draft_path: string
  toml_path: string
  draft_stale: boolean
  entries: ConfigEntryView[]
}

export interface ConfigMutationOutcome {
  written: number
  skipped: string[]
  backup_path: string | null
}
```

- [ ] **Step 4: Add setupApi methods**

Inside `setupApi`:

```ts
configList(signal?: AbortSignal): Promise<ConfigInventoryResponse> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve({
      env_path: '~/.labby/.env',
      draft_path: '~/.labby/.env.draft',
      toml_path: '~/.labby/config.toml',
      draft_stale: false,
      entries: [
        {
          id: 'env:LAB_MCP_HTTP_HOST',
          source: 'env',
          key: 'LAB_MCP_HTTP_HOST',
          path: ['LAB_MCP_HTTP_HOST'],
          group: 'Core',
          label: 'LAB_MCP_HTTP_HOST',
          description: 'Host the lab HTTP server binds to.',
          value: '127.0.0.1',
          value_kind: 'string',
          secret: false,
          known: true,
          required: false,
          editable: true,
          deletable: false,
          file_path: '~/.labby/.env',
        },
      ],
    })
  }
  return setupAction<ConfigInventoryResponse>('config.list', {}, signal)
},

configSet(
  input: { source: ConfigSource; key: string; value: unknown; force?: boolean },
  signal?: AbortSignal,
): Promise<ConfigMutationOutcome> {
  return setupAction<ConfigMutationOutcome>(
    'config.set',
    { source: input.source, key: input.key, value: input.value, force: input.force ?? true, confirm: true },
    signal,
  )
},

configDelete(
  input: { source: ConfigSource; key: string; force?: boolean },
  signal?: AbortSignal,
): Promise<ConfigMutationOutcome> {
  return setupAction<ConfigMutationOutcome>(
    'config.delete',
    { source: input.source, key: input.key, force: input.force ?? true, confirm: true },
    signal,
  )
},
```

- [ ] **Step 5: Run tests**

Run: `pnpm --dir apps/gateway-admin test:unit -- lib/api/setup-client.test.ts`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib/api/setup-client.ts apps/gateway-admin/lib/api/setup-client.test.ts
git commit -m "feat(gateway-admin): add setup config client"
```

---

## Task 6: Frontend Config Inventory Grouping Helpers

**Files:**
- Create: `apps/gateway-admin/lib/setup/configInventory.ts`
- Test: `apps/gateway-admin/lib/setup/configInventory.test.ts`

- [ ] **Step 1: Add failing grouping tests**

Create `configInventory.test.ts`:

```ts
import assert from 'node:assert/strict'
import test from 'node:test'

import type { ConfigEntryView } from '@/lib/api/setup-client'
import { groupConfigEntries, serializeConfigInputValue } from './configInventory.ts'

function entry(partial: Partial<ConfigEntryView>): ConfigEntryView {
  return {
    id: partial.id ?? `${partial.source ?? 'env'}:${partial.key ?? 'KEY'}`,
    source: partial.source ?? 'env',
    key: partial.key ?? 'KEY',
    path: partial.path ?? [partial.key ?? 'KEY'],
    group: partial.group ?? 'Custom',
    label: partial.label ?? partial.key ?? 'KEY',
    description: partial.description ?? '',
    value: partial.value ?? '',
    value_kind: partial.value_kind ?? 'string',
    secret: partial.secret ?? false,
    known: partial.known ?? false,
    required: partial.required ?? false,
    editable: partial.editable ?? true,
    deletable: partial.deletable ?? true,
    file_path: partial.file_path ?? '~/.labby/.env',
  }
}

test('groupConfigEntries preserves defined group order and custom entries', () => {
  const groups = groupConfigEntries([
    entry({ key: 'CUSTOM_FLAG', group: 'Custom' }),
    entry({ key: 'RADARR_URL', group: 'Services' }),
    entry({ key: 'LAB_LOG', group: 'Logging' }),
  ])

  assert.deepEqual(groups.map((group) => group.name), ['Services', 'Logging', 'Custom'])
})

test('serializeConfigInputValue converts bool and number kinds', () => {
  assert.equal(serializeConfigInputValue('true', 'bool'), true)
  assert.equal(serializeConfigInputValue('42', 'number'), 42)
  assert.equal(serializeConfigInputValue('abc', 'string'), 'abc')
})
```

- [ ] **Step 2: Run tests to verify failure**

Run: `pnpm --dir apps/gateway-admin test:unit -- lib/setup/configInventory.test.ts`

Expected: FAIL because the helper file does not exist.

- [ ] **Step 3: Implement helpers**

Create `configInventory.ts`:

```ts
import type { ConfigEntryView, ConfigValueKind } from '@/lib/api/setup-client'

export interface ConfigEntryGroup {
  name: string
  entries: ConfigEntryView[]
}

const GROUP_ORDER = ['Core', 'Services', 'Surfaces', 'Features', 'Auth', 'Logging', 'Config TOML', 'Custom']

export function groupConfigEntries(entries: ConfigEntryView[]): ConfigEntryGroup[] {
  const byGroup = new Map<string, ConfigEntryView[]>()
  for (const entry of entries) {
    const group = entry.group || 'Custom'
    byGroup.set(group, [...(byGroup.get(group) ?? []), entry])
  }
  return [...byGroup.entries()]
    .sort(([a], [b]) => groupRank(a) - groupRank(b) || a.localeCompare(b))
    .map(([name, groupEntries]) => ({
      name,
      entries: groupEntries.sort((a, b) => a.key.localeCompare(b.key)),
    }))
}

function groupRank(group: string): number {
  const index = GROUP_ORDER.indexOf(group)
  return index === -1 ? GROUP_ORDER.length : index
}

export function configValueToInputValue(value: unknown): string {
  if (value == null) return ''
  if (typeof value === 'string') return value
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return JSON.stringify(value, null, 2)
}

export function serializeConfigInputValue(value: string, kind: ConfigValueKind): unknown {
  if (kind === 'bool') return value === 'true'
  if (kind === 'number') {
    const parsed = Number(value)
    if (Number.isNaN(parsed)) throw new Error('Value must be a number')
    return parsed
  }
  if (kind === 'array' || kind === 'object') return JSON.parse(value)
  return value
}
```

- [ ] **Step 4: Run tests**

Run: `pnpm --dir apps/gateway-admin test:unit -- lib/setup/configInventory.test.ts`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/lib/setup/configInventory.ts apps/gateway-admin/lib/setup/configInventory.test.ts
git commit -m "feat(gateway-admin): group config inventory entries"
```

---

## Task 7: Config Value Input Component

**Files:**
- Create: `apps/gateway-admin/components/settings/ConfigValueInput.tsx`
- Test: `apps/gateway-admin/components/settings/ConfigValueInput.test.tsx`

- [ ] **Step 1: Add failing component tests**

Create `ConfigValueInput.test.tsx`:

```tsx
import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render, fireEvent } from '@testing-library/react'

import { ConfigValueInput } from './ConfigValueInput'

test('ConfigValueInput renders secret values as password inputs', () => {
  const view = render(
    <ConfigValueInput value="" valueKind="secret" secret onChange={() => {}} disabled={false} />,
  )

  const input = view.getByLabelText('Secret value') as HTMLInputElement
  assert.equal(input.type, 'password')
})

test('ConfigValueInput renders bool values as a select', () => {
  let next = ''
  const view = render(
    <ConfigValueInput value="false" valueKind="bool" secret={false} onChange={(value) => { next = value }} disabled={false} />,
  )

  fireEvent.change(view.getByLabelText('Boolean value'), { target: { value: 'true' } })
  assert.equal(next, 'true')
})
```

- [ ] **Step 2: Run tests to verify failure**

Run: `pnpm --dir apps/gateway-admin test:unit -- components/settings/ConfigValueInput.test.tsx`

Expected: FAIL because component does not exist.

- [ ] **Step 3: Implement component**

Create `ConfigValueInput.tsx`:

```tsx
'use client'

import { Eye, EyeOff } from 'lucide-react'
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import type { ConfigValueKind } from '@/lib/api/setup-client'

export function ConfigValueInput({
  value,
  valueKind,
  secret,
  disabled,
  onChange,
}: {
  value: string
  valueKind: ConfigValueKind
  secret: boolean
  disabled: boolean
  onChange: (value: string) => void
}): React.ReactElement {
  const [shown, setShown] = useState(false)

  if (valueKind === 'bool') {
    return (
      <select
        aria-label="Boolean value"
        value={value === 'true' ? 'true' : 'false'}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
        className="h-9 rounded-md border border-aurora-border-strong bg-aurora-control-surface px-2 text-sm text-aurora-text-primary"
      >
        <option value="true">true</option>
        <option value="false">false</option>
      </select>
    )
  }

  if (valueKind === 'array' || valueKind === 'object') {
    return (
      <Textarea
        aria-label="Structured value"
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
        className="min-h-24 font-mono text-xs"
      />
    )
  }

  const type = secret && !shown ? 'password' : valueKind === 'number' ? 'number' : 'text'
  return (
    <div className="relative">
      <Input
        aria-label={secret ? 'Secret value' : 'Config value'}
        type={type}
        value={value}
        disabled={disabled}
        autoComplete={secret ? 'new-password' : 'off'}
        onChange={(event) => onChange(event.target.value)}
        className={secret ? 'pr-10' : undefined}
      />
      {secret ? (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label={shown ? 'Hide secret' : 'Show secret'}
          className="absolute right-1 top-1/2 h-7 w-7 -translate-y-1/2"
          onClick={() => setShown((value) => !value)}
        >
          {shown ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
        </Button>
      ) : null}
    </div>
  )
}
```

- [ ] **Step 4: Run tests**

Run: `pnpm --dir apps/gateway-admin test:unit -- components/settings/ConfigValueInput.test.tsx`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/settings/ConfigValueInput.tsx apps/gateway-admin/components/settings/ConfigValueInput.test.tsx
git commit -m "feat(settings): add config value input"
```

---

## Task 8: Config Key/Value Table Component

**Files:**
- Create: `apps/gateway-admin/components/settings/ConfigKeyValueTable.tsx`
- Test: `apps/gateway-admin/components/settings/ConfigKeyValueTable.test.tsx`

- [ ] **Step 1: Add failing table test**

Create `ConfigKeyValueTable.test.tsx`:

```tsx
import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { fireEvent, render, waitFor } from '@testing-library/react'

import type { ConfigEntryView } from '@/lib/api/setup-client'
import { ConfigKeyValueTable } from './ConfigKeyValueTable'

const entry: ConfigEntryView = {
  id: 'env:LAB_LOG',
  source: 'env',
  key: 'LAB_LOG',
  path: ['LAB_LOG'],
  group: 'Logging',
  label: 'LAB_LOG',
  description: 'tracing filter',
  value: 'lab=info',
  value_kind: 'string',
  secret: false,
  known: true,
  required: false,
  editable: true,
  deletable: false,
  file_path: '~/.labby/.env',
}

test('ConfigKeyValueTable saves edited values', async () => {
  let saved: { entry: ConfigEntryView; value: string } | undefined
  const view = render(
    <ConfigKeyValueTable
      groups={[{ name: 'Logging', entries: [entry] }]}
      onSave={async (entry, value) => { saved = { entry, value } }}
      onDelete={async () => {}}
    />,
  )

  fireEvent.change(view.getByDisplayValue('lab=info'), { target: { value: 'lab=debug' } })
  fireEvent.click(view.getByRole('button', { name: 'Save LAB_LOG' }))

  await waitFor(() => assert.equal(saved?.value, 'lab=debug'))
})
```

- [ ] **Step 2: Run test to verify failure**

Run: `pnpm --dir apps/gateway-admin test:unit -- components/settings/ConfigKeyValueTable.test.tsx`

Expected: FAIL because component does not exist.

- [ ] **Step 3: Implement table**

Create `ConfigKeyValueTable.tsx`:

```tsx
'use client'

import { Save, Trash2 } from 'lucide-react'
import { useState } from 'react'

import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import type { ConfigEntryView } from '@/lib/api/setup-client'
import { configValueToInputValue } from '@/lib/setup/configInventory'
import type { ConfigEntryGroup } from '@/lib/setup/configInventory'
import { ConfigValueInput } from './ConfigValueInput'

export function ConfigKeyValueTable({
  groups,
  onSave,
  onDelete,
}: {
  groups: ConfigEntryGroup[]
  onSave: (entry: ConfigEntryView, value: string) => Promise<void>
  onDelete: (entry: ConfigEntryView) => Promise<void>
}): React.ReactElement {
  const [drafts, setDrafts] = useState<Record<string, string>>(() =>
    Object.fromEntries(groups.flatMap((group) => group.entries.map((entry) => [entry.id, configValueToInputValue(entry.value)]))),
  )
  const [busy, setBusy] = useState<string | null>(null)
  const [errors, setErrors] = useState<Record<string, string>>({})

  async function save(entry: ConfigEntryView): Promise<void> {
    setBusy(entry.id)
    setErrors((prev) => ({ ...prev, [entry.id]: '' }))
    try {
      await onSave(entry, drafts[entry.id] ?? '')
    } catch (error) {
      setErrors((prev) => ({
        ...prev,
        [entry.id]: error instanceof Error ? error.message : 'Save failed',
      }))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="space-y-5">
      {groups.map((group) => (
        <section key={group.name} className="rounded-md border border-aurora-border-strong bg-aurora-panel-medium">
          <div className="border-b border-aurora-border-strong px-4 py-3">
            <h2 className="text-sm font-semibold text-aurora-text-primary">{group.name}</h2>
          </div>
          <div className="divide-y divide-aurora-border-default">
            {group.entries.map((entry) => {
              const value = drafts[entry.id] ?? configValueToInputValue(entry.value)
              const dirty = value !== configValueToInputValue(entry.value)
              return (
                <div key={entry.id} className="grid gap-3 px-4 py-3 lg:grid-cols-[minmax(13rem,18rem)_minmax(0,1fr)_auto]">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate font-mono text-xs text-aurora-text-primary">{entry.key}</p>
                      <Badge variant="outline">{entry.source}</Badge>
                      {entry.known ? null : <Badge variant="secondary">custom</Badge>}
                      {entry.secret ? <Badge variant="secondary">secret</Badge> : null}
                    </div>
                    {entry.description ? <p className="mt-1 text-xs text-aurora-text-muted">{entry.description}</p> : null}
                    <p className="mt-1 truncate text-[11px] text-aurora-text-muted">{entry.file_path}</p>
                  </div>
                  <div className="min-w-0">
                    <ConfigValueInput
                      value={value}
                      valueKind={entry.value_kind}
                      secret={entry.secret}
                      disabled={!entry.editable || busy === entry.id}
                      onChange={(next) => setDrafts((prev) => ({ ...prev, [entry.id]: next }))}
                    />
                    {errors[entry.id] ? <p className="mt-1 text-xs text-aurora-error">{errors[entry.id]}</p> : null}
                  </div>
                  <div className="flex items-center gap-2">
                    <Button
                      type="button"
                      size="sm"
                      disabled={!dirty || !entry.editable || busy === entry.id}
                      aria-label={`Save ${entry.key}`}
                      onClick={() => void save(entry)}
                    >
                      <Save className="mr-2 h-4 w-4" />
                      Save
                    </Button>
                    {entry.deletable ? (
                      <Button
                        type="button"
                        size="icon"
                        variant="outline"
                        disabled={busy === entry.id}
                        aria-label={`Delete ${entry.key}`}
                        onClick={() => void onDelete(entry)}
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    ) : null}
                  </div>
                </div>
              )
            })}
          </div>
        </section>
      ))}
    </div>
  )
}
```

- [ ] **Step 4: Run tests**

Run: `pnpm --dir apps/gateway-admin test:unit -- components/settings/ConfigKeyValueTable.test.tsx`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/settings/ConfigKeyValueTable.tsx apps/gateway-admin/components/settings/ConfigKeyValueTable.test.tsx
git commit -m "feat(settings): add config key value table"
```

---

## Task 9: Replace Advanced Stub With All Config Page

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/settings/advanced/page.tsx`
- Modify: `apps/gateway-admin/components/settings/SettingsRail.tsx`

- [ ] **Step 1: Rename rail label**

In `SettingsRail.tsx`, change the `advanced` entry label from `Advanced` to `All Config` and remove the `stub: true` flag:

```ts
{ href: '/settings/advanced/', label: 'All Config', icon: Shield },
```

- [ ] **Step 2: Replace page implementation**

Replace `advanced/page.tsx` with:

```tsx
'use client'

import { useEffect, useMemo, useState } from 'react'
import { Loader2, RefreshCw } from 'lucide-react'

import { ConfigKeyValueTable } from '@/components/settings/ConfigKeyValueTable'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { setupApi, type ConfigEntryView } from '@/lib/api/setup-client'
import { groupConfigEntries, serializeConfigInputValue } from '@/lib/setup/configInventory'

export default function AllConfigPage(): React.ReactElement {
  const [entries, setEntries] = useState<ConfigEntryView[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  async function load(signal?: AbortSignal): Promise<void> {
    setLoading(true)
    setError(undefined)
    try {
      const inventory = await setupApi.configList(signal)
      setEntries(inventory.entries)
    } catch (err) {
      if (err instanceof Error && err.name === 'AbortError') return
      setError(err instanceof Error ? err.message : 'Failed to load config')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [])

  const groups = useMemo(() => groupConfigEntries(entries), [entries])

  async function save(entry: ConfigEntryView, rawValue: string): Promise<void> {
    const value = serializeConfigInputValue(rawValue, entry.value_kind)
    await setupApi.configSet({ source: entry.source, key: entry.key, value, force: true })
    await load()
  }

  async function remove(entry: ConfigEntryView): Promise<void> {
    await setupApi.configDelete({ source: entry.source, key: entry.key, force: true })
    await load()
  }

  return (
    <>
      <h1 className="sr-only">All config settings</h1>
      <Card>
        <CardHeader className="flex flex-row items-start justify-between gap-4 space-y-0">
          <div>
            <CardTitle>All Config</CardTitle>
            <CardDescription>
              Structured key/value settings from <code>~/.labby/.env</code> and <code>~/.labby/config.toml</code>.
            </CardDescription>
          </div>
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Refresh
          </Button>
        </CardHeader>
        <CardContent>
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-aurora-text-muted">
              <Loader2 className="h-4 w-4 animate-spin" /> loading config
            </div>
          ) : null}
          {error ? <p className="text-sm text-aurora-error">{error}</p> : null}
          {!loading && !error ? (
            <ConfigKeyValueTable groups={groups} onSave={save} onDelete={remove} />
          ) : null}
        </CardContent>
      </Card>
    </>
  )
}
```

- [ ] **Step 3: Run frontend tests**

Run: `pnpm --dir apps/gateway-admin test:unit -- lib/api/setup-client.test.ts lib/setup/configInventory.test.ts components/settings/ConfigValueInput.test.tsx components/settings/ConfigKeyValueTable.test.tsx`

Expected: PASS.

- [ ] **Step 4: Run lint**

Run: `pnpm --dir apps/gateway-admin lint`

Expected: PASS for files touched by this plan. If unrelated existing lint failures remain, record exact file/line failures in the final handoff.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/app/'(admin)'/settings/advanced/page.tsx apps/gateway-admin/components/settings/SettingsRail.tsx
git commit -m "feat(settings): replace advanced stub with all config"
```

---

## Task 10: Verification And Tracker Update

**Files:**
- Modify or create Bead after implementation starts.

- [ ] **Step 1: Run backend setup tests**

Run: `cargo test --manifest-path crates/lab/Cargo.toml setup:: --all-features`

Expected: PASS.

- [ ] **Step 2: Run frontend targeted tests**

Run:

```bash
pnpm --dir apps/gateway-admin test:unit -- \
  lib/api/setup-client.test.ts \
  lib/setup/configInventory.test.ts \
  components/settings/ConfigValueInput.test.tsx \
  components/settings/ConfigKeyValueTable.test.tsx
```

Expected: PASS.

- [ ] **Step 3: Run build checks**

Run: `pnpm --dir apps/gateway-admin build`

Expected: PASS and `/settings/advanced/` is present in the static export.

- [ ] **Step 4: Run repo-level Rust check if backend changed broadly**

Run: `cargo nextest run --workspace --all-features`

Expected: PASS. If unrelated failures exist, capture exact failing test names and prove the targeted setup tests passed.

- [ ] **Step 5: Create or update Bead**

If no Bead exists yet, create:

```bash
bd create "Settings: structured all config key/value editor" \
  --type feature \
  --priority 1 \
  --label settings \
  --label setup \
  --label webui \
  --label config \
  --description "Extend the existing Settings rail to edit all ~/.labby/.env keys and structured ~/.labby/config.toml values through setup dispatch. No raw editor. Preserve current setup.draft/env_merge patterns, secret masking, stale detection, and inline save errors." \
  --acceptance "All Config page lists known and custom .env keys plus structured config.toml leaf values; edits go through /v1/setup config actions; .env writes use env_merge; config.toml writes use structured TOML parsing; secrets are masked; stale/save failures are visible; targeted backend/frontend tests pass."
```

- [ ] **Step 6: Final implementation handoff**

Final response must include:

- Plan file path.
- Bead id if created.
- Exact tests run and pass/fail status.
- Known limitations, especially env delete if deferred until `env_merge` delete semantics are added.

---

## Self-Review

**Spec coverage:** This plan covers the corrected goal: structured editing of all config keys/values in Web UI, no raw editor, following current setup dispatch and Settings patterns. It includes `.env`, custom/unknown keys, `config.toml`, secret masking, stale-safe writes, frontend grouping, and verification.

**Placeholder scan:** No implementation step depends on an undefined "do later" placeholder. The only intentionally limited behavior is strict env deletion rejection until `env_merge` has delete semantics, and that limitation is explicitly tested/reported rather than hidden.

**Type consistency:** Backend source names are `env` and `toml`; frontend `ConfigSource` matches. Backend value kinds use snake_case enum serialization; frontend names match `string`, `secret`, `bool`, `number`, `array`, and `object`. The frontend sends `source`, `key`, `value`, `force`, and `confirm`, matching dispatch parsing.
