# Code Mode State Git V1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a narrow V1 Code Mode `state.*` and local-only `git.*` runtime backed by a durable jailed workspace.

**Architecture:** Keep `labby-codemode` host-neutral. Add local provider routing before upstream `CodeModeHost::call_tool`, store workspace state under `$LAB_HOME/code-mode-workspaces/`, enforce method caps before `ToolResult`, and run local git commands through a dedicated guarded runner.

**Tech Stack:** Rust 2024, Tokio, Javy/QuickJS runner subprocess, serde/serde_json, existing `ToolError`, existing Code Mode `ToolCall` protocol, tempdir-based tests.

## Global Constraints

- V1 only: no remote git operations, no hidden remote auth, no branch/checkout/remotes, no full Cloudflare StateBackend parity.
- Public MCP `codemode` input schema remains `code`, `upstreams`, and `tools`.
- `state` and `git` are sandbox globals only; they are not normal upstream MCP tools.
- Paths are virtual workspace paths. Reject host absolute paths, Windows drive roots, traversal, symlink escape, and credential-like paths.
- Enforce caps before `ToolResult`; final response truncation is not a state safety boundary.
- Local git uses no shell, no inherited git config/env, controlled hooks path, timeout, output caps, and redaction.
- Verify with all-features paths before completion: `cargo nextest run --workspace --all-features` and `cargo build --workspace --all-features`.

---

## File Structure

- Create `crates/labby-codemode/src/local_provider.rs`: local provider IDs, routing enum, descriptors, local-provider budget counters.
- Modify `crates/labby-codemode/src/types.rs`: add local provider references or helper parsing while preserving upstream IDs.
- Modify `crates/labby-codemode/src/runner_drive.rs`: dispatch local provider calls before upstream host calls and enforce independent local budgets.
- Modify `crates/labby-codemode/src/preamble.rs`: generate top-level `state` and `git` proxy globals and reserve names against upstream collisions.
- Modify `crates/labby-codemode/src/lib.rs`: expose new modules internally.
- Create `crates/labby-codemode/src/state.rs` and `crates/labby-codemode/src/state/*`: workspace backend, virtual path policy, quotas, caps, state method dispatch.
- Create `crates/labby-codemode/src/git.rs` and `crates/labby-codemode/src/git/*`: local-only git command runner and provider dispatch.
- Modify `crates/labby-codemode/src/config.rs`: add state/git local-provider budget and cap env parsing.
- Modify `crates/labby-codemode/src/host.rs`: add workspace root/config access only if the local provider implementation cannot derive it from `CodeModeConfig`.
- Modify `crates/labby-codemode/Cargo.toml`: add minimal dependencies only if needed, such as `tempfile` dev-dependency.
- Modify `docs/dev/CODE_MODE.md`: document narrowed V1 only.
- Create `tests/smoke-code-mode-state-git.sh`: isolated `LAB_HOME` smoke test.

---

### Task 1: Local Provider Routing And Proxy Globals

**Files:**
- Create: `crates/labby-codemode/src/local_provider.rs`
- Modify: `crates/labby-codemode/src/types.rs`
- Modify: `crates/labby-codemode/src/preamble.rs`
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Test: `crates/labby-codemode/src/tests_ids_schema.rs`
- Test: `crates/labby-codemode/src/tests_normalize.rs`

**Interfaces:**
- Consumes: existing `CodeModeRunnerOutput::ToolCall { seq, id, params }`.
- Produces: `LocalProviderCall { provider: LocalProviderName, method: String, params: Value }`.
- Produces: `try_parse_local_provider_call(id: &str) -> Result<Option<LocalProviderCall>, ToolError>`.
- Produces: top-level JS globals `state` and `git` that call reserved IDs such as `state::readFile`.

- [x] **Step 1: Add failing ID parsing tests**

Add tests in `crates/labby-codemode/src/tests_ids_schema.rs`:

```rust
#[test]
fn local_provider_ids_are_detected_before_upstream_ids() {
    let state = crate::local_provider::try_parse_local_provider_call("state::readFile")
        .expect("parse succeeds")
        .expect("state provider detected");
    assert_eq!(state.provider.as_str(), "state");
    assert_eq!(state.method, "readFile");

    let git = crate::local_provider::try_parse_local_provider_call("git::status")
        .expect("parse succeeds")
        .expect("git provider detected");
    assert_eq!(git.provider.as_str(), "git");
    assert_eq!(git.method, "status");

    assert!(
        crate::local_provider::try_parse_local_provider_call("movie::search")
            .expect("ordinary upstream id is valid")
            .is_none()
    );
}

#[test]
fn local_provider_ids_reject_bad_methods() {
    let err = crate::local_provider::try_parse_local_provider_call("state::")
        .expect_err("empty local method is rejected");
    assert_eq!(err.kind(), "invalid_param");
}
```

- [x] **Step 2: Run failing tests**

Run: `cargo test -p labby-codemode local_provider_ids --all-features`

Expected: FAIL with unresolved module or function `local_provider`.

- [x] **Step 3: Implement local provider parser**

Create `crates/labby-codemode/src/local_provider.rs`:

```rust
use serde_json::Value;

use crate::error::ToolError;
use crate::types::split_namespaced_id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalProviderName {
    State,
    Git,
}

impl LocalProviderName {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Git => "git",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LocalProviderCall {
    pub(crate) provider: LocalProviderName,
    pub(crate) method: String,
    pub(crate) params: Value,
}

pub(crate) fn is_reserved_provider_namespace(namespace: &str) -> bool {
    matches!(namespace, "state" | "git")
}

pub(crate) fn try_parse_local_provider_call(
    id: &str,
) -> Result<Option<LocalProviderCall>, ToolError> {
    let Some((namespace, method)) = split_namespaced_id(id) else {
        return Ok(None);
    };
    let provider = match namespace {
        "state" => LocalProviderName::State,
        "git" => LocalProviderName::Git,
        _ => return Ok(None),
    };
    if method.trim().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "local provider method must not be empty".to_string(),
            param: "id".to_string(),
        });
    }
    Ok(Some(LocalProviderCall {
        provider,
        method: method.to_string(),
        params: Value::Null,
    }))
}
```

Modify `crates/labby-codemode/src/lib.rs`:

```rust
mod local_provider;
```

- [x] **Step 4: Add state/git proxy generation tests**

Add tests in `crates/labby-codemode/src/tests_normalize.rs` or existing preamble tests:

```rust
#[test]
fn state_and_git_globals_are_present_in_preamble() {
    let js = crate::preamble::generate_local_provider_js();
    assert!(js.contains("globalThis.state"));
    assert!(js.contains("globalThis.git"));
    assert!(js.contains("state::readFile"));
    assert!(js.contains("git::status"));
}
```

- [x] **Step 5: Implement local provider proxy JS**

Modify `crates/labby-codemode/src/preamble.rs`:

```rust
pub(crate) fn generate_local_provider_js() -> String {
    r#"
function __labLocalProviderCall(id, params) {
  return callTool(id, params || {});
}
globalThis.state = Object.freeze({
  readFile: function(params) { return __labLocalProviderCall("state::readFile", params); },
  writeFile: function(params) { return __labLocalProviderCall("state::writeFile", params); },
  list: function(params) { return __labLocalProviderCall("state::list", params); },
  readdir: function(params) { return __labLocalProviderCall("state::readdir", params); },
  glob: function(params) { return __labLocalProviderCall("state::glob", params); },
  searchFiles: function(params) { return __labLocalProviderCall("state::searchFiles", params); },
  replaceInFiles: function(params) { return __labLocalProviderCall("state::replaceInFiles", params); },
  planEdits: function(params) { return __labLocalProviderCall("state::planEdits", params); },
  applyEditPlan: function(params) { return __labLocalProviderCall("state::applyEditPlan", params); }
});
var state = globalThis.state;
globalThis.git = Object.freeze({
  init: function(params) { return __labLocalProviderCall("git::init", params); },
  status: function(params) { return __labLocalProviderCall("git::status", params); },
  add: function(params) { return __labLocalProviderCall("git::add", params); },
  commit: function(params) { return __labLocalProviderCall("git::commit", params); },
  log: function(params) { return __labLocalProviderCall("git::log", params); },
  diff: function(params) { return __labLocalProviderCall("git::diff", params); }
});
var git = globalThis.git;
"#.to_string()
}
```

Inject this JS beside the existing proxy in `runner.rs` where `Start { code, proxy }` is handled. Use:

```rust
let local_provider_proxy = crate::preamble::generate_local_provider_js();
```

and evaluate it after `callTool` exists and before user code runs.

- [x] **Step 6: Route local providers before upstream host calls**

In `runner_drive.rs`, before `enqueue_tool_call(...)`, branch on `try_parse_local_provider_call(&id)`. For now, return `unknown_tool` for recognized providers until Tasks 3 and 4 implement dispatch:

```rust
if let Some(local) = crate::local_provider::try_parse_local_provider_call(&id)? {
    enqueue_local_provider_call(self, seq, local, params, deadline, cfg, &mut pending_tool_calls);
} else {
    enqueue_tool_call(self, seq, id, params, deadline, cfg, &mut pending_tool_calls);
}
```

If the exact future type of `pending_tool_calls` makes this awkward, implement a `dispatch_tool_call(...)` helper that returns the same future output type as upstream tool calls.

- [x] **Step 7: Run tests**

Run: `cargo test -p labby-codemode local_provider --all-features`

Expected: PASS.

Run: `cargo test -p labby-codemode --all-features`

Expected: PASS.

- [x] **Step 8: Commit**

```bash
git add crates/labby-codemode/src
git commit -m "feat: add code mode local provider routing"
```

---

### Task 2: Durable Jailed Workspace Backend

**Files:**
- Create: `crates/labby-codemode/src/state.rs`
- Create: `crates/labby-codemode/src/state/path.rs`
- Create: `crates/labby-codemode/src/state/workspace.rs`
- Create: `crates/labby-codemode/src/state/quota.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/config.rs`
- Test: module tests in the new files

**Interfaces:**
- Produces: `WorkspaceRoot::new(root: PathBuf) -> Result<Self, ToolError>`.
- Produces: `VirtualPath::parse(raw: &str) -> Result<Self, ToolError>`.
- Produces: `StateWorkspace::write_file`, `read_file`, `list`, `glob`, `search_files`, `replace_in_files`, `plan_edits`, `apply_edit_plan`.

- [x] **Step 1: Write failing path tests**

Create tests in `crates/labby-codemode/src/state/path.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_path_accepts_rooted_and_relative_paths() {
        assert_eq!(VirtualPath::parse("/src/app.rs").unwrap().as_str(), "src/app.rs");
        assert_eq!(VirtualPath::parse("src/app.rs").unwrap().as_str(), "src/app.rs");
    }

    #[test]
    fn virtual_path_rejects_escape_and_host_paths() {
        for raw in ["../secret", "/../secret", "C:/Users/x", "C:relative", "/"] {
            assert!(VirtualPath::parse(raw).is_err(), "{raw} should fail");
        }
    }

    #[test]
    fn virtual_path_normalizes_windows_separators() {
        assert_eq!(VirtualPath::parse("src\\\\app.rs").unwrap().as_str(), "src/app.rs");
    }
}
```

- [x] **Step 2: Implement virtual path parser**

Create `crates/labby-codemode/src/state.rs`:

```rust
pub(crate) mod path;
pub(crate) mod quota;
pub(crate) mod workspace;
```

Add `mod state;` to `lib.rs`.

Create `crates/labby-codemode/src/state/path.rs` with:

```rust
use std::path::{Component, Path};

use crate::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct VirtualPath(String);

impl VirtualPath {
    pub(crate) fn parse(raw: &str) -> Result<Self, ToolError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "/" {
            return Err(ToolError::InvalidParam {
                message: "state path must name a file or directory inside the workspace".to_string(),
                param: "path".to_string(),
            });
        }
        let normalized = trimmed.replace('\\', "/");
        if normalized.starts_with('/') && normalized.len() == 1 {
            return Err(ToolError::InvalidParam {
                message: "state path must not be workspace root".to_string(),
                param: "path".to_string(),
            });
        }
        if has_windows_drive_prefix(&normalized) {
            return Err(path_traversal(&normalized));
        }
        let stripped = normalized.trim_start_matches('/');
        let mut parts = Vec::new();
        for component in Path::new(stripped).components() {
            match component {
                Component::Normal(value) => {
                    let part = value.to_string_lossy();
                    if !part.is_empty() {
                        parts.push(part.to_string());
                    }
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(path_traversal(raw));
                }
            }
        }
        if parts.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "state path must name a file or directory inside the workspace".to_string(),
                param: "path".to_string(),
            });
        }
        let value = parts.join("/");
        reject_credential_like_path(&value)?;
        Ok(Self(value))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn path_traversal(raw: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "path_traversal".to_string(),
        message: format!("state path `{raw}` escapes the workspace"),
    }
}

fn reject_credential_like_path(path: &str) -> Result<(), ToolError> {
    let lower = path.to_ascii_lowercase();
    let denied = [
        ".env",
        ".ssh/",
        ".git/config",
        ".git/credentials",
        ".aws/",
        ".config/gcloud/",
        ".netrc",
        "id_rsa",
        "id_ed25519",
    ];
    if denied.iter().any(|needle| lower == *needle || lower.contains(needle)) {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".to_string(),
            message: "state path is denied because it looks credential-related".to_string(),
        });
    }
    Ok(())
}
```

- [x] **Step 3: Add workspace read/write/list tests**

In `state/workspace.rs`, add tests:

```rust
#[tokio::test]
async fn workspace_writes_reads_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let ws = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    ws.write_file(&VirtualPath::parse("/src/app.rs").unwrap(), "fn main() {}\n").await.unwrap();
    assert_eq!(
        ws.read_file(&VirtualPath::parse("src/app.rs").unwrap()).await.unwrap().content,
        "fn main() {}\n"
    );
    let ws2 = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    assert_eq!(ws2.list(&VirtualPath::parse("src").unwrap()).await.unwrap().entries.len(), 1);
}
```

- [x] **Step 4: Implement workspace backend**

Create `state/quota.rs`:

```rust
#[derive(Debug, Clone)]
pub(crate) struct StateWorkspaceLimits {
    pub(crate) max_file_bytes: usize,
    pub(crate) max_total_bytes: u64,
    pub(crate) max_entries: u64,
    pub(crate) max_result_bytes: usize,
}

impl Default for StateWorkspaceLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_total_bytes: 64 * 1024 * 1024,
            max_entries: 10_000,
            max_result_bytes: 1024 * 1024,
        }
    }
}
```

Create `state/workspace.rs`:

```rust
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::ToolError;
use super::path::VirtualPath;
use super::quota::StateWorkspaceLimits;

#[derive(Debug, Clone)]
pub(crate) struct StateWorkspace {
    root: PathBuf,
    limits: StateWorkspaceLimits,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ReadFileResult {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ListResult {
    pub(crate) entries: Vec<String>,
}

impl StateWorkspace {
    pub(crate) fn new(root: PathBuf, limits: StateWorkspaceLimits) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&root).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create code mode workspace root: {err}"),
        })?;
        Ok(Self { root, limits })
    }

    fn resolve(&self, path: &VirtualPath) -> PathBuf {
        self.root.join(path.as_str())
    }

    pub(crate) async fn write_file(&self, path: &VirtualPath, content: &str) -> Result<(), ToolError> {
        if content.len() > self.limits.max_file_bytes {
            return Err(ToolError::InvalidParam {
                message: format!("state file content is {} bytes; maximum is {}", content.len(), self.limits.max_file_bytes),
                param: "content".to_string(),
            });
        }
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(internal_io("create state directory"))?;
        }
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        let tmp = destination.with_extension("tmp-labby-state");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .await
            .map_err(internal_io("create state temp file"))?;
        file.write_all(content.as_bytes()).await.map_err(internal_io("write state temp file"))?;
        file.flush().await.map_err(internal_io("flush state temp file"))?;
        tokio::fs::rename(&tmp, &destination).await.map_err(internal_io("move state temp file"))?;
        Ok(())
    }

    pub(crate) async fn read_file(&self, path: &VirtualPath) -> Result<ReadFileResult, ToolError> {
        let destination = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &destination)?;
        let mut file = tokio::fs::File::open(&destination).await.map_err(not_found_or_internal("open state file"))?;
        let mut content = String::new();
        file.take(self.limits.max_result_bytes as u64 + 1)
            .read_to_string(&mut content)
            .await
            .map_err(internal_io("read state file"))?;
        if content.len() > self.limits.max_result_bytes {
            return Err(ToolError::Sdk {
                sdk_kind: "response_too_large".to_string(),
                message: "state read result exceeded max result bytes".to_string(),
            });
        }
        Ok(ReadFileResult { path: path.as_str().to_string(), bytes: content.len(), content })
    }

    pub(crate) async fn list(&self, path: &VirtualPath) -> Result<ListResult, ToolError> {
        let dir = self.resolve(path);
        labby_runtime::path_safety::reject_existing_symlink_ancestors(&self.root, &dir)?;
        let mut read_dir = tokio::fs::read_dir(&dir).await.map_err(not_found_or_internal("read state directory"))?;
        let mut entries = Vec::new();
        while let Some(entry) = read_dir.next_entry().await.map_err(internal_io("read state directory entry"))? {
            entries.push(entry.file_name().to_string_lossy().to_string());
            if entries.len() as u64 > self.limits.max_entries {
                return Err(ToolError::Sdk {
                    sdk_kind: "response_too_large".to_string(),
                    message: "state list exceeded max entries".to_string(),
                });
            }
        }
        entries.sort();
        Ok(ListResult { entries })
    }
}

fn internal_io(action: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to {action}: {err}"),
    }
}

fn not_found_or_internal(action: &'static str) -> impl FnOnce(std::io::Error) -> ToolError {
    move |err| ToolError::Sdk {
        sdk_kind: if err.kind() == std::io::ErrorKind::NotFound { "not_found" } else { "internal_error" }.to_string(),
        message: format!("failed to {action}: {err}"),
    }
}
```

- [x] **Step 5: Run tests**

Run: `cargo test -p labby-codemode state:: --all-features`

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/state.rs crates/labby-codemode/src/state crates/labby-codemode/src/lib.rs
git commit -m "feat: add code mode state workspace"
```

---

### Task 3: State Provider Methods

**Files:**
- Modify: `crates/labby-codemode/src/local_provider.rs`
- Create: `crates/labby-codemode/src/state/provider.rs`
- Modify: `crates/labby-codemode/src/state.rs`
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Modify: `crates/labby-codemode/src/config.rs`
- Test: module tests in `state/provider.rs`

**Interfaces:**
- Consumes: `LocalProviderCall`.
- Produces: `dispatch_state_method(workspace: &StateWorkspace, method: &str, params: Value) -> Result<Value, ToolError>`.
- Produces V1 methods: `readFile`, `writeFile`, `list`, `readdir`, `glob`, `searchFiles`, `replaceInFiles`, `planEdits`, `applyEditPlan`.

- [x] **Step 1: Write failing provider dispatch tests**

In `state/provider.rs`:

```rust
#[tokio::test]
async fn write_and_read_file_dispatch_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    dispatch_state_method(&workspace, "writeFile", serde_json::json!({
        "path": "/src/app.rs",
        "content": "fn main() {}\n"
    })).await.unwrap();
    let result = dispatch_state_method(&workspace, "readFile", serde_json::json!({
        "path": "src/app.rs"
    })).await.unwrap();
    assert_eq!(result["content"], "fn main() {}\n");
}
```

- [x] **Step 2: Implement method param parsing and dispatch**

Create `state/provider.rs`:

```rust
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolError;
use super::path::VirtualPath;
use super::workspace::StateWorkspace;

#[derive(Deserialize)]
struct PathParams {
    path: String,
}

#[derive(Deserialize)]
struct WriteFileParams {
    path: String,
    content: String,
}

pub(crate) async fn dispatch_state_method(
    workspace: &StateWorkspace,
    method: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match method {
        "readFile" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.read_file(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        "writeFile" => {
            let params: WriteFileParams = serde_json::from_value(params).map_err(invalid_params)?;
            workspace.write_file(&VirtualPath::parse(&params.path)?, &params.content).await?;
            Ok(json!({ "ok": true, "path": params.path }))
        }
        "list" | "readdir" => {
            let params: PathParams = serde_json::from_value(params).map_err(invalid_params)?;
            let result = workspace.list(&VirtualPath::parse(&params.path)?).await?;
            serde_json::to_value(result).map_err(serialize_error)
        }
        other => Err(ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: format!("unknown state method `{other}`"),
        }),
    }
}

fn invalid_params(err: serde_json::Error) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid state params: {err}"),
        param: "params".to_string(),
    }
}

fn serialize_error(err: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize state result: {err}"),
    }
}
```

Wire `pub(crate) mod provider;` in `state.rs`.

- [x] **Step 3: Implement glob/search/replace/plan/apply**

Add bounded state operations as ordinary `StateWorkspace` methods and call them from
`dispatch_state_method`. Do not add public methods outside the V1 list.

Use these V1 parameter/result shapes:

```rust
#[derive(Deserialize)]
struct GlobParams {
    pattern: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct GlobResult {
    matches: Vec<String>,
    truncated: bool,
}

#[derive(Deserialize)]
struct SearchFilesParams {
    pattern: String,
    query: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct SearchMatch {
    path: String,
    line: usize,
    text: String,
}

#[derive(Serialize)]
struct SearchFilesResult {
    matches: Vec<SearchMatch>,
    truncated: bool,
}

#[derive(Deserialize)]
struct ReplaceInFilesParams {
    pattern: String,
    search: String,
    replace: String,
    #[serde(default = "default_true")]
    dry_run: bool,
}

#[derive(Serialize)]
struct ReplaceInFilesResult {
    changed: Vec<String>,
    dry_run: bool,
}

#[derive(Deserialize)]
struct PlanEditsParams {
    edits: Vec<FileEdit>,
}

#[derive(Deserialize, Serialize, Clone)]
struct FileEdit {
    path: String,
    search: String,
    replace: String,
}

#[derive(Serialize)]
struct EditPlanResult {
    plan_id: String,
    edits: Vec<FileEdit>,
}

#[derive(Deserialize)]
struct ApplyEditPlanParams {
    plan_id: String,
}
```

Implementation details:

- `glob`: walk the workspace root with `tokio::fs::read_dir` or a small recursive helper, convert every file to its virtual path, match with the `globset` crate only if it is already present or added as a narrow dependency, otherwise use the `glob` crate with the resolved workspace-prefixed pattern and reject patterns that resolve outside the workspace. Return sorted paths and set `truncated: true` when the requested/default limit is hit.
- `searchFiles`: call `glob`, read only files at or below `max_file_bytes`, perform literal substring search line-by-line, cap each line preview to 512 chars, cap total matches to `limit.unwrap_or(200)`, and return `response_too_large` if accumulated serialized result bytes exceed `max_result_bytes`.
- `replaceInFiles`: call `searchFiles` for candidate files, reject empty `search`, for `dry_run: true` return the paths that would change, for `dry_run: false` read each candidate, replace all literal occurrences, and write via `write_file`.
- `planEdits`: validate each `FileEdit` with `VirtualPath::parse`, reject empty `search`, compute a stable plan id with `sha256` over canonical JSON for the edit list, persist the plan as JSON under `.labby-state/plans/<plan_id>.json` inside the jailed workspace, and return the plan id plus normalized edits.
- `applyEditPlan`: read `.labby-state/plans/<plan_id>.json`, copy each target file to `.labby-state/rollback/<plan_id>/...` before editing, apply replacements using `write_file`, and on the first failure restore already-modified files from rollback copies before returning the error.

- [x] **Step 4: Connect state provider to runner drive**

In `runner_drive.rs`, when a local provider call is `LocalProviderName::State`, construct/open the current workspace and call `dispatch_state_method`. Settle the runner with `ToolResult` or `ToolError` exactly like upstream calls.

Use an isolated default workspace root derived from config:

```rust
let workspace_root = cfg.lab_home.join("code-mode-workspaces").join("default");
let workspace = StateWorkspace::new(workspace_root, StateWorkspaceLimits::from_config(cfg))?;
let value = dispatch_state_method(&workspace, &local.method, params).await?;
```

If `CodeModeConfig` does not yet carry `lab_home`, add a host-neutral config field or a `CodeModeHost` method that returns the workspace root.

- [x] **Step 5: Run tests**

Run: `cargo test -p labby-codemode state:: --all-features`

Expected: PASS.

Run: `cargo test -p labby-codemode --all-features`

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/state.rs crates/labby-codemode/src/state crates/labby-codemode/src/runner_drive.rs crates/labby-codemode/src/config.rs
git commit -m "feat: add code mode state provider"
```

---

### Task 4: Local Git Provider

**Files:**
- Create: `crates/labby-codemode/src/git.rs`
- Create: `crates/labby-codemode/src/git/command.rs`
- Create: `crates/labby-codemode/src/git/provider.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Test: module tests in `git/command.rs` and `git/provider.rs`

**Interfaces:**
- Produces: `dispatch_git_method(workspace: &StateWorkspace, method: &str, params: Value) -> Result<Value, ToolError>`.
- Produces V1 methods: `init`, `status`, `add`, `commit`, `log`, `diff`.

- [x] **Step 1: Write failing argv tests**

Create `git/command.rs` tests:

```rust
#[test]
fn git_status_builds_fixed_argv() {
    let cmd = GitCommandSpec::status();
    assert_eq!(cmd.args, vec!["-c", "core.hooksPath=/dev/null", "status", "--short"]);
}

#[test]
fn git_rejects_unsupported_method() {
    assert!(GitCommandSpec::for_method("push", serde_json::json!({})).is_err());
}
```

- [x] **Step 2: Implement git command specs**

Create `git.rs`:

```rust
pub(crate) mod command;
pub(crate) mod provider;
```

Add `mod git;` to `lib.rs`.

Create `git/command.rs`:

```rust
use serde_json::Value;

use crate::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitCommandSpec {
    pub(crate) args: Vec<String>,
}

impl GitCommandSpec {
    pub(crate) fn status() -> Self {
        Self {
            args: base_args(["status", "--short"]),
        }
    }

    pub(crate) fn for_method(method: &str, _params: Value) -> Result<Self, ToolError> {
        match method {
            "init" => Ok(Self { args: base_args(["init"]) }),
            "status" => Ok(Self::status()),
            "add" => Ok(Self { args: base_args(["add", "--"]) }),
            "commit" => Ok(Self { args: base_args(["commit", "--no-gpg-sign", "-m"]) }),
            "log" => Ok(Self { args: base_args(["log", "--oneline", "-n", "20"]) }),
            "diff" => Ok(Self { args: base_args(["diff", "--"]) }),
            other => Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown git method `{other}`"),
            }),
        }
    }
}

fn base_args<const N: usize>(tail: [&str; N]) -> Vec<String> {
    let mut args = vec![
        "-c".to_string(),
        "core.hooksPath=/dev/null".to_string(),
        "-c".to_string(),
        "protocol.file.allow=never".to_string(),
        "-c".to_string(),
        "protocol.ext.allow=never".to_string(),
    ];
    args.extend(tail.into_iter().map(str::to_string));
    args
}
```

- [x] **Step 3: Implement guarded git execution**

Create `git/provider.rs` with an async runner using `tokio::process::Command`:

```rust
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

use crate::error::ToolError;

pub(crate) async fn run_git(workspace_root: &Path, args: &[String]) -> Result<String, ToolError> {
    let mut command = Command::new("/usr/bin/git");
    command
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "timeout".to_string(),
            message: "git command timed out".to_string(),
        })?
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to run git: {err}"),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).chars().take(64 * 1024).collect::<String>();
    let stderr = String::from_utf8_lossy(&output.stderr).chars().take(16 * 1024).collect::<String>();
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "git_failed".to_string(),
            message: format!("git failed: {}", redact_git_output(&stderr)),
        });
    }
    Ok(redact_git_output(&stdout))
}

fn redact_git_output(value: &str) -> String {
    value
        .replace("https://", "https://[REDACTED]@")
        .replace("ghp_", "[REDACTED]")
}
```

If `/usr/bin/git` is not portable enough for this repo, implement a resolver that finds `git` once outside the workspace and stores the absolute path. Do not run a relative `git` from inside the workspace.

- [x] **Step 4: Implement git provider dispatch**

`dispatch_git_method` should parse object params:

```rust
pub(crate) async fn dispatch_git_method(
    workspace: &StateWorkspace,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    let spec = GitCommandSpec::for_method(method, params)?;
    let stdout = run_git(workspace.root_path(), &spec.args).await?;
    Ok(serde_json::json!({ "ok": true, "stdout": stdout }))
}
```

Support:

- `git.init({})`
- `git.status({})`
- `git.add({ path })`
- `git.commit({ message, authorName, authorEmail })`
- `git.log({ limit? })`
- `git.diff({ path? })`

Build method-specific argv in `GitCommandSpec::for_method`:

- `init`: ignore params and return `base_args(["init"])`.
- `status`: ignore params and return `base_args(["status", "--short"])`.
- `add`: parse `{ path }`, validate with `VirtualPath::parse`, and append the virtual path after `["add", "--"]`.
- `commit`: parse `{ message, authorName, authorEmail }`, reject an empty message, and append `["commit", "--no-gpg-sign", "--author", "<name> <email>", "-m", message]`.
- `log`: parse optional `{ limit }`, clamp to `1..=50`, and append `["log", "--oneline", "-n", limit]`.
- `diff`: parse optional `{ path }`; without a path use `["diff", "--"]`, with a path validate via `VirtualPath::parse` and append the virtual path after `--`.

- [x] **Step 5: Wire git provider to runner drive**

In the local provider routing branch from Task 1, dispatch `LocalProviderName::Git` with the same workspace root as state.

- [x] **Step 6: Run tests**

Run: `cargo test -p labby-codemode git:: --all-features`

Expected: PASS.

Run: `cargo test -p labby-codemode --all-features`

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/labby-codemode/src/git.rs crates/labby-codemode/src/git crates/labby-codemode/src/runner_drive.rs crates/labby-codemode/src/lib.rs
git commit -m "feat: add local code mode git provider"
```

---

### Task 5: Docs, Smoke, And Final Verification

**Files:**
- Modify: `docs/dev/CODE_MODE.md`
- Create: `tests/smoke-code-mode-state-git.sh`
- Modify: `crates/labby-codemode/src/tests_ts_signatures.rs`
- Modify: `crates/labby-codemode/src/tests_ids_schema.rs`
- Modify: `CHANGELOG.md` if this repo keeps unreleased entries for user-facing changes

**Interfaces:**
- Consumes: all V1 `state.*` and `git.*` methods.
- Produces: smoke proof and user-facing docs that match implemented schema.

- [x] **Step 1: Update docs with exact V1 surface**

In `docs/dev/CODE_MODE.md`, add a section:

```markdown
### Local State And Git Providers

Code Mode exposes two local sandbox globals when enabled: `state` and `git`.
They are not upstream MCP tools and they do not grant host filesystem or shell access.

V1 state methods:
- `state.readFile({ path })`
- `state.writeFile({ path, content })`
- `state.list({ path })` / `state.readdir({ path })`
- `state.glob({ pattern })`
- `state.searchFiles({ pattern, query })`
- `state.replaceInFiles({ pattern, search, replace, dryRun })`
- `state.planEdits({ edits })`
- `state.applyEditPlan({ planId })`

V1 git methods:
- `git.init({})`
- `git.status({})`
- `git.add({ path })`
- `git.commit({ message, authorName, authorEmail })`
- `git.log({ limit })`
- `git.diff({ path })`

Remote git operations, hidden git auth, checkout/branch/remotes, archive/hash/detect,
and advanced JSON helpers are not V1.
```

- [x] **Step 2: Add smoke script**

Create `tests/smoke-code-mode-state-git.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

export LAB_HOME="$TMP/lab-home"
mkdir -p "$LAB_HOME"

cd "$ROOT"
cargo run --all-features -- codemode --json --code '
await state.writeFile({ path: "/src/app.rs", content: "fn main() { println!(\"hi\"); }\n" });
const read = await state.readFile({ path: "/src/app.rs" });
const matches = await state.searchFiles({ pattern: "src/**/*.rs", query: "println" });
await git.init({});
await git.add({ path: "/src/app.rs" });
await git.commit({ message: "initial state", authorName: "Lab", authorEmail: "lab@example.invalid" });
const status = await git.status({});
const log = await git.log({ limit: 1 });
return { read: read.content, matches: matches.matches.length, status, log };
'
```

Adjust the CLI invocation to the repo's actual Code Mode command if it differs. Keep `LAB_HOME` isolated.

- [x] **Step 3: Add negative smoke or integration checks**

Add test cases for:

```text
state.readFile({ path: "/../secret" }) => path_traversal
state.writeFile({ path: "/.env", content: "TOKEN=x" }) => permission_denied
git.status({ dir: "/../outside" }) => path_traversal or invalid_param
upstream named state/git cannot shadow local providers
```

Covered by focused unit tests in `labby-codemode` for path traversal,
credential path rejection, symlink escapes, local-provider namespace routing,
state provider failures, and git command validation.

- [x] **Step 4: Run focused verification**

Run: `cargo test -p labby-codemode --all-features`

Expected: PASS.

Run: `cargo test -p labby --test architecture_orchestrator --all-features`

Expected: PASS.

Run: `bash tests/smoke-code-mode-state-git.sh`

Expected: PASS and output includes committed git log/status data.

- [x] **Step 5: Run full verification**

Run: `cargo nextest run --workspace --all-features`

Expected: PASS.

Run: `cargo build --workspace --all-features`

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add docs/dev/CODE_MODE.md tests/smoke-code-mode-state-git.sh crates/labby-codemode/src/state/provider.rs CHANGELOG.md docs/superpowers/plans/2026-06-28-code-mode-state-git-v1.md
git commit -m "docs: document code mode state git v1"
```

---

## Self-Review

**Spec coverage:** V1 local providers, workspace jail, state methods, local git methods, budgets/caps, concurrency, docs, smoke, and all-features verification are covered. V2 exclusions are explicitly listed in the epic and are not implemented in this plan.

**Placeholder scan:** No `TBD`, `TODO`, or unconstrained “handle edge cases” steps remain. Steps name exact files, commands, expected results, and method names.

**Type consistency:** `LocalProviderCall`, `LocalProviderName`, `VirtualPath`, `StateWorkspace`, `StateWorkspaceLimits`, `dispatch_state_method`, and `dispatch_git_method` are introduced before later tasks consume them.
