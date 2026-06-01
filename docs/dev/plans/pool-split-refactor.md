# Refactor Plan: Split `dispatch/upstream/pool.rs` into focused runtime modules

**Bead:** [lab-kvji.12](#) — "Split upstream pool responsibilities into smaller runtime modules"
**Parent epic:** lab-kvji.24 — "(EPIC) Architecture and maintainability refactors from comprehensive review"
**Status:** PLAN (no production code changed)
**Hard constraint:** No file (remaining `pool.rs` or any new module) may exceed **500 LOC**, tests included.

---

## 0. Revision (eng-review fold-in)

This plan was revised on **2026-06-01** to fold in every finding from the engineering review
(`pool-split-refactor.REVIEW.md`, verdict: APPROVE-WITH-CHANGES). Each finding was re-verified
against the real `pool.rs` before incorporation — not applied blindly.

1. **Headline / REQUIRED — private-method visibility (`pub(super)`).** New **§2.1** documents that
   the "zero `pub(super)` churn" claim is **field-only**. Verified the review's ~6-method list
   against source via call-site → enclosing-fn → target-module tracing: **confirmed 5, dropped 1,
   added 2.** Final list of **7 items** needing `pub(super)`, each mapped to the migration
   step/bead where the flip must land (load-bearing for the per-step green build, not just the end
   state). **Dropped `cached_prompt_owner`** — the review mis-attributed its callers to
   `prompts_get.rs`; both call sites are in `find_prompt_owner`, same module (`prompts_list.rs`).
   **Added `UpstreamConnection::shutdown` and the `UpstreamRequestLog` constructors** — the
   review's `self.`-only grep missed these (called on local bindings).
2. **MEDIUM — homeless shared mutators.** New **§3.0** + layout-table labels: `replace_catalog_tools`
   → `ensure.rs`, `has_healthy_tools_for_upstream` → `tools.rs`, both marked `[SHARED pub(super)]`.
3. **MEDIUM (document-don't-change) — cap-driven pairs.** New **§3.3** notes the four pairs
   (`resources_list/read`, `prompts_list/get`, `tools/tools_call`, `connect/connect_stdio`) split
   one responsibility across two files to honor the cap, not on a domain seam.
4. **LOW — cap is self-chosen.** **§3.3** states `<500` is self-imposed (bead constraint), shows
   the ~600-cap merge set, and records the decision to keep 22 files anyway.
5. **LOW/MED — two execution-time guards.** **§6** + inline step annotations (steps 7, 16, 17) flag
   the `UpstreamConnection::Drop` SIGTERM→abort ordering and the `subject_scoped_*`
   redaction/subject threading as byte-identical NO-TOUCH moves.

Affected child beads updated in lockstep: `.12.3`, `.12.4`, `.12.6`, `.12.7`, `.12.8` carry the
`pub(super)` timing and shared-mutator/no-touch notes.

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

### 2.1 Private-inherent-method visibility (`pub(super)` flips) — the "zero churn" claim is FIELD-ONLY

> **Correction (eng-review headline, MEDIUM).** §2 above is exactly right about **fields**:
> every `UpstreamPool` and `UpstreamConnection` field is private, and a private field is
> visible to the defining module *and all descendant modules* — so child modules read/write
> pool state with zero annotations. But that guarantee covers **fields only**. It does **not**
> cover **private inherent methods**. Moving an unmarked `fn` body into a child module
> *narrows* its visibility to that submodule; any caller that lives in a *sibling* module (or in
> the residual `pool.rs` ancestor) then fails to compile with **E0624** (private item). The
> general rule the implementer must apply: **any private inherent method — on `UpstreamPool`,
> `UpstreamConnection`, OR any other type (e.g. `UpstreamRequestLog`) — that is called across
> one of the new module boundaries must be promoted to `pub(super)` when its body/def moves.**
> Fields need no annotation; cross-module methods (and any private type whose impl methods are
> called cross-module) do.

**Why this is load-bearing for the per-step green build, not just the end state.** Callee and
callers move in *different* migration steps. The `pub(super)` bump must land **in the same step
the callee moves** — not deferred to the final sweep — or that intermediate step fails E0624.
Two directions:
- **Child → ancestor** (extracted module calls a `pool.rs`-private item): always compiles
  (descendant sees ancestor-private). No action.
- **Ancestor / sibling → just-extracted child** (residual `pool.rs`, or an *already-moved*
  sibling module, calls a method whose body now lives in a child): does **not** compile — an
  ancestor is not a descendant, and siblings don't see each other's privates. This is the case
  that needs the flip, *at the callee's extraction step*.

**Verified cross-module private methods (traced call-site → enclosing-fn → target module against
the real `pool.rs`).** The review named ~6; verification confirms **5 of them**, **drops 1**
(`cached_prompt_owner` — see note), and **adds 2** the review's `self.`-only grep missed
(`UpstreamConnection::shutdown` and the `UpstreamRequestLog` constructors, both called on local
bindings rather than `self.`). Final list: **7 items** needing `pub(super)`.

| Item (def line) | Kind | Target module (step / bead) | Cross-module callers (module @ step/bead) | Flip lands at |
|---|---|---|---|---|
| `acquire_peer` (L816) | method | `connection.rs` (step 7 / **.12.4**) | `tools_call.rs`(15/.12.8), `resources_read.rs`(16/.12.8), `prompts_get.rs`(17/.12.8) | **step 7 / .12.4** |
| `UpstreamConnection::shutdown` (L693) | method | `connection.rs` (step 7 / **.12.4**) | `lifecycle.rs`(7/.12.4, sibling), `probe.rs`(13/.12.7), `ensure.rs`(12/.12.6) | **step 7 / .12.4** |
| `UpstreamRequestLog` + ctors `tool`/`resource`/`prompt`/`with_transport` (L274–321) | type + methods | `logging.rs` (step 3 / **.12.3**) | `tools_call.rs`/`call_tool`(15/.12.8), `resources_read.rs`/`read_upstream_resource`+`subject_scoped_read_resource`(16/.12.8), `prompts_get.rs`(17/.12.8) | **step 3 / .12.3** |
| `replace_catalog_tools` (L1747) | method (shared mutator) | **`ensure.rs`** (step 12 / **.12.6**) — newly assigned, see §3 | `probe.rs`/`reprobe_upstream`(13/.12.7), `ensure.rs` self(12/.12.6) | **step 12 / .12.6** |
| `reprobe_upstream` (L1348) | method | `probe.rs` (step 13 / **.12.7**) | `ensure.rs`/`reprobe_tools_for_upstream`(12/.12.6, moved earlier) | **step 13 / .12.7** |
| `ensure_probe_task` (L1226) | method | `probe.rs` (step 13 / **.12.7**) | `discover.rs`/`discover_all_inner`(11/.12.6, moved earlier) | **step 13 / .12.7** |
| `has_healthy_tools_for_upstream` (L1965) | method | `tools.rs` (step 15 / **.12.8**) | `ensure.rs`(12/.12.6, moved earlier) | **step 15 / .12.8** |

**Reading the "flip lands at" column.** The bump is keyed to the **callee's** extraction step,
regardless of whether callers move before or after:
- `acquire_peer`/`shutdown`/`UpstreamRequestLog`: callers move *after* the callee, so from the
  callee's extraction step onward the residual `pool.rs` (ancestor) calls a now-child-private
  item → flip at the callee's step (7/3).
- `reprobe_upstream`/`ensure_probe_task`/`has_healthy_tools_for_upstream`: the cross-caller
  module (`ensure.rs` or `discover.rs`) moves *first*. While the callee still sits in `pool.rs`,
  the already-moved descendant caller can see it (ancestor-private is visible to descendants), so
  it stays green — until the callee itself extracts into a sibling module, at which point the
  flip is required.

**Dropped — `cached_prompt_owner` (review false positive).** The review (line 29) claimed it is
called from `prompts_get.rs` at L3176/L3185. Verified against source: those two call sites are
both inside `find_prompt_owner` (def L3175), which the §3 layout places in **`prompts_list.rs`** —
the *same* module as `cached_prompt_owner`'s assigned home. `grep` confirms exactly two callers,
both in `find_prompt_owner`; none in the `prompts_get.rs` cluster. **Same-module → no
`pub(super)` needed.** The review mis-attributed the enclosing function.

**Execution guard (eng-review "riskiest relocation").** `acquire_peer` holds the `connections`
`RwLock` read guard while cloning the peer. When widening its visibility, keep the body
**byte-identical** — do not "tidy" it into the `pub(super) fn peer(&self)` accessor pattern, which
could change *which* lock is held and for how long (a short read-guard silently becoming a longer
hold across an `await` → contention under concurrent tool calls). Widen visibility only.

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
│   ├── ensure.rs            # seed_lazy_upstreams, ensure_tools_for_upstream*, install/reprobe_tools, lazy locks, replace_catalog_tools [SHARED pub(super) mutator]
│   ├── capability.rs        # discover_capability_counts
│   ├── probe.rs             # ensure_probe_task + reprobe_upstream
│   ├── registration.rs      # register_in_process_service_peers + _list + _list_with_connector
│   ├── tools.rs             # healthy_tools*, has_healthy_tools_for_upstream [SHARED pub(super) — called from ensure.rs], find_tool*, tool_schema, tool_exposure_rows, cached_upstream_summary, subject_scoped_tools, runtime_metadata, upstream_tool_health
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

### 3.0 Shared cross-cutting mutators — explicit homes (eng-review MEDIUM #2)

Two private methods are genuine shared mutators straddling the discovery/ensure/health cluster
and had no explicit home in the earlier table. Both are now assigned and labelled `[SHARED
pub(super) ...]` above:

| Method | Assigned home | Why here | Cross-callers (need `pub(super)`) |
|---|---|---|---|
| `replace_catalog_tools` | **`ensure.rs`** | Writes `self.catalog` after a tools probe; 3 of its 5 callers (`ensure_tools_for_upstream*`, `install_test_tools_for_upstream`) live here and it pairs with the ensure-tools install flow | `probe.rs` (`reprobe_upstream`, the other 2 callers). Flip lands at .12.6 (see §2.1). |
| `has_healthy_tools_for_upstream` | **`tools.rs`** | A tools query (reads catalog health), already co-located with the other `*_tools*` query methods | `ensure.rs` (all 5 callers). Flip lands at .12.8 (see §2.1). |

Neither relocation changes behavior; both are pure visibility widenings. They are flagged
explicitly because their cross-cluster coupling means a careless "tidy" or a missed `pub(super)`
will break a *specific* migration step (the one that extracts the callee), not the end state.

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

### 3.3 The `<500` cap is self-imposed; four file pairs are cap-driven, not domain seams

**The cap is self-chosen, not external (eng-review LOW #4).** The `<500` LOC-per-file rule is a
*self-imposed* target for this refactor (encoded in bead lab-kvji.12), not a language, lint, or
CI constraint. It is what drives the file count past conceptual necessity. A relaxed **~600 cap**
would merge three of the pairs below into ~17–18 more cohesive files:

| Cap-driven pair | Combined LOC | Under 600? |
|---|---:|:--:|
| `tools.rs` (325) + `tools_call.rs` (201) | 526 | yes → one `tools` module |
| `prompts_list.rs` (269) + `prompts_get.rs` (274) | 543 | yes → one `prompts` module |
| `connect.rs` (385) + `connect_stdio.rs` (187) | 572 | yes → one `connect` module |
| `resources_list.rs` (375) + `resources_read.rs` (255 read methods) | 719 | **no** → stays split even at 600 (genuinely forced) |

**Decision: keep 22 files — do not re-partition.** The `<500` cap is the bead constraint for
this work, so 22 is internally consistent and ships. This subsection exists only to make the
trade-off *explicit* so a reviewer knows the count was **chosen** (cap honored), not forced by a
real domain boundary.

**These four pairs split ONE responsibility across two files purely to honor the cap (eng-review
MEDIUM #3 — document, don't change).** `resources_list`/`resources_read`, `prompts_list`/
`prompts_get`, `tools`/`tools_call`, and `connect`/`connect_stdio` each cleave a *single*
capability/responsibility into two files for LOC reasons, **not** because there is a clean
responsibility boundary between them. Do **not** mistake these splits for domain seams when
navigating or extending the code: e.g. adding a tool-call concern still touches the `tools`
responsibility even though it lands in `tools_call.rs`. (`resources_read.rs` is the one case that
*would* stay separate even at a 600 cap — its two read methods are 255 LOC combined and cannot
merge with list+gateway under 500; that one split is forced and fine.) Thin files `lifecycle.rs`
(86) and `capability.rs` (109) are likewise cap-era artifacts that double as the pre-named
contingency landing zones (§6) — keep them while the 500 cap stands.

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
3. **`logging.rs`** — `UpstreamRequestLog` + log helpers + test. **`pub(super)` FLIP (§2.1):** make `UpstreamRequestLog` itself `pub(super) struct` and its ctors `tool`/`resource`/`prompt`/`with_transport` `pub(super)` — they are constructed cross-module from `tools_call.rs`/`resources_read.rs`/`prompts_get.rs` (steps 15–17). Without it those later steps fail E0624. Build.
4. **`entries.rs`** — entry constructors + `resolve_exposure_policy` + tests. Build.
5. **`validate.rs`** — `validate_upstream_config` + 8 validate_* tests. Build.
6. **`connect.rs` + `connect_stdio.rs`** — connect_* free fns + jitter/oauth helpers; stdio + in-process spawn into `connect_stdio.rs`; oauth-connect tests into `connect.rs`. Build.
7. **`connection.rs` + `lifecycle.rs`** — `UpstreamConnection` impl + `acquire_peer`; `drain_for_swap` into `lifecycle.rs`. **`pub(super)` FLIP (§2.1):** mark `acquire_peer` AND `UpstreamConnection::shutdown` `pub(super)` *now* — `acquire_peer` is called from `tools_call`/`resources_read`/`prompts_get` (steps 15–17) and `shutdown` from `lifecycle.rs` (this step, sibling), `ensure.rs` (12), `probe.rs` (13). From this step the residual `pool.rs` ancestor cannot see them → E0624 unless flipped here. **NO-TOUCH (§6):** `UpstreamConnection::Drop` body (SIGTERM→abort ordering) and `acquire_peer`'s lock-guard scope must move byte-identical — do not reorder Drop, do not route Drop or `acquire_peer` through a new `peer()` accessor. Build.
8. **`testsupport.rs`** — shared fixtures + mock servers as `pub(super)`. Build.
9. **`health.rs`** — circuit-breaker/record_*/should_reprobe*/status + tests. Build.
10. **`capability.rs`** — `discover_capability_counts`. Build.
11. **`discover.rs`** — discover_all*/routable_upstream_peers. Build.
12. **`ensure.rs`** — seed/ensure_tools*/install/reprobe_tools + **`replace_catalog_tools`** (shared mutator, §3.0) + tests. **`pub(super)` FLIP (§2.1):** mark `replace_catalog_tools` `pub(super)` here — `probe.rs` (`reprobe_upstream`, step 13) still calls it from `pool.rs` at this step, and an ancestor cannot see this now-child-private method → E0624 unless flipped now. Build.
13. **`probe.rs`** — ensure_probe_task + reprobe_upstream + tests. **`pub(super)` FLIP (§2.1):** mark `reprobe_upstream` AND `ensure_probe_task` `pub(super)` — `ensure.rs` (already moved, step 12) calls `reprobe_upstream`, and `discover.rs` (already moved, step 11) calls `ensure_probe_task`; both are now sibling-cross-module → E0624 unless flipped here. Build.
14. **`registration.rs`** — in-process registration + tests. Build.
15. **`tools.rs` + `tools_call.rs`** — query methods vs call methods + tests. **`pub(super)` FLIP (§2.1):** mark `has_healthy_tools_for_upstream` `pub(super)` — `ensure.rs` (already moved, step 12) calls it; now sibling-cross-module → E0624 unless flipped here. Build.
16. **`resources_list.rs` + `resources_read.rs`** — list/gateway vs read methods + tests. **NO-TOUCH (§6):** `subject_scoped_read_resource` must retain its subject-argument threading and `redact_resource_uri_for_logging` call after landing in `resources_read.rs`. Build.
17. **`prompts_list.rs` + `prompts_get.rs`** — list/ownership vs get methods + tests. **NO-TOUCH (§6):** `subject_scoped_*` prompt methods must retain their subject-argument threading after the move. (`cached_prompt_owner` + its only callers in `find_prompt_owner` both land in `prompts_list.rs` — same module, **no** `pub(super)` needed; see §2.1 drop note.) Build.
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
- **Private-method visibility is NOT zero-churn — see §2.1.** Fields are zero-churn; **7 private
  inherent items** (`acquire_peer`, `UpstreamConnection::shutdown`, `UpstreamRequestLog` +
  its ctors, `replace_catalog_tools`, `reprobe_upstream`, `ensure_probe_task`,
  `has_healthy_tools_for_upstream`) must be promoted to `pub(super)` **at the step that extracts
  the callee** (steps 3, 7, 12, 13, 15 — annotated inline in §4). This is load-bearing for the
  per-step green build, not just the end state: an ancestor/sibling cannot see a child-private
  method, so a deferred flip fails E0624 at the intermediate step.
- **NO-TOUCH execution guards (eng-review verify-items #5 — flag for the implementer).** Two
  functions move *byte-identical* — visibility-only changes, no reorder, no accessor extraction:
  - **(a) `UpstreamConnection::Drop` SIGTERM→abort ordering** (def ~L659). Drop fires
    process-group SIGTERM→SIGKILL (via `process_guard`) and `handle.abort()` on `_server_task`,
    *in that order*. If the move inverts the order (abort fires before SIGTERM), a process-backed
    stdio upstream **leaks a child process group** on every `drain_for_swap` — silent until PID
    exhaustion. Drop reads private fields `self.runtime`/`self._server_task` (both stay defined in
    `pool.rs`, visible to the descendant `connection.rs`), so access is preserved; do **not**
    route a Drop field through the proposed `peer()` accessor, and do **not** reorder statements.
    The existing `process_guard` test must stay green per-step.
  - **(b) `subject_scoped_*` redaction/subject threading.** `redact_resource_uri_for_logging`
    (13 callers, full-path) and the OAuth-subject argument threading in
    `subject_scoped_read_resource` / `subject_scoped_call_tool` / `subject_scoped_get_prompt`
    must survive the move unchanged. Callers use the full `pool::` path and the re-export is
    preserved (§ Public-surface preservation), so redaction stays wired — but verify each
    `subject_scoped_*` method retains its `subject` argument and the redact call after landing in
    `resources_read.rs` / `tools_call.rs` / `prompts_get.rs`. A dropped redaction silently leaks
    resource URIs to logs (no test, no user-visible signal).
- **`UpstreamConnection` field privacy.** Keep its `impl` in `pool/connection.rs` (a descendant
  of `pool`); `pub(super)` on its fields suffices. Prefer a `pub(super) fn peer(&self)` accessor
  over opening fields when a sibling module needs the peer handle — **except on the `Drop` and
  `acquire_peer` paths, which stay byte-identical per the NO-TOUCH guard above.**
- **Tightest files: `discover.rs` (412), `ensure.rs` (446).** Re-measure with `wc -l` after the
  move; contingency splits are pre-named (peel `routable_upstream_peers` to `capability.rs`;
  split `ensure.rs` on the seed/ensure_tools line) so they do not block.
- **Numbers are measured, not guessed** — but moved code grows by its `use`-block + `impl`
  wrapper. The per-step `wc -l` check (§4) is the authoritative gate; the table is the plan.
