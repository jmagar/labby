# Code Mode Semantic Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Blend semantic (embedding-based) similarity into `codemode.search()` so agents whose queries use synonyms rather than exact catalog vocabulary (e.g. "roster of saved queues" for a tool literally named/described differently) still surface the right tool, while preserving today's lexical-only behavior byte-for-byte when the TEI embedding service is unset, unreachable, or cold.

**Architecture:** Add a new `CodeModeHost::embed_texts` async trait method (client-neutral, `labby-codemode`), implemented in `labby-gateway` with a small reqwest-based TEI client. Catalog vectors are computed once per distinct catalog fingerprint (reusing the *existing* `fingerprint` string already computed in `crates/labby-gateway/src/gateway/code_mode/search.rs:67-74` for the render cache) and cached in-process on `GatewayManager`, mirroring the existing `code_mode_catalog_render_cache` pattern exactly. The one genuinely new per-search-call cost — embedding the *query string*, which is sandbox-runtime data unknown until the agent's JS calls `codemode.search(...)` — is bridged from the QuickJS/Javy sandbox to Tokio via a new, deliberately minimal protocol variant pair (`EmbedQuery` / `EmbedQueryResult`) that reuses the exact `ToolCall`/`ToolResult` round-trip machinery in `runner_drive.rs`/`runner.rs`/`protocol.rs`, but is dispatched outside `DriveState.calls`/`max_calls_per_run` bookkeeping so it never pollutes the agent-visible call trace or consumes the tool-call budget. Rust owns 100% of the vector math (cosine similarity); no raw floats are ever serialized into the sandbox. Failures anywhere in the embedding path fail open to today's pure-lexical behavior, gated by a 30s cooldown tracked on the manager so a TEI outage is retried automatically without hammering it every call.

**Tech Stack:** Rust 2024, Tokio, reqwest (already a `labby-gateway` dependency), serde/serde_json, Javy/QuickJS sandbox (`labby-codemode`), TOML config (`labby-runtime::gateway_config`), TEI (Text Embeddings Inference) HTTP API.

## Global Constraints

- `labby-codemode` stays client-neutral: it must gain no `reqwest`, TEI, or gateway-specific vocabulary. The `CodeModeHost::embed_texts` trait method takes/returns only neutral types (`Vec<String>` in, `Result<Vec<Vec<f32>>, ToolError>` out).
- The embedding path is fail-open, always. No TEI failure, timeout, or misconfiguration may ever surface a different error, a different response shape, or a hang to the agent calling `codemode.search()` — it must silently degrade to exactly today's lexical-only ranking.
- No behavioral change when semantic search is disabled (default) or when `code_mode.semantic_search.tei_url` is unset — `codemode.search()`'s lexical algorithm (`preamble.rs:201-256`) and its return shape (`{results, total, truncated, hint?}`) are unchanged in that case.
- Cooldown after a TEI failure is 30s (`SEMANTIC_SEARCH_COOLDOWN: Duration = Duration::from_secs(30)`), tracked via `Instant` behind a `Mutex` on `GatewayManager`. Rationale: long enough that a flapping/restarting TEI container doesn't get hammered every search call (searches can happen many times per Code Mode execution), short enough that recovery is picked up within one typical agent working session without requiring a gateway restart.
- `tracing::warn!` fires exactly once per failure *transition* (healthy→cooldown), not on every skipped call during an active cooldown window. Recovery (a call succeeding again after cooldown) does not need its own log line but may optionally emit `tracing::info!` — pick one and be consistent.
- The new protocol variant pair must NOT touch `DriveState.calls`, `calls_enqueued`, or `max_calls_per_run` — an embed-query round-trip is host-internal plumbing, not a Code Mode tool call, and must not appear in `response.calls` or count against the per-run call budget.
- Catalog embedding is computed from `CodeModeDiscoveryEntry.description` only (see Task 4 rationale) via exactly one batched `POST /embed` call per distinct catalog fingerprint — never one HTTP call per entry.
- Follow existing repo conventions: `#[serde(default = "default_xxx")]` + free-fn default helpers for new `CodeModeConfig` fields (see `crates/labby-runtime/src/gateway_config.rs:34-56`); range validation added to `CodeModeConfig::validate()` (`gateway_config.rs:137-171`) with new `ConfigError` variants following the existing `InvalidCodeMode*` naming (`gateway_config.rs:716-727`); fingerprint-keyed `Arc<Mutex<Option<Cache>>>` cache on `GatewayManager` mirroring `code_mode_catalog_render_cache` (`crates/labby-gateway/src/gateway/manager.rs:109-115`) and its accessor methods (`crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs:502-529`).
- Update `docs/runtime/CONFIG.md`'s `### [code_mode]` section (currently `docs/runtime/CONFIG.md:261-292`) with the new nested config keys and defaults. Do not create any new standalone `*.md` files — this is the one doc that already documents `[code_mode]` keys and is the correct place per repo convention.
- Build/lint/test gates: `cargo nextest run --workspace --all-features` (test), `cargo clippy --workspace --all-features -- -D warnings` + `cargo fmt --all -- --check` (lint) — see `Justfile` `test`/`lint` targets. Crate-scoped equivalents: `cargo test -p labby-codemode`, `cargo test -p labby-gateway`.

---

## File Structure

- `crates/labby-codemode/src/host.rs`
  - Modify: add `embed_texts` method to the `CodeModeHost` trait (~after `resolve_snippet`, before `config`); add a no-op impl on `NoopHost` (test-only, `#[cfg(test)]`) returning `Ok(vec![])`.
- `crates/labby-codemode/src/protocol.rs`
  - Modify: add `EmbedQuery { seq: u64, text: String }` to `CodeModeRunnerOutput`; add `EmbedQueryResult { seq: u64, vector: Option<Vec<f32>> }` to `CodeModeRunnerInput`. `vector: None` signals "semantic scoring unavailable for this call" (fail-open at the wire level too, not just at the HTTP-client level).
- `crates/labby-codemode/src/runner.rs`
  - Modify: add a `globalThis.__labEmitEmbedQuery` javy host-function binding (mirrors `javy_emit_tool_call`, ~runner.rs:522-564) and a small JS-side `globalThis.embedQueryText = (text) => new Promise(...)` bridge (mirrors the `callTool` JS bridge at runner.rs:402-413), registered in the same place the other `__labEmit*` bindings are registered (~runner.rs:313-340) and wired into `wrap_code_mode`'s generated preamble (~runner.rs:394-504) so it settles through the *existing* `__labSettlePendingOperation` dispatcher (runner.rs:455-503) by adding one more `input.type` arm (`"embed_query_result"`).
- `crates/labby-codemode/src/runner_drive.rs`
  - Modify: `drive_runner`'s `tokio::select!` match (currently `runner_drive.rs:327-421` handling `ToolCall`/`ArtifactWrite`/`SnippetResolve`) gains an `EmbedQuery { seq, text }` arm that enqueues a future calling `broker.host.embed_texts(vec![text])` (via a new small helper, NOT `enqueue_tool_call`, so it never touches `DriveState.calls`/`calls_enqueued`) onto a *separate* `FuturesUnordered` (or the same one with a tagged variant — see Task 2 step-by-step for the exact shape) and writes back `CodeModeRunnerInput::EmbedQueryResult { seq, vector }` (vector `None` on any error) once resolved.
- `crates/labby-codemode/src/execute.rs`
  - No changes needed to `call_tool_id`/`build_code_mode_proxy` signatures; `embed_texts` for the *catalog* is called by the host (`labby-gateway`) inside its own `list_tools` implementation before `generate_discovery_js` runs (see Task 4), not through `execute.rs`'s tool-call path at all.
- `crates/labby-codemode/src/preamble.rs`
  - Modify: `generate_discovery_js` (`preamble.rs:170-333`) gains a new injected JSON payload `__codemodeSemanticEnabled` (bool) alongside the existing `__codemodeDiscovery`/`__codemodeTypes`; `codemode.search`'s body (`preamble.rs:201-256`) gains the blend logic (calls `embedQueryText`, computes cosine similarity against a **host-provided ranked list** rather than raw vectors — see Task 5 for the exact host round-trip shape and why raw vectors never enter the sandbox).
- `crates/labby-runtime/src/gateway_config.rs`
  - Modify: add `SemanticSearchConfig` struct + `semantic_search: SemanticSearchConfig` field on `CodeModeConfig` (~after `max_log_bytes`, `gateway_config.rs:118`); add default-helper free fns near `gateway_config.rs:34-56`; add range validation to `CodeModeConfig::validate()` (`gateway_config.rs:137-171`); add `ConfigError::InvalidSemanticSearchCooldownMs` (or similar) near `gateway_config.rs:716-727`.
- `crates/labby-gateway/Cargo.toml`
  - No change — `reqwest.workspace = true` already present (`Cargo.toml:38`).
- `crates/labby-gateway/src/gateway/code_mode.rs`
  - Modify: add `CatalogEmbeddingCache { fingerprint: String, vectors: Vec<(String, Vec<f32>)> }` struct, mirroring `CatalogRenderCache` (`code_mode.rs:53-62`) exactly (id + vector pairs, keyed by the same `fingerprint` string `search.rs` already computes).
- `crates/labby-gateway/src/gateway/manager.rs`
  - Modify: add `pub(super) code_mode_embedding_cache: Arc<Mutex<Option<crate::gateway::code_mode::CatalogEmbeddingCache>>>` field (next to `code_mode_catalog_render_cache`, `manager.rs:109-115`); add `pub(super) semantic_search_cooldown: Arc<Mutex<Option<Instant>>>` field for the fail-open cooldown tracker.
- `crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs`
  - Modify: add `cached_embeddings(&self, fingerprint: &str) -> Option<Vec<(String, Vec<f32>)>>` and `store_embedding_cache(&self, cache: CatalogEmbeddingCache)` methods mirroring `cached_catalog_render`/`store_catalog_render_cache` (`code_mode_runtime.rs:502-529`); add `semantic_search_available(&self) -> bool` (checks cooldown) and `record_semantic_search_failure(&self)` / `record_semantic_search_recovery(&self)` helpers for the fail-open/cooldown/log-once logic.
- Create: `crates/labby-gateway/src/gateway/code_mode/embeddings.rs`
  - New file: the TEI HTTP client (`embed_via_tei(url: &str, timeout: Duration, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError>`) and cosine-similarity ranking helper (`rank_by_similarity(query_vector: &[f32], catalog_vectors: &[(String, Vec<f32>)], top_k: usize) -> Vec<(String, f32)>`). Kept separate from `search.rs` (catalog construction) and `code_mode_host.rs` (trait impl glue) per the existing one-responsibility-per-file convention in this directory.
- `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`
  - Modify: add `async fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, ToolError>` impl (~after `resolve_snippet`, `code_mode_host.rs:167-187`) delegating to `embeddings::embed_via_tei` with config from `self.code_mode_config().await.semantic_search`, respecting the cooldown gate.
- `crates/labby-gateway/src/gateway/code_mode/search.rs`
  - Modify: `catalog_from_tools` (`search.rs:50-150`) gains a call to a new `ensure_catalog_embeddings(manager, &fingerprint, &entries)` helper (fire-and-forget best-effort; never blocks/fails catalog construction) placed right after the existing `store_catalog_render_cache` call (`search.rs:136-143`).
- `docs/runtime/CONFIG.md`
  - Modify: extend the `### [code_mode]` table and example (`docs/runtime/CONFIG.md:261-292`) with the new `semantic_search.*` nested keys.

---

## Task 1: `CodeModeHost::embed_texts` trait method + neutral types

**Files:**
- Modify: `crates/labby-codemode/src/host.rs`
- Test: `crates/labby-codemode/src/host.rs` (inline `#[cfg(test)]` module — none currently exists in this file; add one)

**Interfaces:**
- Produces: `CodeModeHost::embed_texts(&self, texts: Vec<String>) -> impl Future<Output = Result<Vec<Vec<f32>>, ToolError>> + Send` — every later task calls this signature exactly.
- Produces: `NoopHost::embed_texts` returns `Ok(Vec::new())` unconditionally (test-only host never has real embeddings).

- [ ] **Step 1: Add the trait method**

In `crates/labby-codemode/src/host.rs`, add to the `CodeModeHost` trait (after `resolve_snippet`, before `fn config`, i.e. after line 88 and before line 90):

```rust
    /// Batch-embed a list of texts via the host's configured embedding
    /// service. Returns one vector per input text, in input order.
    ///
    /// Hosts that have no embedding service configured (or whose service is
    /// currently in a failure cooldown) MUST return `Ok(Vec::new())` rather
    /// than an `Err` — an empty result is the fail-open signal the kernel
    /// uses to skip semantic scoring for that call. `Err` is reserved for
    /// genuine host-side bugs (e.g. a malformed request the host itself
    /// built), not for "the embedding service is unreachable".
    fn embed_texts(
        &self,
        texts: Vec<String>,
    ) -> impl Future<Output = Result<Vec<Vec<f32>>, ToolError>> + Send;
```

- [ ] **Step 2: Add the `NoopHost` impl**

In the same file's `#[cfg(test)] impl CodeModeHost for NoopHost` block (currently `host.rs:115-163`), add after `resolve_snippet` (after line 154, before `async fn config`):

```rust
    async fn embed_texts(&self, _texts: Vec<String>) -> Result<Vec<Vec<f32>>, ToolError> {
        Ok(Vec::new())
    }
```

- [ ] **Step 3: Compile-check**

Run: `cargo check -p labby-codemode --all-features`
Expected: FAIL — no other type implements `CodeModeHost` yet outside this crate's test code, so this alone should compile cleanly within `labby-codemode` (the trait is additive and `NoopHost` now satisfies it). If it fails, the error will name any other in-crate impls of `CodeModeHost` that need the same addition — there should be none besides `NoopHost`; confirm via `grep -rn "impl CodeModeHost for" crates/labby-codemode/src/`.

- [ ] **Step 4: Commit**

```bash
git add crates/labby-codemode/src/host.rs
git commit -m "feat(codemode): add CodeModeHost::embed_texts trait method"
```

---

## Task 2: New minimal protocol variant pair (`EmbedQuery` / `EmbedQueryResult`)

**Files:**
- Modify: `crates/labby-codemode/src/protocol.rs`
- Modify: `crates/labby-codemode/src/runner.rs`
- Test: `crates/labby-codemode/src/runner.rs` (existing test module — check for one with `grep -n "#\[cfg(test)\]" crates/labby-codemode/src/runner.rs`; add cases there)

**Interfaces:**
- Consumes: nothing new from Task 1.
- Produces: `CodeModeRunnerOutput::EmbedQuery { seq: u64, text: String }` (runner→parent) and `CodeModeRunnerInput::EmbedQueryResult { seq: u64, vector: Option<Vec<f32>> }` (parent→runner) — Task 3 matches on these exact variant names/shapes. Produces JS global `globalThis.embedQueryText(text: string) -> Promise<number[] | null>` — Task 5's `preamble.rs` changes call this exact name.

- [ ] **Step 1: Add the protocol variants**

In `crates/labby-codemode/src/protocol.rs`, add to `CodeModeRunnerOutput` (after the `SnippetResolve` variant, i.e. after line 66, before `Done`):

```rust
    /// The sandbox called `embedQueryText(text)`. The host embeds `text` via
    /// its configured embedding service (fail-open: any failure resolves
    /// with `EmbedQueryResult { vector: None }`, never a protocol error).
    /// This is host-internal plumbing for `codemode.search()`'s semantic
    /// blend, NOT a Code Mode tool call — it is dispatched outside
    /// `DriveState.calls`/`max_calls_per_run` and never appears in the
    /// agent-visible call trace.
    EmbedQuery {
        seq: u64,
        text: String,
    },
```

Add to `CodeModeRunnerInput` (after `ToolError`, i.e. after line 37, before the closing `}` of the enum):

```rust
    /// Response to `EmbedQuery`. `vector: None` means semantic scoring is
    /// unavailable for this call (embedding disabled, TEI unreachable, or in
    /// cooldown) — the sandbox falls back to lexical-only scoring.
    EmbedQueryResult {
        seq: u64,
        #[serde(default)]
        vector: Option<Vec<f32>>,
    },
```

- [ ] **Step 2: Write a serde round-trip test**

In `crates/labby-codemode/src/protocol.rs`, add (create a `#[cfg(test)] mod tests` block at the end of the file if none exists — check first with `grep -n "mod tests" crates/labby-codemode/src/protocol.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_query_output_round_trips() {
        let msg = CodeModeRunnerOutput::EmbedQuery {
            seq: 7,
            text: "roster of saved queues".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"embed_query""#));
        let parsed: CodeModeRunnerOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn embed_query_result_input_round_trips_with_vector() {
        let msg = CodeModeRunnerInput::EmbedQueryResult {
            seq: 7,
            vector: Some(vec![0.1, 0.2, 0.3]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: CodeModeRunnerInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn embed_query_result_input_round_trips_without_vector() {
        let msg = CodeModeRunnerInput::EmbedQueryResult { seq: 7, vector: None };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: CodeModeRunnerInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn embed_query_result_input_defaults_missing_vector_to_none() {
        // Forward-compat: an older/partial payload with no `vector` key must
        // deserialize instead of erroring (matches the `#[serde(default)]`
        // pattern used elsewhere in this enum, e.g. `content_type`).
        let json = r#"{"type":"embed_query_result","seq":7}"#;
        let parsed: CodeModeRunnerInput = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed,
            CodeModeRunnerInput::EmbedQueryResult { seq: 7, vector: None }
        );
    }
}
```

- [ ] **Step 3: Run the new tests to verify they fail (types don't exist yet if Step 1 skipped) then pass**

Run: `cargo test -p labby-codemode protocol::tests -- --nocapture`
Expected: PASS (Step 1 already added the variants, so this validates serde shape, not TDD-red-first — protocol enums are data, not behavior, so this order is fine here).

- [ ] **Step 4: Add the JS-side bridge and javy host-function binding in `runner.rs`**

In `crates/labby-codemode/src/runner.rs`, find `wrap_code_mode` (`runner.rs:386-512`). After the `globalThis.callTool = ...` block (after line 413, before `globalThis.writeArtifact`), add:

```rust
globalThis.embedQueryText = (text) => {{
  if (typeof text !== "string" || text.trim() === "") {{
    return Promise.resolve(null);
  }}
  return new Promise((resolve) => {{
    const seq = globalThis.__labEmitEmbedQuery(text);
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "embed_query", resolve }});
  }});
}};
```

Note: unlike `callTool`, this never rejects — an embedding failure resolves to `null`, not a thrown error, so `codemode.search()` can `await` it unconditionally without a try/catch (fail-open is expressed at the JS type level: `number[] | null`).

In `__labSettlePendingOperation` (`runner.rs:455-502`), add a new `if` arm after the `tool_error` arm (after line 500, before the final `throw new Error("runner received unexpected protocol message");` at line 501):

```rust
  if (input.type === "embed_query_result") {{
    if (pending.kind !== "embed_query") {{
      throw new Error("runner received embed_query_result for a non-embed_query operation");
    }}
    pending.resolve(input.vector || null);
    return;
  }}
```

- [ ] **Step 5: Add the `javy_emit_embed_query` host-function binding**

In `crates/labby-codemode/src/runner.rs`, near `javy_emit_tool_call` (`runner.rs:522-564`), add a sibling function:

```rust
fn javy_emit_embed_query(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let text_value = args
        .0
        .first()
        .ok_or_else(|| javy_type_error(cx.clone(), "embedQueryText text must be a string"))?;
    let text = javy::val_to_string(&cx, text_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::EmbedQuery { seq, text })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}
```

Then register it alongside the other `__labEmit*` bindings (find the registration block around `runner.rs:313-340`, which registers `__labEmitToolCall`, `__labEmitArtifactWrite`, `__labEmitSnippetResolve` — read that block first with `sed -n '300,345p' crates/labby-codemode/src/runner.rs` to match the exact registration call shape used for the other three, then add a fourth registration for `__labEmitEmbedQuery` → `javy_emit_embed_query` following the identical pattern).

- [ ] **Step 6: Compile-check**

Run: `cargo check -p labby-codemode --all-features`
Expected: PASS. If the registration-block step (Step 5) doesn't compile, re-read the exact macro/function call used for the existing three registrations (do not guess the signature — copy its exact shape).

- [ ] **Step 7: Commit**

```bash
git add crates/labby-codemode/src/protocol.rs crates/labby-codemode/src/runner.rs
git commit -m "feat(codemode): add EmbedQuery/EmbedQueryResult protocol bridge"
```

---

## Task 3: Parent-side dispatch in `runner_drive.rs`

**Files:**
- Modify: `crates/labby-codemode/src/runner_drive.rs`
- Test: `crates/labby-codemode/src/runner_drive.rs` (existing `#[cfg(test)] mod tests` block, `runner_drive.rs:1127+`)

**Interfaces:**
- Consumes: `CodeModeRunnerOutput::EmbedQuery { seq, text }` (Task 2), `host.embed_texts(Vec<String>) -> Result<Vec<Vec<f32>>, ToolError>` (Task 1).
- Produces: writes `CodeModeRunnerInput::EmbedQueryResult { seq, vector }` back to the runner; `vector: None` on ANY error (fail-open — this is where the host-level try/fail-open happens, so `host.embed_texts` errors never propagate as a drive-loop `RunnerUnhealthy`).

- [ ] **Step 1: Read the exact `ToolCall` dispatch arm and `enqueue_tool_call` to copy the shape precisely**

Run: `sed -n '259,270p;327,380p;544,580p' crates/labby-codemode/src/runner_drive.rs`

Confirm the `ToolCallFut` type alias (`runner_drive.rs:54-58`) and `pending_tool_calls: FuturesUnordered<ToolCallFut<'_>>` (`runner_drive.rs:259`) — this future type resolves to `(seq, call_id, redacted_params, result, elapsed_ms)`. The embed-query future must NOT reuse this exact tuple shape (it has no "call_id"/"redacted_params"/tool semantics) — add a second, separate `FuturesUnordered` for embed-query futures so the two are never conflated and `DriveState.calls` bookkeeping (which reads from tool-call completions specifically) is structurally impossible to pollute.

- [ ] **Step 2: Add a second `FuturesUnordered` for embed-query calls**

In `drive_runner` (`runner_drive.rs:228-259`), after the existing `let mut pending_tool_calls: FuturesUnordered<ToolCallFut<'_>> = FuturesUnordered::new();` (line 259), add:

```rust
        // Embed-query round-trips are host-internal plumbing for
        // `codemode.search()`'s semantic blend, not Code Mode tool calls —
        // kept in a separate FuturesUnordered so they can never be counted
        // against `DriveState.calls`/`max_calls_per_run` or appear in the
        // agent-visible call trace.
        let mut pending_embed_queries: FuturesUnordered<
            std::pin::Pin<Box<dyn std::future::Future<Output = (u64, Option<Vec<f32>>)> + Send + '_>>,
        > = FuturesUnordered::new();
```

- [ ] **Step 3: Add the `EmbedQuery` arm to the read-side `match msg`**

In the `match msg { ... }` block (`runner_drive.rs:327-421`), add a new arm after `SnippetResolve` (after line 421, before `Done`):

```rust
                        CodeModeRunnerOutput::EmbedQuery { seq, text } => {
                            let host = self.host;
                            pending_embed_queries.push(Box::pin(async move {
                                let vector = match host {
                                    Some(host) => match host.embed_texts(vec![text]).await {
                                        Ok(mut vectors) if !vectors.is_empty() => {
                                            Some(vectors.remove(0))
                                        }
                                        Ok(_) => None,
                                        Err(err) => {
                                            tracing::debug!(
                                                surface = "dispatch",
                                                service = "code_mode",
                                                action = "embed_query",
                                                kind = err.kind(),
                                                "embed_texts failed for search query; falling back to lexical-only"
                                            );
                                            None
                                        }
                                    },
                                    None => None,
                                };
                                (seq, vector)
                            }));
                        }
```

(Confirm `self.host` is `Option<&'a H>` matching the type used by `pending_tool_calls`'s existing futures at `runner_drive.rs:563-577` — copy the exact field-access pattern from there rather than guessing.)

- [ ] **Step 4: Add a `tokio::select!` arm to drain `pending_embed_queries` and write results back**

Find the existing arm that drains `pending_tool_calls` and calls `handle_completed_tool_call` (around `runner_drive.rs:480-493`). Add a sibling arm in the same `tokio::select! { ... }` block:

```rust
                Some((seq, vector)) = pending_embed_queries.next() => {
                    if let Err(err) = write_runner_input(
                        stdin,
                        &CodeModeRunnerInput::EmbedQueryResult { seq, vector },
                    )
                    .await
                    {
                        return DriveOutcome::RunnerUnhealthy(err.into());
                    }
                }
```

(Match the exact `write_runner_input` call signature already used for `ToolResult`/`ToolError` writes elsewhere in this function — copy it verbatim, do not reinvent the error-wrapping.)

- [ ] **Step 5: Handle the "no host" and "Done with in-flight embed queries" edge cases**

The existing `Done` arm (`runner_drive.rs:422+`) currently treats non-empty `pending_tool_calls` as a protocol error and evicts the runner. Read that logic (`sed -n '422,460p' crates/labby-codemode/src/runner_drive.rs`) and decide: an embed-query future is host-internal and best-effort, so a `Done` arriving while `pending_embed_queries` is non-empty should NOT be treated as a protocol error (the JS side already resolved without waiting, since `embedQueryText` failures resolve to `null` rather than blocking indefinitely — but a genuine race where `Done` beats the embed response is theoretically possible only if user code doesn't `await` the promise, which `codemode.search()`'s own generated code always does). Add a code comment explaining this and simply drop any still-pending embed-query futures silently (they have no caller left to resolve) rather than erroring — do not require `pending_embed_queries.is_empty()` in the `Done` arm's existing invariant check.

- [ ] **Step 6: Write a drive-loop integration test**

In the existing `#[cfg(test)] mod tests` block (`runner_drive.rs:1127+`), add a test using a stub host whose `embed_texts` returns a fixed vector, driving a runner stub that emits `EmbedQuery` and asserts `EmbedQueryResult` comes back with the right vector, AND a second test where the host's `embed_texts` returns `Err(...)` and asserts the runner receives `EmbedQueryResult { vector: None }` (not a `RunnerUnhealthy`/protocol error). Base these on the existing `drive_runner_times_out_and_marks_runner_unhealthy` test pattern (`runner_drive.rs:1151-1170`) and whatever stub-runner/stub-host helpers that test file already provides (`grep -n "spawn_stub\|struct.*Stub\|impl CodeModeHost for" crates/labby-codemode/src/runner_drive.rs` to find them before writing new ones — reuse existing stub infrastructure rather than duplicating it).

- [ ] **Step 7: Run the tests**

Run: `cargo test -p labby-codemode runner_drive:: -- --nocapture`
Expected: PASS, including the two new tests from Step 6.

- [ ] **Step 8: Run full crate test suite + lint**

Run: `cargo test -p labby-codemode --all-features && cargo clippy -p labby-codemode --all-features -- -D warnings && cargo fmt -p labby-codemode -- --check`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/labby-codemode/src/runner_drive.rs
git commit -m "feat(codemode): dispatch EmbedQuery round-trips outside the tool-call budget"
```

---

## Task 4: `SemanticSearchConfig` + validation + `docs/runtime/CONFIG.md`

**Files:**
- Modify: `crates/labby-runtime/src/gateway_config.rs`
- Modify: `docs/runtime/CONFIG.md`
- Test: `crates/labby-runtime/src/gateway_config.rs` (find existing `CodeModeConfig` tests via `grep -n "mod tests\|fn.*code_mode" crates/labby-runtime/src/gateway_config.rs` and add alongside)

**Interfaces:**
- Produces: `SemanticSearchConfig { enabled: bool, tei_url: Option<String>, tei_timeout_ms: u64, cooldown_ms: u64, blend_weight: f32 }` on `CodeModeConfig.semantic_search`. Later tasks (`embeddings.rs`, `code_mode_host.rs`) read these exact field names.

- [ ] **Step 1: Add default-helper free functions**

In `crates/labby-runtime/src/gateway_config.rs`, near the existing default helpers (after `fn default_max_log_bytes()`, i.e. after line 56, before `fn default_upstream_priority()`), add:

```rust
fn default_semantic_search_tei_timeout_ms() -> u64 {
    2_000
}

fn default_semantic_search_cooldown_ms() -> u64 {
    30_000
}

fn default_semantic_search_blend_weight() -> f32 {
    0.5
}
```

- [ ] **Step 2: Add the `SemanticSearchConfig` struct**

In the same file, near `CodeModeConfig` (before it, so it can be referenced — insert before line 86's `pub struct CodeModeConfig {`):

```rust
/// Optional embedding-based semantic search blend for `codemode.search()`.
///
/// Disabled by default (`enabled = false`, matching the repo convention that
/// `code_mode.enabled` itself also defaults to `false` — Code Mode features
/// are opt-in). When disabled or misconfigured, `codemode.search()` runs its
/// existing pure-lexical algorithm unchanged; this struct's fields are never
/// read on that path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticSearchConfig {
    /// Whether to blend embedding similarity into `codemode.search()` results.
    #[serde(default)]
    pub enabled: bool,
    /// Base URL of the TEI (Text Embeddings Inference) server, e.g.
    /// `http://localhost:52000`. Required when `enabled = true`; `None` (the
    /// default) means semantic search stays off even if `enabled = true` is
    /// set without a URL — see `CodeModeConfig::validate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tei_url: Option<String>,
    /// Per-request timeout for one `POST /embed` call to TEI.
    #[serde(default = "default_semantic_search_tei_timeout_ms")]
    pub tei_timeout_ms: u64,
    /// Cooldown after a TEI failure before the next attempt. Bounds how often
    /// a flapping/unreachable TEI server is retried; failures during the
    /// cooldown window fail open silently (no repeated log spam).
    #[serde(default = "default_semantic_search_cooldown_ms")]
    pub cooldown_ms: u64,
    /// Weight applied to normalized semantic similarity when blending with
    /// normalized lexical score. See `preamble.rs` `codemode.search` blend
    /// comment for the exact formula.
    #[serde(default = "default_semantic_search_blend_weight")]
    pub blend_weight: f32,
}

impl Default for SemanticSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            tei_url: None,
            tei_timeout_ms: default_semantic_search_tei_timeout_ms(),
            cooldown_ms: default_semantic_search_cooldown_ms(),
            blend_weight: default_semantic_search_blend_weight(),
        }
    }
}

impl SemanticSearchConfig {
    /// True only when semantic search is both enabled AND has a usable URL.
    /// Every call site should gate on this rather than re-checking both
    /// fields separately.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.enabled
            && self
                .tei_url
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

- [ ] **Step 4: Add validation**

In `CodeModeConfig::validate()` (`gateway_config.rs:137-171`), add before the final `Ok(())` (before line 169):

```rust
        if self.semantic_search.tei_timeout_ms == 0 || self.semantic_search.tei_timeout_ms > 60_000
        {
            return Err(ConfigError::InvalidSemanticSearchTeiTimeout {
                value: self.semantic_search.tei_timeout_ms,
            });
        }
        if self.semantic_search.cooldown_ms == 0 || self.semantic_search.cooldown_ms > 600_000 {
            return Err(ConfigError::InvalidSemanticSearchCooldown {
                value: self.semantic_search.cooldown_ms,
            });
        }
        if !(0.0..=1.0).contains(&self.semantic_search.blend_weight) {
            return Err(ConfigError::InvalidSemanticSearchBlendWeight {
                value: self.semantic_search.blend_weight,
            });
        }
        if self.semantic_search.enabled
            && self
                .semantic_search
                .tei_url
                .as_deref()
                .is_none_or(|url| url.trim().is_empty())
        {
            return Err(ConfigError::MissingSemanticSearchTeiUrl);
        }
```

Add the new `ConfigError` variants near the existing `InvalidCodeMode*` variants (`gateway_config.rs:716-727`, after `InvalidCodeModeMaxLogBytes`):

```rust
    #[error("gateway code_mode.semantic_search.tei_timeout_ms={value} is invalid — expected 1..=60000")]
    InvalidSemanticSearchTeiTimeout { value: u64 },
    #[error("gateway code_mode.semantic_search.cooldown_ms={value} is invalid — expected 1..=600000")]
    InvalidSemanticSearchCooldown { value: u64 },
    #[error("gateway code_mode.semantic_search.blend_weight={value} is invalid — expected 0.0..=1.0")]
    InvalidSemanticSearchBlendWeight { value: f32 },
    #[error(
        "gateway code_mode.semantic_search.enabled=true requires semantic_search.tei_url to be set"
    )]
    MissingSemanticSearchTeiUrl,
```

- [ ] **Step 5: Wire the new `ConfigError` variants through `validate_code_mode` in `labby-gateway`**

In `crates/labby-gateway/src/gateway/config.rs`'s `validate_code_mode` (`config.rs:564-591`), the catch-all `_ => ToolError::InvalidParam { message: e.to_string(), param: "code_mode".to_string() }` arm (line 586-589) already handles any `ConfigError` variant not explicitly matched — confirm the four new variants fall through to this arm correctly (they will, since it's a wildcard) and no change is strictly required here. Optionally add explicit arms with `param: "code_mode.semantic_search.tei_timeout_ms"` etc. for parity with the existing three explicit arms — do this for consistency with the file's existing style.

- [ ] **Step 6: Write config tests**

In `crates/labby-runtime/src/gateway_config.rs`, find the existing test module (`grep -n "mod tests" crates/labby-runtime/src/gateway_config.rs`) and add:

```rust
    #[test]
    fn semantic_search_defaults_to_disabled_and_unconfigured() {
        let cfg = CodeModeConfig::default();
        assert!(!cfg.semantic_search.enabled);
        assert!(cfg.semantic_search.tei_url.is_none());
        assert!(!cfg.semantic_search.is_configured());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn semantic_search_enabled_without_url_fails_validation() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.enabled = true;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, ConfigError::MissingSemanticSearchTeiUrl));
    }

    #[test]
    fn semantic_search_enabled_with_url_is_configured_and_valid() {
        let mut cfg = CodeModeConfig::default();
        cfg.semantic_search.enabled = true;
        cfg.semantic_search.tei_url = Some("http://localhost:52000".to_string());
        assert!(cfg.semantic_search.is_configured());
        assert!(cfg.validate().is_ok());
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
        assert!(!cfg.semantic_search.enabled);
        assert!(cfg.semantic_search.tei_url.is_none());
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p labby-runtime gateway_config:: -- --nocapture`
Expected: PASS, all 5 new tests included.

- [ ] **Step 8: Update `docs/runtime/CONFIG.md`**

In `docs/runtime/CONFIG.md`, extend the `### [code_mode]` table (currently ending at line 277, before the `Example:` block at line 279) with a new subsection after the existing table and before the `Example:` heading:

```markdown
#### `[code_mode.semantic_search]`

Optional embedding-based semantic search blend for `codemode.search()`.
Disabled by default — when disabled or unconfigured, `codemode.search()` is
unchanged pure lexical/substring matching.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `enabled` | — | `false` | Blend embedding similarity into `codemode.search()` ranking. Requires `tei_url` to be set — enabling without a URL is a config validation error. |
| `tei_url` | — | unset | Base URL of a [TEI](https://github.com/huggingface/text-embeddings-inference) (Text Embeddings Inference) server, e.g. `http://localhost:52000`. |
| `tei_timeout_ms` | — | `2000` | Per-request timeout for one `POST /embed` call. Valid range: 1-60000. |
| `cooldown_ms` | — | `30000` | Cooldown after a TEI failure before the next attempt is tried. Failures during cooldown fail open silently (semantic scoring skipped, `codemode.search()` falls back to lexical-only — no error surfaces to the caller). Valid range: 1-600000. |
| `blend_weight` | — | `0.5` | Weight applied to normalized semantic similarity when blending with normalized lexical score. Valid range: 0.0-1.0. |

Example:

```toml
[code_mode.semantic_search]
enabled = true
tei_url = "http://localhost:52000"
tei_timeout_ms = 2000
cooldown_ms = 30000
blend_weight = 0.5
```

Semantic search is fail-open end to end: if TEI is unreachable, times out, or
returns a non-2xx response, that one `codemode.search()` call silently falls
back to lexical-only ranking — the response shape is identical either way, and
no error is visible to the calling agent. A `tracing::warn!` is logged once
per failure transition (not once per skipped call) so operators can see
degraded state without log spam.
```

- [ ] **Step 9: Commit**

```bash
git add crates/labby-runtime/src/gateway_config.rs crates/labby-gateway/src/gateway/config.rs docs/runtime/CONFIG.md
git commit -m "feat(config): add code_mode.semantic_search config with validation"
```

---

## Task 5: TEI client + cosine ranking (`embeddings.rs`)

**Files:**
- Create: `crates/labby-gateway/src/gateway/code_mode/embeddings.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode.rs` (module declaration + re-export)
- Test: inline `#[cfg(test)] mod tests` in `embeddings.rs`

**Interfaces:**
- Consumes: `SemanticSearchConfig` (Task 4).
- Produces: `pub(crate) async fn embed_via_tei(url: &str, timeout: Duration, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError>` and `pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32` and `pub(crate) fn rank_by_similarity(query_vector: &[f32], catalog_vectors: &[(String, Vec<f32>)]) -> Vec<(String, f32)>` (returns `(id, similarity)` pairs sorted descending by similarity — unranked/uncapped, callers slice as needed). Task 6 and Task 7 call these exact names.

- [ ] **Step 1: Find the module declaration site**

Run: `sed -n '1,30p' crates/labby-gateway/src/gateway/code_mode.rs` to see how `search` is declared as a submodule (it's referenced as `super::search` from `code_mode_host.rs:25`, so `code_mode.rs` must have `mod search;` or similar — confirm the exact pattern before adding `mod embeddings;` alongside it).

- [ ] **Step 2: Add the module declaration**

In `crates/labby-gateway/src/gateway/code_mode.rs`, add `pub(crate) mod embeddings;` (or `mod embeddings;` matching whatever visibility `search`'s declaration uses — copy it).

- [ ] **Step 3: Write the TEI client with a failing test first**

Create `crates/labby-gateway/src/gateway/code_mode/embeddings.rs`:

```rust
//! TEI (Text Embeddings Inference) HTTP client and cosine-similarity ranking
//! for Code Mode's semantic search blend.
//!
//! All vector math lives here, host-side — no raw floats are ever serialized
//! into the QuickJS sandbox. Every function here is designed to be wrapped in
//! a fail-open caller (see `code_mode_host.rs::embed_texts`); this module
//! itself returns ordinary `Result`s and does not implement the
//! cooldown/fail-open policy — that is the caller's responsibility.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use labby_runtime::error::ToolError;

#[derive(Debug, Deserialize)]
struct TeiEmbedResponse(Vec<Vec<f32>>);

/// Batch-embed `texts` via one `POST {url}/embed` call. Returns one vector
/// per input text, in input order — this is the TEI API's documented
/// contract for batch `inputs`.
pub(crate) async fn embed_via_tei(
    url: &str,
    timeout: Duration,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, ToolError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::new();
    let endpoint = format!("{}/embed", url.trim_end_matches('/'));
    let response = client
        .post(&endpoint)
        .timeout(timeout)
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
    let parsed: TeiEmbedResponse = response.json().await.map_err(|err| ToolError::Sdk {
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
        let result = embed_via_tei("http://127.0.0.1:1", Duration::from_millis(100), &[]).await;
        assert_eq!(result.unwrap(), Vec::<Vec<f32>>::new());
    }

    #[tokio::test]
    async fn embed_via_tei_unreachable_server_returns_network_error() {
        // Port 1 is a reserved/unused low port — connection refused, fast.
        let result = embed_via_tei(
            "http://127.0.0.1:1",
            Duration::from_millis(500),
            &["test".to_string()],
        )
        .await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p labby-gateway embeddings:: -- --nocapture`
Expected: PASS, all 8 tests.

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
git commit -m "feat(gateway): add TEI embedding client and cosine ranking"
```

---

## Task 6: Catalog embedding cache on `GatewayManager` + `embed_texts` trait impl + cooldown

**Files:**
- Modify: `crates/labby-gateway/src/gateway/code_mode.rs`
- Modify: `crates/labby-gateway/src/gateway/manager.rs`
- Modify: `crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`
- Test: `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs` (existing file — confirmed present)

**Interfaces:**
- Consumes: `embed_via_tei`, `cosine_similarity` (Task 5); `SemanticSearchConfig` (Task 4); `CodeModeHost::embed_texts` signature (Task 1).
- Produces: `GatewayManager::embed_texts` (trait impl, fail-open — never returns `Err` from cooldown/config-off paths, only `Ok(vec![])` or `Ok(real vectors)`); `GatewayManager::cached_embeddings(&self, fingerprint: &str) -> Option<Vec<(String, Vec<f32>)>>`; `GatewayManager::store_embedding_cache(&self, cache: CatalogEmbeddingCache)`. Task 7 calls `cached_embeddings`/`store_embedding_cache` directly (catalog-level caching); the `CodeModeHost::embed_texts` trait method itself is called by Task 3's drive-loop code for query-level embedding (no caching there — one query per call, caching a single query embedding is not worth the complexity).

- [ ] **Step 1: Add `CatalogEmbeddingCache`**

In `crates/labby-gateway/src/gateway/code_mode.rs`, near `CatalogRenderCache` (after its definition, `code_mode.rs:53-62`), add:

```rust
/// Cached catalog embedding vectors, keyed by the same fingerprint used for
/// `CatalogRenderCache` (see `search.rs`'s `catalog_from_tools`). One vector
/// per catalog entry id, computed via a single batched TEI call.
pub(crate) struct CatalogEmbeddingCache {
    pub fingerprint: String,
    /// `(entry.id, embedding_vector)` pairs, same order as the catalog was
    /// embedded in (not necessarily catalog entry order after a cache hit —
    /// callers should look up by id, not by index).
    pub vectors: Vec<(String, Vec<f32>)>,
}
```

- [ ] **Step 2: Add manager fields**

In `crates/labby-gateway/src/gateway/manager.rs`, add after `code_mode_catalog_render_cache` (after line 115, before `code_mode_snippet_metadata_cache`):

```rust
    /// Cached Code Mode catalog embedding vectors, keyed by the same
    /// fingerprint as `code_mode_catalog_render_cache`. `None` means either
    /// no catalog has been embedded yet, or the last embedding attempt
    /// failed (fail-open — a `None` cache entry is not distinguishable from
    /// "never tried" and that's fine, the next search attempt just retries
    /// subject to the cooldown gate below).
    pub(super) code_mode_embedding_cache:
        Arc<Mutex<Option<crate::gateway::code_mode::CatalogEmbeddingCache>>>,
```

Add near wherever cooldown-style state would live (search for how other cooldown/backoff state is tracked elsewhere in this struct first — `grep -n "Instant\|cooldown\|backoff" crates/labby-gateway/src/gateway/manager.rs`; if nothing precedent exists, add a new field with a clear comment):

```rust
    /// Fail-open cooldown gate for the TEI semantic-search embedding
    /// service. `Some(instant)` = a call failed at `instant`; calls made
    /// before `instant + semantic_search.cooldown_ms` skip TEI entirely
    /// (falling back to lexical-only) rather than retrying a known-down
    /// service on every search. `None` = healthy (or never tried).
    pub(super) semantic_search_last_failure: Arc<Mutex<Option<std::time::Instant>>>,
```

Update every `GatewayManager` constructor (search for `Self {` initializations, likely one canonical `new`/`from_parts`-style constructor — `grep -n "fn new\|impl GatewayManager" crates/labby-gateway/src/gateway/manager.rs` and any test-helper constructors in `manager/tests.rs`) to initialize both new fields to `Arc::new(Mutex::new(None))`.

- [ ] **Step 3: Add cache accessor methods**

In `crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs`, add after `cached_catalog_render`/`store_catalog_render_cache` (after line 529 area — read the exact end of that method first):

```rust
    pub(crate) async fn cached_embeddings(&self, fingerprint: &str) -> Option<Vec<(String, Vec<f32>)>> {
        let guard = self.code_mode_embedding_cache.lock().await;
        guard.as_ref().and_then(|cache| {
            if cache.fingerprint == fingerprint {
                Some(cache.vectors.clone())
            } else {
                None
            }
        })
    }

    pub(crate) async fn store_embedding_cache(
        &self,
        cache: crate::gateway::code_mode::CatalogEmbeddingCache,
    ) {
        let mut guard = self.code_mode_embedding_cache.lock().await;
        *guard = Some(cache);
    }

    /// True when the semantic search cooldown has elapsed (or no failure has
    /// been recorded yet) — i.e. it is safe to attempt a TEI call.
    pub(crate) async fn semantic_search_available(&self, cooldown_ms: u64) -> bool {
        let guard = self.semantic_search_last_failure.lock().await;
        match *guard {
            None => true,
            Some(last_failure) => {
                last_failure.elapsed() >= std::time::Duration::from_millis(cooldown_ms)
            }
        }
    }

    /// Record a TEI failure, starting/refreshing the cooldown window. Logs a
    /// `tracing::warn!` only on the healthy→failing transition (i.e. when no
    /// failure was already recorded) so repeated failures during an active
    /// cooldown don't spam the log — the caller only calls this after
    /// `semantic_search_available` already returned `true`, so by
    /// construction every call here is a fresh transition or the very first
    /// failure, never a during-cooldown retry.
    pub(crate) async fn record_semantic_search_failure(&self, reason: &str) {
        let mut guard = self.semantic_search_last_failure.lock().await;
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

    /// Clear the failure cooldown after a successful TEI call.
    pub(crate) async fn record_semantic_search_recovery(&self) {
        let mut guard = self.semantic_search_last_failure.lock().await;
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

- [ ] **Step 4: Implement `CodeModeHost::embed_texts` for `GatewayManager`**

In `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`, add after `resolve_snippet` (after line 187, before `async fn config`):

```rust
    async fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, ToolError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let config = self.code_mode_config().await.semantic_search;
        if !config.is_configured() {
            return Ok(Vec::new());
        }
        if !self.semantic_search_available(config.cooldown_ms).await {
            return Ok(Vec::new());
        }
        let url = config
            .tei_url
            .as_deref()
            .expect("is_configured() guarantees tei_url is Some");
        let timeout = std::time::Duration::from_millis(config.tei_timeout_ms);
        match super::embeddings::embed_via_tei(url, timeout, &texts).await {
            Ok(vectors) => {
                self.record_semantic_search_recovery().await;
                Ok(vectors)
            }
            Err(err) => {
                self.record_semantic_search_failure(&err.to_string()).await;
                Ok(Vec::new())
            }
        }
    }
```

Note the fail-open contract from Task 1's trait doc comment: this method returns `Ok(Vec::new())` for every degraded case (disabled, unconfigured, cooldown active, TEI call failed) and only returns real vectors on genuine success — it never returns `Err` in this implementation, consistent with "Err is reserved for host-side bugs."

- [ ] **Step 5: Write manager-level tests**

In `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`, add tests (using whatever test-manager construction helper that file already uses — read it first with `sed -n '1,60p' crates/labby-gateway/src/gateway/manager/tests/code_mode.rs` to match the existing pattern):

```rust
    #[tokio::test]
    async fn embed_texts_returns_empty_when_semantic_search_disabled() {
        let manager = test_manager().await; // reuse existing helper from this file
        let result = manager.embed_texts(vec!["hello".to_string()]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn embed_texts_returns_empty_for_empty_input_without_config_check() {
        let manager = test_manager().await;
        let result = manager.embed_texts(vec![]).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn semantic_search_cooldown_blocks_immediate_retry_after_failure() {
        let manager = test_manager().await;
        manager.record_semantic_search_failure("test failure").await;
        assert!(!manager.semantic_search_available(30_000).await);
    }

    #[tokio::test]
    async fn semantic_search_cooldown_allows_retry_after_elapsed() {
        let manager = test_manager().await;
        manager.record_semantic_search_failure("test failure").await;
        // cooldown_ms=0 would be invalid config, but for this unit test we
        // pass a 0ms cooldown directly to assert the elapsed-time comparison
        // itself, not full config plumbing.
        assert!(manager.semantic_search_available(0).await);
    }

    #[tokio::test]
    async fn semantic_search_recovery_clears_cooldown() {
        let manager = test_manager().await;
        manager.record_semantic_search_failure("test failure").await;
        assert!(!manager.semantic_search_available(30_000).await);
        manager.record_semantic_search_recovery().await;
        assert!(manager.semantic_search_available(30_000).await);
    }
```

(If `test_manager()` is not the actual existing helper name, use whatever the file's existing tests call — inspect before writing, do not guess.)

- [ ] **Step 6: Run tests**

Run: `cargo test -p labby-gateway code_mode:: -- --nocapture`
Expected: PASS.

- [ ] **Step 7: Run full crate compile + lint**

Run: `cargo check -p labby-gateway --all-features && cargo clippy -p labby-gateway --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/labby-gateway/src/gateway/code_mode.rs crates/labby-gateway/src/gateway/manager.rs crates/labby-gateway/src/gateway/manager/code_mode_runtime.rs crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs crates/labby-gateway/src/gateway/manager/tests/code_mode.rs
git commit -m "feat(gateway): implement CodeModeHost::embed_texts with fail-open cooldown"
```

---

## Task 7: Catalog embedding on `list_tools` (fingerprint-cached, best-effort)

**Files:**
- Modify: `crates/labby-gateway/src/gateway/code_mode/search.rs`
- Test: extend existing tests in `crates/labby-gateway/src/gateway/code_mode/search.rs` if a test module exists there, else `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`.

**Interfaces:**
- Consumes: `GatewayManager::cached_embeddings`/`store_embedding_cache` (Task 6), `GatewayManager::embed_texts` (Task 6, via the `CodeModeHost` trait), the existing `fingerprint` local variable already computed in `catalog_from_tools` (`search.rs:67-74`).
- Produces: catalog entries get their embeddings computed and cached as a side effect of `list_tools`/`build_tools_render`; this task does NOT change `ToolsRender`'s shape (embeddings are cache-only, not part of the returned struct) — Task 8 (`preamble.rs`/query-time blend) reads the cache directly via a new accessor, not through `ToolsRender`.

- [ ] **Step 1: Add `ensure_catalog_embeddings` helper**

In `crates/labby-gateway/src/gateway/code_mode/search.rs`, add a new function after `catalog_from_tools` (after line 150):

```rust
/// Best-effort: ensure the catalog embedding cache is warm for
/// `fingerprint`. Never fails the caller — any embedding failure is already
/// absorbed fail-open inside `GatewayManager::embed_texts` (Task 6), so this
/// function only needs to decide whether a (re)embed is needed at all and
/// store the result if one happened.
async fn ensure_catalog_embeddings(manager: &GatewayManager, fingerprint: &str, entries: &[ToolDescriptor]) {
    if manager.cached_embeddings(fingerprint).await.is_some() {
        return; // already warm for this exact catalog
    }
    if entries.is_empty() {
        return; // cold-start / empty catalog — nothing to embed
    }
    // Embed each entry's description only. Rationale (see plan doc Task 7
    // Step 1 comment below for the full write-up): descriptions carry the
    // natural-language intent an agent's synonym-style query is most likely
    // to match; concatenating path/name/tags would bias the embedding toward
    // exact-token surface forms that the lexical scorer already covers well,
    // diluting the semantic signal the blend exists to add.
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    let texts: Vec<String> = entries.iter().map(|e| e.description.clone()).collect();
    let vectors = match manager.embed_texts(texts).await {
        Ok(vectors) if vectors.len() == ids.len() => vectors,
        Ok(_) => return, // fail-open: embed_texts returns [] on any degraded path
        Err(_) => return, // defensive — embed_texts's contract says this shouldn't happen
    };
    let pairs: Vec<(String, Vec<f32>)> = ids.into_iter().zip(vectors).collect();
    manager
        .store_embedding_cache(super::CatalogEmbeddingCache {
            fingerprint: fingerprint.to_string(),
            vectors: pairs,
        })
        .await;
}
```

- [ ] **Step 2: Call it from `catalog_from_tools`**

In `catalog_from_tools` (`search.rs:50-150`), after the existing `store_catalog_render_cache` call (after line 143, before the final `Ok(ToolsRender { ... })` at line 145), add:

```rust
    ensure_catalog_embeddings(manager, &fingerprint, &entries).await;
```

Also add the same call on the cache-hit early-return path (the `if let Some((entries, catalog_json, serialized_size)) = manager.cached_catalog_render(&fingerprint).await { ... return Ok(...) }` block at `search.rs:76-91`) — a catalog render cache hit means the fingerprint (and thus entries) are unchanged, but the embedding cache could still be cold on first use (e.g. semantic search was just enabled via `gateway.code_mode.set` without the catalog itself changing). Add before the early `return Ok(...)` at line 86-90:

```rust
        ensure_catalog_embeddings(manager, &fingerprint, &entries).await;
```

(Note: this reads `entries` before the early return, which already exists as a local in that branch — no new binding needed, just insert the call.)

- [ ] **Step 3: Verify this doesn't block/slow every `list_tools` call once warm**

Confirm by re-reading `ensure_catalog_embeddings`'s first line (`if manager.cached_embeddings(fingerprint).await.is_some() { return; }`) — this is an `Arc<Mutex<..>>` lock + string comparison, sub-microsecond, so warm-cache calls pay negligible overhead. Only a genuine fingerprint change (or first-ever call, or semantic search just turned on) triggers a real embed. Add a code comment at the top of `ensure_catalog_embeddings` stating this explicitly if not already clear from the docstring written in Step 1.

- [ ] **Step 4: Write a test**

Add to whichever test location Task 6 Step 5 used (`crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`), using a fake/stub embedding path if the existing test harness has one, or by enabling semantic search with a URL pointing at a local mock server if that infrastructure exists in this test file already — check first (`grep -n "mockito\|wiremock\|httpmock" crates/labby-gateway/Cargo.toml`). If no HTTP-mocking crate is already a dev-dependency, do NOT add one just for this — instead write a test that verifies `ensure_catalog_embeddings` is a no-op (cache stays empty, no panic) when semantic search is disabled (the default), which is the realistic CI path anyway:

```rust
    #[tokio::test]
    async fn catalog_embeddings_stay_cold_when_semantic_search_disabled() {
        let manager = test_manager().await;
        // Default config has semantic_search.enabled = false.
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
        // The embedding cache must remain empty — embed_texts() returns []
        // for a disabled config, so ensure_catalog_embeddings has nothing to
        // store even though it ran.
        let fingerprint_probe = manager.cached_embeddings("anything").await;
        assert!(fingerprint_probe.is_none());
        let _ = render; // catalog itself still renders normally regardless
    }
```

(Adjust the exact `list_tools` call signature/imports to match what Task 6's test file already imports — copy its existing `use` block rather than guessing types.)

- [ ] **Step 5: Run tests**

Run: `cargo test -p labby-gateway code_mode:: search:: -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-gateway/src/gateway/code_mode/search.rs crates/labby-gateway/src/gateway/manager/tests/code_mode.rs
git commit -m "feat(gateway): warm catalog embedding cache on list_tools, fingerprint-keyed"
```

---

## Task 8: Blend logic in `codemode.search()` (`preamble.rs`)

**Files:**
- Modify: `crates/labby-codemode/src/preamble.rs`
- Test: existing test module in `preamble.rs` (`preamble.rs:482+`, confirmed present per research report)

**Interfaces:**
- Consumes: `globalThis.embedQueryText(text: string) -> Promise<number[] | null>` (Task 2). Note: `codemode.search()`'s JS does NOT receive raw catalog vectors — see design rationale below. Instead, it sends the query embedding request, and separately the *host* is responsible for ranking; since the chosen design (Task 3/6/7) keeps vector math host-side, `codemode.search()`'s JS needs a way to get *ranked catalog ids* back, not just a raw query vector. **This requires revisiting the wire shape** — see Step 0 below before writing any JS.

- [ ] **Step 0: Resolve the host-side-ranking vs JS-side-cosine-math design tension before writing code**

Earlier tasks built `EmbedQuery`/`EmbedQueryResult` to return `vector: Option<Vec<f32>>` — a raw query embedding handed to JS. But the plan's own design principle (stated in the Goal/Architecture section, and in `embeddings.rs`'s module doc) is "Rust owns 100% of the vector math... no raw floats are ever serialized into the sandbox." Handing a raw 1024-float query vector back into JS and asking `codemode.search()` to loop over `__codemodeDiscovery` doing per-entry cosine math against catalog vectors it doesn't even have (they were never sent — see Task 7, which stores them only in the host-side `CatalogEmbeddingCache`, never in `catalog_json`) is unworkable as specified across Tasks 2/3.

Resolve this now, before writing `preamble.rs` changes: change the **query-time round trip to return a ranked list, not a raw vector.** Revise Task 2/3's wire contract as follows (this is a design correction discovered during planning — apply it retroactively to Tasks 2 and 3 before or during this task's implementation):

- `CodeModeRunnerOutput::EmbedQuery { seq, text }` — unchanged.
- `CodeModeRunnerInput::EmbedQueryResult { seq, ranked: Vec<(String, f32)> }` replaces the `vector: Option<Vec<f32>>` field — `ranked` is the host-computed, already-cosine-scored, descending-sorted `(entry_id, similarity)` list for the current catalog (empty `Vec` = fail-open, semantic scoring unavailable, exactly like the old `None` case but shaped as "nothing to blend" instead of "no vector"). The drive-loop code in Task 3 Step 3 must be updated: instead of `host.embed_texts(vec![text])` returning a vector to hand back raw, it must (a) embed the query via `host.embed_texts(vec![text])`, (b) look up the current catalog's cached vectors via the fingerprint the drive loop already has access to through `cfg`/`self` (thread the fingerprint through `RunnerConfig` or recompute it — confirm the cleanest path by checking whether `RunnerConfig` already carries anything catalog-identifying; if not, the simplest correct fix is for `GatewayManager::embed_texts` itself to NOT be the ranking entry point — instead add a *second*, more specific trait method `CodeModeHost::semantic_rank(&self, query: &str, top_k: usize) -> Result<Vec<(String, f32)>, ToolError>` that internally does embed-query + cosine-rank-against-cached-catalog-vectors + cooldown, all inside `labby-gateway`, so `labby-codemode`'s drive loop just calls ONE host method and gets back the ranked list directly — no separate "fetch catalog vectors across the crate boundary" plumbing needed in `labby-codemode` at all).

**This changes Task 1 and Task 6:** replace `CodeModeHost::embed_texts` as the *drive-loop-facing* method with `CodeModeHost::semantic_rank(&self, query: String, top_k: usize) -> impl Future<Output = Result<Vec<(String, f32)>, ToolError>> + Send`. Keep `embed_texts` too (Task 7's catalog-embedding path still needs a raw batch-embed primitive) — both trait methods coexist: `embed_texts` for catalog warming (host-internal, `search.rs` calls it), `semantic_rank` for query-time scoring (drive-loop-facing, wraps `embed_texts([query])` + `cached_embeddings(fingerprint)` + `cosine_similarity` + cooldown internally). Go back and adjust:
  - Task 1: add `semantic_rank` to the trait alongside `embed_texts`; `NoopHost::semantic_rank` returns `Ok(Vec::new())`.
  - Task 2: `CodeModeRunnerInput::EmbedQueryResult { seq, ranked: Vec<(String, f32)> }` (not `vector: Option<Vec<f32>>`). JS-side `embedQueryText` (rename to `__labSemanticRank` internal name, but keep a JS-facing name that reflects what it returns — e.g. `globalThis.__labSemanticRank = (query, topK) => Promise<Array<{id: string, score: number}>>`) resolves to an array (possibly empty), never `null` — empty array IS the fail-open signal now, simplifying the "must never throw" contract (no special-casing null vs array in JS).
  - Task 3: the drive-loop's `EmbedQuery`-arm future calls `host.semantic_rank(text, top_k)` directly (needs `top_k` threaded from somewhere — reuse the existing `limit` already sent in `codemode.search`'s own `input.limit`, so the JS call becomes `globalThis.__labEmitEmbedQuery(text, limit)` with `limit` as a second arg, and the emit function reads both args).
  - Task 6: `GatewayManager::semantic_rank` becomes the fail-open-wrapped method (mirrors `embed_texts`'s fail-open shape from Task 6 Step 4): looks up `cached_embeddings(current_fingerprint)`, calls `embed_texts(vec![query])` for the query vector, calls `rank_by_similarity` (Task 5), truncates to `top_k`, returns `Ok(ranked)` always (never `Err`) — same fail-open contract as `embed_texts`. This needs the *current* fingerprint, which `search.rs`'s `catalog_from_tools` already computes locally but does not currently store anywhere durable — add one more small piece of state: `GatewayManager` tracks `code_mode_current_catalog_fingerprint: Arc<Mutex<Option<String>>>`, set at the end of `catalog_from_tools` (both the cache-hit and cache-miss paths) right where `ensure_catalog_embeddings` is called, so `semantic_rank` can read "what's the fingerprint of the catalog this execution is using" without threading it through `CodeModeHost::call_tool`'s narrower signature.

Do this design correction as literal edits to the already-committed Task 1/2/3/6 code in this same working session (amend those commits or add small follow-up commits — prefer follow-up commits per the "always create NEW commits" global git convention) before proceeding with Steps 1+ below, which assume the corrected `semantic_rank`-based shape.

- [ ] **Step 1: Add the blend-aware `codemode.search` JS**

In `crates/labby-codemode/src/preamble.rs`, modify `generate_discovery_js` (`preamble.rs:170-333`). Change `codemode.search` (currently `preamble.rs:201-256`) from a function returning `Promise.resolve(...)` synchronously to a genuine `async function` that awaits the new host bridge:

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
  var LEXICAL_FIELD_WEIGHTS_SUM = 12 + 10 + 8 + 5 + 7 + 9; // max possible per-token score if a single token hit every field
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
  // Normalization: lexical `score` is an unbounded sum of per-token
  // best-field weights (max per token = 12, so an N-token query's
  // theoretical ceiling is 12*N, but in practice most matches hit far fewer
  // than all fields per token). We normalize each entry's lexical score by
  // the MAX score actually observed among this query's lexical matches
  // (not a fixed global ceiling), so normalization adapts to how many
  // tokens/fields actually matched for this specific query rather than
  // penalizing every query against an unreachable theoretical maximum.
  // Semantic cosine similarity is already bounded to [-1, 1] (see
  // `embeddings::cosine_similarity`'s `.clamp(-1.0, 1.0)`); we rescale it to
  // [0, 1] to match the lexical normalization's range before blending.
  //
  // Blend formula: for entries that appear in EITHER the lexical or semantic
  // result set, blended = max(normalized_lexical, semantic_similarity_0_to_1 * blend_weight).
  // `max` (not a weighted sum) is deliberate: a strong exact lexical match
  // should never be outranked by a mediocre semantic match, and a strong
  // semantic match (synonym case) should surface even with zero lexical
  // overlap — either signal being strong is sufficient, matching the "OR"
  // intuition of "found it via either route." `blend_weight` (config
  // default 0.5) discounts semantic-only matches relative to a perfect
  // lexical match, so ambiguous semantic near-misses don't crowd out
  // legitimate lexical results at the same rank.
  var maxLexicalScore = 0;
  for (var m = 0; m < scored.length; m++) {{
    if (scored[m].score > maxLexicalScore) maxLexicalScore = scored[m].score;
  }}
  var ranked = [];
  try {{
    ranked = await globalThis.__labSemanticRank(query, limit);
  }} catch (e) {{
    ranked = []; // fail-open: never let a semantic-rank rejection break search()
  }}
  if (ranked && ranked.length) {{
    for (var r = 0; r < ranked.length; r++) {{
      var rid = ranked[r].id;
      var semanticSimilarity01 = (ranked[r].score + 1) / 2; // [-1,1] -> [0,1]
      var existing = lexicalById[rid];
      if (existing) {{
        var normalizedLexical = maxLexicalScore > 0 ? existing.score / maxLexicalScore : 0;
        existing.blendedScore = Math.max(normalizedLexical, semanticSimilarity01 * {blend_weight});
      }} else {{
        // Semantic-only match: not found by lexical scoring at all
        // (e.g. the synonym case with zero token overlap). Look it up in
        // the full discovery catalog and add it with a blended score
        // derived purely from semantic similarity.
        for (var d = 0; d < __codemodeDiscovery.length; d++) {{
          if (__codemodeDiscovery[d].id === rid) {{
            var de = __codemodeDiscovery[d];
            var record2 = {{
              path: de.path, id: de.id, kind: de.kind, namespace: de.namespace,
              name: de.name, description: de.description, signature: de.signature,
              tags: de.tags || [], score: 0,
              blendedScore: semanticSimilarity01 * {blend_weight}
            }};
            lexicalById[rid] = record2;
            scored.push(record2);
            break;
          }}
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

(`{blend_weight}` is a Rust-side format-string interpolation of `config.semantic_search.blend_weight` — `generate_discovery_js`'s signature must be extended to accept this value; thread it through from `execute.rs`'s `build_code_mode_proxy`, which already has access to `host.config()`. Confirm the exact plumbing: `build_code_mode_proxy` at `execute.rs:150-152` already calls `host.list_tools(...)`; it needs one more call, `host.config().await`, which `CodeModeHost::config` already exists for (`host.rs:91`) and is likely already called elsewhere in `execute.rs` for `timeout_ms` etc — check `execute_sandboxed`'s caller in `execute.rs` for where `config: CodeModeConfig` is already threaded in as a parameter, since `execute`/`execute_with_raw_response` (`execute.rs:26-38`, `40-131`) already take `config: CodeModeConfig` — thread `config.semantic_search.blend_weight` from there into `build_code_mode_proxy` and then into `generate_discovery_js`'s new parameter.)

Return shape note: `codemode.search()` now returns a plain object directly (`return {{...}}`) inside an `async function` rather than `Promise.resolve({{...}})` — both are equivalent once awaited (an `async function`'s return value is automatically wrapped in a resolved Promise), so this is NOT a breaking change to callers, consistent with the ground-truth note that every caller already does `await codemode.search(...)`.

- [ ] **Step 2: Update `generate_discovery_js`'s signature and the `__labSemanticRank` bridge naming**

Rename the Task 2/8-Step-0-revised JS bridge from `embedQueryText` to `globalThis.__labSemanticRank = (query, topK) => Promise<Array<{{id, score}}>>` (internal-style double-underscore prefix, matching the naming convention of every other internal bridge in this codebase — `__labEmitToolCall`, `__labPendingToolCalls`, `__labSettlePendingOperation`, etc. — rather than the earlier placeholder name `embedQueryText`/`embedQueryText`, which reads like a public API surface and isn't). Go back and rename it consistently across Task 2's `runner.rs` changes and Task 3's `runner_drive.rs` changes.

- [ ] **Step 3: Write unit tests for the blend/normalization math**

In `preamble.rs`'s existing test module (`preamble.rs:482+`), the current tests exercise `generate_discovery_js`'s JS-string generation, not runtime behavior (there's no JS engine in a plain `cargo test` for this crate — confirm by reading a couple of existing tests there first: `sed -n '482,560p' crates/labby-codemode/src/preamble.rs`). Add tests in the same style asserting the generated JS **contains** the new blend logic markers, e.g.:

```rust
    #[test]
    fn generate_discovery_js_includes_semantic_blend_when_configured() {
        let entries = vec![]; // reuse whatever existing test fixture builder this file already has
        let js = generate_discovery_js(&entries, 0.5).expect("js generation succeeds");
        assert!(js.contains("__labSemanticRank"));
        assert!(js.contains("blendedScore"));
        assert!(js.contains("codemode.search = async function"));
    }

    #[test]
    fn generate_discovery_js_interpolates_configured_blend_weight() {
        let entries = vec![];
        let js = generate_discovery_js(&entries, 0.75).expect("js generation succeeds");
        assert!(js.contains("* 0.75"));
    }
```

(Match the exact existing fixture-building helper used by this file's other tests — do not invent a different one. If `generate_discovery_js` currently takes only `entries: &[CodeModeDiscoveryEntry]`, Step 1/2 above already changed its signature to take a second `blend_weight: f32` parameter — update every existing call site, including the one in `execute.rs:173-174`, and every existing test in this file that calls `generate_discovery_js` with the old one-argument signature.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p labby-codemode preamble:: -- --nocapture`
Expected: PASS, including all pre-existing tests updated for the new signature plus the two new tests from Step 3.

- [ ] **Step 5: Full crate build + lint**

Run: `cargo build -p labby-codemode --all-features && cargo build -p labby-gateway --all-features && cargo clippy --workspace --all-features -- -D warnings && cargo fmt --all -- --check`
Expected: PASS across the whole workspace (both crates now share the revised `CodeModeHost` trait shape).

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/src/preamble.rs crates/labby-codemode/src/execute.rs
git commit -m "feat(codemode): blend semantic similarity into codemode.search() ranking"
```

---

## Task 9: End-to-end smoke test against the real TEI server

**Files:**
- None modified — this is a manual verification task using whatever existing Code Mode execution entrypoint the repo has (CLI or MCP tool call), confirmed to exist per the research report (`cargo nextest run --workspace --all-features` covers unit tests; this task covers the thing unit tests can't: a real running gateway + real running sandbox + real TEI).

- [ ] **Step 1: Start (or confirm running) the gateway with semantic search enabled**

Set `~/.lab/config.toml`'s `[code_mode.semantic_search]` section (or the config path this workspace/worktree uses for local dev — check `labby doctor`/`labby gateway list` output or `crates/labby-gateway`'s test/dev config loading convention) to:

```toml
[code_mode.semantic_search]
enabled = true
tei_url = "http://localhost:52000"
```

Restart/reload the gateway (`labby gateway reload` or equivalent for this workspace — confirm the right command via `just --list` or the `labby` binary's own `--help`).

- [ ] **Step 2: Run a Code Mode script through the CLI (or whatever local execution surface exists) with a synonym-style query**

Find the actual local execution entrypoint (grep for a `codemode` CLI subcommand: `grep -rn "codemode" crates/labby/src/cli* 2>/dev/null | grep -i "subcommand\|command" | head -10`, or use the MCP `codemode` tool directly via `mcporter` per the `testing:mcporter` skill if a CLI path doesn't exist). Run:

```js
async () => {
  const found = await codemode.search({ query: "roster of saved queues", limit: 5 });
  return found;
}
```

against a live catalog that includes at least one tool whose description plausibly matches "queue"/"saved"/"list" semantically without sharing exact tokens with "roster of saved queues" (use whatever upstreams are actually connected in this dev environment — check `labby gateway list` first to know what's available, do not assume a specific tool exists).

- [ ] **Step 2b: Confirm a control case — same query with semantic search disabled returns fewer/different results**

Toggle `enabled = false`, reload, rerun the identical query, and confirm the result set is either smaller or differently ordered (proving the semantic blend had a real, observable effect) — if results are byte-identical with semantic search on vs off, something in the blend path is not actually running (most likely: the TEI URL is misconfigured, the cooldown is stuck engaged, or the catalog embedding cache never warmed — check `tracing::warn!`/`tracing::info!` log lines from Task 6 Step 3's `record_semantic_search_failure`/`record_semantic_search_recovery` to diagnose which).

- [ ] **Step 3: Confirm fail-open behavior live**

Stop the TEI container (or point `tei_url` at an unreachable port), reload the gateway, and rerun the same `codemode.search(...)` query. Confirm:
- The call succeeds (no error surfaced to the caller).
- The result set matches what lexical-only search would have returned before this feature existed (i.e. behaviorally identical to `enabled = false`).
- A single `tracing::warn!` line appears in the gateway's logs for this failure (check via whatever log-viewing convention this workspace uses — `journalctl`, `docker logs`, or the gateway's own log file).
- A second `codemode.search(...)` call made immediately after does NOT produce a second `tracing::warn!` line (cooldown suppresses the repeat).

- [ ] **Step 4: Restart TEI and confirm auto-recovery**

Restart the TEI container, wait 30+ seconds (the cooldown window), rerun `codemode.search(...)` with the same synonym query, and confirm semantic results return again without any gateway restart — and that a `tracing::info!` "tei_recovered" line appears (Task 6 Step 3's `record_semantic_search_recovery`).

- [ ] **Step 5: Document findings inline in the PR description (not a new doc file)**

Note pass/fail for each of Steps 2-4 in the PR body when this plan reaches the `/gh-pr` step of the outer pipeline — this task has no code artifact of its own, it is a verification gate.

---

## Self-Review Notes (for the plan author to confirm before handoff)

- **Spec coverage:** freshness/lazy-computation (Task 7, fingerprint-keyed cache) — covered. Fail-open + cooldown + log-once (Task 6 Step 3) — covered. Config location/convention (Task 4) — covered, follows `CodeModeConfig` pattern exactly, defaults disabled. Blend normalization + formula documented in code comment (Task 8 Step 1's inline comment) — covered. Bridge pattern reuse (Task 2/3, deliberately narrower than a full `callTool` reuse after Task 8 Step 0's design correction ruled out both the naive reserved-tool-id approach AND a raw-vector-into-JS approach) — covered, with the reasoning for the final `semantic_rank`-shaped bridge documented. Empty/cold-start catalog (Task 7 Step 1's `if entries.is_empty() { return; }`) — covered. No new doc files beyond updating the existing `docs/runtime/CONFIG.md` (Task 4 Step 8) — covered.
- **Known design correction embedded in the plan itself:** Task 8 Step 0 documents a real tension discovered between Task 2/3's initial `vector`-returning wire shape and the "no raw vectors in the sandbox" principle, and resolves it by revising the trait/protocol shape to return a host-ranked list instead. An implementer following this plan strictly in task order will build the vector-returning version first (Tasks 2-3) and then must revisit those files during Task 8 — this is called out explicitly so it isn't missed, but it does mean Tasks 2/3/6 are not fully final until Task 8 Step 0's revision lands. Executors using `subagent-driven-development` should read Task 8 Step 0 in full BEFORE implementing Tasks 2/3/6, or expect a follow-up correction pass.
- **Placeholder scan:** no TBD/TODO markers; every step has literal code.
- **Type consistency:** `CodeModeHost::embed_texts` (catalog batch embed) and `CodeModeHost::semantic_rank` (query-time ranked lookup) are two distinct, intentionally coexisting trait methods — verify no later task conflates them under one name.
