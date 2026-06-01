# Refactor Plan: Split `dispatch/upstream/pool.rs` into focused runtime modules

**Bead:** [lab-kvji.12](#) — "Split upstream pool responsibilities into smaller runtime modules"
**Parent epic:** lab-kvji.24 — "(EPIC) Architecture and maintainability refactors from comprehensive review"
**Status:** PLAN (no production code changed)
**Hard constraint:** No file (remaining `pool.rs` or any new module) may exceed **500 LOC**, tests included.

---

## 1. Problem

`crates/lab/src/dispatch/upstream/pool.rs` is **5,502 LOC / 205 KB**. It mixes seven
responsibilities in one module:

| Responsibility | Approx LOC (prod) | Current line range |
|---|---|---|
| Leaf helpers (config knobs, error classification, naming, redaction) | ~120 | L49–168 |
| Request logging helpers (`UpstreamRequestLog`, start/finish/error) | ~122 | L274–395 |
| Capability discovery helpers (`routable_upstream_peers`, `discover_capability_counts`, prompt/resource merge/rewrite, `cached_upstream_tool`) | ~290 | L169–578 |
| `UpstreamConnection` (struct, types, `Drop`, shutdown/acquire impl) | ~167 | L604–770 |
| `UpstreamPool` god-impl: construction, drain/swap, discovery, reprobe, in-process registration, tool queries, call, health/circuit-breaker, resources, prompts | **~2,600** | L771–3377 |
| Connection establishment free fns (`connect_upstream` / `_http` / `_stdio` / `_websocket` / `_in_process`) | ~455 | L3434–3888 |
| Entry constructors + exposure policy resolution | ~119 | L3889–4007 |
| `#[cfg(test)] mod tests` | **~1,495** | L4008–5502 |

Two blocks individually blow the 500-LOC budget by multiples — the `impl UpstreamPool`
block (~2,600) and the test mod (~1,495) — so both **must be distributed**, not merely
relocated.

---

## 2. Strategy: child modules under `pool/`, struct defs stay in `pool.rs`

**Key Rust-privacy fact that makes this clean:** a private struct field is visible in the
defining module *and all descendant modules*. So if `UpstreamPool` and `UpstreamConnection`
**stay defined in `pool.rs`**, child modules `pool/discovery.rs`, `pool/tools.rs`, etc. can
read and mutate their private fields with **zero `pub(super)` annotations**. We move method
*bodies* into child modules as additional `impl UpstreamPool { ... }` blocks; the type
definition is untouched.

Consequences:
- `pool.rs` shrinks to: module header, struct definitions (`UpstreamPool`,
  `UpstreamConnection`, `InProcessRegistration`, connector type aliases), `mod`
  declarations, and `pub use` re-exports. A thin coordinator.
- Cross-module free fns (`connect_*`, `classify_upstream_error`, logging helpers) become
  `pub(super)` (or `pub(crate)` where an external caller exists) and are called via
  `super::connect::connect_upstream(...)` etc.
- `UpstreamConnection`'s own `impl` (shutdown/acquire/Drop) stays co-located with its
  definition. Plan keeps the struct def in `pool.rs` but its `impl`/`Drop` in
  `pool/connection.rs`; fields it touches across that boundary are marked `pub(super)`
  (minimal — they are descendants of `pool`, so `pub(super)` on the `connection` child
  suffices). Prefer a `pub(super) fn peer(&self)` accessor over opening fields when a
  sibling module (`tools.rs`/`resources.rs`) needs the peer handle.

### Public-surface preservation (build must stay green at step 1)

These items are referenced from outside `pool.rs` by their full `pool::` path and **must be
re-exported** from `pool.rs` after their definitions move:

| Item | Visibility | External callers |
|---|---|---|
| `UpstreamPool` | `pub` | `dispatch/gateway/{projection,manager}.rs`, AppState, MCP server |
| `UpstreamCachedSummary` | `pub` | `dispatch/gateway/projection.rs`, `manager.rs` |
| `in_process_upstream_name` | `pub` | `dispatch/gateway/manager.rs` |
| `redact_resource_uri_for_logging` | `pub(crate)` | `mcp/server.rs` (13 call sites, full path) |
| `upstream_discovery_concurrency` | `pub(crate)` | (within upstream tree) |

Re-export form in `pool.rs`:
```rust
mod helpers;      // leaf knobs + redaction + UpstreamCachedSummary
mod logging;      // request log helpers
mod discovery;    // discover_all, capability counts
mod reprobe;      // probe task scheduling
mod registration; // in-process service peers
mod tools;        // tool queries + call_tool
mod health;       // circuit breaker
mod resources;    // resource listing/read
mod prompts;      // prompt listing/get
mod connection;   // UpstreamConnection impl
mod connect;      // connect_* free fns
mod entries;      // entry constructors + exposure policy
#[cfg(test)] mod testsupport;

pub use helpers::{UpstreamCachedSummary, in_process_upstream_name};
pub(crate) use helpers::{redact_resource_uri_for_logging, upstream_discovery_concurrency};
```

---

## 3. Target module layout under `dispatch/upstream/`

`pool/` is a child directory; `pool.rs` is its sibling entrypoint (**no `mod.rs`** — repo rule).
Per-module LOC = production lines + co-located `#[cfg(test)]` tests for that module's logic.


## 3. Target module layout under `dispatch/upstream/`

`pool/` is a child directory; `pool.rs` is its sibling entrypoint (**no `mod.rs`** — repo rule).

> **Sizing note.** The table below uses **measured** per-function LOC (from line-delta
> analysis of the current file), not estimates. An earlier draft of this plan used top-down
> targets and underestimated `discover_all_inner` (273 LOC), the two resource-read methods
> (255 LOC combined), and the `validate_*` test cluster (193 LOC). Summing measured prod **and**
> measured test LOC per file forced a finer partition: **22 files**. The `<500` rule caps file
> size, not file count — more small files is the correct direction.

Every file: `prod + ~20 (use-block + impl wrapper) + co-located test < 500`.

```
dispatch/upstream/
├── pool.rs                  # ~110  coordinator: UpstreamPool/UpstreamConnection struct defs, builders, Default, mod decls, re-exports
├── pool/
│   ├── helpers.rs           # leaf knobs, error classification, naming, redaction, UpstreamCachedSummary, merge/rewrite/cached_upstream_tool helpers
│   ├── logging.rs           # UpstreamRequestLog + log_upstream_request_{start,finish,error}
│   ├── connection.rs        # UpstreamConnection Debug/Drop/shutdown + acquire_peer
│   ├── lifecycle.rs         # drain_for_swap
│   ├── validate.rs          # validate_upstream_config + 8 validate_* tests
│   ├── connect.rs           # connect_upstream/_http/_websocket + jitter/oauth log helpers + runtime_origin_label
│   ├── connect_stdio.rs     # connect_stdio_upstream + connect_in_process_service_peer
│   ├── discover.rs          # discover_all_inner + discover_all{,_for_subject}{,_with_in_process_peers} + routable_upstream_peers
│   ├── ensure.rs            # seed_lazy_upstreams, ensure_tools_for_upstream*, install/reprobe_tools, lazy locks
│   ├── capability.rs        # discover_capability_counts
│   ├── probe.rs             # ensure_probe_task + reprobe_upstream
│   ├── registration.rs      # register_in_process_service_peers + _list + _list_with_connector
│   ├── tools.rs             # healthy_tools*, find_tool*, tool_schema, tool_exposure_rows, cached_upstream_summary, subject_scoped_tools, runtime_metadata, upstream_tool_health
│   ├── tools_call.rs        # call_tool + subject_scoped_call_tool
│   ├── health.rs            # record_*/should_reprobe*/last_error/filter_collisions/upstream_status/count
│   ├── resources_list.rs    # list_upstream_resources, subject_scoped_resources, cached_uris, gateway_* docs/schema
│   ├── resources_read.rs    # read_upstream_resource + subject_scoped_read_resource
│   ├── prompts_list.rs      # list_upstream_prompts, collect_upstream_prompts, ownership_map, find_prompt_owner, cached names/owner
│   ├── prompts_get.rs       # subject_scoped_prompts, subject_scoped_prompt_owner, get_prompt, subject_scoped_get_prompt
│   ├── entries.rs           # lazy/healthy/failed entry constructors, resolve_exposure_policy
│   └── testsupport.rs       # #[cfg(test)] shared fixtures + mock servers (pub(super))
└── (auth.rs, http_client.rs, process_guard.rs, transport.rs, types.rs unchanged)
```

### 3.1 LOC arithmetic — measured prod + measured test, every file < 500

| File | Prod | +wrap | +test | **Total** | <500? |
|---|---:|---:|---:|---:|:--:|
| `pool.rs` | 110 | 0 | 0 | **110** | yes |
| `pool/helpers.rs` | 247 | 20 | 19 | **286** | yes |
| `pool/logging.rs` | 122 | 20 | 58 | **200** | yes |
| `pool/connection.rs` | 188 | 20 | 0 | **208** | yes |
| `pool/lifecycle.rs` | 66 | 20 | 0 | **86** | yes |
| `pool/validate.rs` | 41 | 20 | 193 | **254** | yes |
| `pool/connect.rs` | 288 | 20 | 77 | **385** | yes |
| `pool/connect_stdio.rs` | 167 | 20 | 0 | **187** | yes |
| `pool/discover.rs` | 392 | 20 | 0 | **412** | yes |
| `pool/ensure.rs` | 278 | 20 | 148 | **446** | yes |
| `pool/capability.rs` | 89 | 20 | 0 | **109** | yes |
| `pool/probe.rs` | 258 | 20 | 42 | **320** | yes |
| `pool/registration.rs` | 151 | 20 | 158 | **329** | yes |
| `pool/tools.rs` | 258 | 20 | 47 | **325** | yes |
| `pool/tools_call.rs` | 168 | 20 | 13 | **201** | yes |
| `pool/health.rs` | 170 | 20 | 67 | **257** | yes |
| `pool/resources_list.rs` | 222 | 20 | 133 | **375** | yes |
| `pool/resources_read.rs` | 255 | 20 | 69 | **344** | yes |
| `pool/prompts_list.rs` | 132 | 20 | 117 | **269** | yes |
| `pool/prompts_get.rs` | 241 | 20 | 13 | **274** | yes |
| `pool/entries.rs` | 123 | 20 | 55 | **198** | yes |
| `pool/testsupport.rs` | 0 | 20 | 271 | **291** | yes |

**Max total: `discover.rs` at 412.** No file breaches 500 on the summed measured numbers.
`discover.rs` (412) and `ensure.rs` (446) carry the least headroom — re-measure with `wc -l`
after their extraction; if either crosses 500 (e.g. relocated imports inflate it), the
contingency is to peel `routable_upstream_peers` (88) out of `discover.rs` into `capability.rs`,
or split `ensure.rs` along the seed-vs-ensure_tools line.

### 3.2 Test distribution (measured)

Each test cluster co-locates with the module it exercises under that module's own
`#[cfg(test)] mod tests`. Measured cluster sizes drove the file split:

| Test cluster (measured LOC) | Lands in |
|---|---|
| fixtures: `test_upstream_config`, `named_*`, `test_tool*`, mock `ServerHandler`s, `static_catalog_pool*`, `slow_response_pool`, `oauth_http_config` (~271) | `pool/testsupport.rs` (`pub(super)`) |
| `seed_lazy_upstreams_*` + `ensure_tools_for_upstream_*` (~148) | `pool/ensure.rs` |
| `upstream_request_log_helpers_*` (58) | `pool/logging.rs` |
| `validate_*` ×8 (193) | `pool/validate.rs` |
| oauth-connect: `oauth_*`, `subject_scoped_*_oauth_*`, `shared_discovery_skips_*` (77) | `pool/connect.rs` |
| `merge_upstream_prompts_*` (22) + `successful_prompt_listing` (37) + `prompt_owner_lookup_*` (58) | `pool/prompts_list.rs` |
| `get_prompt_times_out_*` (13) | `pool/prompts_get.rs` |
| `normalize_resource_result_uri_*` (28) + gateway_* tests (105) | `pool/resources_list.rs` |
| `successful_resource_listing_*` (56) + `read_resource_times_out_*` (13) | `pool/resources_read.rs` |
| `hidden_upstream_tools_*` (47) | `pool/tools.rs` |
| `call_tool_times_out_*` (13) | `pool/tools_call.rs` |
| `upstream_last_error_*` + `upstream_tool_last_error_*` (67) | `pool/health.rs` |
| `failed_in_process_entry_from_existing_*` (48) + `invalid_exposure_policy_*` (7) | `pool/entries.rs` |
| `in_process_registration_*` ×2 (158) | `pool/registration.rs` |
| `disabled_upstream_reprobe_is_inert` (16) + `observability_source_*` (26) | `pool/probe.rs` |
| redact tests (19) | `pool/helpers.rs` |

Shared fixtures are `pub(super)` in `pool/testsupport.rs`; each test module does
`use super::super::testsupport::*;`.

---

## 4. Migration sequence (build green after every step)

Verify after **each** step:
```
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features
```
(or `just test`). Commit per step so any regression is bisectable. **After each extraction,
run `wc -l` on the new file(s)** — the table is measured but moved code grows by its
`use`-block + `impl` wrapper, so confirm the real count stays <500.

**Order: leaves first, then connection fns, then the impl clusters.** Each move is a pure
relocation — no logic change — so a clean compile + green tests is the gate.

1. **Scaffold** `pool/` dir + empty `mod` decls in `pool.rs`. Build (no-op).
2. **`helpers.rs`** — leaf free fns + `UpstreamCachedSummary` + merge/rewrite/cached_upstream_tool helpers + re-exports. Proves public paths still resolve (`mcp/server.rs`, `gateway/*`). Build.
3. **`logging.rs`** — `UpstreamRequestLog` + log helpers + test. Build.
4. **`entries.rs`** — entry constructors + `resolve_exposure_policy` + tests. Build.
5. **`validate.rs`** — `validate_upstream_config` + 8 validate_* tests. Build.
6. **`connect.rs` + `connect_stdio.rs`** — connect_* free fns + jitter/oauth helpers; stdio + in-process spawn into `connect_stdio.rs`; oauth-connect tests into `connect.rs`. Build.
7. **`connection.rs` + `lifecycle.rs`** — `UpstreamConnection` impl + `acquire_peer`; `drain_for_swap` into `lifecycle.rs`. Build.
8. **`testsupport.rs`** — shared fixtures + mock servers as `pub(super)`. Build.
9. **`health.rs`** — circuit-breaker/record_*/should_reprobe*/status + tests. Build.
10. **`capability.rs`** — `discover_capability_counts`. Build.
11. **`discover.rs`** — discover_all*/routable_upstream_peers. Build.
12. **`ensure.rs`** — seed/ensure_tools*/install/reprobe_tools + tests. Build.
13. **`probe.rs`** — ensure_probe_task + reprobe_upstream + tests. Build.
14. **`registration.rs`** — in-process registration + tests. Build.
15. **`tools.rs` + `tools_call.rs`** — query methods vs call methods + tests. Build.
16. **`resources_list.rs` + `resources_read.rs`** — list/gateway vs read methods + tests. Build.
17. **`prompts_list.rs` + `prompts_get.rs`** — list/ownership vs get methods + tests. Build.
18. **Final `pool.rs` sweep** — confirm it is only struct defs + builders + `mod` + `pub use`;
    confirm `wc -l` on every file is <500. Run full `just test` + `just lint` + `just build`.

---

## 5. Doc + rule updates (doc-freshness CI will flag these)

`dispatch/upstream/CLAUDE.md` must be updated in the same PR-set:

- **Files table** (currently lists only `upstream.rs`, `pool.rs`, `types.rs`) — add the 21
  `pool/` child modules with one-line purposes.
- **Constants table** — `DISCOVERY_TIMEOUT` and `DEFAULT_MAX_RESPONSE_BYTES` currently say
  "Location: pool.rs". After the move they live in `pool/helpers.rs`. Update the location column.
- **Env-read rule** — current wording: *"Do not read env vars outside `pool.rs::max_response_bytes()`
  and the connection functions."* `max_response_bytes()` moves to `pool/helpers.rs`. Keep env
  reads confined to `pool/helpers.rs` + the connect modules (`pool/connect.rs`,
  `pool/connect_stdio.rs`) and reword the rule to name those modules, preserving the intent
  (env reads stay in a small, named set of places).

---

## 6. Risks / non-goals

- **No behavior change.** Pure structural refactor. No function signatures, error kinds, log
  fields, or env semantics change. If any signature must change to relocate, that is out of
  scope — stop and reconsider.
- **`UpstreamConnection` field privacy.** Keep its `impl` in `pool/connection.rs` (a descendant
  of `pool`); `pub(super)` on its fields suffices. Prefer a `pub(super) fn peer(&self)` accessor
  over opening fields when a sibling module needs the peer handle.
- **Tightest files: `discover.rs` (412), `ensure.rs` (446).** Re-measure with `wc -l` after the
  move; contingency splits are pre-named (peel `routable_upstream_peers` to `capability.rs`;
  split `ensure.rs` on the seed/ensure_tools line) so they do not block.
- **Numbers are measured, not guessed** — but moved code grows by its `use`-block + `impl`
  wrapper. The per-step `wc -l` check (§4) is the authoritative gate; the table is the plan.
