# Code Mode v2 — Drop Lab-Action Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all Lab-action (`lab::<service>.<action>`) support from Code Mode in `crates/lab/src/dispatch/gateway/code_mode.rs` so the surface accepts only `upstream::<server>::<tool>` IDs, with a structured `unknown_tool` envelope that tells agents to use `tool_execute` for Lab actions instead.

**Architecture:** Deletion-heavy refactor over a single file (`code_mode.rs` ~1844 LOC) with light edits to `mcp/server.rs` dispatch handlers and `cli/gateway.rs`. The `CodeModeBroker::search/schema/execute` shape stays; only Lab-action code paths are removed. Stdio parent-broker protocol (`CodeModeRunnerInput`/`Output` enums) is untouched. Bead reference: `lab-elme3.1` under epic `lab-elme3`.

**Tech Stack:** Rust 2024, tokio, rmcp, serde, cargo-nextest. Crate `crates/lab` (binary `labby`) feature `code_mode` (always on; the JS-runner feature gate comes in bead 5a).

---

## File Structure

| File | Responsibility | Edit type |
|---|---|---|
| `crates/lab/src/dispatch/gateway/code_mode.rs` | Code Mode broker, ID parsing, runner protocol, schema construction, TS bindings | Heavy delete + edit |
| `crates/lab/src/mcp/server.rs` | MCP tool registration + dispatch for `code_search`/`code_schema`/`code_execute` | Light edit |
| `crates/lab/src/cli/gateway.rs` | CLI Code Mode subcommands (`gateway code search|schema|exec`) | Light edit (if any `lab::` branches exist) |
| `crates/lab/tests/code_mode_runner.rs` | Stdio protocol integration tests | NO CHANGE (protocol unchanged) |

Pre-implementation audit (Task 0): confirm we don't break any `destructive`-metadata enforcement elsewhere. The bead specifies removal of `CodeModeCaller::can_execute_action(destructive)` because Lab actions had destructive metadata; the audit verifies no other caller relies on this.

---

## Task 0: Pre-flight audit

**Files:**
- Read-only

- [ ] **Step 1: Branch off main**

Run:
```bash
cd /home/jmagar/workspace/lab
git checkout main && git pull --ff-only
git checkout -b code-mode-v2-drop-lab-actions
```

Expected: clean branch, no uncommitted changes.

- [ ] **Step 2: Capture baseline test inventory**

Run:
```bash
cargo nextest list -p labby --all-features 2>&1 | grep -E "code_mode|code_search|code_schema|code_execute" | tee /tmp/baseline-code-mode-tests.txt
```

Expected: a list of every Code Mode test currently registered. Keep this as the baseline so we can confirm which tests we explicitly delete and which we expect to keep passing.

- [ ] **Step 3: Grep for destructive-metadata callers**

Run:
```bash
rg -n "can_execute_action|destructive" crates/lab/src/dispatch/gateway/ crates/lab/src/mcp/ | tee /tmp/destructive-callers.txt
```

Expected: a list of every site that reads `destructive` metadata or calls `can_execute_action`. Confirm:
- `CodeModeCaller::can_execute_action` callers are ONLY inside `code_mode.rs::execute()` Lab-action dispatch path
- Upstream tool destructive-confirmation (the `params.confirm == true` gate) is enforced INSIDE `code_mode_call_upstream_tool()` or in the parent broker, not via `can_execute_action`. If it IS via `can_execute_action`, this task is wrong — STOP and revisit the plan.

- [ ] **Step 4: Snapshot the current file size**

Run:
```bash
wc -l crates/lab/src/dispatch/gateway/code_mode.rs
```

Expected: ~1844 lines. This is the baseline; we'll see ~600 LOC deletion by end of plan.

- [ ] **Step 5: Commit branch state**

```bash
git add -A
git commit --allow-empty -m "chore: branch baseline for code-mode-v2 drop-lab-actions"
```

---

## Task 1: Write failing test — `code_search` returns only upstream candidates

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs` (test module at bottom of file)

- [ ] **Step 1: Add the failing test**

Inside the existing `#[cfg(test)] mod tests { ... }` block at the bottom of `crates/lab/src/dispatch/gateway/code_mode.rs`, add:

```rust
#[tokio::test]
async fn code_search_returns_only_upstream_candidates() {
    let registry = completion_test_registry();
    let broker = CodeModeBroker::new(&registry, None);

    let results = broker
        .search("movie.search", 10, CodeModeCaller::TrustedLocal, CodeModeSurface::Cli)
        .await
        .expect("search ok");

    for candidate in &results {
        assert!(
            !candidate.id.starts_with("lab::"),
            "found lab:: candidate after drop: {}",
            candidate.id
        );
    }
}
```

(`completion_test_registry()` is an existing helper in the test module; if not, use the closest existing factory and adapt.)

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::code_search_returns_only_upstream_candidates
```

Expected: FAIL — results currently include built-in Lab action candidates from `search_builtin_candidates()`. The assertion fires on the first `lab::` candidate.

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "test: code_search returns only upstream candidates (failing)"
```

---

## Task 2: Delete `search_builtin_candidates` and wire `search()` to upstream-only

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs:348-396` (delete `search_builtin_candidates`)
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs:246-289` (rewire `CodeModeBroker::search`)

- [ ] **Step 1: Delete `search_builtin_candidates`**

Open `crates/lab/src/dispatch/gateway/code_mode.rs`, locate the function `search_builtin_candidates` (around lines 348-396). Delete the entire function body, including its `#[doc]` lines and any private helpers used only by it (`compare_code_mode_search_candidates` may also become unused — check with `rg`).

- [ ] **Step 2: Update `CodeModeBroker::search`**

Locate `impl<'a> CodeModeBroker<'a>` (around line 232). The current `search` method merges built-in + upstream candidates. Rewrite to upstream-only:

```rust
pub async fn search(
    &self,
    query: &str,
    top_k: usize,
    _caller: CodeModeCaller,
    _surface: CodeModeSurface,
) -> Result<Vec<CodeModeSearchCandidate>, ToolError> {
    let Some(manager) = self.gateway_manager else {
        return Ok(Vec::new());
    };

    let top_k = top_k.max(1).min(50);
    match manager.search_tools(query, top_k, true).await {
        Ok(upstream_results) => Ok(upstream_results
            .into_iter()
            .map(|r| {
                CodeModeSearchCandidate::upstream_tool(
                    &r.upstream,
                    &r.name,
                    &r.description,
                    r.score,
                    r.input_schema,
                )
            })
            .collect()),
        Err(err) => {
            // Preserve index_warming → empty-result fallback (was: builtin fallback)
            if err.kind() == "index_warming" {
                return Ok(Vec::new());
            }
            Err(err)
        }
    }
}
```

The `_caller` and `_surface` parameters become unused locally but are kept in the signature for forward compatibility with bead #2.

- [ ] **Step 3: Run the failing test — it should now pass**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::code_search_returns_only_upstream_candidates
```

Expected: PASS.

- [ ] **Step 4: Run the full Code Mode test suite to see fallout**

Run:
```bash
cargo nextest run -p labby --all-features code_mode 2>&1 | tee /tmp/run-after-task2.txt
```

Expected: some tests that referenced `search_builtin_candidates` or `lab::` IDs now fail to compile. We'll delete those in Task 3.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "feat(code-mode): drop search_builtin_candidates; code_search upstream-only"
```

---

## Task 3: Delete `lab::` ID parsing branch

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs:39-85` (`CodeModeToolId::parse`)
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs:17-27` (`CodeModeToolRef` enum)
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs:88-90` (`lab_action_id` helper)

- [ ] **Step 1: Write the failing test for `lab::` rejection**

Add to the test module:

```rust
#[test]
fn parse_rejects_lab_action_id() {
    let err = CodeModeToolId::parse("lab::radarr.movie.search")
        .expect_err("lab:: ids should be rejected");
    match err {
        ToolError::Sdk { sdk_kind, .. } => {
            assert_eq!(sdk_kind, "invalid_code_mode_id");
        }
        other => panic!("expected invalid_code_mode_id, got {other:?}"),
    }
}
```

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::parse_rejects_lab_action_id
```

Expected: FAIL — current parser accepts `lab::`.

- [ ] **Step 2: Simplify `CodeModeToolRef` enum**

Locate the enum (around line 17). Replace with the upstream-only variant:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeToolRef {
    UpstreamTool { upstream: String, tool: String },
}
```

(Single-variant enum is fine; bead #2 may remove it entirely or keep for future-proofing.)

- [ ] **Step 3: Simplify `CodeModeToolId::parse`**

Replace the function body (lines 39-85) with:

```rust
impl CodeModeToolId {
    pub fn parse(raw: &str) -> Result<Self, ToolError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        // lab:: ids are no longer supported; emit unknown_tool with hint
        if raw.starts_with("lab::") {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "lab:: IDs are not supported by Code Mode".to_string(),
            });
        }

        if let Some(rest) = raw.strip_prefix("upstream::") {
            let (upstream, tool) = rest.split_once("::").ok_or_else(|| {
                invalid_code_mode_id("upstream Code Mode ids must use upstream::<upstream>::<tool>")
            })?;
            if upstream.trim().is_empty() || tool.trim().is_empty() {
                return Err(invalid_code_mode_id(
                    "upstream Code Mode ids must include upstream and tool",
                ));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: upstream.trim().to_string(),
                    tool: tool.trim().to_string(),
                },
            });
        }

        Err(invalid_code_mode_id(
            "Code Mode ids must start with upstream::",
        ))
    }
}
```

Note: `lab::` returns `unknown_tool` (not `invalid_code_mode_id`) because that's the agent-actionable envelope per the bead's locked decision. Update the test from Step 1 accordingly:

```rust
#[test]
fn parse_rejects_lab_action_id() {
    let err = CodeModeToolId::parse("lab::radarr.movie.search")
        .expect_err("lab:: ids should be rejected");
    match err {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "unknown_tool");
            assert!(message.contains("lab::"));
        }
        other => panic!("expected unknown_tool, got {other:?}"),
    }
}
```

- [ ] **Step 4: Delete `lab_action_id` helper**

Locate and delete the `lab_action_id` free function (around line 88-90). `upstream_tool_id` stays — still used.

- [ ] **Step 5: Delete `CodeModeSearchCandidate::lab_action` constructor**

Locate the `impl CodeModeSearchCandidate` block (around line 104). Delete the `lab_action` constructor; keep `upstream_tool`.

- [ ] **Step 6: Run the parser test**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::parse_rejects_lab_action_id
```

Expected: PASS.

- [ ] **Step 7: Run all parser tests**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::parses_
```

Expected: `parses_lab_action_id` test (existing) FAILS — delete it in Task 4. `parses_upstream_tool_id` PASSES.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "feat(code-mode): reject lab:: IDs in parse; emit unknown_tool envelope"
```

---

## Task 4: Delete Lab-action paths in `schema()` and `execute()`

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs` — `CodeModeBroker::schema`, `code_mode_call_lab_action`, `code_mode_schema_for_lab_action`, `action_input_schema`, `typescript_binding` (Lab-action call site)

- [ ] **Step 1: Write failing test — schema rejects `lab::` ID**

Add to test module:

```rust
#[tokio::test]
async fn schema_rejects_lab_action_id() {
    let registry = completion_test_registry();
    let broker = CodeModeBroker::new(&registry, None);

    let err = broker
        .schema("lab::radarr.movie.search", CodeModeCaller::TrustedLocal, CodeModeSurface::Cli)
        .await
        .expect_err("schema should reject lab:: id");

    match err {
        ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "unknown_tool"),
        other => panic!("expected unknown_tool, got {other:?}"),
    }
}
```

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::schema_rejects_lab_action_id
```

Expected: FAIL — schema() currently dispatches `LabAction` to `code_mode_schema_for_lab_action`.

- [ ] **Step 2: Simplify `CodeModeBroker::schema`**

Locate `schema` method on the broker (around line 291). Rewrite:

```rust
pub async fn schema(
    &self,
    id: &str,
    _caller: CodeModeCaller,
    _surface: CodeModeSurface,
) -> Result<CodeModeSchemaResponse, ToolError> {
    let parsed = CodeModeToolId::parse(id)?;
    let Some(manager) = self.gateway_manager else {
        return Err(ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: "no gateway manager configured".to_string(),
        });
    };
    match parsed.reference {
        CodeModeToolRef::UpstreamTool { upstream, tool } => {
            self.schema_for_upstream_tool(manager, &upstream, &tool).await
        }
    }
}
```

Note: with the parser rejecting `lab::` IDs upstream, the `match` is exhaustive over the now-single variant.

- [ ] **Step 3: Delete Lab-action helpers**

Search and delete:
- `code_mode_schema_for_lab_action` (around lines 980-998)
- `code_mode_call_lab_action` (around lines 730-770 — the dispatch path)
- `action_input_schema` (around lines 1361-1390 — `ActionSpec` → JSON Schema projection)
- `typescript_binding` (around lines 1435-1443 — keep ONLY if used by upstream; otherwise delete with `typescript_type`/`object_typescript_type`/`typescript_property_name`)

Run:
```bash
rg -n "typescript_binding|action_input_schema|code_mode_schema_for_lab_action|code_mode_call_lab_action" crates/lab/src/
```

Expected: zero remaining matches in `code_mode.rs` (test references aside — handled in next step).

- [ ] **Step 4: Run the schema test**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::schema_rejects_lab_action_id
```

Expected: PASS.

- [ ] **Step 5: Update `CodeModeBroker::execute` `callTool` dispatch**

Locate the parent-broker dispatch helper that routes a `callTool` `id` to either Lab dispatch or upstream MCP (`code_mode_call_tool_id` around line 697). Simplify to upstream-only:

```rust
async fn call_tool_id(&self, id: &str, params: Value) -> Result<Value, ToolError> {
    let parsed = CodeModeToolId::parse(id)?;
    let Some(manager) = self.gateway_manager else {
        return Err(ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: "no gateway manager configured".to_string(),
        });
    };
    match parsed.reference {
        CodeModeToolRef::UpstreamTool { upstream, tool } => {
            self.call_upstream_tool(manager, &upstream, &tool, params).await
        }
    }
}
```

The `lab::` rejection happens inside `CodeModeToolId::parse` — agent's JS `callTool` sees a structured error.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "feat(code-mode): drop Lab-action paths in schema/execute"
```

---

## Task 5: Simplify policy types — drop `expose_builtin_services` and `can_execute_action`

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs` — `CodeModeSurface`, `CodeModeCaller`

- [ ] **Step 1: Update `CodeModeSurface` enum**

Locate the `CodeModeSurface` enum (search `pub enum CodeModeSurface`). Remove `expose_builtin_services`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeModeSurface {
    Mcp { allow_destructive_actions: bool },
    Cli,
}
```

- [ ] **Step 2: Delete `CodeModeCaller::can_execute_action`**

Locate `impl CodeModeCaller` (around line 70). Delete the `can_execute_action` method. Keep `can_read`, `can_execute`, `subject`.

- [ ] **Step 3: Fix call sites in `mcp/server.rs`**

Run:
```bash
rg -n "expose_builtin_services|can_execute_action" crates/lab/src/
```

Expected: matches in `crates/lab/src/mcp/server.rs` (the code_search/code_schema/code_execute dispatch handlers around lines 1300-1640).

For each match in `mcp/server.rs`:
- In `CodeModeSurface::Mcp { ... }` constructions, remove the `expose_builtin_services: false` (or true) field. Result: `CodeModeSurface::Mcp { allow_destructive_actions: <existing value> }`.
- Remove any `if !caller.can_execute_action(...)` branches; the upstream confirm flag check (`params.confirm == true`) lives inside `code_mode_call_upstream_tool` and is the single remaining gate.

- [ ] **Step 4: Fix call sites in `cli/gateway.rs`**

Run:
```bash
rg -n "expose_builtin_services|can_execute_action|CodeModeSurface" crates/lab/src/cli/
```

Expected: matches in `cli/gateway.rs`. Same fix: drop `expose_builtin_services` field.

- [ ] **Step 5: Compile**

Run:
```bash
cargo check --manifest-path crates/lab/Cargo.toml --all-features 2>&1 | tail -20
```

Expected: clean compile. If any errors remain, they're cleanup misses — fix in place.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs crates/lab/src/mcp/server.rs crates/lab/src/cli/gateway.rs
git commit -m "refactor(code-mode): drop expose_builtin_services field and can_execute_action method"
```

---

## Task 6: MCP dispatch handlers — remove built-in candidate merging + update descriptions

**Files:**
- Modify: `crates/lab/src/mcp/server.rs:1300-1431` (code_search dispatch)
- Modify: `crates/lab/src/mcp/server.rs:1119-1167` (code_search + code_schema tool descriptions)

- [ ] **Step 1: Read current code_search dispatch**

Run:
```bash
sed -n '1300,1450p' crates/lab/src/mcp/server.rs | head -150
```

- [ ] **Step 2: Remove built-in merge in code_search dispatch**

In `crates/lab/src/mcp/server.rs` around lines 1391-1410, the current code calls `self.search_builtin_code_mode_candidates(...)` and merges with upstream results. Since the broker now returns upstream-only and `search_builtin_code_mode_candidates` is gone (deleted in Task 2), this code is dead — remove the merge logic and call the broker directly.

Replace the call_tool branch for `CODE_SEARCH_TOOL_NAME` with a direct `CodeModeBroker::new(&self.registry, self.gateway_manager.as_deref()).search(...)` and serialize the result.

- [ ] **Step 3: Update tool descriptions**

In `crates/lab/src/mcp/server.rs` around line 1140-1145 (`code_search` Tool::new) and around line 1163-1166 (`code_schema` Tool::new), update the wording:

Before:
```text
Schema-first Code Mode discovery for Lab and proxied upstream tools.
```

After:
```text
Schema-first Code Mode discovery for proxied upstream MCP tools.
```

For `code_schema`:
Before:
```text
Lab ids return the ActionSpec-derived action contract; upstream ids return the upstream JSON Schema exposed by the gateway.
```

After:
```text
Returns the upstream JSON Schema exposed by the gateway for a given upstream:: tool id.
```

- [ ] **Step 4: Run the MCP integration tests**

Run:
```bash
cargo nextest run -p labby --all-features mcp::server 2>&1 | tail -20
```

Expected: all green, or only failures explicitly tied to Lab-action paths (those will be deleted in Task 7).

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/mcp/server.rs
git commit -m "refactor(mcp): code_search/code_schema dispatch upstream-only; description wording"
```

---

## Task 7: Delete stale unit tests + add the rest of the new coverage

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs` — test module (around lines 1527-1843)

- [ ] **Step 1: Inventory remaining stale tests**

Run:
```bash
rg -n "fn .*lab_action|fn .*builtin|fn .*action_input_schema|fn .*typescript_binding" crates/lab/src/dispatch/gateway/code_mode.rs
```

Expected: a handful of test functions targeting deleted code. List them in `/tmp/stale-tests.txt`.

- [ ] **Step 2: Delete the stale tests**

For each test in the list, delete the function. Common ones:
- `parses_lab_action_id` — replaced by `parse_rejects_lab_action_id` from Task 3
- `builds_search_candidate_for_lab_action` — `CodeModeSearchCandidate::lab_action` no longer exists
- `builds_lab_schema_response` — `CodeModeSchemaResponse::lab_action` may still exist (used in tests only) — KEEP for now until bead #2 deletes `CodeModeSchemaResponse` entirely
- `builds_action_input_schema_and_typescript_binding` — `action_input_schema` gone
- `search_expands_builtin_matches_to_action_candidates` — `search_builtin_candidates` gone
- `execute_strips_confirm_before_dispatch` — KEEP if it tests upstream confirm stripping; DELETE if Lab-action-only

- [ ] **Step 3: Add `code_execute_callTool_lab_id_returns_unknown_tool`**

Add to the test module:

```rust
#[tokio::test]
async fn code_execute_callTool_lab_id_returns_unknown_tool() {
    let registry = completion_test_registry();
    let broker = CodeModeBroker::new(&registry, None);

    let response = broker
        .execute(
            r#"await callTool("lab::radarr.movie.search", {query:"Matrix"})"#,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            crate::config::CodeModeConfig {
                enabled: true,
                timeout_ms: 5_000,
                max_tool_calls: 2,
            },
        )
        .await
        .expect("execute returns response, even with failed inner call");

    let first = response.calls.first().expect("one call recorded");
    let kind = first
        .result
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| first.result.pointer("/error/kind").and_then(Value::as_str));
    assert_eq!(kind, Some("unknown_tool"));
}
```

- [ ] **Step 4: Run the new tests**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests
```

Expected: ALL PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "test(code-mode): delete Lab-action tests; add unknown_tool envelope coverage"
```

---

## Task 8: Update the `unknown_tool` hint to the expanded form (research-derived)

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs` — the `unknown_tool` ToolError construction(s) where `lab::` IDs are rejected

- [ ] **Step 1: Locate every `unknown_tool` construction in `code_mode.rs` for `lab::` rejection**

Run:
```bash
rg -n 'sdk_kind.*"unknown_tool"\|"lab::"' crates/lab/src/dispatch/gateway/code_mode.rs
```

Expected: two or three sites (in `parse`, in `schema`, in `call_tool_id` if applicable).

- [ ] **Step 2: Define a single hint constant**

Add near the top of the file (below the existing module docs):

```rust
const LAB_ACTION_UNKNOWN_TOOL_HINT: &str =
    "Code Mode handles upstream MCP tools only. For Lab actions, use the `tool_execute` MCP tool: \
     name=<service> (e.g. \"radarr\"), arguments={action: \"<dotted.action>\", params: {...}}. \
     Example: tool_execute(name=\"radarr\", arguments={action:\"movie.search\", params:{query:\"Matrix\"}}).";
```

- [ ] **Step 3: Use the constant in all `lab::` rejection sites**

For each `ToolError::Sdk { sdk_kind: "unknown_tool", message: ... }` construction in `code_mode.rs` that fires on `lab::` input, refactor to include the hint. Since `ToolError::Sdk` doesn't have a `hint` field, the hint is appended to the message:

```rust
return Err(ToolError::Sdk {
    sdk_kind: "unknown_tool".to_string(),
    message: format!("lab:: IDs are not supported by Code Mode. {LAB_ACTION_UNKNOWN_TOOL_HINT}"),
});
```

- [ ] **Step 4: Update the hint test assertion**

In `parse_rejects_lab_action_id` (and `schema_rejects_lab_action_id`, and `code_execute_callTool_lab_id_returns_unknown_tool`), add:

```rust
assert!(message.contains("tool_execute"));
assert!(message.contains("\"radarr\""));
```

- [ ] **Step 5: Run all rejection tests**

Run:
```bash
cargo nextest run -p labby --all-features dispatch::gateway::code_mode::tests::parse_rejects_lab_action_id dispatch::gateway::code_mode::tests::schema_rejects_lab_action_id dispatch::gateway::code_mode::tests::code_execute_callTool_lab_id_returns_unknown_tool
```

Expected: ALL PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "feat(code-mode): expand unknown_tool hint with tool_execute mechanical example"
```

---

## Task 9: Final greps + full-suite verification

**Files:**
- Read-only verification

- [ ] **Step 1: Confirm zero lab:: code remains**

Run:
```bash
rg "lab::|LabAction|lab_action_id|action_input_schema|search_builtin_candidates" crates/lab/src/dispatch/gateway/code_mode.rs
```

Expected: zero matches (except inside the `unknown_tool` message string `"lab:: IDs are not supported"` and the hint constant — those are intentional).

- [ ] **Step 2: Confirm zero stale references elsewhere**

Run:
```bash
rg "LabAction|lab_action_id|action_input_schema|search_builtin_candidates|expose_builtin_services|can_execute_action" crates/lab/src/
```

Expected: zero matches.

- [ ] **Step 3: Confirm `typescript_binding` Lab-action helpers gone**

Run:
```bash
rg "fn typescript_binding|fn typescript_type|fn object_typescript_type|fn typescript_property_name" crates/lab/src/
```

Expected: zero matches (these were only used by Lab-action schema response construction).

- [ ] **Step 4: Confirm LOC reduction**

Run:
```bash
wc -l crates/lab/src/dispatch/gateway/code_mode.rs
```

Expected: ~1200-1400 lines (down from ~1844). Confirms ~400-600 LOC deletion.

- [ ] **Step 5: Full Code Mode test suite**

Run:
```bash
cargo nextest run -p labby --all-features code_mode
```

Expected: all green.

- [ ] **Step 6: Stdio protocol integration tests**

Run:
```bash
cargo nextest run -p labby --all-features --test code_mode_runner
```

Expected: ALL existing tests pass unchanged (stdio protocol is untouched).

- [ ] **Step 7: Full workspace test**

Run:
```bash
cargo nextest run --workspace --all-features
```

Expected: all green.

- [ ] **Step 8: Clippy + fmt**

Run:
```bash
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --all -- --check
```

Expected: both clean.

- [ ] **Step 9: Final commit (if any cleanup remains)**

If any cleanup happened in Steps 1-8, commit it:

```bash
git add -A
git commit -m "chore(code-mode): final cleanup after lab-action drop"
```

---

## Task 10: Live verification + PR

**Files:**
- Read-only verification against a running gateway

- [ ] **Step 1: Build release binary**

Run:
```bash
cargo build --release --all-features --bin labby
```

Expected: compiles.

- [ ] **Step 2: Live test against the local gateway**

Assuming a running `labby serve` instance (e.g. `lab-prod` at `https://lab.tootie.tv/mcp`):

Run:
```bash
LAB_MCP_HTTP_TOKEN=$(grep '^LAB_MCP_HTTP_TOKEN=' ~/.lab/.env | cut -d= -f2-) \
  mcporter call lab-prod.invoke name=code_search arguments:='{"query":"docker container logs","top_k":5}' 2>&1 | jq '.[].id' | head -10
```

Expected: only `upstream::*` IDs. Zero `lab::*` IDs.

- [ ] **Step 3: Live test of the unknown_tool hint**

Run:
```bash
LAB_MCP_HTTP_TOKEN=$(...) mcporter call lab-prod.invoke name=code_schema arguments:='{"id":"lab::radarr.movie.search"}' 2>&1
```

Expected: structured `unknown_tool` envelope with the expanded hint mentioning `tool_execute`.

- [ ] **Step 4: Push and open PR**

```bash
git push -u origin code-mode-v2-drop-lab-actions
gh pr create --title "feat(code-mode): drop Lab-action support (lab-elme3.1)" --body "$(cat <<'EOF'
## Summary
- Removes `lab::<service>.<action>` ID support from Code Mode entirely
- Only `upstream::<server>::<tool>` IDs accepted
- Returns structured `unknown_tool` envelope with mechanical `tool_execute` example for any `lab::` caller
- Drops `expose_builtin_services` field from `CodeModeSurface::Mcp` and `can_execute_action` policy method
- ~400-600 LOC deletion from `crates/lab/src/dispatch/gateway/code_mode.rs`

Bead: lab-elme3.1 (part of epic lab-elme3 — Code Mode v2 refactor).

## Test plan
- [ ] `cargo nextest run --workspace --all-features` passes locally
- [ ] `cargo clippy --workspace --all-features -- -D warnings` clean
- [ ] Live: `mcporter call lab-prod.invoke name=code_search ...` returns only upstream IDs
- [ ] Live: `mcporter call lab-prod.invoke name=code_schema arguments:='{"id":"lab::..."}'` returns `unknown_tool` envelope with hint
- [ ] Stdio protocol tests (`crates/lab/tests/code_mode_runner.rs`) pass unchanged
EOF
)"
```

- [ ] **Step 5: Close the bead**

Run:
```bash
bd update lab-elme3.1 --status=completed
bd comments add lab-elme3.1 "Closed via PR #<num>; epic lab-elme3 chain continues at lab-elme3.2."
```

---

## Self-Review

**Spec coverage:** Every acceptance criterion from `bd show lab-elme3.1` (Testing + Validation sections) maps to a task:
- `code_search_returns_only_upstream_candidates` → Task 1+2
- `code_schema_rejects_lab_action_id` → Task 4
- `code_execute_callTool_lab_id_returns_unknown_tool` → Task 7
- `parse_rejects_lab_action_id` → Task 3
- Hint expansion → Task 8
- Final greps → Task 9
- Stdio protocol unchanged → confirmed in Task 9 Step 6
- Live tests → Task 10

**Placeholder scan:** all code blocks have full content. The one approximation: `completion_test_registry()` helper may be named differently in the current test module — that's an inspection moment for the implementer, not a placeholder.

**Type consistency:** `CodeModeToolRef` becomes single-variant; matches in `schema/execute/parse` are exhaustive. `CodeModeSurface::Mcp { allow_destructive_actions: bool }` is consistent across files (`code_mode.rs`, `mcp/server.rs`, `cli/gateway.rs` all updated together in Task 5). `LAB_ACTION_UNKNOWN_TOOL_HINT` is defined once and reused in three sites (Task 8).
