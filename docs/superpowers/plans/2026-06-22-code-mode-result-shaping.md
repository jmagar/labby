# Code Mode Result Shaping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in, deterministic final-result shaping for Labby Code Mode without changing sandbox-visible `callTool()` results.

**Architecture:** Result shaping belongs in `crates/lab/src/dispatch/gateway/code_mode`, after a successful raw `CodeModeExecutionResponse` exists and after `apply_ui_opt_in()`, but before envelope truncation and MCP text/structured serialization. The public display response may be shaped; the pre-shape response is only available in-process long enough for `snippets.test` pass/fail evaluation and is never persisted or serialized.

**Tech Stack:** Rust 2024, serde/serde_json, Tokio, rmcp, Lab dispatch gateway Code Mode, Lab snippets dispatch, Markdown docs.

## Global Constraints

- Epic: `lab-iwda8`; children: `lab-iwda8.1` through `lab-iwda8.5`.
- Default behavior remains current behavior: result shaping is disabled unless explicitly configured.
- Implement deterministic Rust-side policies only; do not add arbitrary host JavaScript transforms in v1.
- Shaping applies only to successful completed Code Mode final `response.result` values.
- Sandbox-visible `callTool()` results and `CodeModeExecutedCall` metadata remain unshaped and payload-free.
- Ordering is raw completed response -> `apply_ui_opt_in()` -> result shaping -> envelope truncation -> MCP text/structured serialization.
- Truncation is not redaction. Do not claim secret sanitization from a truncate-only policy.
- Do not claim raw-result audit retention unless a separate artifact/history design stores it.
- Preserve explicit JSON `null` as `Some(Value::Null)` and JavaScript `undefined` as `None`.
- Keep observability metadata-only: policy, changed, original size, shaped size, truncated flag, failure kind; never raw result values.
- Use `writeArtifact()` for intentionally large detailed payloads; shaping is a model-facing safety rail.

---

## File Structure

- Modify `crates/lab/src/config.rs`: add `CodeModeResultShapePolicy`, default/serde handling, validation, and `CodeModeConfig` field.
- Modify `crates/lab/src/dispatch/gateway/catalog.rs`: expose `result_shape_policy` on `gateway.code_mode.set`.
- Modify `crates/lab/src/dispatch/gateway/params.rs`: ensure code-mode config update params deserialize the new field through existing `CodeModeConfig` flow.
- Modify `crates/lab/src/dispatch/gateway/code_mode.rs`: declare and re-export the new shape module/types inside the dispatch boundary.
- Create `crates/lab/src/dispatch/gateway/code_mode/shape.rs`: deterministic final-result shaping helper and metadata type.
- Modify `crates/lab/src/dispatch/gateway/code_mode/types.rs`: add compact optional shape metadata to `CodeModeExecutionResponse`.
- Modify `crates/lab/src/dispatch/gateway/code_mode/execute.rs`: produce a public shaped response and an internal raw/display outcome for snippets.
- Modify `crates/lab/src/dispatch/gateway/code_mode/trace.rs`: include shape metadata in structured content and keep `result` equal to the shaped response.
- Modify `crates/lab/src/mcp/call_tool_codemode.rs`: log shape metadata instead of inferring truncation only from `result.truncated`.
- Modify `crates/lab/src/dispatch/snippets/dispatch.rs`: compute `snippets.test` pass/fail from the pre-shape result while returning shaped display output.
- Modify tests in `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`, `crates/lab/src/mcp/call_tool_codemode/tests.rs`, and snippet tests near `crates/lab/src/dispatch/snippets.rs` or the existing snippets test module.
- Modify docs: `docs/dev/CODE_MODE.md`, `docs/services/GATEWAY.md`, `docs/runtime/CONFIG.md`, `docs/snippets/README.md`, `plugins/labby/skills/using-labby/references/code-mode.md`, `plugins/labby/skills/using-labby/references/config-reference.md`, `plugins/labby/skills/creating-snippets/SKILL.md`.

---

### Task 1: Config and Public Contract

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/dispatch/gateway/catalog.rs`
- Modify: `crates/lab/src/dispatch/gateway/params.rs`
- Test: existing config tests in `crates/lab/src/config.rs` or adjacent test module

**Interfaces:**
- Produces: `CodeModeResultShapePolicy` with serde strings `off` and `truncate`.
- Produces: `CodeModeConfig.result_shape_policy: CodeModeResultShapePolicy`.
- Consumed by Task 2 and Task 3.

- [ ] **Step 1: Add failing config/default tests**

Add tests that assert default off, TOML accepts truncate, and invalid strings fail:

```rust
#[test]
fn code_mode_result_shape_policy_defaults_to_off() {
    let config = CodeModeConfig::default();
    assert_eq!(config.result_shape_policy, CodeModeResultShapePolicy::Off);
}

#[test]
fn code_mode_result_shape_policy_parses_truncate() {
    let toml = r#"
        [gateway.code_mode]
        enabled = true
        result_shape_policy = "truncate"
    "#;
    let parsed: LabConfig = toml::from_str(toml).unwrap();
    assert_eq!(
        parsed.gateway.code_mode.result_shape_policy,
        CodeModeResultShapePolicy::Truncate
    );
}

#[test]
fn code_mode_result_shape_policy_rejects_unknown_value() {
    let toml = r#"
        [gateway.code_mode]
        result_shape_policy = "summarize"
    "#;
    let err = toml::from_str::<LabConfig>(toml).unwrap_err().to_string();
    assert!(err.contains("result_shape_policy"), "{err}");
}
```

- [ ] **Step 2: Run the focused tests and confirm failure**

Run:

```bash
cargo test -p labby --all-features code_mode_result_shape_policy
```

Expected: FAIL because `CodeModeResultShapePolicy` and `result_shape_policy` do not exist.

- [ ] **Step 3: Implement the config enum and field**

Add to `crates/lab/src/config.rs` near `CodeModeConfig`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeResultShapePolicy {
    #[default]
    Off,
    Truncate,
}
```

Add this field to `CodeModeConfig`:

```rust
/// Optional model-facing final-result shaping policy.
/// This never affects sandbox-visible callTool results.
#[serde(default)]
pub result_shape_policy: CodeModeResultShapePolicy,
```

Add the default value in `impl Default for CodeModeConfig`:

```rust
result_shape_policy: CodeModeResultShapePolicy::Off,
```

- [ ] **Step 4: Expose the setting in the gateway action catalog**

Add this `ParamSpec` to `gateway.code_mode.set` in `crates/lab/src/dispatch/gateway/catalog.rs`:

```rust
ParamSpec {
    name: "result_shape_policy",
    ty: "string",
    required: false,
    description: "Final-result shaping policy for completed Code Mode runs: off or truncate",
},
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p labby --all-features code_mode_result_shape_policy
```

Expected: PASS.

- [ ] **Step 6: Commit Task 1**

```bash
git add crates/lab/src/config.rs crates/lab/src/dispatch/gateway/catalog.rs crates/lab/src/dispatch/gateway/params.rs
git commit -m "feat: add code mode result shaping config"
```

### Task 2: Deterministic Shape Helper

**Files:**
- Create: `crates/lab/src/dispatch/gateway/code_mode/shape.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/types.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs`

**Interfaces:**
- Consumes: `CodeModeResultShapePolicy`.
- Produces: `shape_final_result(result, policy, max_response_bytes, max_response_tokens, token_estimate_divisor) -> ShapedResult`.
- Produces: `CodeModeResultShapeMetadata`.

- [ ] **Step 1: Add failing helper tests**

Add tests in `tests_runtime.rs`:

```rust
#[test]
fn shape_policy_off_preserves_result_exactly() {
    let input = Some(json!({"ok": true, "items": [1, 2]}));
    let shaped = shape_final_result(input.clone(), CodeModeResultShapePolicy::Off, 100, 100, 4);
    assert_eq!(shaped.result, input);
    assert_eq!(shaped.metadata.policy, CodeModeResultShapePolicy::Off);
    assert!(!shaped.metadata.changed);
}

#[test]
fn shape_policy_truncate_preserves_small_json() {
    let input = Some(json!({"ok": true}));
    let shaped = shape_final_result(input.clone(), CodeModeResultShapePolicy::Truncate, 4096, 1000, 4);
    assert_eq!(shaped.result, input);
    assert_eq!(shaped.metadata.policy, CodeModeResultShapePolicy::Truncate);
    assert!(!shaped.metadata.changed);
}

#[test]
fn shape_policy_truncate_stringifies_large_object() {
    let input = Some(json!({"payload": "x".repeat(5000)}));
    let shaped = shape_final_result(input, CodeModeResultShapePolicy::Truncate, 1400, 6000, 4);
    let result = shaped.result.unwrap();
    let text = result.as_str().expect("large shaped result is a marker string");
    assert!(text.contains("[code mode result truncated]"), "{text}");
    assert!(text.contains("\"payload\""), "{text}");
    assert!(shaped.metadata.changed);
    assert!(shaped.metadata.truncated);
    assert!(shaped.metadata.original_size_bytes > shaped.metadata.shaped_size_bytes);
}

#[test]
fn shape_policy_preserves_none_and_null_distinction() {
    let none = shape_final_result(None, CodeModeResultShapePolicy::Truncate, 100, 100, 4);
    assert!(none.result.is_none());

    let null = shape_final_result(Some(Value::Null), CodeModeResultShapePolicy::Truncate, 100, 100, 4);
    assert_eq!(null.result, Some(Value::Null));
}
```

- [ ] **Step 2: Run helper tests and confirm failure**

Run:

```bash
cargo test -p labby --all-features shape_policy
```

Expected: FAIL because the module and helper do not exist.

- [ ] **Step 3: Add the shape module**

Create `crates/lab/src/dispatch/gateway/code_mode/shape.rs`:

```rust
use serde::Serialize;
use serde_json::Value;

use crate::config::CodeModeResultShapePolicy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodeModeResultShapeMetadata {
    pub policy: CodeModeResultShapePolicy,
    pub changed: bool,
    pub truncated: bool,
    pub original_size_bytes: usize,
    pub shaped_size_bytes: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapedResult {
    pub result: Option<Value>,
    pub metadata: CodeModeResultShapeMetadata,
}

pub(in crate::dispatch::gateway::code_mode) fn shape_final_result(
    result: Option<Value>,
    policy: CodeModeResultShapePolicy,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> ShapedResult {
    let original_size = result
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .map(|bytes| bytes.len())
        .unwrap_or(0);

    match (policy, result) {
        (CodeModeResultShapePolicy::Off, result) => unchanged(result, policy, original_size),
        (CodeModeResultShapePolicy::Truncate, None) => unchanged(None, policy, original_size),
        (CodeModeResultShapePolicy::Truncate, Some(value)) => {
            shape_truncate(value, policy, original_size, max_response_bytes, max_response_tokens, token_estimate_divisor)
        }
    }
}

fn unchanged(
    result: Option<Value>,
    policy: CodeModeResultShapePolicy,
    original_size_bytes: usize,
) -> ShapedResult {
    ShapedResult {
        result,
        metadata: CodeModeResultShapeMetadata {
            policy,
            changed: false,
            truncated: false,
            original_size_bytes,
            shaped_size_bytes: original_size_bytes,
        },
    }
}

fn shape_truncate(
    value: Value,
    policy: CodeModeResultShapePolicy,
    original_size_bytes: usize,
    max_response_bytes: usize,
    max_response_tokens: usize,
    token_estimate_divisor: u32,
) -> ShapedResult {
    let token_budget_bytes = max_response_tokens
        .max(1)
        .saturating_mul(token_estimate_divisor.max(1) as usize);
    let budget = max_response_bytes.min(token_budget_bytes).max(256);
    if original_size_bytes <= budget {
        return unchanged(Some(value), policy, original_size_bytes);
    }

    let serialized = match &value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),
    };
    let marker_prefix = format!(
        "[code mode result truncated: original_size_bytes={original_size_bytes}, max_size_bytes={budget}]\n"
    );
    let room = budget.saturating_sub(marker_prefix.len()).max(0);
    let preview = serialized.chars().take(room).collect::<String>();
    let marker = format!("{marker_prefix}{preview}");
    let shaped_size_bytes = marker.len();

    ShapedResult {
        result: Some(Value::String(marker)),
        metadata: CodeModeResultShapeMetadata {
            policy,
            changed: true,
            truncated: true,
            original_size_bytes,
            shaped_size_bytes,
        },
    }
}
```

- [ ] **Step 4: Wire module exports and response metadata**

In `code_mode.rs`, add:

```rust
mod shape;
```

and re-export for tests:

```rust
pub(crate) use shape::{shape_final_result, CodeModeResultShapeMetadata, ShapedResult};
```

In `types.rs`, import the metadata type and add to `CodeModeExecutionResponse`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub result_shape: Option<CodeModeResultShapeMetadata>,
```

Update every test literal `CodeModeExecutionResponse { ... }` touched by compilation errors with:

```rust
result_shape: None,
```

- [ ] **Step 5: Run helper tests**

Run:

```bash
cargo test -p labby --all-features shape_policy
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs crates/lab/src/dispatch/gateway/code_mode/shape.rs crates/lab/src/dispatch/gateway/code_mode/types.rs crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs
git commit -m "feat: add code mode result shaping helper"
```

### Task 3: Broker and MCP Wiring

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/execute.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/trace.rs`
- Modify: `crates/lab/src/mcp/call_tool_codemode.rs`
- Test: `crates/lab/src/mcp/call_tool_codemode/tests.rs`

**Interfaces:**
- Consumes: `shape_final_result`.
- Produces: public shaped `CodeModeExecutionResponse`.
- Produces: metadata visible in response text and structured trace.

- [ ] **Step 1: Add failing regression tests**

Add one broker/MCP-oriented test that checks text and structured results agree after shaping. Use the existing helper style in `call_tool_codemode/tests.rs`; the assertion must verify:

```rust
assert_eq!(
    text_json.get("result"),
    structured_json.get("result"),
    "MCP text JSON and structuredContent must use the same shaped result"
);
assert_eq!(structured_json["result_shape"]["policy"], json!("truncate"));
assert_eq!(structured_json["result_shape"]["truncated"], json!(true));
```

Also add a unit-level test around `code_mode_execute_trace` if MCP harness setup is heavy:

```rust
#[test]
fn execute_trace_includes_shape_metadata_and_shaped_result() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        result: Some(json!("[code mode result truncated]\n{}")),
        result_shape: Some(CodeModeResultShapeMetadata {
            policy: CodeModeResultShapePolicy::Truncate,
            changed: true,
            truncated: true,
            original_size_bytes: 5000,
            shaped_size_bytes: 256,
        }),
        ui: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };
    let trace = code_mode_execute_trace(&response);
    assert_eq!(trace["result"], json!("[code mode result truncated]\n{}"));
    assert_eq!(trace["result_shape"]["policy"], json!("truncate"));
    assert_eq!(trace["result_shape"]["truncated"], json!(true));
}
```

- [ ] **Step 2: Run the focused test and confirm failure**

Run:

```bash
cargo test -p labby --all-features code_mode_execute_trace
```

Expected: FAIL because trace does not yet include shape metadata.

- [ ] **Step 3: Shape the response in `CodeModeBroker::execute`**

In `execute.rs`, after `apply_ui_opt_in(&mut response)` and before `response_within_budget`, add:

```rust
let shaped = shape_final_result(
    response.result.take(),
    config.result_shape_policy,
    config.max_response_bytes,
    config.max_response_tokens,
    config.token_estimate_divisor,
);
response.result = shaped.result;
response.result_shape = Some(shaped.metadata);
```

Add the import:

```rust
use super::shape::shape_final_result;
```

- [ ] **Step 4: Use shape metadata in trace**

In `trace.rs`, prefer `response.result_shape` over `compact_result_shape`:

```rust
if let Some(shape) = &response.result_shape {
    trace.insert(
        "result_shape".to_string(),
        serde_json::to_value(shape).unwrap_or_else(|_| json!({ "type": "unknown" })),
    );
} else {
    trace.insert(
        "result_shape".to_string(),
        response
            .result
            .as_ref()
            .map(compact_result_shape)
            .unwrap_or_else(|| json!({ "type": "undefined" })),
    );
}
```

- [ ] **Step 5: Log metadata instead of marker shape only**

In `call_tool_codemode.rs`, replace marker-only truncation detection with:

```rust
let truncated = response
    .result_shape
    .as_ref()
    .map(|shape| shape.truncated)
    .or_else(|| {
        response
            .result
            .as_ref()
            .and_then(|result| result.get("truncated"))
            .and_then(Value::as_bool)
    })
    .unwrap_or(false);
let result_shape_policy = response
    .result_shape
    .as_ref()
    .map(|shape| format!("{:?}", shape.policy))
    .unwrap_or_else(|| "legacy".to_string());
```

Add `result_shape_policy` to the tracing event.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p labby --all-features code_mode_execute_trace
cargo test -p labby --all-features call_tool_codemode
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

```bash
git add crates/lab/src/dispatch/gateway/code_mode/execute.rs crates/lab/src/dispatch/gateway/code_mode/trace.rs crates/lab/src/mcp/call_tool_codemode.rs crates/lab/src/mcp/call_tool_codemode/tests.rs
git commit -m "feat: shape code mode display responses"
```

### Task 4: Snippets Raw/Display Split

**Files:**
- Modify: `crates/lab/src/dispatch/snippets/dispatch.rs`
- Test: `crates/lab/src/dispatch/snippets.rs` or existing snippets tests

**Interfaces:**
- Produces: internal helper returning both raw and display response for snippets.
- Public `snippets.exec` still returns the shaped display response.
- Public `snippets.test` computes `passed` from raw response, returns display response.

- [ ] **Step 1: Add failing snippets tests**

Add a test that creates or uses a test snippet returning a large object with `ok: true`, enables truncate shaping, and verifies:

```rust
assert_eq!(test_result["passed"], json!(true));
assert!(test_result["response"]["result"].as_str().unwrap().contains("[code mode result truncated]"));
```

Add the negative case:

```rust
assert_eq!(test_result["passed"], json!(false));
assert!(test_result["response"]["result"].as_str().unwrap().contains("[code mode result truncated]"));
```

- [ ] **Step 2: Run snippets tests and confirm failure**

Run:

```bash
cargo test -p labby --all-features snippets
```

Expected: FAIL because `snippets.test` evaluates the shaped display response.

- [ ] **Step 3: Introduce an internal snippet execution outcome**

In `dispatch/snippets/dispatch.rs`, add:

```rust
struct SnippetExecutionOutcome {
    raw_response: CodeModeExecutionResponse,
    display_response: CodeModeExecutionResponse,
}
```

Add a broker API in Task 3 or here if needed:

```rust
pub(crate) async fn execute_with_raw_response(
    ...
) -> Result<CodeModeExecutionOutcome, CodeModeExecutionError>
```

The implementation should clone the raw response immediately after `apply_ui_opt_in()` and before `shape_final_result`.

- [ ] **Step 4: Update `snippets.exec` and `snippets.test`**

Keep `snippets.exec` display-only:

```rust
let outcome = execute_snippet_outcome(manager, &name, params.params).await?;
to_json(outcome.display_response)
```

Use raw for pass/fail in `snippets.test`:

```rust
let outcome = execute_snippet_outcome(manager, &name, params.params).await?;
let raw_value = serde_json::to_value(&outcome.raw_response)?;
let passed = snippet_response_passed(&raw_value);
to_json(json!({
    "name": name,
    "passed": passed,
    "response": outcome.display_response,
}))
```

Do the same inside `test_all_snippets`.

- [ ] **Step 5: Run snippets tests**

Run:

```bash
cargo test -p labby --all-features snippets
```

Expected: PASS.

- [ ] **Step 6: Commit Task 4**

```bash
git add crates/lab/src/dispatch/snippets/dispatch.rs crates/lab/src/dispatch/snippets.rs
git commit -m "fix: evaluate snippet tests before display shaping"
```

### Task 5: Documentation and Smoke Verification

**Files:**
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `docs/services/GATEWAY.md`
- Modify: `docs/runtime/CONFIG.md`
- Modify: `docs/snippets/README.md`
- Modify: `plugins/labby/skills/using-labby/references/code-mode.md`
- Modify: `plugins/labby/skills/using-labby/references/config-reference.md`
- Modify: `plugins/labby/skills/creating-snippets/SKILL.md`

**Interfaces:**
- Produces: documented config key `result_shape_policy`.
- Produces: documented ordering and caveats.

- [ ] **Step 1: Update Code Mode docs**

Add this contract text to Code Mode docs:

```markdown
### Final Result Shaping

Code Mode can optionally shape the final model-facing `result` of a successful execution. This is disabled by default.

Ordering:

1. The sandbox finishes and returns the raw final value.
2. Labby applies the existing `__ui` compatibility unwrap.
3. Labby applies the configured final-result shaping policy.
4. Labby applies the envelope budget truncation.
5. MCP text JSON and `structuredContent` are built from the same shaped response.

This does not change values seen by sandbox code through `callTool()` or `codemode.<upstream>.<tool>()`. It also does not add raw-result audit retention. Use `writeArtifact()` when a snippet needs to preserve a large detailed payload.

The `truncate` policy bounds model-facing output; it is not a redaction policy and must not be used to sanitize secrets.
```

- [ ] **Step 2: Update config docs**

Document:

```toml
[gateway.code_mode]
result_shape_policy = "off"      # off | truncate
```

- [ ] **Step 3: Update snippet docs**

Add:

```markdown
`snippets.test` evaluates pass/fail from the pre-shape result so `{ "ok": true }` remains reliable even when the displayed response is shaped. `snippets.exec` returns the same shaped display response as Code Mode when shaping is enabled.
```

- [ ] **Step 4: Run formatting and focused verification**

Run:

```bash
cargo fmt --all --check
cargo test -p labby --all-features shape_policy
cargo test -p labby --all-features code_mode_execute_trace
cargo test -p labby --all-features call_tool_codemode
cargo test -p labby --all-features snippets
cargo check --workspace --all-features
```

Expected: all PASS.

- [ ] **Step 5: Run final repo gate**

Run:

```bash
git diff --check
```

Expected: no whitespace errors.

- [ ] **Step 6: Commit Task 5**

```bash
git add docs/dev/CODE_MODE.md docs/services/GATEWAY.md docs/runtime/CONFIG.md docs/snippets/README.md plugins/labby/skills/using-labby/references/code-mode.md plugins/labby/skills/using-labby/references/config-reference.md plugins/labby/skills/creating-snippets/SKILL.md
git commit -m "docs: document code mode result shaping"
```

---

## Final Integration Checklist

- [ ] `bd show lab-iwda8 --json` confirms the epic and comments have been reviewed.
- [ ] `cargo fmt --all --check` passes.
- [ ] `cargo test -p labby --all-features shape_policy` passes.
- [ ] `cargo test -p labby --all-features code_mode_execute_trace` passes.
- [ ] `cargo test -p labby --all-features call_tool_codemode` passes.
- [ ] `cargo test -p labby --all-features snippets` passes.
- [ ] `cargo check --workspace --all-features` passes.
- [ ] `git diff --check` passes.
- [ ] Bead comments are updated with any implementation deviations.

## Self-Review

- Spec coverage: The plan covers all five beads: config/public semantics, helper implementation, broker/MCP wiring, snippets raw/display behavior, and docs/smoke validation.
- Placeholder scan: No forbidden placeholder phrases remain. Each code-changing task includes exact files, test intent, code skeletons, commands, and expected outcomes.
- Type consistency: `CodeModeResultShapePolicy`, `CodeModeResultShapeMetadata`, `ShapedResult`, and `shape_final_result` are introduced before later tasks consume them. The response metadata field is consistently named `result_shape`.
