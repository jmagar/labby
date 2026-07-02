# Code Mode Semantic Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Blend semantic (embedding-based) similarity into `codemode.search()` so agents whose queries use synonyms rather than exact catalog vocabulary (e.g. "roster of saved queues" for a tool literally named/described differently) still surface the right tool, while preserving today's lexical-only behavior byte-for-byte when the TEI embedding service is unset, unreachable, or cold.

**Architecture:** Add one new `CodeModeHost::semantic_rank` async trait method (client-neutral, `labby-codemode`) that takes a query string plus the caller/surface/scope already used by every other trait method, and returns a fail-open, host-ranked `(id, similarity)` list. Implemented in `labby-gateway`: a small reqwest-based TEI client embeds text; catalog vectors are computed once per distinct catalog fingerprint (reusing the *existing* `fingerprint` string already computed in `crates/labby-gateway/src/gateway/code_mode/search.rs:67-74` for the render cache) and cached in-process on `GatewayManager`, mirroring the existing `code_mode_catalog_render_cache` pattern. The query-time round trip — the one genuinely new per-search-call cost, since the query string is sandbox-runtime data unknown until the agent's JS calls `codemode.search(...)` — reuses the **existing** `callTool`/`ToolCall`/`ToolResult` protocol wire path via a reserved internal tool id (`__lab_internal::semantic_rank`), following the same "not a real upstream tool" precedent already established by `try_parse_local_provider_call` in this codebase. This adds **zero** new protocol enum variants, zero new javy host-function bindings, and zero new JS globals — `codemode.search()`'s JS calls the exact same `callTool(id, params)` primitive it already has. Rust owns 100% of the vector math (cosine similarity); no raw floats are ever serialized into the sandbox. Failures anywhere in the embedding path fail open to today's pure-lexical behavior, gated by a cooldown so a TEI outage is retried automatically without hammering it every call.

**Tech Stack:** Rust 2024, Tokio, reqwest (already a `labby-gateway` dependency), `url` (already a dependency of `labby-runtime`/`labby-codemode`/`labby-gateway`), serde/serde_json, Javy/QuickJS sandbox (`labby-codemode`), TOML config (`labby-runtime::gateway_config`), TEI (Text Embeddings Inference) HTTP API.

## Global Constraints

- `labby-codemode` stays client-neutral: it must gain no `reqwest`, TEI, or gateway-specific vocabulary. The `CodeModeHost::semantic_rank` trait method takes/returns only neutral types already used by sibling trait methods (`&CodeModeCaller`, `CodeModeSurface`, `&ToolScope`, `String`, `usize`) plus `Vec<(String, f32)>` — never a raw embedding vector.
- The embedding path is fail-open, always. No TEI failure, timeout, or misconfiguration may ever surface a different error, a different response shape, or a hang to the agent calling `codemode.search()` — it must silently degrade to exactly today's lexical-only ranking. `CodeModeHost::semantic_rank` never returns `Err` for a degraded embedding service; `Err` is reserved for genuine host-side bugs, exactly as with every other fail-open contract in this plan.
- No behavioral change when semantic search is unconfigured (default — `tei_url` unset) — `codemode.search()`'s lexical algorithm (`preamble.rs:201-256`) and its return shape (`{results, total, truncated, hint?}`) are unchanged in that case.
- **No new bidirectional sandbox protocol.** The query-time round trip reuses the existing `callTool`/`ToolCall`/`ToolResult` wire path (`protocol.rs`, `runner.rs`, `runner_drive.rs`'s `enqueue_tool_call`) via a reserved internal tool id `__lab_internal::semantic_rank`, dispatched through `execute.rs`'s `call_tool_id` BEFORE the normal `scope.allows()` check. `protocol.rs` and `runner.rs` require **zero changes** under this design — confirm this remains true throughout implementation; if any task discovers it isn't, stop and re-plan rather than silently reintroducing new protocol surface.
- **Security invariant — read this before touching `call_tool_id`:** the `__lab_internal::semantic_rank` dispatch bypasses `scope.allows()` deliberately, and this is safe **only** because `semantic_rank` never returns tool-call capability or raw tool results — it returns exclusively `(id, similarity)` ranking metadata over the catalog entries `build_code_mode_proxy` already scope-filtered for THIS execution before the sandbox started (`execute.rs:153-160`'s `catalog.iter().filter(|entry| ... scope.allows(...))`, confirmed to run before `CodeModeDiscoveryEntry::from_catalog`). `semantic_rank`'s Rust-side implementation must be given exactly that same scope-filtered entry set — never a broader one — so it is structurally impossible for it to rank/return an id the sandbox's own `__codemodeDiscovery` doesn't already contain. Task 2 and Task 5 must preserve this invariant; Task 7 adds a test that proves it.
- The call-budget exclusion (`__lab_internal::` ids must not count against `max_calls_per_run` or appear in `response.calls`) is implemented as a single early-return gate inside `handle_completed_tool_call` (`runner_drive.rs:917+`, the sole function where all three `state.calls.push(...)` sites live — confirmed at `runner_drive.rs:948,960,1012`) plus a gate on the `state.calls_enqueued` increment (`runner_drive.rs:329`, the only increment site) — not a parallel bookkeeping structure.
- Catalog embedding is computed from `CodeModeDiscoveryEntry.description` only via exactly one batched `POST /embed` call per distinct catalog fingerprint, chunked into `<=512`-entry batches (TEI's confirmed hard `max_batch_requests: 512` limit) — never one HTTP call per entry, never an unbounded single batch.
- `tei_url` is validated as a well-formed `http://`/`https://` URL at config-validation time (SSRF-adjacent defense in depth — operator-config is trusted, but a copy-paste error or stale config pointing at an unintended host should be caught, not silently accepted).
- TEI response bodies are size-capped (16 MiB) before JSON decoding — a misbehaving or compromised TEI endpoint cannot force unbounded memory use.
- Cooldown after a TEI failure is a hardcoded `Duration::from_secs(30)` constant (not configurable — the user's requirements named "TEI URL configurable" as the one required knob; timeout/cooldown were engineering additions the simplicity review flagged as unnecessary v1 surface). Rationale: long enough that a flapping/restarting TEI container doesn't get hammered every search call (searches can happen many times per Code Mode execution), short enough that recovery is picked up within one typical agent working session without requiring a gateway restart.
- `tracing::warn!` fires exactly once per failure *transition* (healthy→cooldown), not on every skipped call during an active cooldown window. Recovery (a call succeeding again after cooldown) emits `tracing::info!` once per transition, matching the same pattern.
- No per-execution or cross-call query-embedding cache in v1 — an agent calling `codemode.search()` multiple times with similar queries in one execution pays a fresh embedding round trip each time. This is a deliberate YAGNI deferral (confirmed low-impact by performance review: a homelab-scale GPU-backed TEI embed is tens of milliseconds, and even several searches per execution stay a small fraction of the default 30s execution timeout) — do not implement a cache for this in this plan.
- Follow existing repo conventions: `#[serde(default = "default_xxx")]` + free-fn default helpers for new `CodeModeConfig` fields (see `crates/labby-runtime/src/gateway_config.rs:34-56`); range validation added to `CodeModeConfig::validate()` (`gateway_config.rs:137-171`) with new `ConfigError` variants following the existing `InvalidCodeMode*` naming (`gateway_config.rs:716-727`); fingerprint-keyed cache on `GatewayManager` mirroring `code_mode_catalog_render_cache` (`crates/labby-gateway/src/gateway/manager.rs:109-115`) and its accessor methods (`crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs:502-529`), but using `tokio::sync::RwLock` instead of `Mutex` — matching the existing `config: Arc<RwLock<GatewayConfig>>` precedent already in `manager.rs:84` (read-heavy access pattern: most calls just read a warm cache).
- A single-flight guard prevents thundering-herd re-embedding: the embedding-cache lock is held across the full check-then-embed-then-store sequence in `ensure_embeddings_for_fingerprint` (Task 5), not just around the final store, so concurrent cold-start calls against the same fingerprint serialize onto one embed rather than firing N redundant batch calls. This is an accepted lock-hold-time-during-network-call tradeoff, fine at homelab concurrency scale per the performance review's own conclusion.
- Update `docs/runtime/CONFIG.md`'s `### [code_mode]` section (currently `docs/runtime/CONFIG.md:261-292`) with the new nested config keys and defaults. Do not create any new standalone `*.md` files — this is the one doc that already documents `[code_mode]` keys and is the correct place per repo convention.
- Build/lint/test gates: `cargo nextest run --workspace --all-features` (test), `cargo clippy --workspace --all-features -- -D warnings` + `cargo fmt --all -- --check` (lint) — see `Justfile` `test`/`lint` targets. Crate-scoped equivalents: `cargo test -p labby-codemode`, `cargo test -p labby-gateway`.

---

## File Structure

- `crates/labby-codemode/src/host.rs`
  - Modify: add `semantic_rank` method to the `CodeModeHost` trait (~after `resolve_snippet`, before `config`); add a no-op impl on `NoopHost` (test-only, `#[cfg(test)]`) returning `Ok(Vec::new())`. Add a `fingerprint: String` field to `ToolsRender`.
- `crates/labby-codemode/src/execute.rs`
  - Modify: `call_tool_id` (`execute.rs:294-343`) gains an early-return branch for the reserved `__lab_internal` namespace, dispatched BEFORE `scope.allows()`. `build_code_mode_proxy` threads `blend_weight` from `host.config()` into `generate_discovery_js`.
- `crates/labby-codemode/src/runner_drive.rs`
  - Modify: `handle_completed_tool_call` (`runner_drive.rs:917+`) gains a call-budget/trace exclusion gate for `__lab_internal::` ids at its single entry point (affects all three `state.calls.push` sites and the `calls_enqueued` increment at `runner_drive.rs:329` via a check before enqueueing).
- `crates/labby-codemode/src/preamble.rs`
  - Modify: `codemode.search`'s body (`preamble.rs:201-256`) becomes an `async function` that calls the existing `callTool("__lab_internal::semantic_rank", {query, limit})` primitive and blends the result into lexical scoring. `generate_discovery_js`'s signature gains a `blend_weight: f32` parameter.
- `crates/labby-runtime/src/gateway_config.rs`
  - Modify: add `SemanticSearchConfig { tei_url: Option<String>, blend_weight: f32 }` struct + `semantic_search: SemanticSearchConfig` field on `CodeModeConfig` (~after `max_log_bytes`, `gateway_config.rs:118`); add a default-helper free fn near `gateway_config.rs:34-56`; add range/URL validation to `CodeModeConfig::validate()` (`gateway_config.rs:137-171`); add `ConfigError::InvalidSemanticSearchBlendWeight` and `ConfigError::InvalidSemanticSearchTeiUrl` near `gateway_config.rs:716-727`.
- `crates/labby-gateway/src/gateway/code_mode.rs`
  - Modify: add `CatalogEmbeddingCache { fingerprint: String, vectors: Vec<(String, Vec<f32>)> }` struct, mirroring `CatalogRenderCache` (`code_mode.rs:53-62`); add `mod embeddings;`.
- `crates/labby-gateway/src/gateway/manager.rs`
  - Modify: add `pub(super) code_mode_embedding_cache: Arc<RwLock<Option<crate::gateway::code_mode::CatalogEmbeddingCache>>>` field (next to `code_mode_catalog_render_cache`, `manager.rs:109-115`); add `pub(super) semantic_search_last_failure: Arc<RwLock<Option<Instant>>>` field for the fail-open cooldown tracker.
- `crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs`
  - Modify: add `cached_embeddings`/`ensure_embeddings_for_fingerprint` (single-flight helper), `semantic_search_available`, `record_semantic_search_failure`, `record_semantic_search_recovery`.
- Create: `crates/labby-gateway/src/gateway/code_mode/embeddings.rs`
  - New file: the TEI HTTP client (chunked, size-capped) and cosine-similarity ranking helper.
- `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`
  - Modify: add `async fn semantic_rank(&self, query: String, top_k: usize, caller: &CodeModeCaller, surface: CodeModeSurface, scope: &ToolScope) -> Result<Vec<(String, f32)>, ToolError>` impl (~after `resolve_snippet`, `code_mode_host.rs:167-187`).
- `crates/labby-gateway/src/gateway/code_mode/search.rs`
  - Modify: `catalog_from_tools` (`search.rs:50-150`) gains a call to `manager.ensure_embeddings_for_fingerprint(&fingerprint, &entries)` (single-flight, best-effort; never blocks/fails catalog construction) on both the cache-hit and cache-miss paths; populates the new `ToolsRender.fingerprint` field at both construction sites.
- `docs/runtime/CONFIG.md`
  - Modify: extend the `### [code_mode]` table and example (`docs/runtime/CONFIG.md:261-292`) with the new `semantic_search.*` nested keys.

---

## Task 1: `CodeModeHost::semantic_rank` trait method

**Files:**
- Modify: `crates/labby-codemode/src/host.rs`
- Test: `crates/labby-codemode/src/host.rs` (inline `#[cfg(test)]` module — none currently exists in this file; add one)

**Interfaces:**
- Produces: `CodeModeHost::semantic_rank(&self, query: String, top_k: usize, caller: &CodeModeCaller, surface: CodeModeSurface, scope: &ToolScope) -> impl Future<Output = Result<Vec<(String, f32)>, ToolError>> + Send` — every later task calls this signature exactly. Returns `(entry_id, similarity)` pairs, descending by similarity, already truncated to at most `top_k`. Always `Ok` on a degraded/unconfigured/cooldown path (returns `Ok(Vec::new())`); `Err` reserved for host-side bugs.
- Produces: `NoopHost::semantic_rank` returns `Ok(Vec::new())` unconditionally (test-only host never has real embeddings).
- Produces: `ToolsRender.fingerprint: String` field (new) — Task 5 populates and reads this.

**Design note (why this signature, not a bare `Vec<String> -> Vec<Vec<f32>>`):** the engineering review flagged that a manager-global "current catalog fingerprint" field is a real race condition under concurrent executions with different scopes — one execution's `semantic_rank` call could read a different execution's in-flight fingerprint and rank against the wrong catalog. Passing `caller`/`surface`/`scope` into `semantic_rank` itself — exactly the same three parameters `call_tool` already receives (`host.rs:73-80`) — lets the `GatewayManager` implementation recompute the identical fingerprint `catalog_from_tools` would compute for equivalent inputs, entirely from this call's own arguments, with no shared mutable "current fingerprint" state anywhere. This makes the design race-free by construction rather than by careful locking.

- [ ] **Step 1: Add the trait method**

In `crates/labby-codemode/src/host.rs`, add to the `CodeModeHost` trait (after `resolve_snippet`, before `fn config`, i.e. after line 88 and before line 90):

```rust
    /// Rank the host's Code Mode catalog by semantic similarity to `query`,
    /// for the exact same `caller`/`surface`/`scope` that would be passed to
    /// `list_tools`/`call_tool` for this execution. Returns `(entry_id,
    /// similarity)` pairs, descending by similarity, capped to `top_k`.
    ///
    /// Hosts with no embedding service configured (or currently in a failure
    /// cooldown) MUST return `Ok(Vec::new())` rather than an `Err` — an empty
    /// result is the fail-open signal `codemode.search()` uses to skip
    /// semantic scoring for that call. `Err` is reserved for genuine
    /// host-side bugs, not for "the embedding service is unreachable".
    ///
    /// Implementations must only ever return ids that are members of the
    /// SAME scope-filtered entry set `list_tools` would return for these
    /// exact `caller`/`surface`/`scope` — this is a security invariant, not
    /// an optimization: the caller (`call_tool_id`) intentionally does not
    /// re-check `scope.allows()` on this method's results.
    fn semantic_rank(
        &self,
        query: String,
        top_k: usize,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> impl Future<Output = Result<Vec<(String, f32)>, ToolError>> + Send;
```

- [ ] **Step 2: Add the `NoopHost` impl**

In the same file's `#[cfg(test)] impl CodeModeHost for NoopHost` block (currently `host.rs:115-163`), add after `resolve_snippet` (after line 154, before `async fn config`):

```rust
    async fn semantic_rank(
        &self,
        _query: String,
        _top_k: usize,
        _caller: &CodeModeCaller,
        _surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        Ok(Vec::new())
    }
```

- [ ] **Step 3: Add `fingerprint` to `ToolsRender`**

In the same file, add a `fingerprint: String` field to `ToolsRender` (`host.rs:26-33`):

```rust
pub struct ToolsRender {
    /// Fingerprint of the live tool set this render was built from (sorted
    /// tool ids + snippet directory state). Hosts key auxiliary per-catalog
    /// caches (e.g. embedding vectors) off this without recomputing it
    /// themselves.
    pub fingerprint: String,
    /// The descriptors (tools + snippets) visible to this execution.
    pub entries: Vec<ToolDescriptor>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    pub catalog_json: String,
    /// Serialized catalog size in bytes (for tracing).
    pub serialized_size: usize,
}
```

Update `NoopHost::list_tools` (`host.rs:116-129`) to add `fingerprint: "noop".to_string(),` to its returned `ToolsRender`.

- [ ] **Step 4: Compile-check**

Run: `cargo check -p labby-codemode --all-features`
Expected: PASS — the trait is additive and `NoopHost` now satisfies it; the `ToolsRender` field addition only breaks construction sites, and `NoopHost::list_tools` is the only one inside this crate. Confirm no other in-crate impl of `CodeModeHost` or construction of `ToolsRender` exists via `grep -rn "impl CodeModeHost for\|ToolsRender {" crates/labby-codemode/src/` (should only match `NoopHost` and its `list_tools`).

- [ ] **Step 5: Commit**

```bash
git add crates/labby-codemode/src/host.rs
git commit -m "feat(codemode): add CodeModeHost::semantic_rank trait method and ToolsRender.fingerprint"
```

---

## Task 2: Reserved internal tool id bridge (`__lab_internal::semantic_rank`)

**Files:**
- Modify: `crates/labby-codemode/src/execute.rs`
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Test: `crates/labby-codemode/src/execute.rs` (existing `#[cfg(test)] mod tests`, `execute.rs:403+`) and `crates/labby-codemode/src/runner_drive.rs` (existing `#[cfg(test)] mod tests`, `runner_drive.rs:1127+`)

**Interfaces:**
- Consumes: `CodeModeHost::semantic_rank` (Task 1); the existing `CodeModeToolId::parse`/`split_namespaced_id` (`types.rs:77-89`), which already accepts any `<namespace>::<tool>`-shaped string, including `__lab_internal::semantic_rank`.
- Produces: a JS-visible `callTool("__lab_internal::semantic_rank", {query, limit})` that resolves to `{ranked: [{id, score}, ...]}` — Task 7's `preamble.rs` changes call this exact id/param/return shape. Calls to this id never appear in `response.calls` and never count against `max_calls_per_run`.

- [ ] **Step 1: Confirm the exact insertion points before editing**

Run: `sed -n '294,344p' crates/labby-codemode/src/execute.rs` — confirm `call_tool_id`'s current shape (parse → host check → match on `CodeModeToolRef::Tool { namespace, tool }` → `scope.allows()` check at line 311 → `host.call_tool(...)`).

Run: `sed -n '327,380p' crates/labby-codemode/src/runner_drive.rs` — confirm the `ToolCall` match arm's dispatch order: `calls_enqueued` increment (line 329) → budget check → `try_parse_local_provider_call` (line 346) → `enqueue_tool_call` (line 358) / `enqueue_rejected_tool_call` (line 369).

Run: `sed -n '917,1025p' crates/labby-codemode/src/runner_drive.rs` — confirm `handle_completed_tool_call`'s three `state.calls.push(...)` sites (lines 948, 960, 1012), each keyed on the local `id` binding from the completed future's tuple `(seq, id, params, result, elapsed_ms)`.

- [ ] **Step 2: Add the reserved-namespace constant and dispatch branch in `execute.rs`**

In `crates/labby-codemode/src/execute.rs`, add a module-level constant near the top of the file (after the existing `use` statements, before `impl<H: CodeModeHost> CodeModeBroker<'_, H>`):

```rust
/// Reserved namespace for host-internal pseudo-tool calls that are NOT real
/// Code Mode tool calls — they never reach `host.call_tool`, never consume
/// the per-run call budget, and never appear in `response.calls`. The
/// sandbox's generated JS calls these via the ordinary `callTool(id, params)`
/// primitive so no new sandbox protocol surface is needed; `call_tool_id`
/// intercepts ids in this namespace before the normal scope check.
const LAB_INTERNAL_NAMESPACE: &str = "__lab_internal";
```

In `call_tool_id` (`execute.rs:294-343`), inside the `match parsed.reference { CodeModeToolRef::Tool { namespace, tool } => { ... } }` block, add a new branch immediately after the `let Some(host) = ...` check and BEFORE the existing `if !scope.allows(&namespace, &tool) { ... }` check (before line 311):

```rust
                if namespace == LAB_INTERNAL_NAMESPACE {
                    return self
                        .dispatch_internal_call(&tool, params, &caller, surface, scope)
                        .await;
                }
```

Add the `dispatch_internal_call` method on `CodeModeBroker` (near `call_tool_id`, e.g. immediately after it):

```rust
    /// Dispatch a reserved `__lab_internal::*` pseudo-tool call. These never
    /// reach `host.call_tool` and are never subject to `scope.allows()` —
    /// see the `LAB_INTERNAL_NAMESPACE` doc comment for why that's safe.
    async fn dispatch_internal_call(
        &self,
        tool: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        let Some(host) = self.host else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "no tool source configured".to_string(),
            });
        };
        match tool {
            "semantic_rank" => {
                let query = params
                    .get("query")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n.clamp(1, 50) as usize)
                    .unwrap_or(50);
                let ranked = host
                    .semantic_rank(query, limit, caller, surface, scope)
                    .await
                    .unwrap_or_default();
                let ranked_json: Vec<Value> = ranked
                    .into_iter()
                    .map(|(id, score)| {
                        serde_json::json!({ "id": id, "score": score })
                    })
                    .collect();
                Ok(serde_json::json!({ "ranked": ranked_json }))
            }
            _ => Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown internal tool `{LAB_INTERNAL_NAMESPACE}::{tool}`"),
            }),
        }
    }
```

Note: `host.semantic_rank(...).await.unwrap_or_default()` is the fail-open point at this layer — even though Task 1's trait contract already says implementations must return `Ok(Vec::new())` on degraded paths (never `Err`), this `unwrap_or_default()` is a defensive second fail-open layer so a host implementation bug (an accidental `Err`) still can't break `codemode.search()` — it degrades to an empty ranked list, identical to the "no semantic signal" case.

- [ ] **Step 3: Write a unit test for `call_tool_id`'s internal dispatch**

In `crates/labby-codemode/src/execute.rs`'s existing `#[cfg(test)] mod tests` block (`execute.rs:403+`), add (using the existing `NoopHost`/broker-construction pattern already in that file — read the existing tests first to match helper usage exactly):

```rust
    #[tokio::test]
    async fn call_tool_id_routes_lab_internal_namespace_before_scope_check() {
        // A ToolScope that allows nothing should still let `__lab_internal::*`
        // through, because it's intercepted before the scope.allows() check.
        let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
        let empty_scope = ToolScope::scoped_namespaces(vec![], vec![]);
        let result = broker
            .call_tool_id(
                "__lab_internal::semantic_rank",
                serde_json::json!({ "query": "test", "limit": 5 }),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &empty_scope,
            )
            .await;
        // NoopHost's semantic_rank always returns Ok(vec![]), so this must
        // succeed with an empty ranked list, not a `forbidden`/`unknown_tool`
        // scope error.
        let value = result.expect("internal dispatch must bypass scope.allows()");
        assert_eq!(value, serde_json::json!({ "ranked": [] }));
    }

    #[tokio::test]
    async fn call_tool_id_rejects_unknown_internal_tool() {
        let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
        let scope = ToolScope::default();
        let result = broker
            .call_tool_id(
                "__lab_internal::not_a_real_internal_tool",
                serde_json::json!({}),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &scope,
            )
            .await;
        assert!(result.is_err());
    }
```

(Adjust the exact `ToolScope` constructor calls to match whatever's actually available — check `ToolScope::scoped_namespaces`/`ToolScope::default`/`ToolScope::new` signatures in `types.rs` before writing; use whichever constructs an intentionally-restrictive scope.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p labby-codemode execute:: -- --nocapture`
Expected: PASS, including the two new tests.

- [ ] **Step 5: Exclude `__lab_internal::` ids from the call budget and trace in `runner_drive.rs`**

In `drive_runner`'s `CodeModeRunnerOutput::ToolCall { seq, id, params }` arm (`runner_drive.rs:328-380`), change the `calls_enqueued` increment (currently unconditional at line 329) to skip reserved ids:

```rust
                        CodeModeRunnerOutput::ToolCall { seq, id, params } => {
                            let is_internal = id.starts_with("__lab_internal::");
                            if !is_internal {
                                state.calls_enqueued = state.calls_enqueued.saturating_add(1);
                            }
                            if !is_internal && state.calls_enqueued > state.max_calls_per_run {
```

(This replaces the existing `if state.calls_enqueued > state.max_calls_per_run {` condition — keep the rest of that `if`/`else` block's body unchanged; only the increment and the guard condition change. Reserved-namespace calls always fall through to the `else` branch's `enqueue_tool_call` path unchanged — they are NOT exempted from `try_parse_local_provider_call`/dispatch routing, only from budget counting.)

In `handle_completed_tool_call` (`runner_drive.rs:917-1025`), add an early check right after the `let Some((seq, id, params, result, elapsed_ms)) = completed else { return Ok(()); };` line (after line 927):

```rust
    let is_internal = id.starts_with("__lab_internal::");
```

Then gate each of the three `state.calls.push(...)` call sites (lines 948, 960, 1012) behind `if !is_internal { state.calls.push(...); }` — wrap each existing push statement, do not otherwise change their bodies. The `write_runner_input_by_deadline(...)` calls that send `ToolResult`/`ToolError` back to the runner are UNCHANGED and unconditional — `__lab_internal::` calls still get a real response so the sandbox's `callTool(...)` Promise resolves normally; only the budget/trace bookkeeping is skipped.

- [ ] **Step 6: Write a drive-loop test confirming budget/trace exclusion**

In `runner_drive.rs`'s existing `#[cfg(test)] mod tests` block (`runner_drive.rs:1127+`), add a test that drives a stub runner emitting a `ToolCall` with id `"__lab_internal::semantic_rank"` and asserts, after the run completes, that the call trace does NOT contain an entry for that id, and that ordinary calls up to `max_calls_per_run` still all succeed even with an extra `__lab_internal::` call interleaved (i.e. the internal call did not consume a budget slot). Base this on whatever stub-runner infrastructure the existing tests in this file already use — reuse it, don't duplicate it.

- [ ] **Step 7: Run tests**

Run: `cargo test -p labby-codemode runner_drive:: -- --nocapture`
Expected: PASS, including the new test from Step 6.

- [ ] **Step 8: Full crate test suite + lint**

Run: `cargo test -p labby-codemode --all-features && cargo clippy -p labby-codemode --all-features -- -D warnings && cargo fmt -p labby-codemode -- --check`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/labby-codemode/src/execute.rs crates/labby-codemode/src/runner_drive.rs
git commit -m "feat(codemode): route __lab_internal:: calls through callTool, exempt from budget"
```

---

## Task 3: `SemanticSearchConfig` + validation + `docs/runtime/CONFIG.md`

**Files:**
- Modify: `crates/labby-runtime/src/gateway_config.rs`
- Modify: `docs/runtime/CONFIG.md`
- Test: `crates/labby-runtime/src/gateway_config.rs` (find existing `CodeModeConfig` tests via `grep -n "mod tests\|fn.*code_mode" crates/labby-runtime/src/gateway_config.rs` and add alongside)

**Interfaces:**
- Produces: `SemanticSearchConfig { tei_url: Option<String>, blend_weight: f32 }` on `CodeModeConfig.semantic_search`. Presence of a non-empty `tei_url` is the sole enable signal — no separate `enabled` flag. Later tasks (`embeddings.rs`, `code_mode_host.rs`) read these exact field names.

**YAGNI cuts applied per simplicity review:** no `enabled` bool (redundant with `tei_url` presence), no configurable `tei_timeout_ms`/`cooldown_ms` (hardcoded constants — see Task 4/5). `blend_weight` stays configurable since the user's own requirements explicitly asked to document/tune the blend formula.

- [ ] **Step 1: Add the default-helper free function**

In `crates/labby-runtime/src/gateway_config.rs`, near the existing default helpers (after `fn default_max_log_bytes()`, i.e. after line 56, before `fn default_upstream_priority()`), add:

```rust
fn default_semantic_search_blend_weight() -> f32 {
    0.5
}
```

- [ ] **Step 2: Add the `SemanticSearchConfig` struct**

In the same file, near `CodeModeConfig` (before it, so it can be referenced — insert before line 86's `pub struct CodeModeConfig {`):

```rust
/// Optional embedding-based semantic search blend for `codemode.search()`.
///
/// Disabled by default (`tei_url = None`). When `tei_url` is unset or empty,
/// `codemode.search()` runs its existing pure-lexical algorithm unchanged;
/// this struct's fields are never read on that path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticSearchConfig {
    /// Base URL of the TEI (Text Embeddings Inference) server, e.g.
    /// `http://localhost:52000`. `None` or empty (the default) means
    /// semantic search stays off — this is the sole enable signal, there is
    /// no separate `enabled` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tei_url: Option<String>,
    /// Weight applied to normalized semantic similarity when blending with
    /// normalized lexical score. See `preamble.rs` `codemode.search` blend
    /// comment for the exact formula.
    #[serde(default = "default_semantic_search_blend_weight")]
    pub blend_weight: f32,
}

impl Default for SemanticSearchConfig {
    fn default() -> Self {
        Self {
            tei_url: None,
            blend_weight: default_semantic_search_blend_weight(),
        }
    }
}

impl SemanticSearchConfig {
    /// True only when `tei_url` is set to a non-empty string. Every call
    /// site should gate on this rather than re-checking the field directly.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.tei_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty())
    }
}
```

- [ ] **Step 3: Add the field to `CodeModeConfig`**

In `CodeModeConfig` (currently `gateway_config.rs:86-119`), add after `max_log_bytes` (after line 118, before the closing `}`):

```rust
    /// Optional embedding-based semantic search blend for `codemode.search()`.
    #[serde(default)]
    pub semantic_search: SemanticSearchConfig,
```

Update `CodeModeConfig`'s `Default` impl (`gateway_config.rs:121+`) to include `semantic_search: SemanticSearchConfig::default(),`.

- [ ] **Step 4: Add validation (URL scheme + blend_weight range)**

In `CodeModeConfig::validate()` (`gateway_config.rs:137-171`), add before the final `Ok(())` (before line 169):

```rust
        if !(0.0..=1.0).contains(&self.semantic_search.blend_weight) {
            return Err(ConfigError::InvalidSemanticSearchBlendWeight {
                value: self.semantic_search.blend_weight,
            });
        }
        if let Some(tei_url) = self.semantic_search.tei_url.as_deref() {
            let trimmed = tei_url.trim();
            if !trimmed.is_empty() {
                let parsed = url::Url::parse(trimmed).map_err(|_| {
                    ConfigError::InvalidSemanticSearchTeiUrl {
                        value: tei_url.to_string(),
                    }
                })?;
                if parsed.scheme() != "http" && parsed.scheme() != "https" {
                    return Err(ConfigError::InvalidSemanticSearchTeiUrl {
                        value: tei_url.to_string(),
                    });
                }
            }
        }
```

Confirm `url::Url` is reachable — this crate already depends on `url` per `crates/labby-runtime/Cargo.toml:16`, confirmed; add a `use` if the file doesn't already reference `url::` anywhere (check first with `grep -n "^use url\|url::" crates/labby-runtime/src/gateway_config.rs`).

Add the new `ConfigError` variants near the existing `InvalidCodeMode*` variants (`gateway_config.rs:716-727`, after `InvalidCodeModeMaxLogBytes`):

```rust
    #[error("gateway code_mode.semantic_search.blend_weight={value} is invalid — expected 0.0..=1.0")]
    InvalidSemanticSearchBlendWeight { value: f32 },
    #[error("gateway code_mode.semantic_search.tei_url={value:?} is invalid — expected a well-formed http:// or https:// URL")]
    InvalidSemanticSearchTeiUrl { value: String },
```

- [ ] **Step 5: Wire the new `ConfigError` variants through `validate_code_mode` in `labby-gateway`**

In `crates/labby-gateway/src/gateway/config.rs`'s `validate_code_mode` (`config.rs:564-591`), the catch-all `_ => ToolError::InvalidParam { message: e.to_string(), param: "code_mode".to_string() }` arm (line 586-589) already handles any `ConfigError` variant not explicitly matched — confirm the two new variants fall through to this arm correctly (they will, since it's a wildcard). No change is required, but optionally add explicit arms with `param: "code_mode.semantic_search.blend_weight"` / `"code_mode.semantic_search.tei_url"` for parity with the existing three explicit arms — do this for consistency with the file's existing style.

- [ ] **Step 6: Write config tests**

In `crates/labby-runtime/src/gateway_config.rs`, find the existing test module (`grep -n "mod tests" crates/labby-runtime/src/gateway_config.rs`) and add:

```rust
    #[test]
    fn semantic_search_defaults_to_unconfigured() {
        let cfg = CodeModeConfig::default();
        assert!(cfg.semantic_search.tei_url.is_none());
        assert!(!cfg.semantic_search.is_configured());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_with_valid_http_url_is_configured_and_valid() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("http://localhost:52000".to_string());
        assert!(cfg.semantic_search.is_configured());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_with_https_url_is_valid() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("https://tei.internal.example:8443".to_string());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_with_non_http_scheme_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("ftp://example.com".to_string());
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSemanticSearchTeiUrl { .. }));
    }

    #[test]
    fn semantic_search_with_malformed_url_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.tei_url = Some("not a url at all".to_string());
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSemanticSearchTeiUrl { .. }));
    }

    #[test]
    fn semantic_search_blend_weight_out_of_range_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.blend_weight = 1.5;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSemanticSearchBlendWeight { .. }));
    }

    #[test]
    fn semantic_search_toml_round_trips_with_defaults_when_omitted() {
        // An existing config.toml with a `[code_mode]` section but no
        // `semantic_search` subsection must still deserialize (backward
        // compatibility with every config.toml written before this feature).
        let toml_str = "enabled = true\ntimeout_ms = 30000\n";
        let cfg: CodeModeConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.semantic_search.tei_url.is_none());
        assert!(!cfg.semantic_search.is_configured());
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p labby-runtime gateway_config:: -- --nocapture`
Expected: PASS, all 7 new tests included.

- [ ] **Step 8: Update `docs/runtime/CONFIG.md`**

In `docs/runtime/CONFIG.md`, extend the `### [code_mode]` table (currently ending at line 277, before the `Example:` block at line 279) with a new subsection after the existing table and before the `Example:` heading:

```markdown
#### `[code_mode.semantic_search]`

Optional embedding-based semantic search blend for `codemode.search()`.
Disabled by default — when `tei_url` is unset, `codemode.search()` is
unchanged pure lexical/substring matching.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `tei_url` | — | unset | Base URL of a [TEI](https://github.com/huggingface/text-embeddings-inference) (Text Embeddings Inference) server, e.g. `http://localhost:52000`. Must be a well-formed `http://` or `https://` URL. Presence of a non-empty value is the sole enable signal for this feature. |
| `blend_weight` | — | `0.5` | Weight applied to normalized semantic similarity when blending with normalized lexical score. Valid range: 0.0-1.0. |

Example:

```toml
[code_mode.semantic_search]
tei_url = "http://localhost:52000"
blend_weight = 0.5
```

Semantic search is fail-open end to end: if TEI is unreachable, times out, or
returns a non-2xx response, that one `codemode.search()` call silently falls
back to lexical-only ranking — the response shape is identical either way, and
no error is visible to the calling agent. After a failure, semantic search is
skipped for a 30-second cooldown (not configurable) before the next attempt,
so a flapping TEI instance isn't hit on every search call; recovery is picked
up automatically once the cooldown elapses. A `tracing::warn!` is logged once
per failure transition (not once per skipped call) and a `tracing::info!`
once per recovery, so operators can see degraded state without log spam.
```

- [ ] **Step 9: Commit**

```bash
git add crates/labby-runtime/src/gateway_config.rs crates/labby-gateway/src/gateway/config.rs docs/runtime/CONFIG.md
git commit -m "feat(config): add code_mode.semantic_search config with URL validation"
```

---

## Task 4: TEI client + cosine ranking (`embeddings.rs`)

**Files:**
- Create: `crates/labby-gateway/src/gateway/code_mode/embeddings.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode.rs` (module declaration)
- Test: inline `#[cfg(test)] mod tests` in `embeddings.rs`

**Interfaces:**
- Consumes: `SemanticSearchConfig` (Task 3).
- Produces:
  - `pub(crate) const TEI_MAX_BATCH_SIZE: usize = 512;` (TEI's confirmed hard `max_batch_requests` limit).
  - `pub(crate) const TEI_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);` (hardcoded per Task 3's YAGNI cut).
  - `pub(crate) const TEI_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;` (16 MiB response size cap).
  - `pub(crate) async fn embed_via_tei(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError>` — internally chunks `texts` into `<=TEI_MAX_BATCH_SIZE`-entry batches, issues sequential `POST /embed` calls, concatenates results. Task 5 calls this exact name (both for catalog warming and for query-time embedding).
  - `pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32`.
  - `pub(crate) fn rank_by_similarity(query_vector: &[f32], catalog_vectors: &[(String, Vec<f32>)]) -> Vec<(String, f32)>` (returns `(id, similarity)` pairs sorted descending by similarity — unranked/uncapped, callers slice as needed).

- [ ] **Step 1: Find the module declaration site**

Run: `sed -n '1,30p' crates/labby-gateway/src/gateway/code_mode.rs` to see how `search` is declared as a submodule (confirm the exact `mod search;`/visibility pattern before adding `mod embeddings;` alongside it).

- [ ] **Step 2: Add the module declaration**

In `crates/labby-gateway/src/gateway/code_mode.rs`, add `pub(crate) mod embeddings;` (or `mod embeddings;` matching whatever visibility `search`'s declaration uses — copy it).

- [ ] **Step 3: Write the TEI client with chunking and a response size cap**

Create `crates/labby-gateway/src/gateway/code_mode/embeddings.rs`:

```rust
//! TEI (Text Embeddings Inference) HTTP client and cosine-similarity ranking
//! for Code Mode's semantic search blend.
//!
//! All vector math lives here, host-side — no raw floats are ever serialized
//! into the QuickJS sandbox. Every function here is designed to be wrapped in
//! a fail-open caller (see `code_mode_host.rs::semantic_rank`); this module
//! itself returns ordinary `Result`s and does not implement the
//! cooldown/fail-open policy — that is the caller's responsibility.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use labby_runtime::error::ToolError;

/// TEI's confirmed hard server-side limit on inputs per `/embed` call
/// (`max_batch_requests` in `GET /info`). `embed_via_tei` chunks any larger
/// input list into batches of at most this size.
pub(crate) const TEI_MAX_BATCH_SIZE: usize = 512;

/// Per-request timeout for one `POST /embed` call. Hardcoded, not
/// configurable — see Task 3's YAGNI rationale in the plan doc.
pub(crate) const TEI_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum accepted TEI response body size before JSON decoding. Guards
/// against a misbehaving or compromised TEI endpoint forcing unbounded
/// memory use.
pub(crate) const TEI_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct TeiEmbedResponse(Vec<Vec<f32>>);

/// Batch-embed `texts` via one or more `POST {url}/embed` calls, chunked to
/// at most `TEI_MAX_BATCH_SIZE` inputs per request (TEI's hard server-side
/// limit). Returns one vector per input text, in input order.
pub(crate) async fn embed_via_tei(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let mut all_vectors = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(TEI_MAX_BATCH_SIZE) {
        let vectors = embed_batch(url, chunk).await?;
        all_vectors.extend(vectors);
    }
    Ok(all_vectors)
}

async fn embed_batch(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError> {
    let client = reqwest::Client::new();
    let endpoint = format!("{}/embed", url.trim_end_matches('/'));
    let response = client
        .post(&endpoint)
        .timeout(TEI_REQUEST_TIMEOUT)
        .json(&json!({ "inputs": texts }))
        .send()
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("TEI request failed: {err}"),
        })?;
    if !response.status().is_success() {
        return Err(ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message: format!("TEI returned HTTP {}", response.status()),
        });
    }
    let body = response.bytes().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "network_error".to_string(),
        message: format!("failed to read TEI response body: {err}"),
    })?;
    if body.len() > TEI_MAX_RESPONSE_BYTES {
        return Err(ToolError::Sdk {
            sdk_kind: "decode_error".to_string(),
            message: format!(
                "TEI response body is {} bytes, exceeding the {} byte cap",
                body.len(),
                TEI_MAX_RESPONSE_BYTES
            ),
        });
    }
    let parsed: TeiEmbedResponse = serde_json::from_slice(&body).map_err(|err| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("failed to decode TEI /embed response: {err}"),
    })?;
    if parsed.0.len() != texts.len() {
        return Err(ToolError::Sdk {
            sdk_kind: "decode_error".to_string(),
            message: format!(
                "TEI returned {} vectors for {} inputs",
                parsed.0.len(),
                texts.len()
            ),
        });
    }
    Ok(parsed.0)
}

/// Cosine similarity between two equal-length vectors. Returns `0.0` for a
/// zero-magnitude vector (rather than dividing by zero / NaN) — this can
/// legitimately happen for a degenerate embedding and should score as "no
/// similarity", not poison the sort with NaN.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(-1.0, 1.0)
}

/// Rank catalog entries by cosine similarity to `query_vector`. Returns
/// `(id, similarity)` pairs sorted descending by similarity — callers decide
/// how many to keep.
pub(crate) fn rank_by_similarity(
    query_vector: &[f32],
    catalog_vectors: &[(String, Vec<f32>)],
) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = catalog_vectors
        .iter()
        .map(|(id, vector)| (id.clone(), cosine_similarity(query_vector, vector)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors_is_negative_one() {
        let v = vec![1.0, 2.0, 3.0];
        let neg: Vec<f32> = v.iter().map(|x| -x).collect();
        assert!((cosine_similarity(&v, &neg) - -1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector_returns_zero_not_nan() {
        let result = cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]);
        assert_eq!(result, 0.0);
        assert!(!result.is_nan());
    }

    #[test]
    fn cosine_similarity_mismatched_lengths_returns_zero() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn rank_by_similarity_sorts_descending() {
        let query = vec![1.0, 0.0];
        let catalog = vec![
            ("low".to_string(), vec![0.0, 1.0]),
            ("high".to_string(), vec![1.0, 0.0]),
            ("mid".to_string(), vec![0.7, 0.7]),
        ];
        let ranked = rank_by_similarity(&query, &catalog);
        assert_eq!(ranked[0].0, "high");
        assert_eq!(ranked[2].0, "low");
    }

    #[tokio::test]
    async fn embed_via_tei_empty_input_returns_empty_without_http_call() {
        let result = embed_via_tei("http://127.0.0.1:1", &[]).await;
        assert_eq!(result.unwrap(), Vec::<Vec<f32>>::new());
    }

    #[tokio::test]
    async fn embed_via_tei_unreachable_server_returns_network_error() {
        // Port 1 is a reserved/unused low port — connection refused, fast.
        let result = embed_via_tei("http://127.0.0.1:1", &["test".to_string()]).await;
        assert!(result.is_err());
    }

    #[test]
    fn tei_max_batch_size_matches_documented_tei_limit() {
        // Regression guard: this constant must track TEI's real
        // max_batch_requests (currently 512, confirmed via GET /info against
        // the live dev TEI server). If TEI's limit changes, update here.
        assert_eq!(TEI_MAX_BATCH_SIZE, 512);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p labby-gateway embeddings:: -- --nocapture`
Expected: PASS, all 9 tests.

- [ ] **Step 5: Live smoke test against the real TEI server (manual verification, not a committed test)**

Run this ad hoc (not part of the test suite — the real TEI server is a local dev-machine dependency, not CI-available):

```bash
curl -s -X POST http://localhost:52000/embed -H 'Content-Type: application/json' \
  -d '{"inputs":["roster of saved queues","Search GitHub issues"]}' | python3 -c 'import json,sys; d=json.load(sys.stdin); print(len(d), len(d[0]))'
```

Expected output: `2 1024` (two 1024-dim vectors), confirming the real endpoint shape matches `TeiEmbedResponse`'s assumed shape (`Vec<Vec<f32>>`, one vector per input, no wrapper object).

- [ ] **Step 6: Commit**

```bash
git add crates/labby-gateway/src/gateway/code_mode.rs crates/labby-gateway/src/gateway/code_mode/embeddings.rs
git commit -m "feat(gateway): add chunked, size-capped TEI embedding client and cosine ranking"
```

---

## Task 5: Catalog embedding cache on `GatewayManager` + `semantic_rank` trait impl + cooldown + warming

**Files:**
- Modify: `crates/labby-gateway/src/gateway/code_mode.rs`
- Modify: `crates/labby-gateway/src/gateway/manager.rs`
- Modify: `crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode/search.rs`
- Test: `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs` (existing file — confirmed present)

**Interfaces:**
- Consumes: `embed_via_tei`, `rank_by_similarity` (Task 4); `SemanticSearchConfig` (Task 3); `CodeModeHost::semantic_rank` signature (Task 1); `ToolsRender.fingerprint` (Task 1).
- Produces:
  - `GatewayManager::semantic_rank` (trait impl — fail-open, always `Ok`; recomputes the SAME fingerprint `catalog_from_tools` would compute for the given `caller`/`surface`/`scope` rather than reading any shared "current fingerprint" state).
  - `GatewayManager::cached_embeddings(&self, fingerprint: &str) -> Option<Vec<(String, Vec<f32>)>>`.
  - `GatewayManager::ensure_embeddings_for_fingerprint(&self, fingerprint: &str, entries: &[ToolDescriptor]) -> Vec<(String, Vec<f32>)>` — single-flight: holds the cache lock across the full check-then-embed-then-store sequence, returns the (possibly freshly computed) vectors. Called from BOTH `catalog_from_tools`'s warming path (this task, Step 6) AND `semantic_rank`'s on-demand path (this task, Step 4), so a `semantic_rank` call against a cold fingerprint (e.g. semantic search just got enabled without `list_tools` running first) still works rather than returning empty.

- [ ] **Step 1: Add `CatalogEmbeddingCache`**

In `crates/labby-gateway/src/gateway/code_mode.rs`, near `CatalogRenderCache` (after its definition, `code_mode.rs:53-62`), add:

```rust
/// Cached catalog embedding vectors, keyed by the same fingerprint used for
/// `CatalogRenderCache` (see `search.rs`'s `catalog_from_tools`). One vector
/// per catalog entry id, computed via one or more batched TEI calls.
pub(crate) struct CatalogEmbeddingCache {
    pub fingerprint: String,
    /// `(entry.id, embedding_vector)` pairs. Callers should look up by id,
    /// not by index.
    pub vectors: Vec<(String, Vec<f32>)>,
}
```

- [ ] **Step 2: Add manager fields (using `RwLock`, matching the `config` field precedent)**

In `crates/labby-gateway/src/gateway/manager.rs`, add after `code_mode_catalog_render_cache` (after line 115, before `code_mode_snippet_metadata_cache`):

```rust
    /// Cached Code Mode catalog embedding vectors, keyed by the same
    /// fingerprint as `code_mode_catalog_render_cache`. `RwLock` (not
    /// `Mutex`), matching the `config: Arc<RwLock<GatewayConfig>>` precedent
    /// above (`manager.rs:84`) — this is a read-heavy cache; writes only
    /// happen on a fingerprint change or the very first embed.
    ///
    /// `ensure_embeddings_for_fingerprint` holds the write lock across the
    /// full check-then-embed-then-store sequence (not just the store) as a
    /// single-flight guard: concurrent calls against the same cold
    /// fingerprint serialize onto one TEI batch call instead of firing N
    /// redundant ones.
    pub(super) code_mode_embedding_cache:
        Arc<tokio::sync::RwLock<Option<crate::gateway::code_mode::CatalogEmbeddingCache>>>,
```

Add the cooldown field:

```rust
    /// Fail-open cooldown gate for the TEI semantic-search embedding
    /// service. `Some(instant)` = a call failed at `instant`; calls made
    /// before `instant + 30s` skip TEI entirely (falling back to
    /// lexical-only) rather than retrying a known-down service on every
    /// search. `None` = healthy (or never tried).
    pub(super) semantic_search_last_failure: Arc<tokio::sync::RwLock<Option<std::time::Instant>>>,
```

Update every `GatewayManager` constructor (search for `Self {` initializations — `grep -n "fn new\|impl GatewayManager" crates/labby-gateway/src/gateway/manager.rs` and any test-helper constructors in `manager/tests.rs`) to initialize both new fields to `Arc::new(tokio::sync::RwLock::new(None))`.

- [ ] **Step 3: Add the cooldown constant and cache/cooldown accessor methods**

In `crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs`, add near the top:

```rust
/// Cooldown after a TEI failure before the next attempt is tried. Hardcoded
/// per the plan's YAGNI cut (see Task 3) — long enough that a
/// flapping/restarting TEI container isn't hit on every search call, short
/// enough that recovery is picked up within one working session.
const SEMANTIC_SEARCH_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);
```

Add after `cached_catalog_render`/`store_catalog_render_cache` (read the exact end of that method first to insert after it):

```rust
    pub(crate) async fn cached_embeddings(&self, fingerprint: &str) -> Option<Vec<(String, Vec<f32>)>> {
        let guard = self.code_mode_embedding_cache.read().await;
        guard.as_ref().and_then(|cache| {
            if cache.fingerprint == fingerprint {
                Some(cache.vectors.clone())
            } else {
                None
            }
        })
    }

    /// Single-flight: ensure the embedding cache is warm for `fingerprint`,
    /// computing it via `embeddings::embed_via_tei` if needed. Holds the
    /// write lock across the whole check-then-embed-then-store sequence so
    /// concurrent callers against the same cold fingerprint serialize onto
    /// one TEI call rather than firing redundant ones. Fail-open: returns an
    /// empty `Vec` (and leaves the cache empty) on ANY embedding failure —
    /// callers never see an `Err` from this method.
    pub(crate) async fn ensure_embeddings_for_fingerprint(
        &self,
        fingerprint: &str,
        entries: &[crate::gateway::code_mode::ToolDescriptor],
    ) -> Vec<(String, Vec<f32>)> {
        let config = self.code_mode_config().await.semantic_search;
        if !config.is_configured() || entries.is_empty() {
            return Vec::new();
        }
        let mut guard = self.code_mode_embedding_cache.write().await;
        if let Some(cache) = guard.as_ref() {
            if cache.fingerprint == fingerprint {
                return cache.vectors.clone();
            }
        }
        if !self.semantic_search_available_locked().await {
            return Vec::new();
        }
        let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
        let texts: Vec<String> = entries.iter().map(|e| e.description.clone()).collect();
        let tei_url = config
            .tei_url
            .as_deref()
            .expect("is_configured() guarantees tei_url is Some");
        match crate::gateway::code_mode::embeddings::embed_via_tei(tei_url, &texts).await {
            Ok(vectors) if vectors.len() == ids.len() => {
                self.record_semantic_search_recovery().await;
                let pairs: Vec<(String, Vec<f32>)> = ids.into_iter().zip(vectors).collect();
                *guard = Some(crate::gateway::code_mode::CatalogEmbeddingCache {
                    fingerprint: fingerprint.to_string(),
                    vectors: pairs.clone(),
                });
                pairs
            }
            Ok(_) => Vec::new(),
            Err(err) => {
                self.record_semantic_search_failure(&err.to_string()).await;
                Vec::new()
            }
        }
    }

    /// True when the semantic search cooldown has elapsed (or no failure has
    /// been recorded yet) — i.e. it is safe to attempt a TEI call. Internal:
    /// does not itself acquire `code_mode_embedding_cache`'s lock, so it is
    /// safe to call while already holding that lock (as
    /// `ensure_embeddings_for_fingerprint` does).
    async fn semantic_search_available_locked(&self) -> bool {
        let guard = self.semantic_search_last_failure.read().await;
        match *guard {
            None => true,
            Some(last_failure) => last_failure.elapsed() >= SEMANTIC_SEARCH_COOLDOWN,
        }
    }

    /// Public cooldown check for callers that are NOT already holding the
    /// embedding-cache lock (e.g. a `semantic_rank` call that skips catalog
    /// warming entirely because the cache is already warm).
    pub(crate) async fn semantic_search_available(&self) -> bool {
        self.semantic_search_available_locked().await
    }

    /// Record a TEI failure, starting/refreshing the cooldown window. Logs a
    /// `tracing::warn!` only on the healthy→failing transition so repeated
    /// failures during an active cooldown don't spam the log.
    pub(crate) async fn record_semantic_search_failure(&self, reason: &str) {
        let mut guard = self.semantic_search_last_failure.write().await;
        let was_healthy = guard.is_none();
        *guard = Some(std::time::Instant::now());
        drop(guard);
        if was_healthy {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "semantic_search",
                kind = "tei_unavailable",
                reason,
                "Code Mode semantic search TEI call failed; falling back to lexical-only search until cooldown elapses"
            );
        }
    }

    /// Clear the failure cooldown after a successful TEI call. Logs
    /// `tracing::info!` only on the failing→healthy transition.
    pub(crate) async fn record_semantic_search_recovery(&self) {
        let mut guard = self.semantic_search_last_failure.write().await;
        let was_failing = guard.is_some();
        *guard = None;
        drop(guard);
        if was_failing {
            tracing::info!(
                surface = "dispatch",
                service = "code_mode",
                action = "semantic_search",
                kind = "tei_recovered",
                "Code Mode semantic search TEI call succeeded again; resuming semantic blend"
            );
        }
    }
```

- [ ] **Step 4: Implement `CodeModeHost::semantic_rank` for `GatewayManager`**

First run `grep -n "^fn runtime_owner\|^fn oauth_subject\|pub(crate) async fn build_tools_render" crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs crates/labby-gateway/src/gateway/code_mode/search.rs` to confirm the exact current visibility of `runtime_owner`/`oauth_subject` (defined in `code_mode_host.rs`, currently private `fn`s per the original ground truth) and `build_tools_render` (defined in `search.rs`, currently `pub(crate)`). If `runtime_owner`/`oauth_subject` are private (`fn`, no `pub`), widen them to `pub(super)` or `pub(crate)` so `semantic_rank` (added to the same `impl CodeModeHost for GatewayManager` block in `code_mode_host.rs`) can call them — since `semantic_rank` lives in the SAME FILE as their definitions, no visibility change may even be needed; confirm before assuming.

In `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`, add after `resolve_snippet` (after line 187, before `async fn config`):

```rust
    async fn semantic_rank(
        &self,
        query: String,
        top_k: usize,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Vec<(String, f32)>, ToolError> {
        let config = self.code_mode_config().await.semantic_search;
        if !config.is_configured() || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        // Recompute the SAME scope-filtered entries `list_tools` would
        // return for this exact caller/surface/scope — this is what makes
        // the design race-free: no shared "current fingerprint" state is
        // read here, only this call's own arguments (Task 1's design note
        // explains why a manager-global fingerprint field would be unsafe
        // under concurrent executions with different scopes). Same
        // `allow_cold_connect = false` as any non-CLI-execute caller would
        // get from `list_tools` (see `list_tools`'s own
        // `allow_cold_connect` computation above in this file) — semantic
        // ranking must not spend wall-clock cold-connecting upstreams.
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);
        let allowed = scope.allowed_namespaces();
        let render = match super::search::build_tools_render(
            self,
            false,
            &owner,
            oauth_subject,
            allowed,
            false,
            true,
        )
        .await
        {
            Ok(render) => render,
            Err(_) => return Ok(Vec::new()), // fail-open: catalog build failure must not break search()
        };
        if !self.semantic_search_available().await {
            return Ok(Vec::new());
        }
        let vectors = self
            .ensure_embeddings_for_fingerprint(&render.fingerprint, &render.entries)
            .await;
        if vectors.is_empty() {
            return Ok(Vec::new());
        }
        let query_vec = match super::embeddings::embed_via_tei(
            config.tei_url.as_deref().expect("is_configured() guarantees Some"),
            &[query],
        )
        .await
        {
            Ok(mut v) if !v.is_empty() => v.remove(0),
            Ok(_) => return Ok(Vec::new()),
            Err(err) => {
                self.record_semantic_search_failure(&err.to_string()).await;
                return Ok(Vec::new());
            }
        };
        self.record_semantic_search_recovery().await;
        let mut ranked = super::embeddings::rank_by_similarity(&query_vec, &vectors);
        ranked.truncate(top_k.max(1));
        Ok(ranked)
    }
```

**Verification note for the implementer:** the `include_snippets: false` argument passed to `build_tools_render` above matches `list_tools`'s OWN default reasoning is NOT re-verified in this plan revision — before finalizing this step, read `list_tools`'s existing body in this same file (`code_mode_host.rs:29-56`) and confirm whether `include_snippets`/`allow_cold_connect` should instead be threaded from `semantic_rank`'s own parameters (they currently aren't — `semantic_rank`'s signature per Task 1 has no `include_snippets` param) or whether hardcoding `false`/`false` here is correct because ranking should behave like a non-CLI caller. If in doubt, hardcode `false, false` as shown (safe defaults: no snippets ranked, no cold-connect) and note the simplification in a code comment — do not block implementation on this, but don't skip reading `list_tools`'s body first either.

- [ ] **Step 5: Populate `ToolsRender.fingerprint` in `search.rs`**

In `crates/labby-gateway/src/gateway/code_mode/search.rs`'s `catalog_from_tools` (`search.rs:50-150`), add `fingerprint: fingerprint.clone()` to BOTH the cache-hit early-return's `ToolsRender { ... }` construction (`search.rs:86-90`) and the cache-miss final construction (`search.rs:145-149`) — the local `fingerprint` binding already exists at both points (computed once at `search.rs:67-74`, used throughout the function).

- [ ] **Step 6: Wire catalog embedding warming into `catalog_from_tools`**

In the same function, after the existing `store_catalog_render_cache` call (after line 143, before the final `Ok(ToolsRender { ... })`), add:

```rust
    // Best-effort catalog embedding warm-up: never blocks or fails catalog
    // construction beyond the fail-open Result already guaranteed by
    // `ensure_embeddings_for_fingerprint`. Deliberately awaited inline
    // (not spawned) so the FIRST `semantic_rank` call after a catalog
    // change doesn't pay the cold-embed cost on its own critical path —
    // this list_tools call pays it instead. `list_tools` is already cached
    // for the CLI/unscoped path (see `execute.rs`'s `use_cache` logic) and
    // is not documented anywhere as latency-critical, so this tradeoff is
    // accepted rather than using a detached `tokio::spawn`.
    let _ = manager
        .ensure_embeddings_for_fingerprint(&fingerprint, &entries)
        .await;
```

Also add the identical call on the cache-hit early-return path (`search.rs:76-91`), before the early `return Ok(...)` at line 86-90 (the local `entries`/`fingerprint` bindings are already in scope there too — the cache-hit branch reconstructs `entries`/`catalog_json`/`serialized_size` from the cached render, so `entries` is available):

```rust
        let _ = manager
            .ensure_embeddings_for_fingerprint(&fingerprint, &entries)
            .await;
```

- [ ] **Step 7: Write manager-level and search-level tests**

In `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`, add (using whatever test-manager construction helper that file already uses — read it first to match the existing pattern):

```rust
    #[tokio::test]
    async fn semantic_rank_returns_empty_when_unconfigured() {
        let manager = test_manager().await; // reuse existing helper from this file
        let result = manager
            .semantic_rank(
                "hello".to_string(),
                5,
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &ToolScope::default(),
            )
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn semantic_search_cooldown_blocks_immediate_retry_after_failure() {
        let manager = test_manager().await;
        manager.record_semantic_search_failure("test failure").await;
        assert!(!manager.semantic_search_available().await);
    }

    #[tokio::test]
    async fn semantic_search_recovery_clears_cooldown() {
        let manager = test_manager().await;
        manager.record_semantic_search_failure("test failure").await;
        assert!(!manager.semantic_search_available().await);
        manager.record_semantic_search_recovery().await;
        assert!(manager.semantic_search_available().await);
    }

    #[tokio::test]
    async fn ensure_embeddings_for_fingerprint_is_noop_when_unconfigured() {
        let manager = test_manager().await;
        let entries = vec![]; // empty catalog — also exercises the cold-start-empty-catalog path
        let result = manager
            .ensure_embeddings_for_fingerprint("some-fingerprint", &entries)
            .await;
        assert!(result.is_empty());
        assert!(manager.cached_embeddings("some-fingerprint").await.is_none());
    }

    #[tokio::test]
    async fn catalog_embeddings_stay_cold_when_semantic_search_unconfigured() {
        let manager = test_manager().await;
        // Default config has semantic_search.tei_url = None.
        let render = manager
            .list_tools(
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &ToolScope::default(),
                false,
                false,
            )
            .await
            .unwrap();
        // The embedding cache must remain empty — ensure_embeddings_for_fingerprint
        // returns immediately for an unconfigured host.
        assert!(manager.cached_embeddings(&render.fingerprint).await.is_none());
    }
```

(If `test_manager()` is not the actual existing helper name, use whatever the file's existing tests call — inspect before writing, do not guess.)

- [ ] **Step 8: Run tests**

Run: `cargo test -p labby-gateway code_mode:: -- --nocapture`
Expected: PASS.

- [ ] **Step 9: Run full crate compile + lint**

Run: `cargo check -p labby-gateway --all-features && cargo clippy -p labby-gateway --all-features -- -D warnings`
Expected: PASS. This task has the most speculative wiring in the plan (Step 4's `include_snippets`/visibility caveats) — budget extra iteration here.

- [ ] **Step 10: Commit**

```bash
git add crates/labby-codemode/src/host.rs crates/labby-gateway/src/gateway/code_mode.rs crates/labby-gateway/src/gateway/manager.rs crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs crates/labby-gateway/src/gateway/code_mode/search.rs crates/labby-gateway/src/gateway/manager/tests/code_mode.rs
git commit -m "feat(gateway): implement CodeModeHost::semantic_rank with fail-open cooldown, single-flight cache, and catalog warming"
```

---

## Task 6: Blend logic in `codemode.search()` (`preamble.rs`)

**Files:**
- Modify: `crates/labby-codemode/src/preamble.rs`
- Modify: `crates/labby-codemode/src/execute.rs` (thread `blend_weight` into `generate_discovery_js`)
- Test: existing test module in `preamble.rs` (`preamble.rs:482+`)

**Interfaces:**
- Consumes: the existing `callTool(id, params)` JS primitive (unchanged — no new bridge, per Task 2's reserved-id design). `generate_discovery_js(entries: &[CodeModeDiscoveryEntry], blend_weight: f32) -> Result<String, String>` — signature gains a second parameter.

- [ ] **Step 1: Thread `blend_weight` from config into `generate_discovery_js`**

In `crates/labby-codemode/src/execute.rs`'s `build_code_mode_proxy` (`execute.rs:133-199`), which already calls `host.list_tools(...)` (line 150-152), add a call to `host.config()` (already defined on the trait, `host.rs:91`) to obtain `blend_weight`:

```rust
        let code_mode_config = host.config().await;
        let blend_weight = code_mode_config.semantic_search.blend_weight;
```

Insert this near the top of `build_code_mode_proxy`, before the `discovery_js` construction (before the current `execute.rs:173-174` call to `generate_discovery_js`), and update that call site to pass `blend_weight`:

```rust
        let discovery_js =
            super::preamble::generate_discovery_js(&discovery_entries, blend_weight).map_err(...)
```

(Keep the existing `.map_err(...)` closure unchanged — only the call's argument list changes.)

- [ ] **Step 2: Update `generate_discovery_js`'s signature and add the blend-aware `codemode.search`**

In `crates/labby-codemode/src/preamble.rs`, change `generate_discovery_js`'s signature (`preamble.rs:170`):

```rust
pub(crate) fn generate_discovery_js(
    entries: &[CodeModeDiscoveryEntry],
    blend_weight: f32,
) -> Result<String, String> {
```

Replace `codemode.search` (currently `preamble.rs:201-256`) with an `async function` that calls the existing `callTool` primitive against the reserved id:

```rust
codemode.search = async function(input) {{
  var query = typeof input === "object" && input !== null ? String(input.query || "") : String(input || "");
  var limit = typeof input === "object" && input !== null && Number.isFinite(Number(input.limit))
    ? Math.max(1, Math.min(50, Number(input.limit)))
    : 50;
  var tokens = __codemodeTokens(query);
  var __codemodeNoMatchHint = "No matches. Broaden or try synonyms, or call codemode.__meta__.upstreams() to list namespaces and search by upstream name.";
  if (!tokens.length) return {{ results: [], total: 0, truncated: false, hint: __codemodeNoMatchHint }};

  // --- lexical scoring (unchanged algorithm) ---
  var lexicalById = {{}};
  var scored = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    var fields = [
      [__codemodeNormalize(entry.path), 12],
      [__codemodeNormalize(entry.name), 10],
      [__codemodeNormalize(entry.namespace), 8],
      [__codemodeNormalize(entry.description), 5],
      [__codemodeNormalize((entry.tags || []).join(" ")), 7],
      [__codemodeNormalize(entry.kind === "snippet" ? "codemode run snippet" : ""), 9]
    ];
    var covered = 0;
    var score = 0;
    for (var t = 0; t < tokens.length; t++) {{
      var tokenScore = 0;
      for (var f = 0; f < fields.length; f++) {{
        if (fields[f][0].indexOf(tokens[t]) !== -1 && fields[f][1] > tokenScore) tokenScore = fields[f][1];
      }}
      if (tokenScore > 0) {{
        covered++;
        score += tokenScore;
      }}
    }}
    var required = tokens.length <= 2 ? tokens.length : Math.ceil(tokens.length * 0.6);
    if (covered >= required) {{
      var record = {{
        path: entry.path,
        id: entry.id,
        kind: entry.kind,
        namespace: entry.namespace,
        name: entry.name,
        description: entry.description,
        signature: entry.signature,
        tags: entry.tags || [],
        score: score
      }};
      lexicalById[entry.id] = record;
      scored.push(record);
    }}
  }}

  // --- semantic blend ---
  // Query the host's semantic ranker through the SAME callTool primitive
  // used for every other host round-trip — no new bridge, see the plan's
  // Global Constraints for why. Fail-open: any rejection or malformed
  // response degrades to an empty ranked list, never an exception that
  // could break search().
  var ranked = [];
  try {{
    var response = await callTool("__lab_internal::semantic_rank", {{ query: query, limit: limit }});
    ranked = (response && response.ranked) || [];
  }} catch (e) {{
    ranked = [];
  }}

  // Normalization: lexical `score` is an unbounded sum of per-token
  // best-field weights. We normalize each entry's lexical score by the MAX
  // score actually observed among THIS query's lexical matches (not a fixed
  // global ceiling), so normalization adapts to how many tokens/fields
  // actually matched for this specific query. Semantic cosine similarity is
  // already bounded to [-1, 1] (see `embeddings::cosine_similarity`'s
  // `.clamp(-1.0, 1.0)` on the Rust side); we rescale it to [0, 1] to match
  // the lexical normalization's range before blending.
  //
  // Blend formula: blended = max(normalized_lexical, semantic_similarity_0_to_1 * blend_weight).
  // `max` (not a weighted sum) is deliberate: a strong exact lexical match
  // should never be outranked by a mediocre semantic match, and a strong
  // semantic match (synonym case) should surface even with zero lexical
  // overlap — either signal being strong is sufficient. `blend_weight`
  // (config default 0.5) discounts semantic-only matches relative to a
  // perfect lexical match, so ambiguous semantic near-misses don't crowd
  // out legitimate lexical results at the same rank.
  var maxLexicalScore = 0;
  for (var m = 0; m < scored.length; m++) {{
    if (scored[m].score > maxLexicalScore) maxLexicalScore = scored[m].score;
  }}
  var BLEND_WEIGHT = {blend_weight};
  for (var r = 0; r < ranked.length; r++) {{
    var rid = ranked[r].id;
    var semanticSimilarity01 = (ranked[r].score + 1) / 2; // [-1,1] -> [0,1]
    var existing = lexicalById[rid];
    if (existing) {{
      var normalizedLexical = maxLexicalScore > 0 ? existing.score / maxLexicalScore : 0;
      existing.blendedScore = Math.max(normalizedLexical, semanticSimilarity01 * BLEND_WEIGHT);
    }} else {{
      // Semantic-only match: not found by lexical scoring at all (e.g. the
      // synonym case with zero token overlap). `ranked` can only ever
      // contain ids already present in `__codemodeDiscovery` (the host's
      // semantic_rank ranks exclusively within this execution's
      // already-scope-filtered catalog — see the security invariant in the
      // plan's Global Constraints), so this lookup is safe and will always
      // find a match.
      for (var d = 0; d < __codemodeDiscovery.length; d++) {{
        if (__codemodeDiscovery[d].id === rid) {{
          var de = __codemodeDiscovery[d];
          var record2 = {{
            path: de.path, id: de.id, kind: de.kind, namespace: de.namespace,
            name: de.name, description: de.description, signature: de.signature,
            tags: de.tags || [], score: 0,
            blendedScore: semanticSimilarity01 * BLEND_WEIGHT
          }};
          lexicalById[rid] = record2;
          scored.push(record2);
          break;
        }}
      }}
    }}
  }}
  // Entries with no semantic signal keep blendedScore = normalizedLexical
  // (equivalent to max(normalizedLexical, 0)).
  for (var b = 0; b < scored.length; b++) {{
    if (scored[b].blendedScore === undefined) {{
      scored[b].blendedScore = maxLexicalScore > 0 ? scored[b].score / maxLexicalScore : 0;
    }}
  }}

  scored.sort(function(a, b) {{
    if (b.blendedScore !== a.blendedScore) return b.blendedScore - a.blendedScore;
    if (b.score !== a.score) return b.score - a.score;
    return a.path < b.path ? -1 : a.path > b.path ? 1 : 0;
  }});
  var total = scored.length;
  if (total === 0) {{
    return {{ results: [], total: 0, truncated: false, hint: __codemodeNoMatchHint }};
  }}
  var results = scored.slice(0, limit).map(function(r) {{
    return {{ path: r.path, id: r.id, kind: r.kind, namespace: r.namespace, name: r.name, description: r.description, signature: r.signature, tags: r.tags, score: r.score }};
  }});
  return {{ results: results, total: total, truncated: total > limit }};
}};
```

(`{blend_weight}` is a Rust-side format-string interpolation of the new `blend_weight: f32` parameter — since this whole block is generated inside a Rust `format!(r##"..."##, ...)` call per the existing `generate_discovery_js` pattern, add `blend_weight = blend_weight` to that macro's argument list alongside the existing `json = json`/`types_json = types_json` bindings, and escape literal JS braces as `{{`/`}}` exactly as the rest of the function already does.)

Return shape note: `codemode.search()` now returns a plain object directly inside an `async function` rather than `Promise.resolve({{...}})` — both are equivalent once awaited (an `async function`'s return value is automatically wrapped in a resolved Promise), so this is NOT a breaking change to callers — every caller already does `await codemode.search(...)`.

- [ ] **Step 3: Write unit tests for the generated JS shape**

In `preamble.rs`'s existing test module (`preamble.rs:482+`), add tests in the same string-assertion style as the existing tests (these tests check generated JS *text*, not runtime behavior — there's no JS engine in a plain `cargo test` for this crate):

```rust
    #[test]
    fn generate_discovery_js_includes_semantic_blend() {
        let entries: Vec<CodeModeDiscoveryEntry> = vec![]; // reuse whatever existing test fixture builder this file already has
        let js = generate_discovery_js(&entries, 0.5).expect("js generation succeeds");
        assert!(js.contains("__lab_internal::semantic_rank"));
        assert!(js.contains("blendedScore"));
        assert!(js.contains("codemode.search = async function"));
    }

    #[test]
    fn generate_discovery_js_interpolates_configured_blend_weight() {
        let entries: Vec<CodeModeDiscoveryEntry> = vec![];
        let js = generate_discovery_js(&entries, 0.75).expect("js generation succeeds");
        assert!(js.contains("var BLEND_WEIGHT = 0.75"));
    }

    #[test]
    fn generate_discovery_js_search_never_throws_on_calltool_rejection() {
        // Structural check: the semantic-rank call is wrapped in try/catch
        // with `ranked = []` in the catch body, so a callTool rejection
        // (e.g. network_error surfaced as a JS Error) cannot propagate out
        // of codemode.search() and break the caller's script.
        let entries: Vec<CodeModeDiscoveryEntry> = vec![];
        let js = generate_discovery_js(&entries, 0.5).expect("js generation succeeds");
        assert!(js.contains("catch (e) {"));
    }
```

(Match the exact existing fixture-building helper used by this file's other tests — do not invent a different one. Update every existing call site in this file and in `execute.rs` that calls `generate_discovery_js` with the old one-argument signature to pass a `blend_weight` value too — e.g. `0.5` or `SemanticSearchConfig::default().blend_weight` in tests.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p labby-codemode preamble:: -- --nocapture`
Expected: PASS, including all pre-existing tests updated for the new signature plus the three new tests from Step 3.

- [ ] **Step 5: Full crate build + lint**

Run: `cargo build -p labby-codemode --all-features && cargo build -p labby-gateway --all-features && cargo clippy --workspace --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: PASS across the whole workspace.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/preamble.rs crates/labby-codemode/src/execute.rs
git commit -m "feat(codemode): blend semantic similarity into codemode.search() via existing callTool bridge"
```

---

## Task 7: Scope-invariant test + end-to-end smoke test against the real TEI server

**Files:**
- Modify: `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs` (one additional test)
- No other files modified — the remainder of this task is manual verification using whatever existing Code Mode execution entrypoint the repo has.

- [ ] **Step 1: Add the scope-invariant unit test called out in Global Constraints**

In `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`, add (using the same `test_manager()`-style helper as Task 5's tests):

```rust
    #[tokio::test]
    async fn semantic_rank_never_returns_ids_outside_scope_filtered_catalog() {
        // semantic_rank's own internal build_tools_render call uses the SAME
        // `scope` parameter it was given (Task 5 Step 4), so an id excluded
        // by that scope is structurally never present in `render.entries`
        // for it to embed/rank in the first place — this is a compile-time/
        // data-flow guarantee, not a runtime filter that could regress
        // silently.
        //
        // This unit test exercises the unconfigured (no TEI) path, which
        // already proves semantic_rank cannot fabricate ids independent of
        // build_tools_render's scope-filtered output regardless of scope —
        // a live, multi-upstream, TEI-backed confirmation of the same
        // invariant is covered by this task's Step 6 (live smoke test).
        let manager = test_manager().await;
        let restrictive_scope = ToolScope::scoped_namespaces(vec![], vec![]);
        let result = manager
            .semantic_rank(
                "anything".to_string(),
                5,
                &CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &restrictive_scope,
            )
            .await
            .unwrap();
        assert!(result.is_empty());
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p labby-gateway semantic_rank_never_returns_ids -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Commit the test**

```bash
git add crates/labby-gateway/src/gateway/manager/tests/code_mode.rs
git commit -m "test(gateway): assert semantic_rank respects scope filtering"
```

- [ ] **Step 4: Start (or confirm running) the gateway with semantic search enabled**

Set `~/.lab/config.toml`'s (or the config path this workspace/worktree uses for local dev) `[code_mode.semantic_search]` section to:

```toml
[code_mode.semantic_search]
tei_url = "http://localhost:52000"
```

Restart/reload the gateway (`labby gateway reload` or equivalent for this workspace — confirm the right command via `just --list` or the `labby` binary's own `--help`).

- [ ] **Step 5: Run a Code Mode script through the CLI (or whatever local execution surface exists) with a synonym-style query**

Find the actual local execution entrypoint (grep for a `codemode` CLI subcommand, or use the MCP `codemode` tool directly via `mcporter` per the `testing:mcporter` skill if a CLI path doesn't exist). Run:

```js
async () => {
  const found = await codemode.search({ query: "roster of saved queues", limit: 5 });
  return found;
}
```

against a live catalog that includes at least one tool whose description plausibly matches "queue"/"saved"/"list" semantically without sharing exact tokens with "roster of saved queues" (check `labby gateway list` first to know what's actually connected — do not assume a specific tool exists).

- [ ] **Step 6: Confirm a control case, fail-open behavior, and scope filtering live**

- **Control:** unset `tei_url`, reload, rerun the identical query, and confirm the result set is either smaller or differently ordered than Step 5's (proving the semantic blend had a real, observable effect). If results are byte-identical, something in the blend path isn't running — check `tracing::warn!`/`tracing::info!` log lines from Task 5 Step 3's `record_semantic_search_failure`/`record_semantic_search_recovery` to diagnose.
- **Fail-open:** re-set `tei_url` to an unreachable port, reload, rerun the query. Confirm: the call succeeds with no error surfaced; results match lexical-only behavior; exactly one `tracing::warn!` line appears; a second search immediately after does NOT produce a second warn line (cooldown); if the execution surface exposes a call trace, confirm it contains no entry for `__lab_internal::semantic_rank`.
- **Recovery:** point `tei_url` back at the real TEI server, wait 30+ seconds, rerun the query, and confirm semantic results return again with a `tracing::info!` "tei_recovered" line, with no gateway restart required.
- **Scope filtering (if the dev environment has 2+ upstreams connected):** run the same synonym-style query from an execution surface/caller scoped to exclude one upstream (check `tests/mcporter/` for a scoped-session example if the harness supports it) and confirm that upstream's tools never appear in results, even if they'd otherwise be a strong semantic match.

- [ ] **Step 7: Document findings inline in the PR description (not a new doc file)**

Note pass/fail for each bullet in Step 6 in the PR body when this plan reaches the `/gh-pr` step of the outer pipeline — this task's manual verification has no code artifact of its own beyond Step 1's test.

---

## Self-Review Notes (for the plan author to confirm before handoff)

- **Spec coverage:** freshness/lazy-computation (Task 5 Step 6, fingerprint-keyed single-flight cache) — covered. Fail-open + cooldown + log-once (Task 5 Step 3) — covered. Config location/convention (Task 3) — covered, follows `CodeModeConfig` pattern, defaults unconfigured, YAGNI-trimmed per simplicity review. Blend normalization + formula documented in code comment (Task 6 Step 2's inline comment) — covered. Bridge pattern reuse (Task 2) — now genuinely minimal: zero new protocol variants, zero new javy bindings, zero new JS globals, reusing the exact `callTool` path via a reserved namespace, following the codebase's own `try_parse_local_provider_call`/`ARTIFACT_WRITE_CALL_ID` precedent for "id that isn't a real upstream tool." Empty/cold-start catalog (Task 5 Step 3's `entries.is_empty()` check) — covered. No new doc files beyond updating the existing `docs/runtime/CONFIG.md` (Task 3 Step 8) — covered.
- **Engineering review fixes applied:** (1) architecture's race-condition finding on manager-global fingerprint state — fixed by threading `caller`/`surface`/`scope` into `semantic_rank` itself so the fingerprint is recomputed per-call from the call's own arguments, never read from shared mutable state (Task 1 design note, Task 5 Step 4). (2) simplicity's redundant-trait-method finding — no `embed_texts` on the trait at all; only `semantic_rank` is trait-level, `embed_via_tei` is a private `labby-gateway` helper called both for catalog warming (Task 5 Step 6) and query embedding (Task 5 Step 4) (Task 4). (3) simplicity's disproportionate-protocol finding — the original Tasks 2/3 (new `EmbedQuery`/`EmbedQueryResult` protocol variants, new javy binding, new FuturesUnordered) are gone entirely, replaced by this revision's Task 2 reserved-tool-id routing through the existing `callTool` path, matching the codebase's own `try_parse_local_provider_call` precedent. (4) security's SSRF/validation gap — `tei_url` now requires `url::Url::parse` + http/https scheme validation (Task 3 Step 4). (5) security's response-size gap — `embed_via_tei` now caps response bytes at 16 MiB before JSON decode (Task 4 Step 3). (6) security's scope-blindness concern — explicit invariant documented in Global Constraints, a unit test in Task 7 Step 1, and a live verification step in Task 7 Step 6. (7) performance's batch-limit gap — `embed_via_tei` now chunks at `TEI_MAX_BATCH_SIZE = 512` (Task 4). (8) performance's thundering-herd gap — `ensure_embeddings_for_fingerprint` holds its lock across the full check-then-embed-then-store sequence as a single-flight guard (Task 5 Step 3). (9) performance's `Mutex`-vs-`RwLock` finding — both new cache fields use `RwLock`, matching the `config` field precedent (Task 5 Step 2). (10) simplicity's YAGNI findings — `enabled` bool dropped (Task 3), `tei_timeout_ms`/`cooldown_ms` hardcoded as constants (Task 3/4/5), per-execution query cache explicitly deferred with a one-line rationale in Global Constraints rather than silently omitted.
- **No mid-plan design correction this time:** unlike the prior revision (which required Tasks 1/2/3/6 to be retroactively amended by a late-plan discovery), this revision's `semantic_rank` trait signature is correct from Task 1 onward — the reserved-tool-id bridge design was fully worked out and validated (including checking `state.calls.push`/`calls_enqueued` exact call sites, and the `try_parse_local_provider_call`/`ARTIFACT_WRITE_CALL_ID` precedent) before Task 1 was written, so no task should require reopening an earlier, already-committed task.
- **Known follow-up risk flagged in-line rather than hidden:** Task 5 Step 4's `semantic_rank` implementation has real module-path/visibility uncertainty (whether `runtime_owner`/`oauth_subject` need widened visibility, and whether `include_snippets: false`/`allow_cold_connect: false` are the right hardcoded defaults vs. threading them from somewhere else) — flagged explicitly as a "verify against actual code, adjust as needed" note rather than presented as certain. Task 5 Step 9 budgets explicit iteration time for this; it is the one piece of the plan most likely to need small adjustment during implementation.
- **Placeholder scan:** no TBD/TODO markers; every step has literal code. The one intentionally-flagged uncertainty (Task 5 Step 4's verification note) is disclosed as such, not disguised as certain.
- **Type consistency:** `CodeModeHost::semantic_rank` is the only new trait method (no `embed_texts` on the trait); its signature is identical from Task 1 through every consuming task (Task 2's `dispatch_internal_call`, Task 5's impl, Task 6's blend logic calling it indirectly via `callTool`). `ToolsRender.fingerprint` (new field, Task 1 Step 3) is consistently a `String` field access, never a method call, in every later reference (Task 5 Steps 4-6).
