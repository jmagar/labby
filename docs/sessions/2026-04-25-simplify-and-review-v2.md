---
date: 2026-04-25 23:57:51 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: 82478a0b
agent: Claude (Opus 4.7 1M)
session id: fca5994f-4375-4cb4-97b1-e83c3c1dc987
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/fca5994f-4375-4cb4-97b1-e83c3c1dc987.jsonl
working directory: /home/jmagar/workspace/lab
pr: #29 — fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29
---

## User Request

Run `/simplify` over the working-tree diff (then `/lavra-review` over the resulting changes), and address every actionable finding the reviewers surfaced.

## Session Overview

Three passes on the same set of files:

1. **`/simplify` initial sweep** — three review agents (reuse, quality, efficiency) ran against a 7,183-line working-tree diff. Applied the small subset of high-confidence wins (4 real bugs, 1 dead branch).
2. **Deferred refactors** — the user asked to address the 6 items I had skipped. Did 5 of 6 cleanly; the sixth (plugins.list disk-walk) was constrained by frontend coupling and got the lighter TOCTOU cleanup instead.
3. **`/lavra-review` of my simplify edits** — four review agents (Rust, TypeScript, security, simplicity) ran against just the files I touched. Three actionable P2s emerged; all three were fixed. Then one residual P3 follow-up (`oauth_required_env` awkward callsite) was inlined.

Net effect: 11 files touched in `crates/lab/` + `apps/gateway-admin/`, all review findings resolved, `cargo test --all-features` green at 2295 passed / 1 ignored.

## Sequence of Events

1. Captured the full HEAD diff (7,183 lines via `rtk proxy "git diff HEAD"`) into `/tmp/simplify-diff.patch`.
2. Dispatched three parallel review agents (reuse, quality, efficiency); collected ~80 findings.
3. User interrupted mid-edit ("did you fuck with my routes?") to confirm scope; clarified only one file had been touched at that point. User approved continuing.
4. Applied 4 surgical bug fixes from pass 1 (UTF-8 truncation panic; `json_to_toml` Null; `${nodeId}:${client}` colon-split; dead `'name'` sort branch).
5. User asked for the 6 deferred items. Created a TaskList, worked them sequentially.
6. Completed home_dir dedup, embedded asset zero-copy via `Bytes`, table-drove `run_auth_checks`, frontend memo consolidation, `build_stdio_command` extraction. The plugins.list invariant turned out to be incompatible with the frontend catalog's component expansion — kept the disk walk, removed the TOCTOU pre-check, documented the constraint.
7. Ran `/lavra-review` against just my changes via four parallel review agents. The first attempt used a non-existent agent type for Rust; retried with `systems-programming:rust-pro`.
8. Synthesized 3 P2 actionable findings and applied them: env-write order regression (BTreeMap → Vec), inlined `isClientTargetSelected`, added `node_id` char-set validation for log-injection hardening.
9. User asked whether to address the 8 P3 confirmations. Pushed back: only one (`oauth_required_env` awkward call) was actionable; the rest were "your fix is correct" confirmations. Inlined the awkward LAB_PUBLIC_URL branch.
10. All 2295 tests passed at every checkpoint.

## Key Findings

- `crates/lab/src/dispatch/doctor/service.rs:401` — `&msg[..120]` would panic on multi-byte UTF-8 boundary; fixed to `chars().take(120)`.
- `crates/lab/src/node/install.rs:838` — `json_to_toml` silently mapped `Value::Null` → `String::new()`. TOML has no null; this was a leaky abstraction. Now returns `ERR_VALIDATION`.
- `apps/gateway-admin/components/marketplace/mcp-install-modal.tsx:67-68` (pre-fix) — `${nodeId}:${client}` Set keys would have broken if any `node_id` contained a colon. Switched to typed array; bug eliminated.
- `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx:166` — `activeFilterLabels` checked `filters.sort === 'name'` but the default sort is `'updated'`. Dead branch; default sort always rendered as an active-filter pill. Switched to `DEFAULT_FILTERS.sort`.
- `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs` — `mcp_client_config` and `install_stdio` independently duplicated ~120 lines of stdio-argv build + env resolution. Extracted `build_stdio_command(pkg, server_name)` and routed `install_stdio` through `resolve_mcp_env_values`.
- `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:504` — `resolve_mcp_env_values` returning `BTreeMap` would have changed user-visible `.env` order from package declaration order to alphabetical. Returned to `Vec<(String,String)>` to preserve declaration order. Reviewer caught this.
- `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:401-410` — `parse_mcp_client_targets` accepted any non-empty `node_id`; the value flows into structured tracing. Added `[A-Za-z0-9_\-.]` char-set guard against log-injection.
- `crates/lab/src/dispatch/marketplace/backends/claude.rs:list_plugins` — disk-walk per plugin is required by the frontend catalog (`marketplace-state.ts:233` expands each plugin into one item per component); the no-walk invariant only applies to the base `build_plugin` path. Documented the constraint instead of breaking the catalog.
- `crates/lab/src/api/web.rs:118` — `Body::from(bytes.to_vec())` cloned static embedded asset bytes per request. Switched signature to `Bytes`; embedded path is now zero-copy via `Bytes::from_static`.

## Technical Decisions

- **Defer the plugins.list disk-walk fix.** A truly invariant-preserving fix (e.g. component caching keyed by marketplace+plugin) needed coordinated frontend changes and was out of scope for `/simplify`. Removed the TOCTOU `.exists()` pre-check, kept the walk, added a comment pointing to the no-walk invariant test on `build_plugin`.
- **Keep `oauth_required_env` despite only 3 callers.** Simplicity reviewer flagged it as marginal. Three callers is the minimum threshold and the helper meaningfully replaces three near-identical 12-line if/else blocks. Subsequently inlined the LAB_PUBLIC_URL branch (where the helper didn't fit) so the helper now serves only its clean callers.
- **Inline `isClientTargetSelected`** (1 caller) per the simplicity reviewer's recommendation. The `.some(...)` predicate is more readable inline than behind a named helper.
- **Don't touch the 8 P3 confirmations.** They were the reviewers saying "your fix is correct," not findings. Treating them as TODOs would have been busywork.
- **Char-set guard on `node_id` is defense-in-depth, not a CVE.** The string is logged but never reaches a path or shell. The guard closes a log-injection surface and matches the rest of the node registry's character contract.

## Files Modified

- `crates/lab/src/dispatch/doctor/service.rs` — UTF-8 panic fix
- `crates/lab/src/dispatch/doctor/system.rs` — extracted `auth_finding` and `oauth_required_env` helpers; rewrote `run_auth_checks`; restored docstring after linter merged it
- `crates/lab/src/node/install.rs` — `json_to_toml` rejects Null; `home_dir()` delegates to `crate::config::home_dir`
- `crates/lab/src/dispatch/marketplace/client.rs` — `home_dir()` delegates to `crate::config::home_dir`
- `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs` — extracted `build_stdio_command`; refactored `install_stdio` and `mcp_client_config` to share env resolution; added `node_id` char-set validation
- `crates/lab/src/dispatch/marketplace/backends/claude.rs` — removed TOCTOU `.exists()` pre-check in `list_plugins`; documented disk-walk constraint
- `crates/lab/src/tui/ecosystem.rs` — `home_dir()` delegates to `crate::config::home_dir`
- `crates/lab/src/tui/marketplace.rs` — `home_dir()` delegates to `crate::config::home_dir`
- `crates/lab/src/api/web.rs` — `web_asset_response` accepts `Bytes`; embedded path uses `Bytes::from_static`, filesystem path uses `Bytes::from(Vec<u8>)`
- `apps/gateway-admin/components/marketplace/mcp-install-modal.tsx` — `selectedClientTargets` typed array; inline `.some(...)` predicate; eliminated colon-split
- `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx` — `DEFAULT_FILTERS.sort` reference fix; consolidated four facet maps into single `useMemo`; memoized `activeLabels`

## Commands Executed

- `rtk proxy "git diff HEAD" > /tmp/simplify-diff.patch` — captured full unfiltered working-tree diff for review agents
- `rtk cargo check --all-features` — run after every Rust edit; clean each time
- `rtk cargo test --all-features --tests --no-fail-fast` — final verification: `2295 passed, 1 ignored (47 suites, 4.12s)`
- `rtk tsc --noEmit | grep -E "marketplace-list-content|mcp-install-modal"` — confirmed no TS errors in edited files (workspace has unrelated pre-existing errors elsewhere)

## Errors Encountered

- **Wrong agent type for Rust review.** Initial dispatch used `beagle-rust:rust-code-review` which doesn't exist in this install. Tool returned the available-agents list. Retried with `systems-programming:rust-pro`, which produced the desired Rust-correctness review.
- **Linter merged docstring during /lavra-review.** A linter ran between my edits to `system.rs` and merged the `run_auth_checks` doc comment into `auth_finding` (the orphan attached to whichever fn followed). Restored both comments to their correct functions.

## Behavior Changes (Before/After)

| Surface | Before | After |
|---|---|---|
| `lab doctor` truncated service-error messages | Could panic on non-ASCII byte boundary | Truncates to 120 chars cleanly via `chars().take(120)` |
| `mcp.install` Codex with `null` field | Silently wrote empty string into TOML | Returns `ERR_VALIDATION` with actionable message |
| `mcp.install` `client_targets[].node_id` containing `:` | Frontend would have miscoded the Set key, server would have parsed the bad key as separate target | Frontend uses typed object; server rejects non-`[A-Za-z0-9_\-.]` chars |
| `~/.labby/.env` write order from `mcp.install` | Reverted briefly to alphabetical (during my BTreeMap detour); fixed back to package declaration order | Package declaration order preserved (matches pre-session behavior) |
| Embedded web asset response | Cloned `&'static [u8]` to `Vec<u8>` per request | Zero-copy via `Bytes::from_static` |
| Marketplace catalog facet-map computation | 3 separate scans of `items` plus an unmemoized `activeLabels` rebuild every render | Single-pass `useMemo` over `items`; `activeLabels` memoized |
| Marketplace active-filters pill row | Always rendered the default sort as an "active filter" | Hides the default sort (correct behavior) |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `rtk cargo check --all-features` | clean | clean | ✅ |
| `rtk cargo test --all-features --tests --no-fail-fast` | all pass | 2295 passed, 1 ignored, 47 suites, 4.12s | ✅ |
| `rtk tsc --noEmit` (filtered to edited files) | clean | clean | ✅ |
| `git status` after autosave/commit | clean | `clean — nothing to commit` (changes shipped in 82478a0b) | ✅ |

## Risks and Rollback

- **`json_to_toml` Null rejection** is a behavior change for any external caller posting `mcp.install` with `null` fields to a Codex install path. Production callers (`mcp_client_config` output) never produce nulls, so the blast radius is limited to handcrafted RPC payloads. Rollback: restore the `Value::Null => Ok(toml::Value::String(String::new()))` arm.
- **`node_id` char-set guard** rejects any pre-existing `node_id` containing characters outside `[A-Za-z0-9_\-.]`. The node registry uses the same charset elsewhere, so no in-prod node IDs should break, but a misconfigured deployment with exotic hostnames as node IDs would now fail to install MCP servers via that node. Rollback: drop the new guard block in `parse_mcp_client_targets`.
- **`home_dir()` consolidation** preserves all four prior error-type/fallback variants. Reviewer confirmed branch-by-branch. Rollback: copy the four inlined bodies back if any caller starts misbehaving.

## Decisions Not Taken

- **Caching `components_from_manifest_and_layout` per (marketplace, plugin).** Would eliminate the per-plugin disk walk in `list_plugins` cleanly, but requires invalidation logic on plugin install/remove. Out of scope.
- **Frontend pagination/virtualization for the marketplace catalog.** The catalog currently renders all `filteredItems` at once; reasonable concern at scale but a separate frontend bead.
- **Restoring the `marketplace-v2-state.test.ts` harness.** The deleted test had asserted the no-walk invariant indirectly. Decided not to revive it because the assertion now lives correctly on `build_plugin`, which is the primitive that the invariant applies to.

## References

- PR #29 — https://github.com/jmagar/lab/pull/29
- Commit `910037d3` (lab-zxx5.14) introduced the `build_plugin_leaves_cache_path_and_components_none` invariant test that constrained the plugins.list fix
- `apps/gateway-admin/components/marketplace/marketplace-state.ts:233` — frontend catalog code that requires `plugin.components` in list responses

## Next Steps

**Started but not completed:** none — every task from this session is closed.

**Follow-on work not yet started:**

- Consider component caching in `list_plugins` so the per-plugin disk walk runs once per (marketplace, plugin) tuple instead of once per `plugins.list` call.
- Consider `Bytes::from_static` for any other `Body::from(vec)` hot-path callsites (the embedded UI is the only one I changed).
- The `lab-auth/src/config.rs:home_dir()` is the sixth definition; lives in a separate crate and was not consolidated. If `lab-auth` ever depends on `lab` (or vice versa via a shared util crate), consolidate then.
- Frontend marketplace catalog grows to hundreds/thousands of items: revisit virtualization/pagination.
