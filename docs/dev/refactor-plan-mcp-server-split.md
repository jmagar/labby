# Refactor Plan — Split `crates/lab/src/mcp/server.rs` by responsibility

**Bead:** `lab-kvji.24.1` — "Split oversized crates/lab/src/mcp/server.rs by responsibility" (P1, child of epic `lab-kvji.24`)
**Status:** Planning only. No production code in this document.
**Constraint:** After the refactor **no `.rs` file under `crates/lab/src/mcp/` may exceed 500 LOC** — including the remaining `server.rs` and every new module and `tests.rs`.

> Note: the bead description originally said the file was 2540 LOC; the file is now **3492 LOC** (it grew with Code Mode + subject-scoped upstream routing). The bead has been corrected.

---

## 1. Why this file is oversized

`server.rs` mixes four concern families in one module:

1. **MCP protocol handlers** — the `impl ServerHandler for LabMcpServer` block (10 methods: `get_info`, `set_level`, `on_initialized`, `complete`, `list_prompts`, `get_prompt`, `list_resources`, `read_resource`, `list_tools`, `call_tool`).
2. **Dispatch routing** — `call_tool` alone (~808 LOC) routes across gateway meta-tools (`search`/`execute` Code Mode), built-in service dispatch, raw upstream proxy, and subject-scoped upstream proxy.
3. **Request-context / auth / formatting helpers** — free fns and inherent helper methods for subject extraction, scope checks, envelope/result formatting, token estimation.
4. **Tests** — a single `#[cfg(test)] mod tests` block of ~866 LOC.

### Empirical LOC map (line-counted, not estimated)

| Region | Lines | LOC | Concern |
|---|---|---|---|
| Preamble: imports, `CODE_EXECUTE_DESCRIPTION`, `CODE_MODE_MAX_CODE_BYTES` | 1–109 | 109 | module setup + Code Mode execute tool description |
| Completion/schema free helpers | 110–188 | 79 | `action_schema`, `completion_info`, `complete_prompt_arg`, `service_name_completions`, `string_array_arg` |
| Struct + startup self-test | 190–230 | 41 | `LabMcpServer`, `verify_upstream_subject_resolution_support` |
| Small `ServerHandler` handlers | 232–343 | 112 | `get_info`, `set_level`, `on_initialized`, `complete` |
| Prompt handlers | 345–659 | 315 | `list_prompts`, `get_prompt` |
| Resource handlers | 661–1139 | 479 | `list_resources`, `read_resource` |
| `list_tools` | 1141–1326 | 186 | tool advertisement |
| `call_tool` | 1328–2135 | 808 | the mega-dispatcher |
| `inject_gateway_origin_param`, `redact_subject_for_logging` | 2140–2163 | 24 | helpers |
| Inherent `impl LabMcpServer` helpers | 2165–2303 | 139 | request-context, oauth config, `notify_catalog_changes` |
| Free helper fns | 2305–2625 | 321 | envelope/auth/scope/token/result-normalization |
| `mod tests` | 2627–3492 | 866 | tests |

Sub-division of `call_tool` (the piece that must itself be split):

| Sub-region | Lines | LOC |
|---|---|---|
| entry: arg parse + svc lookup | 1328–1351 | 24 |
| `search` + `execute` (Code Mode) meta-tool branches | 1352–1588 | 237 |
| gates + builtin dispatch branch | 1590–1755 | 166 |
| raw + subject-scoped upstream proxy | 1756–2135 | 380 |

---

## 2. The load-bearing Rust constraint

`impl ServerHandler for LabMcpServer` is a **trait impl**. Rust requires **all trait methods in one `impl` block in one file** — two `impl ServerHandler for LabMcpServer` blocks is a conflicting-implementations error. **You cannot split a trait impl across files.**

**Mechanism:** keep a *thin* `impl ServerHandler` in `server.rs` where each heavy handler delegates one line to an **inherent** `impl LabMcpServer` method in a sibling file:

```rust
// server.rs — thin trait impl
impl ServerHandler for LabMcpServer {
    async fn read_resource(&self, req: ReadResourceRequestParams, ctx: RequestContext<RoleServer>)
        -> Result<ReadResourceResult, ErrorData>
    { self.read_resource_impl(req, ctx).await }   // body lives in handlers_resources.rs
    // ... 9 more one-line delegators ...
}
```

This is **already the repo's idiom**: `catalog.rs` (`catalog.rs:58`) and `logging.rs` (`logging.rs:72`) each carry an inherent `impl LabMcpServer { ... }` block for this same struct (Rust permits multiple *inherent* impl blocks across files; only the *trait* impl is single-file). The plan extends that pattern. (Correction, Revision 2: an earlier draft also cited `upstream.rs` here — verified false. `upstream.rs` carries only free `pub(crate)` fns, **no** impl block. Only `catalog.rs` and `logging.rs` carry the inherent-impl block. The idiom is real; the third citation was not.)

Other binding conventions (from `crates/lab/CLAUDE.md` and `mcp/CLAUDE.md`):
- **No `mod.rs`** anywhere. A module `foo` is declared in `foo.rs` sibling to its `foo/` dir. Tests use `#[cfg(test)] mod tests;` → `<module>/tests.rs` (a child-module *file*, not a `mod.rs`) so `super::` private access is preserved. **Note (Revision 2):** this `<module>/tests.rs` child-file pattern is a **newly-introduced** convention for this crate, not an existing one — verified zero `tests.rs` files and zero file-backed `mod tests;` declarations exist anywhere under `crates/lab/src` today. It is valid and `mod.rs`-free, but bead `.6` owns *establishing* it; treat it as new, not as an idiom to copy.
- **Native `async fn in trait`** only, never `#[async_trait]`.
- **All-features build is the source of truth.** Verify every step with `cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features` (or `just test`).
- **No business logic in `mcp/`** — this is a pure mechanical move; gateway meta-tools (`search`/`execute`) remain the sanctioned MCP-owned exception, calling `dispatch/gateway/code_mode.rs`.

---

## 3. Target module layout

All new files live under `crates/lab/src/mcp/` and are registered in `mcp.rs`. Inherent `impl LabMcpServer` methods unless noted as free fns.

| File | Responsibility | Moves in | Est. LOC |
|---|---|---|---|
| `server.rs` (slimmed) | `LabMcpServer` struct; `verify_upstream_subject_resolution_support`; thin `impl ServerHandler` (10 delegators); small handler bodies `get_info`/`set_level`/`on_initialized`/`complete`; `inject_gateway_origin_param` | stays | **<300** |
| `completion.rs` | `action_schema`, `completion_info`, `complete_prompt_arg`, `service_name_completions` | 110–188 (minus `string_array_arg`) | ~80 |
| `result_format.rs` | `format_dispatch_result`, `extract_error_info`, `tool_error_envelope`, `hash_arguments`, `estimate_tokens*` | 2305–2625 (subset, **minus** `normalize_upstream_result` — see Revision 2 / §7) | ~170 |
| `context.rs` | request-context inherent methods (`request_subject*`, `request_actor_key`, `request_runtime_owner`, `code_mode_surface`, `oauth_upstream_config(s)`) + free fns (`redact_subject_for_logging`, `*_from_extensions`, `oauth_upstream_subject_for_request`, scope checks: `tool_search_scope_allowed`, `tool_execute_scope_allowed`, `tool_execute_builtin_action_allowed`, `builtin_action_requires_admin`) | 2160, 2165–2217, 2324–2395 | ~250 |

**Visibility note (Revision 2 — finding #3).** The four scope/admin gate fns — `tool_search_scope_allowed`, `tool_execute_scope_allowed`, `tool_execute_builtin_action_allowed`, `builtin_action_requires_admin` (private `fn`s today at `server.rs:2352–2395`) — must be **widened to `pub(crate)`** in `context.rs` so `call_tool.rs` / `call_tool_codemode.rs` can call them. This is a **visibility change only, no logic change**: the auth surface is unchanged (same-crate widening). Their tests (`gateway_builtin_actions_require_admin_scope`, `tool_search_scope_allows_read_but_tool_execute_does_not`, `setup_destructive_builtin_actions_require_admin_scope`) must stay green in `context/tests.rs`. A reviewer should see only the `pub(crate)` keyword move with the function body byte-identical.
| `handlers_prompts.rs` | `list_prompts_impl`, `get_prompt_impl` (and optionally `complete_impl`) | 345–659 (+290–343) | ~400 |
| `handlers_resources.rs` | `list_resources_impl`, `read_resource_impl` skeleton (local `lab://catalog` + `lab://<svc>/actions` branches) | 661–1139 (skeleton) | ~250 |
| `resource_proxy.rs` | `read_resource`'s three proxy branches: gateway-synthetic, upstream, subject-scoped (inherent methods) | 748–1059 | ~250 |
| `handlers_tools.rs` | `list_tools_impl` + gateway meta-tool input-schema construction | 1141–1326 | ~230 |
| `call_tool.rs` | `call_tool_impl`: arg parse + svc lookup, gates (visibility/scope/admin/elicitation/unknown), builtin dispatch branch; routes to codemode/upstream helpers. **Seam-pinned in bead `.5` (finding #1).** | 1328–1351, 1590–1755 | ~300 |
| `call_tool_codemode.rs` | `search` + `execute` (Code Mode) branches (each **self-`return`s**); `string_array_arg`; owns `CODE_EXECUTE_DESCRIPTION` + `CODE_MODE_MAX_CODE_BYTES` consts | 1352–1588, 132–155, 41–105 | ~330 |
| `call_tool_upstream.rs` | raw upstream proxy + subject-scoped upstream proxy branches **+ the no-dispatcher-wired fallback** (owns the entire 1756–2134 tail, returns unconditionally — see bead `.5` seam spec). `normalize_upstream_result` is **not** here; it consolidates into `upstream.rs` (Revision 2). | 1756–2134 | ~390 |
| `upstream.rs` *(existing, consolidated)* | **`normalize_upstream_result`** (the live `canonical_kind`-based body, moved from `server.rs:2496`); the dead duplicate + dead `static_kind` are **deleted** | absorbs 2496–2578 (~83) | 110 → ~165 |
| `notify.rs` *(optional)* | `notify_catalog_changes` — **or** fold into existing `catalog.rs` (278 LOC, has room) | 2219–2302 | ~95 |

`server.rs` becomes a **thin assembler**: struct definition, the trait impl made of one-line delegators, the four genuinely-small handlers, and the startup self-test. Everything heavy lives behind an inherent `*_impl` method in a sibling.

### `call_tool` seam spec (Revision 2 — finding #1, HIGH)

The `call_tool` split (bead `.5`) is **seam-based, not line-range-based**. Derived from the real body (`server.rs:1329–2136`). The trait method becomes a one-line delegator:

```rust
// server.rs — thin trait impl (drop #[allow(clippy::too_many_lines)])
async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>)
    -> Result<CallToolResult, ErrorData>
{ self.call_tool_impl(request, context).await }
```

**Shared top-scope locals** computed once at the top of `call_tool_impl` (1334–1349) and consumed by downstream branches: `service: String`, `raw_arguments: Option<JsonObject>` (the un-defaulted clone — threaded verbatim into both upstream branches as the upstream `arguments`), `args: JsonObject`, `action: String`, `params: Value`, `instance: Option<String>`, `param_key_count: usize`, and `svc: Option<&RegisteredService>` (a borrow of `self.registry.services()`). `start`/`subject`/`actor_key` are computed just before builtin dispatch (1710–1712).

> **Borrow note:** `svc` is a borrow held across `&self` awaits. The codemode/upstream helpers take `&self`; rather than thread the borrowed `&RegisteredService` across a helper boundary (fighting the borrow checker), have each helper that needs it re-derive `let svc = self.registry.services().iter().find(|s| s.name == service);` or thread the **owned** data it needs (e.g. `entry.actions`). Pass `&str`/`&Value` references where the borrow allows; clone `params`/`args` only where a helper must own them (the perf review flags this as a negligible per-call allocation).

**Three-way `*_impl` signatures** (all inherent `impl LabMcpServer`, all `&self`, all return `Result<CallToolResult, ErrorData>`):

```rust
async fn call_tool_impl(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>)
    -> Result<CallToolResult, ErrorData>;
// codemode helpers — called only after the service-name match; each fully self-contained:
async fn call_tool_search_impl(&self, service: &str, args: &JsonObject, context: &RequestContext<RoleServer>)
    -> Result<CallToolResult, ErrorData>;   // body = 1352–1450 (search branch)
async fn call_tool_execute_impl(&self, service: &str, args: &JsonObject, context: &RequestContext<RoleServer>)
    -> Result<CallToolResult, ErrorData>;   // body = 1453–1587 (execute branch)
// upstream tail — owns the ENTIRE 1756–2134 tail and returns unconditionally:
async fn call_tool_upstream_impl(&self, service: &str, action: &str, raw_arguments: Option<JsonObject>,
    start: Instant, subject: &str, actor_key: Option<&str>, context: &RequestContext<RoleServer>)
    -> Result<CallToolResult, ErrorData>;   // body = 1756–2134 incl. no-dispatcher-wired fallback
```

> `call_tool_upstream_impl` needs **`action`** and **`actor_key`** in addition to the upstream-proxy locals: the no-dispatcher-wired fallback (2127–2134) calls `format_dispatch_result(Err(err), &service, &action, elapsed_ms, &subject, actor_key)` and `emit_dispatch_notification(&context, &service, &action, …)`. `action` is not re-derivable inside the helper (it comes from `args.get("action")`); pass it explicitly. `actor_key` is re-derivable via `self.request_actor_key(context)` but is listed for parity with the builtin branch. (Exact arg lists are implementer discretion **only** insofar as each helper must receive every local its body reads — the fallback's use of `action`/`actor_key` makes those non-optional here. The **contracts below** are not discretionary.)

**Preserved early-return ordering** (every step is an early `return` *except* the upstream tail, which is reached by fall-through — see contract). `call_tool_impl` runs them in this exact order:

1. **`search` scope+dispatch** (`service == TOOL_SEARCH_TOOL_NAME`) → `call_tool_search_impl`. Scope gate `tool_search_scope_allowed` runs **inside** this branch and self-returns on denial. **Self-returns.**
2. **`execute` scope+dispatch** (`service == TOOL_EXECUTE_TOOL_NAME`) → `call_tool_execute_impl`. Scope gate `tool_execute_scope_allowed` inside; self-returns. **Self-returns.**
3. **`service_visible_on_mcp` gate** (1590, only when `svc.is_some()`) → `not_found` envelope. **Self-returns.**
4. **`action_allowed_on_mcp` gate** (1602) → `unknown_action` envelope. **Self-returns.**
5. **`tool_search_visibility().hides_raw_tools()` gate** (1622) → `not_found` envelope. **Self-returns.**
6. **admin-scope gate** `tool_execute_builtin_action_allowed` (1634) → `forbidden` envelope. **Self-returns.**
7. **destructive elicitation gate** (1655) → `Confirmed` proceeds; `Declined`/`Cancelled`/`Failed` and `NotSupported`-without-`confirm` self-return `confirmation_required`. **Self-returns on refusal.**
8. **dispatch-start log** (1718), then **builtin dispatch branch** (1731, when `svc.is_some()`): `inject_gateway_origin_param` for gateway, `(entry.dispatch)(…)`, `format_dispatch_result`, `emit_dispatch_notification`. **Self-returns.**
9. **upstream tail** (1756–2134) → `call_tool_upstream_impl`. This is the fall-through path when `svc.is_none()`.

**Upstream-tail contract (the critical seam).** `call_tool_upstream_impl` owns the **entire** 1756–2134 tail and **returns unconditionally** — nothing falls through past it. Specifically it owns: raw-resolution (`resolve_raw_upstream_tool`, 1767), the hard-error early-return for non-`unknown_tool`/`not_found` kinds (1780, uses `canonical_kind`), the raw upstream branch (`if let Some(pool) … && let Some(Ok(...))`, 1811), the subject-scoped branch (`if let Some(oauth_subject) … { if let Some(owner) … }`, 1992), **and** the `no-dispatcher-wired` fallback (2127–2134). Because the raw and subject-scoped branches are conditional `if let` blocks that *fall through* when unmatched, the helper must keep the fallback inside itself and return at the end — do **not** signal "didn't match" via `Option` back to `call_tool_impl`, which would reintroduce ordering ambiguity. `call_tool_impl`'s last line is simply `self.call_tool_upstream_impl(...).await`.

**Side-effect checklist** — bead `.5` must assert every one of these fires on the same path as today:

| Side effect | Site (current line) | Branch | Must fire when |
|---|---|---|---|
| `pool.record_failure` | 1857 | raw, `counts_as_failure` | normalized result counts as failure |
| `pool.record_success` | 1876 | raw, success | normalized result is ok and not failure |
| `pool.record_failure` | 1904 | raw, transport `Err` | upstream call returned `Some(Err)` |
| `pool.record_failure` | 1946 | raw, `None` | upstream connection gone |
| `notify_catalog_changes` | 1900, 1906, 1952 | raw (all 3 arms) | after every raw-branch outcome |
| `emit_dispatch_notification` | 1796, 1891, 1928, 1974 | raw (resolution-fail + 3 arms) | each raw outcome |
| `notify_catalog_changes` | 1563, 1568 | `execute` branch | before returning from execute (ok + err) |
| `pool` `record_*` | — | subject-scoped | **none** — subject-scoped branch has no `record_*` (note the asymmetry; do not add one) |
| `emit_dispatch_notification` | 2075, 2108 | subject-scoped | ok + err arms |
| `format_dispatch_result` + `emit_dispatch_notification` | 2130–2132 | no-dispatcher-wired fallback | service matched nothing |

`normalize_upstream_result` is called at 1843 (raw) and 2033 (subject-scoped); after Revision 2 it lives in `upstream.rs` and both call sites import it from there (same `canonical_kind` body, zero behavior change).

### Proposed file tree (after)

```
crates/lab/src/mcp/
├── mcp.rs                 # (parent: crates/lab/src/mcp.rs) registers all modules
├── server.rs              # <300  — struct + thin ServerHandler (delegators) + small handlers
├── completion.rs          # ~80   + completion/tests.rs
├── result_format.rs       # ~170  + result_format/tests.rs
├── context.rs             # ~250  + context/tests.rs
├── handlers_prompts.rs    # ~400
├── handlers_resources.rs  # ~250
├── resource_proxy.rs      # ~250
├── handlers_tools.rs      # ~230  + handlers_tools/tests.rs (or catalog/tests.rs)
├── call_tool.rs           # ~300
├── call_tool_codemode.rs  # ~330  + call_tool_codemode/tests.rs
├── call_tool_upstream.rs  # ~390  (owns full 1756–2134 tail; normalize_upstream_result NOT here — consolidated into upstream.rs)
│   # existing siblings (upstream.rs now consolidates normalize_upstream_result):
├── catalog.rs   278   ├── logging.rs 188   ├── upstream.rs ~165 (+ upstream/tests.rs)
├── envelope.rs  324   ├── error.rs   285   ├── prompts.rs  307
├── peers.rs     139   ├── resources.rs 42  ├── elicitation.rs 151
├── meta.rs       28   ├── registry.rs   3  ├── services.rs   18
```

### Test distribution (the ~866-LOC block)

Tests follow the code they exercise, each in a `<module>/tests.rs` child file. Each `tests.rs` is independently <500 LOC.

| `tests.rs` host | Tests | ~LOC |
|---|---|---|
| `result_format/tests.rs` | `estimate_tokens*`, `extract_error_info_*`, `tool_error_envelope_preserves_structured_extras`, `canonical_kind_round_trips_all_tool_error_kinds` | ~155 |
| `upstream/tests.rs` | `normalize_upstream_result_preserves_user_errors_without_poisoning_health` (tracks the fn into `upstream.rs` — Revision 2) | ~15 |
| `call_tool_codemode/tests.rs` | `code_mode_filter_arg_*`, `code_execute_description_contains_protocol_contract`, `gateway_search_input_schema_is_code_only` | ~70 |
| `completion/tests.rs` | `completion_*` + fixtures (`noop_dispatch`, `completion_test_registry`, `TEST_ACTIONS_*`) | ~120 |
| `handlers_tools/tests.rs` or `catalog/tests.rs` | `snapshot_catalog_*`, `server_reads_current_pool_from_gateway_manager`, `service_actions_json_filters_to_allowed_mcp_actions` (`#[ignore]`) | ~230 |
| `context/tests.rs` | scope/admin tests, `oauth_upstream_subject_*`, `server_reads_subject_scoped_upstream_pool_from_request_extensions`, `make_auth` fixture | ~250 |
| `server.rs` (kept minimal) | `server_capabilities_advertise_list_changed_support`, `upstream_subject_resolution_self_test_passes_for_plan_a` | ~40 |

Shared fixtures (`noop_dispatch`, `completion_test_registry`, `make_auth`, `TEST_ACTIONS_*`): prefer minimal duplication of the small ones over introducing a new shared test module, unless duplication would exceed ~40 LOC.

---

## 4. Migration sequence (build stays green at every step)

Each step ends with a green `cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features`. Visibility bumps (`pub(crate)` / `pub(super)`) on moved free fns are caught immediately by the per-step compile.

1. **Leaf free-fn modules** (bead `.1`) — `completion.rs`, `result_format.rs`, `context.rs`. No delegation dependency; pure move + visibility edits. Confirm `extract_error_info` (currently `pub`, used only inside `server.rs`/tests) relocates as `pub(crate)`; re-export from `server.rs` only if a grep shows an external consumer.
2. **Prompt handlers** (bead `.2`) — delegate `list_prompts`/`get_prompt` → `handlers_prompts.rs`.
3. **Resource handlers** (bead `.3`) — delegate `list_resources`/`read_resource`; pull proxy branches into `resource_proxy.rs`. **Same seam discipline as `.5`, lower risk (finding #4):** `read_resource` (731–1141) computes `subject` (737) and `uri` (738) at the top and threads them into the gateway-synthetic, upstream, and subject-scoped branches. Define the skeleton↔`resource_proxy` method signatures, pin which locals each branch receives (`subject`, `uri`, the resolved pool), and assert the three-branch ordering is preserved. The side effects here are **structured logging + `pool.read_upstream_resource` ordering only — no circuit-breaker `record_*`** (that is why this is lower-risk than `.5`, but it is *not* a free move). (Independent of `.2`; parallelizable.)
4. **`list_tools`** (bead `.4`) — delegate → `handlers_tools.rs`; assign `CODE_EXECUTE_DESCRIPTION` ownership here. (Independent of `.2`/`.3`.)
5. **`call_tool` fan-out** (bead `.5`) — **last** extraction; the most entangled. Delegate `call_tool` → `call_tool.rs` + `call_tool_codemode.rs` + `call_tool_upstream.rs`, following the **§3 seam spec** (signatures, ordering, upstream-tail unconditional-return contract, side-effect checklist). Sequenced after `.4` to settle the `CODE_EXECUTE_DESCRIPTION` const owner. **Also consolidates `normalize_upstream_result`** (Revision 2 / finding #2): move the live `canonical_kind`-based copy from `server.rs:2496` into `upstream.rs`, delete the dead `pub(crate)` duplicate and the dead `static_kind` (both in `upstream.rs`), repoint the 1843/2033 call sites' import. Net effect on §7 budget: improves (see Revision 2).
6. **Distribute tests** (bead `.6`) — move the `mod tests` block into per-module `tests.rs` child files. Requires all target code moved first.
7. **Final verification** (bead `.7`) — `find crates/lab/src/mcp -name '*.rs' | xargs wc -l | sort -n` proves max <500; `just test` + `just lint` green; drop dead `#[allow(...)]` (e.g. `clippy::too_many_lines` on `call_tool`).

### File-scope conflict note
Every bead edits `crates/lab/src/mcp.rs` (one `pub mod` line each). The dependency DAG serializes most; the only parallel-eligible trio (`.2`/`.3`/`.4` after `.1`) appends distinct non-overlapping lines — low risk, flagged in each bead. If executed truly concurrently, sequence the `mcp.rs` edits.

### Verification command
```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features   # per-step
just test    # workspace --all-features
just lint    # clippy -D warnings + fmt --check
find crates/lab/src/mcp -name '*.rs' -exec wc -l {} + | sort -n          # <500 gate
```

---

## 5. Bead breakdown

| Bead | Title | Depends on |
|---|---|---|
| `lab-kvji.24.1` | (parent) Split oversized `mcp/server.rs` by responsibility | — |
| `lab-kvji.24.1.1` | Extract leaf free-fn modules (`completion`, `result_format`, `context`) | — |
| `lab-kvji.24.1.2` | Extract prompt + completion handlers → `handlers_prompts.rs` | `.1` |
| `lab-kvji.24.1.3` | Extract resource handlers → `handlers_resources.rs` + `resource_proxy.rs` | `.1` |
| `lab-kvji.24.1.4` | Extract `list_tools` → `handlers_tools.rs` | `.1` |
| `lab-kvji.24.1.5` | Split `call_tool` → `call_tool.rs` + `call_tool_codemode.rs` + `call_tool_upstream.rs` | `.1`, `.4` |
| `lab-kvji.24.1.6` | Distribute tests into per-module `tests.rs` child modules | `.2`,`.3`,`.4`,`.5` |
| `lab-kvji.24.1.7` | Final verification: every `mcp/` file <500 LOC, all-features test/clippy green | `.6` |

---

## 6. Risks & guardrails

- **Trait-impl single-file rule** is the one constraint that, if missed, produces a non-compiling result. Every heavy handler must remain a one-line delegator in `server.rs`.
- **`call_tool` control flow is side-effect-heavy** (elicitation gate, scope gates, circuit-breaker `record_success`/`record_failure`, catalog-change notifications). The split must preserve ordering and every side effect byte-for-byte. Bead `.5` is the highest-risk; it ships last. This is **operationalized** in the §3 "`call_tool` seam spec" — exact `*_impl` signatures, the preserved early-return ordering, the upstream-tail unconditional-return contract, and a per-side-effect checklist (every `record_failure`/`record_success`, each `notify_catalog_changes`, each `emit_dispatch_notification`, the elicitation outcomes). Bead `.5` must verify each row of that checklist fires on the same path as today. The single highest-value guard: a failing upstream call's `record_failure` must not get stranded behind an early return added during the split, or the circuit breaker silently stops tripping.
- **`CODE_EXECUTE_DESCRIPTION` is consumed twice** (tool registration in `list_tools`, and the `execute` branch). Exactly one definition; ownership assigned in bead `.4`, imported by `.5`.
- **Pure mechanical refactor** — zero behavior change. Success = the identical test set, green, with every file <500 LOC.

---

## 7. Plan review (advisor)

Two advisor passes shaped this plan:

1. **Trait-impl constraint (design-defining).** The first review flagged that `impl ServerHandler for LabMcpServer` is a trait impl and cannot be split across files. The plan was reframed around the thin-trait-impl + one-line-delegator + inherent-`*_impl`-method pattern (§2), which matches the repo's existing inherent-impl idiom (`catalog.rs`, `logging.rs` — see the §2 Revision-2 correction: *not* `upstream.rs`). This is the one constraint that, if missed, yields a non-compiling result.

2. **`call_tool_upstream.rs` budget rebalance (post-plan fix).** The second review caught that `call_tool_upstream.rs` was under-counted: raw + subject-scoped proxy is 380 LOC and `normalize_upstream_result` adds ~83, giving ~463 body LOC before imports — too close to 500 for the file nearest the limit. The original fix parked `normalize_upstream_result` in `result_format.rs`. **Superseded by Revision 2 (§8):** the cleaner fix consolidates `normalize_upstream_result` into `upstream.rs` (its semantic home, deleting a dead duplicate) instead, which frees the same ~83 LOC *and* removes dead code — `call_tool_upstream.rs` lands ~390 (now owning the no-dispatcher-wired fallback too). Bead `.2`'s "keep `complete` in `server.rs`" was also promoted from optional to default to hold `handlers_prompts.rs` (the second-tightest file at ~370–400) under budget.

No other blind spots were found in pass 2: the `<module>/tests.rs` child pattern is `mod.rs`-free and preserves `super::` private access, and the `search`/`execute`-before-gates control-flow ordering in `call_tool` is preserved in bead `.5`.

---

## 8. Revision 2 (eng-review fold-in)

This revision folds the five findings from the engineering review (`docs/dev/refactor-plan-mcp-server-split.review.md`, verdict **APPROVE-WITH-CHANGES**) into the plan and child beads. Each fix was verified against the real source in this worktree, not applied blindly.

1. **HIGH — `call_tool` split is now seam-based, not line-range-based.** Added the **"`call_tool` seam spec"** subsection in §3: the three `*_impl` signatures (`call_tool_impl`, codemode `search`/`execute` helpers that self-`return`, `call_tool_upstream_impl`), the shared top-scope locals (`service`/`raw_arguments`/`args`/`action`/`params`/`instance`/`param_key_count`/`svc` + `start`/`subject`/`actor_key`) and how they thread into each helper, the preserved 9-step early-return ordering (visibility → action → tool_search-hidden → admin-scope → elicitation → builtin → upstream tail), the **upstream-tail unconditional-return contract** (the helper owns the entire 1756–2134 tail *including* the no-dispatcher-wired fallback, because the raw + subject-scoped branches fall through when unmatched — it must not signal "didn't match" via `Option`), and a **per-side-effect checklist** mapping every `record_failure`/`record_success`, `notify_catalog_changes`, and `emit_dispatch_notification` to its branch. §6 and bead `.5` reference it.

2. **MEDIUM — `normalize_upstream_result` consolidated into `upstream.rs` (resolution: keep the live `canonical_kind` body).** Verified the live private copy (`server.rs:2496`, called at 1843/2033) and the dead `pub(crate)` copy (`upstream.rs:28`, `#[allow(dead_code)]` beside dead `static_kind`) differ in exactly two lines: visibility, and the kind-mapper (`canonical_kind` vs `static_kind`). **The two kind-mappers are NOT equivalent** — `canonical_kind` maps `"conflict" → "conflict"`, while `static_kind` lacks that arm and maps it to `"internal_error"`. The divergence is confined entirely to `static_kind`, which is **dead** (grep-confirmed: only self-references in `upstream.rs`; the dead `normalize_upstream_result` copy has no callers). Resolution: consolidate the **live `canonical_kind`-based body** into `upstream.rs` (its semantic home), delete the dead duplicate **and** the dead `static_kind`. This is zero-behavior-change (the live path already uses `canonical_kind`) *and* removes dead code — strictly safer than both the original relocation-to-`result_format.rs` (which preserved the duplication and dead code) and a naive reconcile. This supersedes plan §3/§7 and bead `.5`'s "prefer `call_tool_upstream.rs`" discretion line; `result_format.rs` drops to ~170, `upstream.rs` rises to ~165, the `normalize_upstream_result_*` test tracks to `upstream/tests.rs`.

3. **MEDIUM — `context.rs` scope-fn visibility note.** Added to the §3 `context.rs` row: `tool_search_scope_allowed`, `tool_execute_scope_allowed`, `tool_execute_builtin_action_allowed`, `builtin_action_requires_admin` are **widened to `pub(crate)`, no logic change**; their tests stay green in `context/tests.rs`.

4. **MEDIUM-LOW — bead `.3` (`resource_proxy.rs`) gets the same seam discipline as `.5`.** §4 step 3 + bead `.3` now pin: thread `subject` (737) + `uri` (738) into the three proxy branches, define the skeleton↔`resource_proxy` signatures, preserve three-branch ordering. Explicitly noted lower-risk — **logging + `read_upstream_resource` ordering only, no `record_*`** — but *not* a free move.

5. **LOW — two citation fixes.** (a) §2/§7 + bead `.1` Context corrected: `upstream.rs` carries **no** inherent `impl LabMcpServer` block — only `catalog.rs` and `logging.rs` do (`upstream.rs` holds free `pub(crate)` fns). (b) The `<module>/tests.rs` child-file pattern is reframed as a **newly-introduced** convention (zero `tests.rs` / file-backed `mod tests;` exist under `crates/lab/src` today), not an established idiom; bead `.6` owns establishing it.

**Beads updated:** `.1` (Context idiom fix + scope-fn visibility note), `.3` (resource_proxy seam discipline), `.5` (seam spec + normalize consolidation), `.6` (test colocation: `normalize_upstream_result_*` → `upstream/tests.rs`). Verdict carried: APPROVE-WITH-CHANGES, all changes folded.
