# Code Mode State And Git V2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Extend Labby Code Mode's V1 local `state.*` and `git.*` providers with the remaining V2 workspace APIs, local git branch/remote controls, guarded remote git operations, docs, and smoke coverage.

**Architecture:** Keep V2 additive on top of the V1 local-provider route. All new state methods stay in the jailed `StateWorkspace`; all git methods still run host-side through structured argv and the existing process guard. Remote git operations are opt-in, no hidden credential injection, and never expose host paths or shell strings.

**Tech Stack:** Rust 2024, Tokio fs/process, serde/serde_json, sha2/hex, regex, existing Labby `ToolError`, existing QuickJS/Javy Code Mode runner.

## Global Constraints

- Base this work on branch `codex/code-mode-state-git-v1`; do not reimplement V1.
- Keep `state` and `git` as Code Mode local providers only; do not register them as normal upstream MCP tools.
- Keep all state paths as virtual workspace paths; reject host absolute paths, traversal, Windows drive roots, symlink escapes, and credential-like paths.
- Keep git execution shell-free: structured argv, controlled environment, timeout, output caps, hook/config isolation, and redaction.
- V2 remote git operations are explicit, unauthenticated, and restricted to `https://github.com/...` URLs by default. Do not inject hidden GitHub tokens, OAuth tokens, credential helpers, or host git config.
- Do not add Node/Deno/Bun/fs/fetch/import access to the QuickJS runner.
- Use modern Rust module layout; no `mod.rs`.
- The final truth is `cargo nextest run --workspace --all-features` and `cargo build --workspace --all-features`.

---

## File Structure

- Modify `crates/labby-codemode/src/state/provider.rs`: add request structs and dispatch arms for V2 state methods.
- Modify `crates/labby-codemode/src/state/workspace.rs`: implement filesystem mutation, JSON, hash, detect, and archive helpers inside the workspace jail.
- Modify `crates/labby-codemode/src/state/path.rs`: add optional directory-path parsing support if needed by `mkdir`, `rm`, `cp`, and `mv`.
- Modify `crates/labby-codemode/src/git/command.rs`: add structured argv builders for branch, checkout, remotes, and remote operations.
- Modify `crates/labby-codemode/src/git/provider.rs`: add remote URL validation and per-method guard behavior while preserving the existing process wrapper.
- Modify `crates/labby-codemode/Cargo.toml`: add minimal archive/detect dependencies only if the implementation cannot use the standard library.
- Modify `docs/dev/CODE_MODE.md` and `CHANGELOG.md`: document the exact V2 surface and boundaries.
- Create `tests/smoke-code-mode-state-git-v2.sh`: isolated `LAB_HOME` smoke for V2 state and local git behavior.

---

### Task 1: Broad Safe State Filesystem Helpers

**Files:**
- Modify: `crates/labby-codemode/src/state/provider.rs`
- Modify: `crates/labby-codemode/src/state/workspace.rs`
- Modify: `crates/labby-codemode/src/state/path.rs` only if directory paths need a separate parser

**Interfaces:**
- Consumes: `StateWorkspace`, `VirtualPath::parse`, `dispatch_state_method(workspace, method, params)`.
- Produces:
  - `StateWorkspace::exists(&VirtualPath) -> Result<ExistsResult, ToolError>`
  - `StateWorkspace::stat(&VirtualPath) -> Result<StatResult, ToolError>`
  - `StateWorkspace::mkdir(&VirtualPath) -> Result<MutationResult, ToolError>`
  - `StateWorkspace::append_file(&VirtualPath, &str) -> Result<MutationResult, ToolError>`
  - `StateWorkspace::remove(&VirtualPath, recursive: bool) -> Result<MutationResult, ToolError>`
  - `StateWorkspace::copy(&VirtualPath, &VirtualPath) -> Result<MutationResult, ToolError>`
  - `StateWorkspace::move_path(&VirtualPath, &VirtualPath) -> Result<MutationResult, ToolError>`
  - `StateWorkspace::walk_tree(&VirtualPath, limit: usize) -> Result<WalkTreeResult, ToolError>`

- [x] **Step 1: Write failing provider tests**

Add these tests to `crates/labby-codemode/src/state/provider.rs`:

```rust
#[tokio::test]
async fn v2_state_filesystem_methods_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let workspace =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();

    dispatch_state_method(&workspace, "mkdir", json!({"path": "src"})).await.unwrap();
    dispatch_state_method(&workspace, "writeFile", json!({"path": "src/app.rs", "content": "fn main() {}\n"})).await.unwrap();
    dispatch_state_method(&workspace, "appendFile", json!({"path": "src/app.rs", "content": "// tail\n"})).await.unwrap();

    let stat = dispatch_state_method(&workspace, "stat", json!({"path": "src/app.rs"})).await.unwrap();
    assert_eq!(stat["kind"], "file");
    assert!(stat["bytes"].as_u64().unwrap() > 0);

    let exists = dispatch_state_method(&workspace, "exists", json!({"path": "src/app.rs"})).await.unwrap();
    assert_eq!(exists["exists"], true);

    dispatch_state_method(&workspace, "cp", json!({"from": "src/app.rs", "to": "src/copy.rs"})).await.unwrap();
    dispatch_state_method(&workspace, "mv", json!({"from": "src/copy.rs", "to": "src/moved.rs"})).await.unwrap();
    let tree = dispatch_state_method(&workspace, "walkTree", json!({"path": "src", "limit": 10})).await.unwrap();
    assert!(tree["entries"].as_array().unwrap().iter().any(|entry| entry["path"] == "src/moved.rs"));

    dispatch_state_method(&workspace, "rm", json!({"path": "src/moved.rs"})).await.unwrap();
    let gone = dispatch_state_method(&workspace, "exists", json!({"path": "src/moved.rs"})).await.unwrap();
    assert_eq!(gone["exists"], false);
}
```

- [x] **Step 2: Run tests and confirm failure**

Run: `cargo test -p labby-codemode v2_state_filesystem_methods_round_trip --all-features`

Expected: FAIL with `unknown state method`.

- [x] **Step 3: Add result and param types**

In `state/workspace.rs`, add serializable result types:

```rust
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MutationResult {
    pub(crate) ok: bool,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ExistsResult {
    pub(crate) path: String,
    pub(crate) exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StatResult {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WalkEntry {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WalkTreeResult {
    pub(crate) entries: Vec<WalkEntry>,
    pub(crate) truncated: bool,
}
```

In `state/provider.rs`, add params:

```rust
#[derive(Deserialize)]
struct OptionalRecursivePathParams {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct FromToParams {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct WalkTreeParams {
    path: String,
    limit: Option<usize>,
}
```

- [x] **Step 4: Implement workspace methods**

Implement with existing quota and symlink checks:

```rust
pub(crate) async fn append_file(&self, path: &VirtualPath, content: &str) -> Result<MutationResult, ToolError> {
    let existing = match self.read_file(path).await {
        Ok(file) => file.content,
        Err(err) if err.kind() == "not_found" => String::new(),
        Err(err) => return Err(err),
    };
    let next = format!("{existing}{content}");
    self.write_file(path, &next).await?;
    Ok(MutationResult { ok: true, path: path.as_str().to_string() })
}
```

Use the same pattern for:

```text
mkdir: create_dir_all after symlink ancestor checks.
exists: return false only for NotFound; other metadata errors are internal_error.
stat: metadata after symlink ancestor checks; kind is "file" or "directory"; reject other file types with permission_denied.
rm: remove_file for files; remove_dir for empty dirs; remove_dir_all only when recursive is true and path is not ".labby-state".
cp: read source through read_file and write destination through write_file.
mv: cp then rm source, or rename only after source/destination symlink checks and quota checks.
walkTree: bounded recursive walk from the requested directory, excluding ".labby-state/".
```

- [x] **Step 5: Add provider dispatch arms**

Add arms in `dispatch_state_method`:

```rust
"appendFile" => { /* parse WriteFileParams, call append_file */ }
"exists" => { /* parse PathParams, call exists */ }
"stat" => { /* parse PathParams, call stat; symlinks are rejected in V2 */ }
"mkdir" => { /* parse PathParams, call mkdir */ }
"rm" => { /* parse OptionalRecursivePathParams, call remove */ }
"cp" => { /* parse FromToParams, call copy */ }
"mv" => { /* parse FromToParams, call move_path */ }
"walkTree" | "summarizeTree" => { /* parse WalkTreeParams, call walk_tree */ }
```

- [x] **Step 6: Run focused tests**

Run: `cargo test -p labby-codemode state:: --all-features`

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/labby-codemode/src/state/provider.rs crates/labby-codemode/src/state/workspace.rs crates/labby-codemode/src/state/path.rs
git commit -m "feat: add code mode state filesystem v2"
```

---

### Task 2: JSON, Hash, And Detect State Helpers

**Files:**
- Modify: `crates/labby-codemode/src/state/provider.rs`
- Modify: `crates/labby-codemode/src/state/workspace.rs`

**Interfaces:**
- Consumes: Task 1 state mutation helpers and V1 `read_file` / `write_file`.
- Produces:
  - `readJson({ path })`
  - `writeJson({ path, value, pretty })`
  - `hashFile({ path, algorithm })`
  - `detectFile({ path })`

- [x] **Step 1: Write failing tests**

Add to `state/provider.rs` tests:

```rust
#[tokio::test]
async fn v2_json_hash_and_detect_methods_round_trip() {
    let temp = tempfile::tempdir().unwrap();
    let workspace =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();

    dispatch_state_method(&workspace, "writeJson", json!({
        "path": "data/config.json",
        "value": {"enabled": true, "count": 2},
        "pretty": true
    })).await.unwrap();

    let json_value = dispatch_state_method(&workspace, "readJson", json!({"path": "data/config.json"})).await.unwrap();
    assert_eq!(json_value["value"]["enabled"], true);

    let hash = dispatch_state_method(&workspace, "hashFile", json!({"path": "data/config.json", "algorithm": "sha256"})).await.unwrap();
    assert_eq!(hash["algorithm"], "sha256");
    assert_eq!(hash["hex"].as_str().unwrap().len(), 64);

    let detected = dispatch_state_method(&workspace, "detectFile", json!({"path": "data/config.json"})).await.unwrap();
    assert_eq!(detected["extension"], "json");
    assert_eq!(detected["text"], true);
}
```

- [x] **Step 2: Run tests and confirm failure**

Run: `cargo test -p labby-codemode v2_json_hash_and_detect_methods_round_trip --all-features`

Expected: FAIL with `unknown state method`.

- [x] **Step 3: Add workspace implementations**

Implement:

```text
read_json: read_file, serde_json::from_str, return { path, value }.
write_json: serialize value with serde_json::to_string_pretty when pretty=true, else to_string; add trailing newline; write_file.
hash_file: only "sha256" supported in V2; read file bytes using tokio::fs::read after path/symlink checks and result caps; return hex and bytes.
detect_file: return extension, text bool, json bool, bytes. Detect text by UTF-8 validation on capped file bytes; detect json by extension or successful serde_json parse.
```

Result structs:

```rust
#[derive(Debug, Clone, Serialize)]
pub(crate) struct JsonReadResult {
    pub(crate) path: String,
    pub(crate) value: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct HashFileResult {
    pub(crate) path: String,
    pub(crate) algorithm: String,
    pub(crate) hex: String,
    pub(crate) bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DetectFileResult {
    pub(crate) path: String,
    pub(crate) extension: String,
    pub(crate) text: bool,
    pub(crate) json: bool,
    pub(crate) bytes: usize,
}
```

- [x] **Step 4: Add provider dispatch arms**

Add:

```rust
"readJson" => { /* PathParams */ }
"writeJson" => { /* WriteJsonParams { path, value, pretty } */ }
"hashFile" => { /* HashFileParams { path, algorithm } */ }
"detectFile" => { /* PathParams */ }
```

Reject unsupported hash algorithms with `InvalidParam { param: "algorithm" }`.

- [x] **Step 5: Run focused tests**

Run: `cargo test -p labby-codemode state:: --all-features`

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/state/provider.rs crates/labby-codemode/src/state/workspace.rs
git commit -m "feat: add code mode state json and hash helpers"
```

---

### Task 3: Archive Helpers Without Host Escape

**Files:**
- Modify: `crates/labby-codemode/Cargo.toml` only if adding `tar`
- Modify: `crates/labby-codemode/src/state/provider.rs`
- Modify: `crates/labby-codemode/src/state/workspace.rs`

**Interfaces:**
- Consumes: `walk_tree`, `read_file`, `write_file`, `VirtualPath`.
- Produces:
  - `archiveCreate({ source, destination })`
  - `archiveList({ path, limit })`

- [x] **Step 1: Write failing archive tests**

Add to `state/provider.rs` tests:

```rust
#[tokio::test]
async fn v2_archive_create_and_list_stays_in_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let workspace =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();

    dispatch_state_method(&workspace, "writeFile", json!({"path": "src/a.txt", "content": "a"})).await.unwrap();
    dispatch_state_method(&workspace, "writeFile", json!({"path": "src/b.txt", "content": "b"})).await.unwrap();
    dispatch_state_method(&workspace, "archiveCreate", json!({"source": "src", "destination": "out/src.tar"})).await.unwrap();
    let listing = dispatch_state_method(&workspace, "archiveList", json!({"path": "out/src.tar", "limit": 10})).await.unwrap();
    assert!(listing["entries"].as_array().unwrap().iter().any(|entry| entry == "a.txt"));
    assert!(listing["entries"].as_array().unwrap().iter().any(|entry| entry == "b.txt"));
}
```

- [x] **Step 2: Run tests and confirm failure**

Run: `cargo test -p labby-codemode v2_archive_create_and_list_stays_in_workspace --all-features`

Expected: FAIL with `unknown state method`.

- [x] **Step 3: Add minimal tar support**

If the standard library is insufficient, add this dependency:

```toml
tar = "0.4"
```

Implement uncompressed `.tar` only in V2. Reject other archive extensions with `InvalidParam { param: "destination" }`.

Result structs:

```rust
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchiveCreateResult {
    pub(crate) ok: bool,
    pub(crate) destination: String,
    pub(crate) entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ArchiveListResult {
    pub(crate) path: String,
    pub(crate) entries: Vec<String>,
    pub(crate) truncated: bool,
}
```

Security rules:

```text
source and destination are VirtualPath values.
archive entries are relative to source, never absolute.
archiveCreate excludes ".labby-state/" and refuses to archive the destination itself.
archiveList rejects absolute, parent-dir, and Windows-prefix archive member paths.
No archive extraction is included in V2.
```

- [x] **Step 4: Add provider dispatch arms**

Add:

```rust
"archiveCreate" => { /* ArchiveCreateParams { source, destination } */ }
"archiveList" => { /* ArchiveListParams { path, limit } */ }
```

- [x] **Step 5: Run focused tests**

Run: `cargo test -p labby-codemode state:: --all-features`

Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add crates/labby-codemode/Cargo.toml crates/labby-codemode/src/state/provider.rs crates/labby-codemode/src/state/workspace.rs
git commit -m "feat: add code mode state archive helpers"
```

---

### Task 4: Git Branch, Remote, And Guarded Remote Operations

**Files:**
- Modify: `crates/labby-codemode/src/git/command.rs`
- Modify: `crates/labby-codemode/src/git/provider.rs`
- Modify: `crates/labby-codemode/src/state/path.rs` only if path validation must be shared

**Interfaces:**
- Consumes: `GitCommandSpec::for_method(method, params)` and `dispatch_git_method`.
- Produces V2 git methods:
  - `branch({ name, delete })`
  - `checkout({ ref, create })`
  - `remoteList({})`
  - `remoteAdd({ name, url })`
  - `remoteRemove({ name })`
  - `clone({ url, directory })`
  - `fetch({ remote })`
  - `pull({ remote, branch })`
  - `push({ remote, branch })`

- [x] **Step 1: Write failing command tests**

Add to `git/command.rs` tests:

```rust
#[test]
fn git_v2_rejects_unsafe_remote_urls() {
    for url in ["file:///tmp/repo", "ssh://host/repo", "git@github.com:owner/repo.git", "https://user:token@example.com/repo.git"] {
        let err = GitCommandSpec::for_method("remoteAdd", serde_json::json!({"name": "origin", "url": url})).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }
}

#[test]
fn git_v2_builds_branch_checkout_and_remote_args() {
    let branch = GitCommandSpec::for_method("branch", serde_json::json!({"name": "feature/demo"})).unwrap();
    assert!(branch.args.ends_with(&["branch".to_string(), "feature/demo".to_string()]));

    let checkout = GitCommandSpec::for_method("checkout", serde_json::json!({"ref": "feature/demo"})).unwrap();
    assert!(checkout.args.ends_with(&["checkout".to_string(), "--".to_string(), "feature/demo".to_string()]));

    let remote = GitCommandSpec::for_method("remoteAdd", serde_json::json!({"name": "origin", "url": "https://github.com/jmagar/example.git"})).unwrap();
    assert!(remote.args.iter().any(|arg| arg == "remote"));
}
```

- [x] **Step 2: Run tests and confirm failure**

Run: `cargo test -p labby-codemode git_v2_ --all-features`

Expected: FAIL with unsupported methods.

- [x] **Step 3: Add param structs and validators**

In `git/command.rs`:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchParams {
    name: String,
    #[serde(default)]
    delete: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutParams {
    #[serde(rename = "ref")]
    git_ref: String,
    #[serde(default)]
    create: bool,
}

#[derive(Deserialize)]
struct RemoteNameParams {
    name: String,
}

#[derive(Deserialize)]
struct RemoteAddParams {
    name: String,
    url: String,
}

#[derive(Deserialize)]
struct CloneParams {
    url: String,
    directory: String,
}

#[derive(Deserialize)]
struct PullPushParams {
    remote: Option<String>,
    branch: Option<String>,
}
```

Validators:

```text
remote names: ASCII alnum, "-", "_", "." only; 1..64 chars.
branch/ref names: reject empty, whitespace, "..", "~", "^", ":", "?", "*", "[", "\\", leading "-", trailing "/", and lock suffix.
remote URLs: allow only https://github.com/... URLs with no username/password and no query/fragment.
clone directory: VirtualPath, must not be "." or include ".git".
```

- [x] **Step 4: Add command arms**

Add structured argv arms:

```text
branch: git branch <name> or git branch -D <name>
checkout: git checkout <ref> or git checkout -b <ref>
remoteList: git remote -v
remoteAdd: git remote add <name> <url>
remoteRemove: git remote remove <name>
clone: git clone --depth 1 -- <url> <directory>
fetch: git fetch <remote-or-origin>
pull: git pull --ff-only <remote-or-origin> <branch-or-HEAD>
push: git push <remote-or-origin> <branch-or-HEAD>
```

Keep the existing `base_args` config isolation. Do not add credential helpers.

- [x] **Step 5: Add provider tests**

Add to `git/provider.rs`:

```rust
#[tokio::test]
async fn git_v2_branch_checkout_and_remote_list_work_locally() {
    let temp = tempfile::tempdir().unwrap();
    let workspace =
        StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default()).unwrap();
    workspace.write_file(&crate::state::path::VirtualPath::parse("README.md").unwrap(), "hi\n").await.unwrap();
    dispatch_git_method(&workspace, "init", json!({})).await.unwrap();
    dispatch_git_method(&workspace, "add", json!({"path": "README.md"})).await.unwrap();
    dispatch_git_method(&workspace, "commit", json!({"message": "init", "authorName": "Lab", "authorEmail": "lab@example.invalid"})).await.unwrap();
    dispatch_git_method(&workspace, "branch", json!({"name": "feature/demo"})).await.unwrap();
    dispatch_git_method(&workspace, "checkout", json!({"ref": "feature/demo"})).await.unwrap();
    let remotes = dispatch_git_method(&workspace, "remoteList", json!({})).await.unwrap();
    assert_eq!(remotes["ok"], true);
}
```

- [x] **Step 6: Run focused tests**

Run: `cargo test -p labby-codemode git:: --all-features`

Expected: PASS.

- [x] **Step 7: Commit**

```bash
git add crates/labby-codemode/src/git/command.rs crates/labby-codemode/src/git/provider.rs
git commit -m "feat: add guarded code mode git v2 commands"
```

---

### Task 5: Docs, Smoke, Beads, And Final Verification

**Files:**
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `CHANGELOG.md`
- Create: `tests/smoke-code-mode-state-git-v2.sh`
- Modify: `docs/superpowers/plans/2026-06-28-code-mode-state-git-v2.md`

**Interfaces:**
- Consumes: representative V2 state and git methods from Tasks 1-4.
- Produces: user-facing V2 documentation, representative smoke proof, and completed plan checklist.

- [x] **Step 1: Update docs with exact V2 surface**

In `docs/dev/CODE_MODE.md`, extend the local providers section with:

```markdown
V2 state methods add:

- `state.appendFile({ path, content })`
- `state.exists({ path })`
- `state.stat({ path })`
- `state.mkdir({ path })`
- `state.rm({ path, recursive })`
- `state.cp({ from, to })`
- `state.mv({ from, to })`
- `state.walkTree({ path, limit })` / `state.summarizeTree({ path, limit })`
- `state.readJson({ path })`
- `state.writeJson({ path, value, pretty })`
- `state.hashFile({ path, algorithm: "sha256" })`
- `state.detectFile({ path })`
- `state.archiveCreate({ source, destination })`
- `state.archiveList({ path, limit })`

V2 git methods add:

- `git.branch({ name, delete, cwd })`
- `git.checkout({ ref, create, cwd })`
- `git.remoteList({ cwd })` returns `stdout` and structured `remotes`
- `git.remoteAdd({ name, url, cwd })`
- `git.remoteRemove({ name, cwd })`
- `git.clone({ url, directory, cwd })`
- `git.fetch({ remote, cwd })`
- `git.pull({ remote, branch, cwd })`
- `git.push({ remote, branch, cwd })`

Remote git URLs must be explicit `https://github.com/...` URLs without embedded
credentials. Labby does not inject hidden credentials or host git config into
Code Mode. Use `cwd` to run git commands inside a workspace-relative child repo,
for example after cloning into `directory: "repo"`.
```

- [x] **Step 2: Add changelog entry**

Under `[Unreleased]`, add:

```markdown
- **Code Mode state/git V2** — expanded local Code Mode workspace APIs with
  safe filesystem mutation helpers, JSON/hash/detect/archive helpers, guarded
  git branch/remote commands, and explicit unauthenticated GitHub remote git operations.
```

- [x] **Step 3: Add smoke script**

Create `tests/smoke-code-mode-state-git-v2.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

export LAB_HOME="$TMP/lab-home"
mkdir -p "$LAB_HOME"

cd "$ROOT"
cargo run --all-features -- --json gateway code exec --code 'async () => {
  await state.mkdir({ path: "src" });
  await state.writeJson({ path: "src/config.json", value: { enabled: true }, pretty: true });
  await state.appendFile({ path: "src/app.rs", content: "fn main() {}\n" });
  const hash = await state.hashFile({ path: "src/config.json", algorithm: "sha256" });
  const detect = await state.detectFile({ path: "src/config.json" });
  await state.archiveCreate({ source: "src", destination: "out/src.tar" });
  const archive = await state.archiveList({ path: "out/src.tar", limit: 10 });
  await git.init({});
  await git.add({ path: "src/app.rs" });
  await git.commit({ message: "v2 smoke", authorName: "Lab", authorEmail: "lab@example.invalid" });
  await git.branch({ name: "feature/v2-smoke" });
  await git.checkout({ ref: "feature/v2-smoke" });
  const status = await git.status({});
  return { hash: hash.hex.length, json: detect.json, archive: archive.entries.length, status: status.stdout };
}'
```

- [x] **Step 4: Run focused verification**

Run:

```bash
cargo test -p labby-codemode --all-features
cargo test -p labby --test architecture_orchestrator --all-features
bash tests/smoke-code-mode-state-git-v2.sh
```

Expected: all PASS.

- [x] **Step 5: Run full verification**

Run:

```bash
cargo nextest run --workspace --all-features
cargo build --workspace --all-features
```

Expected: all PASS. The existing empty `labby-web` embedded-assets warning is acceptable if it appears unchanged.

- [x] **Step 6: Commit**

```bash
git add docs/dev/CODE_MODE.md CHANGELOG.md tests/smoke-code-mode-state-git-v2.sh docs/superpowers/plans/2026-06-28-code-mode-state-git-v2.md
git commit -m "docs: document code mode state git v2"
```

---

## Mandatory Work-It Review Gates

After the implementation agent completes this plan and local verification is green:

1. Create the PR immediately.
2. Run `lavra-review` in the V2 worktree and fix every finding.
3. Run three `code_simplifier` passes over touched implementation, tests, and docs.
4. Run every available `pr-review-toolkit` role over the PR/touched files.
5. Fetch PR comments, fix every actionable comment, push fixes, and resolve comments only after the fix is present remotely.
6. Save a session markdown note before final `git add .`.
7. Run final all-features verification and push final cleanup.

## Self-Review

**Spec coverage:** The plan covers all deferred V2 categories from the V1 epic: broad state file/tree helpers, JSON/hash/detect/archive helpers, branch/checkout/remotes, guarded remote git operations, docs, smoke, and full verification. Hidden remote auth remains intentionally excluded from implementation; V2 documents the no-hidden-auth boundary and supports explicit unauthenticated GitHub HTTPS URLs only.

**Placeholder scan:** No unconstrained placeholders remain. Each task names exact files, methods, test names, command invocations, and expected outcomes.

**Type consistency:** Provider method names use camelCase matching sandbox JavaScript calls. Rust helper names are snake_case and scoped to `StateWorkspace` or `GitCommandSpec`. The smoke script uses only methods defined in this plan.
