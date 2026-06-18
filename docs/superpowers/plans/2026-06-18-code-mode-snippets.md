# Code Mode Snippets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Lab Code Mode snippet-aware: snippets appear in `codemode.search()` and `codemode.describe()`, sandbox code can call `codemode.run(name, input)`, runtime errors give better agent-facing hints, and prior Code Mode executions can be promoted into reusable snippets.

**Architecture:** Keep the public Code Mode surface as one MCP tool. Add snippets to the sandbox discovery catalog as metadata only, then lazy-resolve snippet source through the runner protocol when `codemode.run()` is called. This preserves the Javy/QuickJS sandbox and avoids injecting every snippet's JavaScript into every run.

**Tech Stack:** Rust 2024, Tokio, rmcp, serde/serde_json, Javy/QuickJS runner subprocess, existing `dispatch::snippets` store, existing gateway Code Mode broker/pool/history.

## Global Constraints

- Preserve the single public `codemode` MCP tool. Do not restore public `search` or `execute` tools.
- Keep using the existing Javy/QuickJS runner path. Do not replace it with Node, Deno, Boa-only execution, or host-side JavaScript execution.
- Do not inject snippet source into the startup proxy. Search/describe may include compact snippet metadata, but executable snippet code must be fetched lazily when `codemode.run()` is called.
- `codemode.run()` must execute snippet JavaScript inside the same sandbox runtime as the caller so snippets can compose with `await`, `codemode.<upstream>.<tool>()`, `callTool()`, `writeArtifact()`, and other sandbox helpers.
- Host-side action permissioning remains host-side. Snippets must not bypass the existing tool dispatch filters, destructive gates, route scope, or caller/surface handling.
- Snippet execution through `codemode.run()` uses the same authorization posture as existing snippet execution: admin/trusted-local only unless the existing snippet service policy is deliberately changed in code and tests. Non-admin or read-scoped Code Mode callers must not see user snippet metadata and must receive a structured authorization error from `codemode.run()`.
- Snippet visibility is route/caller scoped. Protected Code Mode routes must not discover or resolve snippets outside their route scope/actor policy.
- Snippet names must use the existing `dispatch::snippets::store::validate_snippet_name` validation. Do not add a parallel name validator.
- Promotion stores raw Code Mode source only in a bounded in-memory source store keyed by execution id. It is best-effort and live-gateway scoped: source is not available after restart, deploy, process hop, or retention eviction. Do not expose source in public history resources by default.
- Promotion is admin-only and destructive because it writes executable plaintext snippet files. It must require normal destructive confirmation/elicitation, and overwrite or builtin-shadow behavior must require explicit intent.
- Preserve dirty-worktree hygiene. Only touch Code Mode, snippet, docs, and tests needed for this feature.
- Baseline from the new worktree: `cargo check --workspace --all-features` passes. Existing warning: `apps/gateway-admin/out not found -- embedding empty web assets`.
- Engineering review amendments folded into this plan: no standalone cross-process CLI promotion, route/actor-bound source lookup, shared discovery projection, strict snippet listing failures, snippet recursion/count/byte budgets, `spawn_blocking` filesystem access, metadata caching, no structured-error hint corruption, and atomic promoted snippet writes.

---

## Task 1: Add snippet metadata to the Code Mode discovery catalog

- [ ] Extend `crates/lab/src/dispatch/gateway/code_mode/types.rs`.
  - Add a serializable kind field to `CodeModeCatalogEntry`, for example:
    ```rust
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum CodeModeCatalogKind {
        Tool,
        Snippet,
    }
    ```
  - Add `kind: CodeModeCatalogKind` to `CodeModeCatalogEntry`.
  - Add compact snippet-only metadata fields where needed, for example `tags: Vec<String>` and `inputs: Vec<SnippetInputSpec>`, or a small `SnippetDiscoveryMeta` field. Do not add per-snippet `dts`; `codemode.run(name, input?)` is one global helper.
  - Keep `CodeModeCatalogEntry::upstream_tool(...)` setting `kind: Tool`.
  - Add `CodeModeCatalogEntry::snippet(info: &SnippetInfo) -> Self` with:
    - `id = format!("snippet::{}", info.name)`
    - `name = info.name.clone()`
    - `upstream = "snippet".to_string()`
    - `description = info.description.clone().unwrap_or_else(...)`
    - `schema = Some(json!({ "type": "object", "properties": ... }))` built from `info.inputs`
    - `output_schema = None`
    - `signature = format!("codemode.run({:?}, input?)", info.name)`
    - `tags` and `inputs` copied from snippet metadata for search/describe.
  - Add the same `kind` to `CodeModeDiscoveryEntry`, using `"tool"` and `"snippet"` in serialized JavaScript.
  - Add a single shared projection helper, for example `CodeModeDiscoveryEntry::from_catalog(entry: &CodeModeCatalogEntry)`, so `search_allowed` and `build_code_mode_proxy` cannot drift.

- [ ] Load snippets into `crates/lab/src/dispatch/gateway/code_mode/search.rs`.
  - In `code_mode_catalog_allowed(...)`, obtain Lab home/builtin snippet dir through existing config/store helpers.
  - Add a gateway-manager-level snippet metadata cache keyed by a cheap directory snapshot for user and builtin snippet dirs: `(dir_path, dir_mtime, dir_len, visible_route_scope/actor policy version)`.
  - Compute the cheap directory fingerprint first. Only call `dispatch::snippets::store::list_snippets(...)` on cache miss.
  - Run filesystem directory scans and snippet metadata parsing under `tokio::task::spawn_blocking`; do not block Tokio workers on per-file reads.
  - Continue silently only when snippet directories are absent. Permission errors, unreadable files, and parse failures that the store treats as fatal must return a structured error rather than hiding snippets.
  - Apply caller/route visibility rules before appending snippets. Non-admin/read-scoped callers should not receive user snippet entries unless existing snippet service policy explicitly allows them.
  - Append `CodeModeCatalogEntry::snippet(&info)` to the catalog after upstream tools.
  - Include the cheap snippet directory fingerprint and snippet visibility policy fingerprint in the cached render fingerprint. Do not include snippet code.
  - Sort catalog entries by `(kind, upstream, name)` for deterministic output.

- [ ] Update `crates/lab/src/dispatch/gateway/code_mode/execute.rs`.
  - When converting `CodeModeCatalogEntry` to `CodeModeDiscoveryEntry`:
    - For tools, keep current `path = "<upstream>.<name>"` and helper `codemode.<namespace>.<tool>()`.
    - For snippets, use `path = format!("snippet.{}", entry.name)`, helper `codemode.run("<name>", input)`, and kind `Snippet`.
  - Ensure snippet pseudo-upstream does not create `codemode.snippet.<name>()` namespace helpers.
  - Use the shared projection helper from `types.rs` rather than duplicating the conversion.

- [ ] Update `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`.
  - Add `"run"` to `CODEMODE_TOP_LEVEL_RESERVED`.
  - Make `codemode.search(query, options?)` return `kind` for every result.
  - Make snippet results rank on name, description, tags, path, and `codemode.run`.
  - Make `codemode.describe("snippet.<name>")`, `codemode.describe("snippet::<name>")`, and `codemode.describe("<name>")` work with deterministic collision rules:
    - exact `id`, `path`, and `helper` matches win.
    - bare `<name>` is allowed only when exactly one entry across tools and snippets has that name.
    - ambiguous matches return `ambiguous_target` with all valid targets.
  - For snippets, render concise markdown containing:
    - `Kind: snippet`
    - `Name`
    - `Description`
    - `Run: codemode.run("<name>", input)`
    - input fields and defaults from `SnippetInputSpec`
  - Do not render full snippet source in describe output.

- [ ] Add focused tests.
  - `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`:
    - search returns snippet entries with `kind: "snippet"`.
    - describe renders `codemode.run("<name>", input)`.
    - ambiguous bare names still return `ambiguous_target`.
  - `crates/lab/src/dispatch/gateway/code_mode/search.rs` or broker tests:
    - catalog render includes snippets and tools together.
    - fingerprint changes when snippet metadata changes.
    - cached lookup does not reread every snippet file when the directory fingerprint is unchanged.
    - non-admin/read-scoped route cannot see user snippets.
    - `codemode.search("snippet")` and `codemode.describe("snippet.foo")` work inside a normal `codemode` execution, not only in isolated discovery tests.

---

## Task 2: Implement lazy in-sandbox `codemode.run(name, input)`

- [ ] Extend `crates/lab/src/dispatch/gateway/code_mode/protocol.rs`.
  - Add runner output:
    ```rust
    SnippetResolve {
        seq: u64,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    }
    ```
  - Add runner input:
    ```rust
    SnippetResolved {
        seq: u64,
        code: String,
        input: serde_json::Value,
    }
    ```
  - Reuse existing `ToolError { seq, kind, message }` to reject snippet resolution failures.
  - Add a typed pending-operation category in runner state (`tool`, `artifact`, `snippet`) or rename/generalize the current tool-call settlement path. Do not overload `__labSettleToolCall` semantics invisibly.

- [ ] Add JavaScript glue in `crates/lab/src/dispatch/gateway/code_mode/runner.rs`.
  - Add `globalThis.__labRunSnippet = (name, input = {}) => Promise`.
  - Validate `name` is a string before emitting to the host; reject with a JSON-like `bad_snippet_name` error when not.
  - Emit `CodeModeRunnerOutput::SnippetResolve { seq, name, input }`.
  - Add a settlement function for `SnippetResolved` that:
    - receives `{ code, input }`
    - evaluates and invokes the validated snippet using the same contract as existing snippet execution: `return await (${code})(input)`.
    - resolves/rejects the original `codemode.run` promise with the snippet result/error.
  - Keep `callTool`, `writeArtifact`, and snippet execution promises sharing the same pending-promise machinery.
  - Track snippet call stack and counters in the runner runtime. Return structured errors for:
    - `snippet_recursion_limit` or `snippet_depth_exceeded`
    - `snippet_resolve_limit`
    - `snippet_budget_exceeded`

- [ ] Add `codemode.run` to the generated proxy in `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`.
  - Define it before upstream namespace helpers:
    ```js
    codemode.run = (name, input = {}) => globalThis.__labRunSnippet(name, input);
    ```
  - Keep `codemode.search`, `codemode.describe`, and `codemode.step` unchanged except for reserved-name handling.

- [ ] Resolve snippets host-side in `crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs`.
  - Add a `SnippetResolve` arm in the drive loop.
  - Implement a helper like:
    ```rust
    async fn handle_snippet_resolve(
        broker: &CodeModeBroker<'_>,
        stdin: &mut ChildStdin,
        seq: u64,
        name: String,
        input: Value,
    ) -> Result<(), CodeModeExecutionError>
    ```
  - Use existing snippet store functions:
    - `resolve_snippet(lab_home, builtin_dir, &name)`
    - `code_for_snippet(&resolved)`
    - `merge_snippet_input(&resolved, input)`
  - Run synchronous snippet resolution/parsing in `tokio::task::spawn_blocking` and bound it with the existing execution deadline via `timeout_at`.
  - Cache resolved snippet code per execution by `(source, name, path, mtime, len)` so repeated calls do not reread files.
  - Enforce per-execution limits before resolving:
    - max snippet depth
    - max snippet resolves
    - cumulative resolved-code bytes
    - active-stack cycle detection for self-recursion and mutual recursion.
  - Enforce caller authorization and route visibility again during host-side `SnippetResolve`; discovery filtering is not a permission boundary.
  - On success, write `CodeModeRunnerInput::SnippetResolved { seq, code, input: merged_input }`.
  - On failure, write `CodeModeRunnerInput::ToolError { seq, kind, message }` using stable snippet-related `ToolError` kinds.
  - Log a structured debug/info event with fields `surface`, `snippet`, `seq`, and elapsed time. Do not log snippet source or secret input values.
  - Track pending snippet resolves parent-side alongside pending tool calls. At `Done`, assert there is no pending tool/artifact/snippet work.
  - Treat protocol mismatches as runner-unhealthy and evict pooled runners: unknown snippet seq, invalid `SnippetResolved`, oversized code payload, settlement invariant failure. User snippet runtime throws remain normal reusable execution errors.

- [ ] Preserve sandbox composition.
  - A snippet invoked with `await codemode.run("foo", input)` must be able to call `await codemode.gateway.gateway_list(...)`, `await callTool(...)`, and `await writeArtifact(...)`.
  - Nested snippet calls are allowed only within the per-run snippet depth/count/byte budgets and the existing overall Code Mode timeout.

- [ ] Add focused tests.
  - Protocol serde round trip for `SnippetResolve` and `SnippetResolved`.
  - Runner JS unit/runtime test where `codemode.run("demo", { x: 2 })` resolves a host-provided snippet and returns `4`.
  - Runner integration test where a snippet calls `callTool(...)` and the normal tool-call protocol still settles.
  - Error test for missing snippet returns a structured, informative error and keeps the runner reusable.
  - Non-admin/read-scoped caller cannot run a user snippet.
  - Direct recursion and mutual recursion fail quickly with a stable snippet recursion/budget error.
  - Snippet throw, snippet syntax/eval failure, nested snippet success, and snippet-calls-tool success are covered.

---

## Task 3: Add friendlier sandbox error hints

- [ ] Update runtime error formatting in `crates/lab/src/dispatch/gateway/code_mode/runner.rs`.
  - Add a small helper:
    ```rust
    fn add_code_mode_hint(kind: &str, message: &str) -> String
    ```
  - Append hints only after final runtime errors are classified as unstructured uncaught JS errors. Do not append hints to `ToolError` JSON, snippet resolution failures, or errors caught inside user code.
  - When final unstructured `kind == "ReferenceError"` or message contains `" is not defined"`, append a hint like:
    `Available globals: codemode, codemode.run, codemode.search, codemode.describe, codemode.step, callTool, writeArtifact. Node/Deno globals such as require, process, fs, fetch, and Bun are not available in the sandbox.`
  - When message indicates calling a non-function under `codemode`, append:
    `Use await codemode.search("...") or await codemode.describe("...") to find the exact helper name.`
  - Keep the original error kind and stack/message content intact; append hints, do not replace.

- [ ] Update `crates/lab/src/mcp/call_tool_codemode.rs`.
  - Mention `codemode.run("<snippet>", input)` in the tool description.
  - Mention that snippets are discoverable through `codemode.search` and `codemode.describe`.

- [ ] Add tests.
  - `ReferenceError: require is not defined` includes available globals and no-Node hint.
  - Misspelled helper/type error includes the search/describe recovery hint.
  - Existing structured error JSON remains parseable by current tests.
  - Tool errors and snippet resolution errors do not receive appended global hints.

---

## Task 4: Implement promotion from prior Code Mode execution to snippet

- [ ] Add a bounded private source store near Code Mode history.
  - Locate the existing gateway manager methods that record Code Mode history and traces.
  - Add a private type similar to:
    ```rust
    #[derive(Debug, Clone)]
    pub struct CodeModeExecutionSource {
        pub execution_id: String,
        pub created_at_ms: i64,
        pub actor_key: Option<String>,
        pub is_admin: bool,
        pub route_scope: String,
        pub surface: CodeModeSurface,
        pub capability_filter_fingerprint: String,
        pub code: String,
    }
    ```
  - Store entries in a bounded `VecDeque` under the gateway manager with both `max_entries` and `max_bytes`, mirroring `CodeModeHistory` byte discipline.
  - Reject or do not retain oversized source entries above the existing Code Mode source-size limit.
  - This store is intentionally not exposed through history resources.
  - Add `GatewayManager::resolve_code_mode_source(execution_id, actor/route context)` that requires:
    - admin/trusted-local caller
    - same actor when actor is known, unless an admin explicitly promotes across actor/scope with confirmation
    - compatible route scope/capability filter.
  - Unknown/evicted/restarted source errors must include a clear message: promotion source is ephemeral and may have expired, been evicted, or lived in another gateway process.

- [ ] Add execution ids to Code Mode responses.
  - In `crates/lab/src/mcp/call_tool_codemode.rs`, generate a ULID/string execution id before `broker.execute(...)`.
  - Include `execution_id` in:
    - structured content trace
    - user-visible success summary
    - Code Mode history entry metadata
  - On successful admin/trusted-local execution, store the submitted source in the private source store with actor/route metadata.
  - On execution error, do not store source unless tests or existing UX require failed executions to be promotable. Prefer success-only for the first implementation.

- [ ] Add `snippets.promote` action in `crates/lab/src/dispatch/snippets/catalog.rs` and `dispatch.rs`.
  - Params:
    ```json
    {
      "execution_id": "01J...",
      "name": "gateway-summary",
      "description": "optional",
      "force": false,
      "shadow_builtin": false
    }
    ```
  - Mark `ActionSpec.destructive = true` and `requires_admin = true` according to existing snippet execution policy.
  - Resolve the private source by execution id from the gateway manager using caller/route/actor context, not a global lookup.
  - Validate the target name with `validate_snippet_name`.
  - If `name` matches a builtin snippet, reject unless `shadow_builtin: true` and destructive confirmation are present. Return metadata clearly stating when a user snippet shadows a builtin.
  - Use or add an atomic user snippet write helper:
    - render body
    - write to temp file in the snippets dir
    - fsync where practical
    - rename into place
    - serialize concurrent writes per snippet name.
  - Return `SnippetInfo` plus the originating `execution_id`.
  - Errors:
    - unknown execution id: `unknown_execution`
    - missing gateway manager/source store: `gateway_unavailable`
    - authorization failure: existing stable auth/admin error kind
    - route/actor mismatch: `forbidden`
    - builtin shadow without flag: `builtin_shadow_requires_confirmation`
    - existing snippet without force: existing store error

- [ ] Do not add a standalone local CLI promotion command in this implementation.
  - Reason: the promotable source store is deliberately in-memory and owned by the live gateway process, so a fresh CLI process cannot reliably see prior MCP/API executions.
  - If CLI promotion is desired later, implement it as a gateway/API client command or persist source blobs under Lab home with strict file permissions, byte caps, and cleanup.
  - Documentation must state that promotion from execution id is live-gateway scoped and ephemeral.

- [ ] Add tests.
  - Successful Code Mode call records source by execution id without exposing code in public history JSON.
  - `snippets.promote` creates a user snippet whose extracted code matches the prior execution source.
  - `snippets.promote` rejects unknown execution ids with `unknown_execution`.
  - promotion rejects cross-route/cross-actor lookup unless admin/trusted-local policy allows it with confirmation.
  - promotion rejects builtin shadowing unless `shadow_builtin: true` is present.
  - concurrent/partial write regression for atomic promoted snippet writes.

---

## Task 5: Documentation, examples, and generated inventories

- [ ] Update docs that describe Code Mode.
  - Likely files:
    - `docs/surfaces/MCP.md`
    - any Code Mode docs under `docs/`
    - README snippets section if one exists
  - Include examples:
    ```js
    async () => {
      const found = await codemode.search("snippet gateway");
      const docs = await codemode.describe(found[0].id);
      return { found, docs };
    }
    ```
    ```js
    async () => {
      const summary = await codemode.run("gateway-summary", { includeHealth: true });
      await writeArtifact("gateway-summary.json", JSON.stringify(summary, null, 2), {
        contentType: "application/json"
      });
      return summary;
    }
    ```
    ```js
    // Through the live gateway snippets action, not a standalone local CLI:
    snippets({ action: "promote", params: {
      execution_id: "01JEXAMPLE",
      name: "gateway-summary",
      description: "Summarize gateway health"
    }})
    ```
  - Document that promoted source is written as plaintext executable snippet content and may contain anything the original Code Mode source contained.

- [ ] Regenerate docs/inventories if required by this repo.
  - Run:
    ```bash
    cargo run --package labby --all-features -- docs generate
    cargo run --package labby --all-features -- docs check
    ```
  - If generation changes unrelated files, inspect before staging and keep only relevant generated updates.

---

## Task 6: End-to-end verification

- [ ] Run focused checks first:
  ```bash
  cargo test --package labby --all-features code_mode
  cargo test --package labby --all-features snippets
  ```

- [ ] Run full repo verification:
  ```bash
  cargo fmt --all -- --check
  cargo clippy --workspace --all-features -- -D warnings
  cargo nextest run --workspace --all-features
  cargo check --workspace --all-features
  ```

- [ ] Build and smoke-test the actual binary:
  ```bash
  cargo build --workspace --all-features --bin labby
  ```

- [ ] Use mcporter against a local stdio server or the running Labby gateway when available.
  - Verify `codemode` tool description mentions snippets and `codemode.run`.
  - Call `codemode` with:
    ```js
    async () => await codemode.search("snippet")
    ```
  - Call `codemode` with:
    ```js
    async () => await codemode.describe("snippet.<known-snippet-name>")
    ```
  - Call `codemode` with:
    ```js
    async () => await codemode.run("<known-snippet-name>", {})
    ```
  - Promote a successful prior execution through the live gateway snippets action, with destructive/admin confirmation as required:
    ```json
    {
      "action": "promote",
      "params": {
        "execution_id": "<execution-id>",
        "name": "promoted-smoke",
        "description": "Promoted smoke snippet",
        "force": true
      }
    }
    ```
  - Re-run through Code Mode:
    ```js
    async () => await codemode.run("promoted-smoke", {})
    ```

- [ ] Capture final evidence in the PR:
  - commands run
  - any skipped command and exact reason
  - mcporter smoke output summary
  - known limitation: `codemode.run` is bounded by the same overall Code Mode timeout, not a separate snippet timeout.
  - known limitation: promotion source is ephemeral/live-gateway scoped until a future persistent source store exists.

---

## Engineering Review Findings Addressed

- Architecture: removed unreliable standalone CLI promotion, added route/actor-bound source lookup, shared discovery projection, strict snippet listing failure policy, snippet recursion budget, and destructive promotion semantics.
- Simplicity: reused the existing `resolve_snippet -> code_for_snippet -> merge_snippet_input -> return await (${code})(input)` contract, dropped per-snippet DTS, and defined deterministic bare-name collision behavior.
- Security: made `codemode.run` and `snippets.promote` admin/trusted-local scoped, required route/caller visibility checks in discovery and resolution, protected builtin shadowing, and documented plaintext promoted source.
- Performance: added snippet metadata caching, `spawn_blocking` filesystem work, per-execution resolved-code caching, snippet depth/count/byte limits, parent-side pending tracking, and source-store byte caps.
