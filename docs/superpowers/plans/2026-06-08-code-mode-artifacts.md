# Code Mode Artifacts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a host-brokered `writeArtifact()` helper to Labby Code Mode so snippets can persist large markdown or JSON outputs to disk and return a compact receipt instead of losing useful work to final-response truncation.

**Architecture:** Keep Code Mode response caps in place. Add one new runner-to-host protocol event for artifact writes, implement artifact persistence in a small `artifacts.rs` module, inject `globalThis.writeArtifact(name, content, options)` beside `callTool`, and teach snippets to return `{ artifact, summary, timings }` instead of embedding full research reports in the execution result. This follows Cloudflare Code Mode's artifact pattern (`state.writeJson`) while preserving Labby's stronger gateway-controlled filesystem boundary.

**Tech Stack:** Rust 2024, Tokio, serde/serde_json, sha2 0.11, ulid, Labby Code Mode stdio protocol, Javy/Boa JavaScript runner, cargo nextest.

---

## Current Evidence

Cloudflare's raw Code Mode tool returns the sandbox result directly, but its MCP-facing wrappers intentionally truncate text responses around 6,000 tokens. The useful pattern in Cloudflare is not "remove truncation"; it is "write large output inside the sandbox and return a small result." Their README example uses `await state.writeJson("/report.json", results); return results.length;`.

Labby has an explicit final response budget:

- `crates/lab/src/config.rs` defaults `[code_mode].max_response_bytes = 24 * 1024` and `max_response_tokens = 6000`.
- `crates/lab/src/dispatch/gateway/code_mode/execute.rs` calls `response_within_budget()` and `truncate_execution_response()`.
- `crates/lab/src/dispatch/gateway/code_mode/truncate.rs` replaces oversized final results with a truncation marker.

The plan below keeps those caps. The fix is to make Code Mode snippets artifact-first.

## File Structure

- Create `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs`
  - Owns artifact path validation, root resolution, file writes, digesting, and returned receipt shape.
  - Uses `crate::dispatch::helpers::{lab_home, reject_path_traversal, redact_home}`.

- Modify `crates/lab/src/dispatch/gateway/code_mode.rs`
  - Registers the new `artifacts` module.
  - Re-exports artifact helpers for in-crate tests only if needed.

- Modify `crates/lab/src/dispatch/gateway/code_mode/protocol.rs`
  - Adds a `CodeModeRunnerOutput::ArtifactWrite` variant.
  - Reuses existing `ToolResult` / `ToolError` runner inputs to settle the promise by sequence number.

- Modify `crates/lab/src/dispatch/gateway/code_mode/runner.rs`
  - Injects `globalThis.writeArtifact(name, content, options = {})`.
  - Adds a Javy host callback that emits `ArtifactWrite`.
  - Keeps `callTool` behavior unchanged.

- Modify `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`
  - Handles `ArtifactWrite` events from the runner.
  - Writes artifacts through `artifacts.rs`.
  - Records the artifact operation as a lightweight executed call with id `code_mode::write_artifact`.

- Modify `crates/lab/src/dispatch/gateway/code_mode/types.rs`
  - Adds artifact receipt structs.
  - Adds `artifacts: Vec<CodeModeArtifactReceipt>` to `CodeModeExecutionResponse` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.

- Modify `crates/lab/src/dispatch/gateway/code_mode/truncate.rs`
  - Preserves artifact receipts when the final `result` is truncated.
  - Ensures truncation previews mention artifact receipts if present.

- Modify `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`
  - Adds unit tests for artifact path validation, write receipt, and truncation behavior with artifacts.

- Modify `crates/lab/src/dispatch/gateway/code_mode/tests_ids_schema.rs`
  - Adds protocol serde coverage for `ArtifactWrite`.

- Modify `docs/runtime/CONFIG.md`
  - Documents the fixed artifact root and the reason artifacts do not bypass Code Mode execution timeout or tool-call limits.

- Modify `docs/snippets/README.md`
  - Documents the artifact-first snippet convention.

- Modify `docs/snippets/axon-fanout.md`
  - Updates the Axon fanout snippet to call `writeArtifact(...)` and return a compact receipt.

## Design Contract

`writeArtifact` signature available in Code Mode:

```js
await writeArtifact("axon/axum-timeout.md", markdown, {
  contentType: "text/markdown"
});
```

Returned receipt:

```json
{
  "path": "code-mode-artifacts/01J.../axon/axum-timeout.md",
  "absolute_path": "~/.lab/code-mode-artifacts/01J.../axon/axum-timeout.md",
  "content_type": "text/markdown",
  "bytes": 18342,
  "sha256": "8b4f..."
}
```

Rules:

- Artifact writes are host-brokered through the runner protocol; JavaScript does not get raw filesystem access.
- Root is `$LAB_HOME/code-mode-artifacts/<run_id>/` or `$HOME/.lab/code-mode-artifacts/<run_id>/`.
- `name` must be a non-empty relative path, cannot be absolute, and cannot contain `..`.
- `content` must be a string.
- `options.contentType` is optional and defaults to `text/plain`.
- A single artifact maxes out at 1 MiB for this first implementation.
- The final Code Mode response remains capped by existing `[code_mode]` settings.
- Artifact receipts survive final result truncation.

---

### Task 1: Add Artifact Types And Persistence

**Files:**
- Create: `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`

- [ ] **Step 1: Write failing tests for artifact validation and write receipts**

Append these tests to `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`:

```rust
use tempfile::TempDir;

use super::artifacts::{CodeModeArtifactWrite, write_code_mode_artifact};

#[tokio::test]
async fn write_code_mode_artifact_rejects_absolute_paths() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "/tmp/escape.md".to_string(),
        content: "# nope".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let err = write_code_mode_artifact(root.path(), &request)
        .await
        .expect_err("absolute artifact path must be rejected");

    assert_eq!(err.kind(), "invalid_param");
    assert!(
        err.to_string().contains("relative path"),
        "error should explain relative path requirement: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_rejects_parent_dir_paths() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "../escape.md".to_string(),
        content: "# nope".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let err = write_code_mode_artifact(root.path(), &request)
        .await
        .expect_err("parent dir artifact path must be rejected");

    assert_eq!(err.kind(), "invalid_param");
    assert!(
        err.to_string().contains("path traversal"),
        "error should mention traversal: {err}"
    );
}

#[tokio::test]
async fn write_code_mode_artifact_persists_content_and_returns_receipt() {
    let root = TempDir::new().expect("temp root");
    let request = CodeModeArtifactWrite {
        path: "axon/brief.md".to_string(),
        content: "# Brief\n\nUseful output.\n".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let receipt = write_code_mode_artifact(root.path(), &request)
        .await
        .expect("artifact write succeeds");

    assert_eq!(receipt.path, "axon/brief.md");
    assert_eq!(receipt.content_type, "text/markdown");
    assert_eq!(receipt.bytes, 23);
    assert_eq!(receipt.sha256.len(), 64);

    let written = tokio::fs::read_to_string(root.path().join("axon/brief.md"))
        .await
        .expect("artifact file exists");
    assert_eq!(written, "# Brief\n\nUseful output.\n");
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo nextest run -p lab write_code_mode_artifact --all-features
```

Expected:

```text
FAIL ... unresolved import `super::artifacts`
```

- [ ] **Step 3: Register the artifact module**

In `crates/lab/src/dispatch/gateway/code_mode.rs`, add the module beside the other private modules:

```rust
mod artifacts;
```

Add this test-only re-export near the existing `#[cfg(test)]` re-exports:

```rust
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) use artifacts::{
    CodeModeArtifactReceipt, CodeModeArtifactWrite, write_code_mode_artifact,
};
```

- [ ] **Step 4: Implement artifact persistence**

Create `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs`:

```rust
//! Host-brokered artifact writes for Code Mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{lab_home, redact_home, reject_path_traversal};

const DEFAULT_CONTENT_TYPE: &str = "text/plain";
const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::dispatch::gateway::code_mode) struct CodeModeArtifactWrite {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::dispatch::gateway::code_mode) struct CodeModeArtifactReceipt {
    pub path: String,
    pub absolute_path: String,
    pub content_type: String,
    pub bytes: usize,
    pub sha256: String,
}

#[must_use]
pub(in crate::dispatch::gateway::code_mode) fn code_mode_artifact_root(run_id: &str) -> PathBuf {
    lab_home().join("code-mode-artifacts").join(run_id)
}

pub(in crate::dispatch::gateway::code_mode) async fn write_code_mode_artifact(
    root: &Path,
    request: &CodeModeArtifactWrite,
) -> Result<CodeModeArtifactReceipt, ToolError> {
    let rel_path = normalize_artifact_path(&request.path)?;
    let bytes = request.content.as_bytes();
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!(
                "artifact content is {} bytes; maximum is {} bytes",
                bytes.len(),
                MAX_ARTIFACT_BYTES
            ),
            param: "content".to_string(),
        });
    }

    let destination = root.join(&rel_path);
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|err| ToolError::Sdk {
            sdk_kind: "artifact_write_failed".to_string(),
            message: format!("failed to create artifact directory: {err}"),
        })?;
    }

    let mut file = tokio::fs::File::create(&destination)
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "artifact_write_failed".to_string(),
            message: format!("failed to create artifact file: {err}"),
        })?;
    file.write_all(bytes).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "artifact_write_failed".to_string(),
        message: format!("failed to write artifact file: {err}"),
    })?;
    file.flush().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "artifact_write_failed".to_string(),
        message: format!("failed to flush artifact file: {err}"),
    })?;

    let sha256 = Sha256::digest(bytes);
    let content_type = request
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_CONTENT_TYPE)
        .to_string();

    Ok(CodeModeArtifactReceipt {
        path: rel_path,
        absolute_path: redact_home(&destination.display().to_string()),
        content_type,
        bytes: bytes.len(),
        sha256: format!("{sha256:x}"),
    })
}

fn normalize_artifact_path(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a non-empty relative path".to_string(),
            param: "path".to_string(),
        });
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: "artifact path must be a relative path".to_string(),
            param: "path".to_string(),
        });
    }
    reject_path_traversal(trimmed)?;
    Ok(trimmed.replace('\\', "/"))
}
```

- [ ] **Step 5: Run the focused tests and verify they pass**

Run:

```bash
cargo nextest run -p lab write_code_mode_artifact --all-features
```

Expected:

```text
PASS write_code_mode_artifact_rejects_absolute_paths
PASS write_code_mode_artifact_rejects_parent_dir_paths
PASS write_code_mode_artifact_persists_content_and_returns_receipt
```

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs \
  crates/lab/src/dispatch/gateway/code_mode/artifacts.rs \
  crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs
git commit -m "feat: add code mode artifact persistence"
```

---

### Task 2: Add Artifact Receipts To Execution Responses

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/types.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/truncate.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`

- [ ] **Step 1: Write failing truncation test for artifact receipts**

Append this test to `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`:

```rust
#[test]
fn truncation_preserves_artifact_receipts() {
    let response = CodeModeExecutionResponse {
        result: Some(serde_json::json!({
            "markdown": "x".repeat(10_000),
            "artifact": {
                "path": "code-mode-artifacts/run/brief.md"
            }
        })),
        calls: vec![],
        logs: vec![],
        artifacts: vec![CodeModeArtifactReceipt {
            path: "brief.md".to_string(),
            absolute_path: "~/.lab/code-mode-artifacts/run/brief.md".to_string(),
            content_type: "text/markdown".to_string(),
            bytes: 10_000,
            sha256: "a".repeat(64),
        }],
    };

    let truncated = truncate_execution_response(response, 1400, 6000, 4);

    assert_eq!(truncated.artifacts.len(), 1);
    assert_eq!(truncated.artifacts[0].path, "brief.md");
    let result = truncated.result.expect("truncated marker result");
    assert_eq!(result["truncated"], true);
    assert_eq!(result["artifacts"][0]["path"], "brief.md");
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo nextest run -p lab truncation_preserves_artifact_receipts --all-features
```

Expected:

```text
FAIL ... struct `CodeModeExecutionResponse` has no field named `artifacts`
```

- [ ] **Step 3: Add the response field**

In `crates/lab/src/dispatch/gateway/code_mode/types.rs`, import the receipt:

```rust
use super::artifacts::CodeModeArtifactReceipt;
```

Extend `CodeModeExecutionResponse`:

```rust
pub struct CodeModeExecutionResponse {
    /// The final return value of the async function. None when the function
    /// returns undefined, null, or throws (the throw case surfaces via ToolError).
    pub result: Option<Value>,
    pub calls: Vec<CodeModeExecutedCall>,
    /// Captured console.log/warn/error lines from the runner. Sourced from the
    /// javy runner subprocess (drained from its stderr); the current javy path
    /// returns no protocol-carried logs, so this is empty until console capture
    /// is wired through.
    pub logs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<CodeModeArtifactReceipt>,
}
```

- [ ] **Step 4: Update existing response constructors**

Search all constructors:

```bash
rg -n "CodeModeExecutionResponse \\{" crates/lab/src/dispatch/gateway/code_mode
```

For every constructor that does not already set artifacts, add:

```rust
artifacts: vec![],
```

At minimum update constructors in:

- `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`
- `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`

- [ ] **Step 5: Preserve receipts in truncation markers**

In `crates/lab/src/dispatch/gateway/code_mode/truncate.rs`, update the truncation marker construction so artifact receipts are copied into the marker result:

```rust
let artifacts = response.artifacts.clone();
response.result = Some(serde_json::json!({
    "truncated": true,
    "reason": reason,
    "preview": preview,
    "artifacts": artifacts,
}));
```

If the existing marker uses different local variable names, keep those names and add only the `"artifacts": response.artifacts.clone()` field to the JSON object.

- [ ] **Step 6: Run the focused test and verify it passes**

Run:

```bash
cargo nextest run -p lab truncation_preserves_artifact_receipts --all-features
```

Expected:

```text
PASS truncation_preserves_artifact_receipts
```

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/types.rs \
  crates/lab/src/dispatch/gateway/code_mode/truncate.rs \
  crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs \
  crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs
git commit -m "feat: include code mode artifact receipts"
```

---

### Task 3: Extend The Runner Protocol

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/protocol.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_ids_schema.rs`

- [ ] **Step 1: Write failing protocol serde test**

Append this test to `crates/lab/src/dispatch/gateway/code_mode/tests_ids_schema.rs`:

```rust
use super::protocol::CodeModeRunnerOutput;

#[test]
fn artifact_write_protocol_round_trips() {
    let output = CodeModeRunnerOutput::ArtifactWrite {
        seq: 7,
        path: "axon/brief.md".to_string(),
        content: "# Brief".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let encoded = serde_json::to_string(&output).expect("serialize protocol");
    assert_eq!(
        encoded,
        r#"{"type":"artifact_write","seq":7,"path":"axon/brief.md","content":"# Brief","content_type":"text/markdown"}"#
    );

    let decoded: CodeModeRunnerOutput =
        serde_json::from_str(&encoded).expect("deserialize protocol");
    assert_eq!(decoded, output);
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo nextest run -p lab artifact_write_protocol_round_trips --all-features
```

Expected:

```text
FAIL ... no variant named `ArtifactWrite`
```

- [ ] **Step 3: Add the protocol variant**

In `crates/lab/src/dispatch/gateway/code_mode/protocol.rs`, add this variant to `CodeModeRunnerOutput` after `ToolCall`:

```rust
    ArtifactWrite {
        seq: u64,
        path: String,
        content: String,
        #[serde(default)]
        content_type: Option<String>,
    },
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
cargo nextest run -p lab artifact_write_protocol_round_trips --all-features
```

Expected:

```text
PASS artifact_write_protocol_round_trips
```

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/protocol.rs \
  crates/lab/src/dispatch/gateway/code_mode/tests_ids_schema.rs
git commit -m "feat: add code mode artifact protocol event"
```

---

### Task 4: Inject `writeArtifact` Into The JavaScript Runtime

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/runner.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`

- [ ] **Step 1: Write failing test for runner wrapper contents**

If `runner.rs` has no direct wrapper test hook, add this test to `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs` after exposing a helper in Step 3:

```rust
#[test]
fn code_mode_runner_wrapper_exposes_write_artifact() {
    let wrapped = super::runner::wrap_code_mode_for_test(
        "async () => 'ok'",
        "var codemode = {};",
    );

    assert!(wrapped.contains("globalThis.writeArtifact"));
    assert!(wrapped.contains("__labEmitArtifactWrite"));
    assert!(wrapped.contains("writeArtifact path must be a non-empty string"));
    assert!(wrapped.contains("writeArtifact content must be a string"));
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo nextest run -p lab code_mode_runner_wrapper_exposes_write_artifact --all-features
```

Expected:

```text
FAIL ... cannot find function `wrap_code_mode_for_test`
```

- [ ] **Step 3: Extract a wrapper helper for testability**

In `crates/lab/src/dispatch/gateway/code_mode/runner.rs`, move the existing `let invoker = ...; let wrapped = format!(...)` body into a helper:

```rust
fn wrap_code_mode(code: &str, proxy: &str) -> String {
    let invoker = code_mode_main_invoker(code);
    format!(
        r#"
globalThis.__labPendingToolCalls = new Map();
{codec}
globalThis.callTool = (id, params = {{}}) => {{
  if (typeof id !== "string" || id.trim() === "") {{
    throw new TypeError("callTool id must be a non-empty string");
  }}
  if (params === null || typeof params !== "object" || Array.isArray(params)) {{
    throw new TypeError("callTool params must be a JSON object");
  }}
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitToolCall(id, __labEncodeResult(params));
    globalThis.__labPendingToolCalls.set(seq, {{ resolve, reject }});
  }});
}};
globalThis.writeArtifact = (path, content, options = {{}}) => {{
  if (typeof path !== "string" || path.trim() === "") {{
    throw new TypeError("writeArtifact path must be a non-empty string");
  }}
  if (typeof content !== "string") {{
    throw new TypeError("writeArtifact content must be a string");
  }}
  if (options === null || typeof options !== "object" || Array.isArray(options)) {{
    throw new TypeError("writeArtifact options must be a JSON object");
  }}
  const contentType = typeof options.contentType === "string" ? options.contentType : null;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitArtifactWrite(path, content, contentType);
    globalThis.__labPendingToolCalls.set(seq, {{ resolve, reject }});
  }});
}};
globalThis.__labSettleToolCall = (message) => {{
  const input = JSON.parse(message);
  const pending = globalThis.__labPendingToolCalls.get(input.seq);
  if (!pending) {{
    throw new Error("runner received a response for an unknown tool call");
  }}
  globalThis.__labPendingToolCalls.delete(input.seq);
  if (input.type === "tool_result") {{
    pending.resolve(__labDecodeResult(input.result));
    return;
  }}
  if (input.type === "tool_error") {{
    pending.reject(new Error(JSON.stringify({{kind: input.kind, message: input.message}})));
    return;
  }}
  throw new Error("runner received unexpected protocol message");
}};
{proxy}
globalThis.__labMainPromise = (async () => {{
{invoker}}})();
"#,
        codec = CODE_MODE_VALUE_CODEC_JS,
        invoker = invoker,
        proxy = proxy,
    )
}

#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) fn wrap_code_mode_for_test(
    code: &str,
    proxy: &str,
) -> String {
    wrap_code_mode(code, proxy)
}
```

Then replace the old inline wrapper construction in `run_code_mode_runner()` with:

```rust
let wrapped = wrap_code_mode(&code, &proxy);
```

- [ ] **Step 4: Add the Javy host callback**

In `crates/lab/src/dispatch/gateway/code_mode/runner.rs`, add `CodeModeRunnerOutput::ArtifactWrite` to the existing protocol imports if needed.

Add a function beside the existing `javy_emit_tool_call`:

```rust
#[javy::host_fn]
fn javy_emit_artifact_write(
    path: String,
    content: String,
    content_type: Option<String>,
) -> Result<u64, javy::Error> {
    RUNNER_STATE.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let state = borrowed
            .as_mut()
            .ok_or_else(|| javy::Error::new("runner state not initialized"))?;
        let seq = state.next_seq;
        state.next_seq += 1;
        let output = CodeModeRunnerOutput::ArtifactWrite {
            seq,
            path,
            content,
            content_type,
        };
        serde_json::to_writer(&mut state.writer, &output)
            .map_err(|err| javy::Error::new(format!("failed to serialize artifact write: {err}")))?;
        state
            .writer
            .write_all(b"\n")
            .map_err(|err| javy::Error::new(format!("failed to write artifact write: {err}")))?;
        state
            .writer
            .flush()
            .map_err(|err| javy::Error::new(format!("failed to flush artifact write: {err}")))?;
        Ok(seq)
    })
}
```

Register it in the same place `__labEmitToolCall` is registered:

```rust
runtime
    .context()
    .global_object()
    .set(
        "__labEmitArtifactWrite",
        javy_emit_artifact_write.into_js_function(runtime.context())?,
        false,
        runtime.context(),
    )?;
```

- [ ] **Step 5: Run the focused wrapper test**

Run:

```bash
cargo nextest run -p lab code_mode_runner_wrapper_exposes_write_artifact --all-features
```

Expected:

```text
PASS code_mode_runner_wrapper_exposes_write_artifact
```

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/runner.rs \
  crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs
git commit -m "feat: expose writeArtifact in code mode"
```

---

### Task 5: Broker Artifact Writes In The Parent Process

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`

- [ ] **Step 1: Write focused test for run artifact root creation**

Append this test to `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`:

```rust
use super::artifacts::code_mode_artifact_root;

#[test]
fn code_mode_artifact_root_uses_run_id_under_lab_home() {
    let root = code_mode_artifact_root("01JTEST");
    let text = root.display().to_string();

    assert!(text.ends_with(".lab/code-mode-artifacts/01JTEST") || text.ends_with("lab/code-mode-artifacts/01JTEST"));
}
```

- [ ] **Step 2: Run the test and verify it passes**

Run:

```bash
cargo nextest run -p lab code_mode_artifact_root_uses_run_id_under_lab_home --all-features
```

Expected:

```text
PASS code_mode_artifact_root_uses_run_id_under_lab_home
```

- [ ] **Step 3: Add run id and artifact state to runner drive**

In `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`, add imports:

```rust
use serde_json::json;
use ulid::Ulid;

use super::artifacts::{
    CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
```

At the top of `run_in_runner()`, after deadline setup, add:

```rust
let artifact_run_id = Ulid::new().to_string();
let artifact_root = code_mode_artifact_root(&artifact_run_id);
let mut artifacts: Vec<CodeModeArtifactReceipt> = Vec::new();
```

- [ ] **Step 4: Handle `ArtifactWrite` output**

In the main `match output` loop in `runner_drive.rs`, add this arm beside `CodeModeRunnerOutput::ToolCall`:

```rust
CodeModeRunnerOutput::ArtifactWrite {
    seq,
    path,
    content,
    content_type,
} => {
    let started = std::time::Instant::now();
    let request = CodeModeArtifactWrite {
        path,
        content,
        content_type,
    };

    match write_code_mode_artifact(&artifact_root, &request).await {
        Ok(receipt) => {
            let result = json!(receipt);
            artifacts.push(receipt);
            calls.push(CodeModeExecutedCall {
                id: "code_mode::write_artifact".to_string(),
                ok: true,
                elapsed_ms: started.elapsed().as_millis(),
                error_kind: None,
            });
            write_runner_input(
                &mut child_stdin,
                &CodeModeRunnerInput::ToolResult { seq, result },
            )
            .await?;
        }
        Err(err) => {
            calls.push(CodeModeExecutedCall {
                id: "code_mode::write_artifact".to_string(),
                ok: false,
                elapsed_ms: started.elapsed().as_millis(),
                error_kind: Some(err.kind().to_string()),
            });
            write_runner_input(
                &mut child_stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind: err.kind().to_string(),
                    message: err.to_string(),
                },
            )
            .await?;
        }
    }
}
```

Use the existing stdin variable name from the file. If it is not `child_stdin`, use the local name already passed to `write_runner_input` for `ToolCall` responses.

- [ ] **Step 5: Include artifacts in successful responses**

In the `CodeModeRunnerOutput::Done` response constructor, add:

```rust
artifacts,
```

If the code needs to clone because `artifacts` is still used after the constructor, use:

```rust
artifacts: artifacts.clone(),
```

- [ ] **Step 6: Run compile check**

Run:

```bash
cargo check -p lab --all-features
```

Expected:

```text
Finished `dev` profile
```

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs \
  crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs
git commit -m "feat: broker code mode artifact writes"
```

---

### Task 6: Add End-To-End CLI Smoke Verification

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`
- Runtime verification only: no permanent test file required for the live gateway smoke.

- [ ] **Step 1: Build the all-features binary**

Run:

```bash
cargo build -p lab --all-features
```

Expected:

```text
Finished `dev` profile
```

- [ ] **Step 2: Run a direct Code Mode artifact smoke**

Run:

```bash
target/debug/labby gateway code exec --json --code 'async () => {
  const artifact = await writeArtifact("smoke/hello.md", "# Hello\n\nArtifact smoke.\n", { contentType: "text/markdown" });
  return { ok: true, artifact };
}' | jq '.result'
```

Expected shape:

```json
{
  "ok": true,
  "artifact": {
    "path": "smoke/hello.md",
    "absolute_path": "~/.lab/code-mode-artifacts/...",
    "content_type": "text/markdown",
    "bytes": 26,
    "sha256": "..."
  }
}
```

- [ ] **Step 3: Verify the artifact file exists**

Run:

```bash
ARTIFACT_PATH=$(target/debug/labby gateway code exec --json --code 'async () => {
  const artifact = await writeArtifact("smoke/exists.md", "# Exists\n", { contentType: "text/markdown" });
  return artifact;
}' | jq -r '.result.absolute_path' | sed "s#^~#$HOME#")
test -f "$ARTIFACT_PATH"
sed -n '1,5p' "$ARTIFACT_PATH"
```

Expected:

```text
# Exists
```

- [ ] **Step 4: Verify large final output is still capped but receipt survives**

Run:

```bash
target/debug/labby gateway code exec --json --code 'async () => {
  const markdown = "# Large\n\n" + "x".repeat(50000);
  const artifact = await writeArtifact("smoke/large.md", markdown, { contentType: "text/markdown" });
  return { markdown, artifact };
}' | jq '{truncated: .result.truncated, artifacts: .artifacts, marker_artifacts: .result.artifacts}'
```

Expected:

```json
{
  "truncated": true,
  "artifacts": [
    {
      "path": "smoke/large.md"
    }
  ],
  "marker_artifacts": [
    {
      "path": "smoke/large.md"
    }
  ]
}
```

- [ ] **Step 5: Commit if smoke exposed code changes**

If no code changes were made during smoke, skip this commit. If a fix was required, commit the exact files changed:

```bash
git add crates/lab/src/dispatch/gateway/code_mode
git commit -m "fix: stabilize code mode artifact smoke"
```

---

### Task 7: Update Axon Snippets To Use Artifacts

**Files:**
- Modify: `docs/snippets/README.md`
- Modify: `docs/snippets/axon-fanout.md`

- [ ] **Step 1: Update the snippets README**

In `docs/snippets/README.md`, add this section near the Code Mode snippet guidance:

```markdown
## Artifact-First Output

Code Mode snippets should return compact execution receipts and write large composed outputs as artifacts.

Use this pattern whenever a snippet creates markdown, source tables, screenshots, crawl manifests, or follow-up snippets:

```js
async () => {
  const markdown = renderMarkdownReport(data);
  const artifact = await writeArtifact("reports/example.md", markdown, {
    contentType: "text/markdown"
  });

  return {
    summary: "Generated report",
    artifact,
    timings
  };
}
```

The final return value is still subject to `[code_mode].max_response_bytes` and `[code_mode].max_response_tokens`. Artifacts are written under `$LAB_HOME/code-mode-artifacts/<run_id>/` and the receipt includes the path, byte count, content type, and SHA-256 digest.
```

- [ ] **Step 2: Update the Axon fanout snippet return**

In `docs/snippets/axon-fanout.md`, replace the final return shape with this artifact-first form:

```js
const artifact = await writeArtifact(
  `axon/${slug(topic)}.md`,
  markdown,
  { contentType: "text/markdown" }
);

return {
  workflow: "axon_fanout_topic",
  topic,
  summary: brief,
  artifact,
  selected_sources: selectedSources.map((source) => ({
    title: source.title,
    url: source.url,
    reason: source.reason,
    score: source.score
  })),
  gaps_and_risks: gapsAndRisks,
  followup_calls: followupCalls,
  timings
};
```

Ensure the generated `markdown` string contains:

```markdown
## Evidence Table
## Selected Sources
## Gaps And Risks
## Follow-Up Code Mode Snippet
## Timings
```

- [ ] **Step 3: Run docs grep checks**

Run:

```bash
rg -n "writeArtifact|Artifact-First Output|Follow-Up Code Mode Snippet" docs/snippets/README.md docs/snippets/axon-fanout.md
```

Expected:

```text
docs/snippets/README.md:...:## Artifact-First Output
docs/snippets/README.md:...:await writeArtifact("reports/example.md", markdown
docs/snippets/axon-fanout.md:...:await writeArtifact(
docs/snippets/axon-fanout.md:...:## Follow-Up Code Mode Snippet
```

- [ ] **Step 4: Commit**

```bash
git add docs/snippets/README.md docs/snippets/axon-fanout.md
git commit -m "docs: make code mode snippets artifact-first"
```

---

### Task 8: Document Runtime Behavior

**Files:**
- Modify: `docs/runtime/CONFIG.md`

- [ ] **Step 1: Add artifact documentation under `[code_mode]`**

In `docs/runtime/CONFIG.md`, add this after the existing Code Mode limit documentation:

```markdown
#### Code Mode Artifacts

`execute` exposes a sandbox helper for large outputs:

```js
const artifact = await writeArtifact("reports/brief.md", markdown, {
  contentType: "text/markdown"
});
return { artifact, summary: "Brief generated" };
```

Artifacts are host-brokered writes, not direct sandbox filesystem access. The runner emits an artifact request, Labby validates the relative path, writes the content under `$LAB_HOME/code-mode-artifacts/<run_id>/`, and returns a receipt:

```json
{
  "path": "reports/brief.md",
  "absolute_path": "~/.lab/code-mode-artifacts/01J.../reports/brief.md",
  "content_type": "text/markdown",
  "bytes": 18342,
  "sha256": "..."
}
```

Artifact writes do not bypass `timeout_ms`, `max_tool_calls`, or final response caps. They are the preferred way to keep large markdown reports, source tables, crawl manifests, and follow-up snippets out of the final JSON response while still making them available on disk.
```

- [ ] **Step 2: Run markdown sanity check**

Run:

```bash
rg -n "Code Mode Artifacts|writeArtifact|code-mode-artifacts" docs/runtime/CONFIG.md
```

Expected:

```text
docs/runtime/CONFIG.md:...:#### Code Mode Artifacts
docs/runtime/CONFIG.md:...:const artifact = await writeArtifact
docs/runtime/CONFIG.md:...:$LAB_HOME/code-mode-artifacts
```

- [ ] **Step 3: Commit**

```bash
git add docs/runtime/CONFIG.md
git commit -m "docs: document code mode artifacts"
```

---

### Task 9: Verify Full Code Mode Behavior With Axon

**Files:**
- Runtime verification only.
- Optional generated output: `docs/snippets/axon-artifact-smoke-output.md`

- [ ] **Step 1: Confirm gateway and Axon availability**

Run:

```bash
labby gateway list | rg -n "axon|code"
```

Expected:

```text
axon ... connected
```

- [ ] **Step 2: Run a real Axon artifact fanout smoke**

Run:

```bash
target/debug/labby gateway code exec --json --code 'async () => {
  const topic = "Axum request timeout middleware";
  const search = await callTool("axon::axon", {
    action: "search",
    query: "Axum request timeout middleware TimeoutLayer HandleErrorLayer",
    limit: 3
  });
  const ask = await callTool("axon::axon", {
    action: "ask",
    question: "How should an Axum service implement request timeouts?"
  });
  const markdown = [
    "# Axon Fanout Smoke",
    "",
    "## Topic",
    topic,
    "",
    "## Search",
    "```json",
    JSON.stringify(search, null, 2),
    "```",
    "",
    "## Ask",
    "```json",
    JSON.stringify(ask, null, 2),
    "```",
    "",
    "## Follow-Up Code Mode Snippet",
    "```js",
    "async () => {",
    "  return await callTool(\"axon::axon\", { action: \"ask\", question: \"What timeout errors must Axum map to 408?\" });",
    "}",
    "```"
  ].join("\n");
  const artifact = await writeArtifact("axon/axum-timeout-smoke.md", markdown, { contentType: "text/markdown" });
  return {
    topic,
    artifact,
    calls: ["axon search", "axon ask"],
    markdown_bytes: markdown.length
  };
}' | tee /tmp/code-mode-axon-artifact-smoke.json | jq '.result'
```

Expected:

```json
{
  "topic": "Axum request timeout middleware",
  "artifact": {
    "path": "axon/axum-timeout-smoke.md",
    "absolute_path": "~/.lab/code-mode-artifacts/...",
    "content_type": "text/markdown",
    "bytes": 1000,
    "sha256": "..."
  },
  "calls": [
    "axon search",
    "axon ask"
  ],
  "markdown_bytes": 1000
}
```

The exact byte count will differ; it must be greater than zero.

- [ ] **Step 3: Copy the artifact into docs for review**

Run:

```bash
ARTIFACT_PATH=$(jq -r '.result.artifact.absolute_path' /tmp/code-mode-axon-artifact-smoke.json | sed "s#^~#$HOME#")
cp "$ARTIFACT_PATH" docs/snippets/axon-artifact-smoke-output.md
sed -n '1,80p' docs/snippets/axon-artifact-smoke-output.md
```

Expected:

```text
# Axon Fanout Smoke
```

- [ ] **Step 4: Commit generated smoke output if it is useful**

If the smoke output is readable and source-backed, commit it:

```bash
git add docs/snippets/axon-artifact-smoke-output.md
git commit -m "docs: add axon artifact smoke output"
```

If it is noisy, do not commit it; remove only this assistant-generated file:

```bash
rm -f docs/snippets/axon-artifact-smoke-output.md
```

---

### Task 10: Run Full Verification

**Files:**
- No code changes expected.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --all
```

Expected: command exits 0.

- [ ] **Step 2: Run all Code Mode tests**

Run:

```bash
cargo nextest run -p lab code_mode --all-features
```

Expected:

```text
PASS
```

- [ ] **Step 3: Run all-features Lab check**

Run:

```bash
cargo check --workspace --all-features
```

Expected:

```text
Finished `dev` profile
```

- [ ] **Step 4: Run full repo test path if time permits**

Run:

```bash
just test
```

Expected:

```text
PASS
```

- [ ] **Step 5: Inspect git status**

Run:

```bash
git status --short
```

Expected: only intentional files from this implementation remain modified or untracked.

- [ ] **Step 6: Final commit if verification required fixups**

If formatting or tests caused fixups, commit those exact files:

```bash
git add crates/lab/src/dispatch/gateway/code_mode docs/runtime/CONFIG.md docs/snippets
git commit -m "fix: finalize code mode artifacts"
```

---

## Acceptance Criteria

- `writeArtifact(path, content, options)` is available in Code Mode `execute`.
- Artifact writes are routed through the host protocol, not raw sandbox filesystem access.
- Invalid artifact paths are rejected with `invalid_param`.
- Successful writes return a receipt with path, redacted absolute path, content type, bytes, and SHA-256.
- `CodeModeExecutionResponse.artifacts` includes receipts for host-brokered artifact writes.
- Existing final response truncation remains active.
- When final result truncates, artifact receipts remain visible in the response.
- `docs/snippets/axon-fanout.md` uses artifact-first output and includes the generated follow-up Code Mode snippet inside the markdown artifact.
- Real CLI smoke can write and read a markdown artifact.

## Self-Review

Spec coverage:

- The plan addresses the user's confusion about truncation by making generated markdown an artifact before the capped final response is returned.
- The plan avoids fake snippets and targets actual Labby Code Mode code paths.
- The plan keeps MCP/CLI Code Mode caps rather than pretending sandbox output bypasses response limits.
- The plan updates Axon snippets so the generated report includes evidence tables, selected sources, gaps, follow-up calls, timings, and the follow-up snippet in the markdown artifact.

Placeholder scan:

- No `TBD`, `TODO`, or "implement later" steps are present.
- Every code-changing task includes concrete code or an exact replacement shape.
- Every test task includes concrete commands and expected outcomes.

Type consistency:

- JavaScript helper is named `writeArtifact`.
- Runner protocol event is named `ArtifactWrite` and serializes as `artifact_write`.
- Rust write request is `CodeModeArtifactWrite`.
- Rust receipt is `CodeModeArtifactReceipt`.
- Execution response field is `artifacts`.

