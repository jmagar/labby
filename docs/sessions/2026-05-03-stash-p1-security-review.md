---
date: 2026-05-03 18:00:08 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 35036109
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 7f84ade5-9b5d-4843-8584-2c6601e25849
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/7f84ade5-9b5d-4843-8584-2c6601e25849.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  35036109 [bd-work/mcp-gateway-review-remediation]
pr: "#40 — Integrate service wave and CI updates — https://github.com/jmagar/lab/pull/40"
---

## User Request

Run `/lavra:lavra-review all stash related code` — an exhaustive multi-agent code review of all stash-related files introduced on the current branch, followed by `/lavra-work all the p1` to implement all P1 critical findings.

## Session Overview

Dispatched 6 specialized review agents against ~5,200 lines of new stash service code (38 files), synthesized 6 P1 + 8 P2 + 5 P3 findings into beads, began Wave 1 of the P1 fix cycle. Two of the 6 P1 fixes were partially implemented before the session ended.

## Sequence of Events

1. Invoked `/lavra:lavra-review all stash related code` — no specific bead ID, reviewing all stash files in the branch diff
2. Collected the full stash diff (5,207 insertions across 38 files, nearly all new code)
3. Recalled stash knowledge from prior sessions — found several relevant LEARNED/MUST-CHECK entries about lock patterns, digest format, path safety, and async dispatch
4. Read key stash files directly: `store.rs` (1,136 lines), `service.rs` (671 lines), `revision.rs` (456 lines), `import.rs` (613 lines), `export.rs` (619 lines), `dispatch.rs` (172 lines), `provider.rs` (71 lines), `filesystem.rs` (242 lines), `params.rs` (224 lines)
5. Dispatched 6 review agents in parallel: security auditor, architecture reviewer, Rust async reviewer, performance oracle, code simplicity reviewer, silent failure hunter
6. Synthesized all agent findings, de-duplicated, classified by severity
7. Created 6 P1 beads, 8 P2 beads, 5 P3 beads; logged LEARNED/MUST-CHECK knowledge on each P1/P2
8. Began `/lavra-work` on all 6 P1 beads — Wave 1 started
9. Implemented `lab-4sd2` (index corruption fix in `store.rs`) and partially implemented `lab-9d4b` (import source restriction in `path_safety.rs` + `import.rs`)
10. Session interrupted before completing remaining 4 P1 beads and tests for lab-9d4b

## Key Findings

**P1 — Critical blockers:**

- `crates/lab/src/dispatch/stash/store.rs:326,491` — `unwrap_or_default()` on index decode causes permanent data loss: corrupt index → next append overwrites all prior revision IDs with a 1-entry vec
- `crates/lab/src/dispatch/stash/store.rs:344-360,511-527` — Corrupt-but-present index blocks the fallback scan (`!index_path.exists()` gate), making revisions permanently invisible
- `crates/lab/src/dispatch/stash/service.rs:35-640` — Nearly every sync service function (`components_list`, `component_get`, `component_create`, `providers_list`, `target_add`, `provider_link`, `provider_push`, `provider_pull`, etc.) performs blocking `std::fs::*` I/O and/or holds `fd_lock::RwLock::write()` directly from async `dispatch_with_store` without `spawn_blocking` — only `component_deploy`, `component_import`, `component_save` are correct
- `crates/lab/src/dispatch/stash/dispatch.rs:92` — `ensure_dirs()` (7x `create_dir_all`) called on every non-help request
- `crates/lab/src/dispatch/stash/params.rs:48-62` — Import `source_path` only validates `is_absolute()`, no system-path restriction — enables arbitrary-file-read via import→export of `/etc/shadow` as `Script` kind
- `crates/lab/src/dispatch/stash/catalog.rs:295` — `target.add` has `destructive: false`, no MCP elicitation prompt — AI agent can silently register any filesystem path as deploy target; denylist misses container/k8s roots (`/app`, `/workspace`, `/data`, `/config`, `/mnt`, `/media`, `/storage`)
- `crates/lab/src/dispatch/stash/service.rs:259-271,389-393` — `canonicalize(...).unwrap_or_else(|_| normalize_path(...))` silently degrades denylist check to lexical-only on EACCES/EIO/ELOOP — fails open instead of closed
- `crates/lab/src/dispatch/stash/providers/filesystem.rs:148-150` + `service.rs:551-563` — `provider.pull_latest` calls `write_revision_meta` (which appends to index) WITHOUT holding the component advisory lock; concurrent `component.save` can race and drop revision IDs

**P2 — Important:**

- `import.rs:133-409` — No cleanup on partial workspace copy; failed import leaves partial files, next `component.save` silently snapshots them
- `catalog.rs:218` — `provider.link` not destructive + filesystem provider root unconstrained
- `export.rs:163-179,239-278` — `force=true` export merges with pre-existing stale files, not clean replace
- `revision.rs:99-211,export.rs:221-294` — Double I/O in `save_revision` (reads every file twice); export buffers entire revision in memory before writing
- `import.rs:133-210` — No `MAX_FILE_COUNT` limit — 100k zero-byte files pass all size checks
- `dispatch.rs:44,53,61` — `surface="mcp"` hardcoded for all surfaces; destructive actions not logged with intent/outcome
- `store.rs:278-282` — `delete_component_record` leaks workspace, revisions, providers — only removes the component JSON
- `api/services/stash.rs:42-44` — `is_none_or` auth gate defaults to allow when no `AuthContext` (no bearer token configured)

## Technical Decisions

- **Index corruption fix (lab-4sd2)**: Chose dual approach: (1) append path returns `decode_error` on corrupt index (never overwrites), (2) read path falls back to O(R) scan on corrupt index rather than returning empty. This is safer than pure error propagation because it recovers data automatically while preventing the catastrophic overwrite.
- **Import source restriction (lab-9d4b)**: Added `canonicalize_and_reject_system_path` to the existing `path_safety.rs` shared module. Fails closed on canonicalize failure (returns `path_traversal` error) rather than falling back to lexical check. Also added the same constant list extended with container/k8s roots.
- **Wave ordering**: lab-p760 (spawn_blocking refactor) scheduled last because it touches every service function — doing specific function fixes first prevents merge conflicts.
- **Shared `SYSTEM_PATH_DENYLIST`**: Extracted to `path_safety.rs` so both import (source) and deploy (destination) share one canonical list.

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab/src/dispatch/stash/store.rs` | lab-4sd2: Fix index corruption — replace `unwrap_or_default()` with decode_error on append, fallback scan on read; add 4 tests |
| `crates/lab/src/dispatch/path_safety.rs` | lab-9d4b: Add `SYSTEM_PATH_DENYLIST`, `reject_system_path`, `canonicalize_and_reject_system_path` |
| `crates/lab/src/dispatch/stash/import.rs` | lab-9d4b: Wire `canonicalize_and_reject_system_path` into `import_component` before spawn_blocking |

## Commands Executed

```bash
# Review: find all stash-related Rust files
find . -type f -name "*.rs" | xargs grep -l -i "stash" 2>/dev/null | grep -v target/

# Get branch diff scoped to stash files (5,207 insertions)
git diff "origin/main"...HEAD -- 'crates/lab-apis/src/stash*' 'crates/lab/src/dispatch/stash*' ...

# Recall prior stash knowledge
"$PROJECT_ROOT/.lavra/memory/recall.sh" "stash spawn_blocking index corrupt deploy canonicalize provider pull lock"

# Create P1 beads
bd create "Stash: sync I/O blocks Tokio workers..." --type bug --priority 1 --labels "review,stash,async"
# (6 P1 beads total: lab-p760, lab-4sd2, lab-9d4b, lab-gxhk, lab-n4fb, lab-qytb)

# Knowledge logging
bd comments add lab-p760 "LEARNED: stash dispatch_with_store..."
bd comments add lab-4sd2 "MUST-CHECK: Any code that reads a JSON array index with unwrap_or_default() before appending..."
```

## Behavior Changes (Before/After)

| Behavior | Before | After |
|----------|--------|-------|
| Corrupt revision index on save | `unwrap_or_default()` → silently overwrites all prior revision IDs with a 1-entry vec | Returns `decode_error`, refuses to overwrite |
| Corrupt revision index on list | Returns empty list, recovery scan blocked | Falls back to O(R) full scan, warns via tracing |
| Corrupt provider index | Same as revision — silent data loss | Same fix applied |
| Import source path `/etc/shadow` | Accepted, copied to workspace | Rejected with `path_traversal` error |
| Import source path in container `/app/config` | Accepted | Rejected (new extended denylist includes `/app`) |

## Risks and Rollback

- **lab-4sd2**: The decode-error-on-append change is a breaking behavior change for operators with manually-edited or externally-written stash stores. An operator who has a corrupt index must now remove it manually before saving new revisions. This is intentional (fail-loud rather than silent data loss) but could surprise.
- **lab-9d4b (partial)**: The `canonicalize_and_reject_system_path` call fails closed on non-existent paths — if `source` doesn't exist yet, the error says "cannot verify path is safe" rather than "not found". The symlink check runs first (`reject_symlink`) which catches most not-found cases, but the ordering means a non-existent path returns `path_traversal` before `not_found`. This is intentional (security over UX).
- **Rollback**: All changes are in `aad75295` (bundled with lab-686q Task 13). `git revert aad75295` would revert both lab-686q and the stash fixes together. Use `git checkout aad75295~ -- crates/lab/src/dispatch/stash/store.rs crates/lab/src/dispatch/path_safety.rs crates/lab/src/dispatch/stash/import.rs` to revert just the stash files.

## Decisions Not Taken

- **Positive allowlist instead of denylist for import/deploy**: Security agent recommended replacing the denylist with a configurable positive allowlist (e.g. `~/.claude`, `~/.config/labby`). Deferred — denylist is the minimal intervention that fixes the P1; allowlist is a P2 improvement filed as part of lab-gxhk scope.
- **Async trait for StashProvider**: Architecture agent flagged `Box<dyn StashProvider>` as violating the project's concrete-types preference. Not addressed — it's a P3 and changing it in the middle of P1 fixes would increase merge conflict risk.
- **Propagate decode error on read path instead of fallback scan**: Could return a hard error when index is corrupt rather than doing the recovery scan. Rejected — automatic recovery (scan + warn) is better UX and preserves data access without operator intervention.

## Open Questions

- `lab-9d4b` tests: `import_component` tests for the new system-path rejection were not written before the session ended. Need tests for: (a) `/etc/passwd` rejected, (b) `/proc/self/environ` rejected, (c) valid user path accepted, (d) container root `/app/file` rejected.
- `aad75295` commit bundling: The stash P1 fixes (lab-4sd2, lab-9d4b) were included in the `aad75295 feat(lab-686q): Task 13` commit rather than their own commits. This complicates attribution and revert. Consider splitting in a future cleanup commit.
- The `source_path` check in `import_component` runs before `spawn_blocking`. The `canonicalize_and_reject_system_path` call does filesystem I/O (canonicalize). If the path doesn't exist, the function currently returns `path_traversal` rather than `not_found`. This ordering may need adjustment once `reject_symlink` semantics are reviewed.

## Next Steps

**Unfinished from this session (started, not completed):**

- `lab-9d4b`: Add tests for `canonicalize_and_reject_system_path` in `path_safety.rs` and integration tests for import rejection in `import.rs`
- `lab-9d4b`: The import path also needs `params.rs` to surface a better error message — currently returns `path_traversal` but the user may not understand why their absolute path is rejected

**Follow-on P1 fixes not yet started:**

- `lab-n4fb` (`service.rs:259-271,389-393`): Replace `canonicalize(...).unwrap_or_else(|_| normalize_path(...))` with fail-closed variant — use the new `canonicalize_and_reject_system_path` from `path_safety.rs`
- `lab-gxhk` (`catalog.rs:295`, `service.rs:597-633`, `api/services/stash.rs:13`): Mark `target.add` as `destructive: true`, add to `STASH_WRITE_ACTIONS`, validate target path at registration time using the new `SYSTEM_PATH_DENYLIST`
- `lab-qytb` (`service.rs:551-563`, `providers/filesystem.rs:148-150`): Move `write_revision_meta` call inside the component advisory lock in `provider_pull` — refactor `pull_latest` to return files/metadata without writing to store; have `service.rs` write under lock
- `lab-p760` (`service.rs:35-640`, `dispatch.rs:82-171`): Wrap all remaining sync dispatch arms in `spawn_blocking`; move `ensure_dirs()` out of per-request path; make `with_component_lock` calls safe from async context

**Remaining P2 beads** (filed, not started): lab-k9kz, lab-thqv, lab-z2k3, lab-fwet, lab-se5t, lab-6n05, lab-3mjv

**Suggested next command**: `/lavra-work lab-n4fb lab-gxhk lab-qytb lab-p760` to finish the remaining P1 fixes, then `/lavra-work lab-k9kz lab-thqv lab-z2k3 lab-fwet lab-se5t` for P2s.
