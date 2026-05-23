---
date: 2026-05-22 20:06:02 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/scout-security-fixes
head: 217e89ce
session id: ef305b13-fb4c-4ebc-900a-15210ef44f95
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/ef305b13-fb4c-4ebc-900a-15210ef44f95.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  217e89ce [bd-work/scout-security-fixes]
---

## User Request

Run a comprehensive code review of the gateway tool-search subsystem (`scout` action, post v0.17.0), then research and begin implementing the critical findings.

## Session Overview

Long multi-phase session covering: (1) full 5-phase comprehensive code review of the gateway scout/semantic search pipeline, (2) lavra-research gathering evidence for the five critical issues, (3) lavra-eng-review revealing additional gaps not in the original plan, (4) lavra-work-multi initiating parallel implementation — the scope check (`lab-mqd6f.1`) completed successfully in a worktree, the priority-bypass fix (`lab-mqd6f.2`) was interrupted by the user.

## Sequence of Events

1. Ran `/comprehensive-review:full-review` scoped to the gateway tool search — produced 5 phase reports in `.full-review/` (archived prior May-14 review to `.full-review-archive-2026-05-14/`)
2. Review found 81 findings (5 critical, 22 high, 25 medium, 29 low) — most notable: missing scout scope check, priority=0 bypass via RRF semantic search, entire semantic pipeline unverified in CI, 4738-line `manager.rs` violating project layout rules
3. Created epic `lab-mqd6f` with 5 child beads tracking the critical issues; ran `/lavra:lavra-research lab-mqd6f` to gather evidence via 4 parallel agents
4. Research resolved key implementation decisions: use `tool_search_scope_allowed` (not reusing invoke's function), `rrf_fuse` signature change with `upstream_priority: &HashMap<String,f32>`, inline `let-else` instead of new `resolved_semantic_urls()` method, Option B (document) for dispatch exception
5. Ran `/lavra:lavra-eng-review lab-mqd6f` — 4 parallel agents found 3 additional critical gaps not in plan: invoke path has no priority check (shipping blocker), triple `config.read().await` in `search_tools`, Qdrant/TEI clients have no auth headers
6. Applied all 10 engineering review recommendations to child bead descriptions; created `lab-9ycyb` (add auth to Qdrant/TEI clients)
7. Ran `/lavra:lavra-work lab-mqd6f` → routed to MULTI path → set up file-conflict deps, wave plan, created branch `bd-work/scout-security-fixes`
8. Wave 1 launched two parallel agents: `.1` (scope check) and `.2` (priority bypass) in isolated worktrees
9. **lab-mqd6f.1 completed successfully** — 135 lines added to `server.rs` (scope gate + 3 regression tests + `include_schema` suppression)
10. **lab-mqd6f.2 was rejected** by user mid-execution — changes not applied
11. User asked "whats the deal" — status explained; session saved

## Key Findings

- **SEC-H1** (`server.rs:1215`): `scout` MCP tool had zero scope/auth code while `invoke` at line 1356 calls `tool_execute_scope_allowed()` — any OAuth session could enumerate full tool catalog
- **SEC-M1** (`semantic.rs:531-563`): `rrf_fuse` built `priority_by_upstream` from lexical hits only; a priority=0 upstream has no lexical hits, so `unwrap_or(1.0)` defaulted it to reachable — the safety comment at line 528-533 was exactly backwards
- **SEC-M1 + BLOCKER**: Invoke path also has no priority check — `find_tool_candidates` in `pool.rs:1648-1663` has no priority gate; `UpstreamTool` type has no priority field at all
- **CI-C1**: `QdrantClient` and `TeiClient` have no auth headers — any process with the Qdrant URL can poison the semantic index
- **ARCH**: `search_tools` takes 3 separate `config.read().await` — the plan to add a 4th for the priority map was flagged; must consolidate to 1
- **SIMPLICITY**: `resolved_semantic_urls()` new method is over-engineering — use inline `let-else` at each of the 3 call sites (pattern already used 10+ times in `manager.rs`)

## Technical Decisions

- **`tool_search_scope_allowed` accepts `lab:read | lab | lab:admin`** (not reusing `tool_execute_scope_allowed` which excludes `lab:read`). The static bearer path in `router.rs:828` grants `["lab:read", "lab:admin"]` but NOT bare `"lab"`, so `lab:read` is a real production scope.
- **`is_none_or` for stdio transport** preserved — `mcp/CLAUDE.md` explicitly documents stdio as trusted-by-design with no per-request AuthContext.
- **`include_schema` gate**: when caller has only `lab:read` (no `lab`/`lab:admin`), `include_schema` is forced to `false` — prevents schema disclosure for admin-only built-in tools.
- **Option B (document) for CA1**: scout/invoke are inline `Tool::new()` registrations in `server.rs:1079-1116` — not `mcp/services/` adapters. The rejection guard at `dispatch.rs:893-903` was added intentionally after this decision. Migrating to shared dispatch requires 5+ new files with no second surface consumer.
- **lab-mqd6f.3 blocked**: wiremock tests must wait for `lab-9ycyb` (Qdrant/TEI auth headers) — writing tests first would lock in the no-auth behavior.

## Files Changed

| File | Status | Purpose |
|------|--------|---------|
| `crates/lab/src/mcp/server.rs` | Modified (uncommitted) | Added `tool_search_scope_allowed`, `tool_search_schema_visible`, scope gate in scout branch, `include_schema` suppression for `lab:read`, 3 regression tests |
| `.full-review/00-scope.md` through `05-final-report.md` | Created | Comprehensive review output (5-phase, 81 findings) |
| `.full-review-archive-2026-05-14/` | Created | Archived prior review session |

## Tools and Skills Used

- `/comprehensive-review:full-review` — ran 8 parallel sub-agents across 5 phases; produced `.full-review/` reports
- `/lavra:lavra-research lab-mqd6f` — 4 parallel research agents (architecture-strategist, security-sentinel, best-practices-researcher, pattern-recognition-specialist)
- `/lavra:lavra-eng-review lab-mqd6f` — 4 parallel review agents; found 3 additional critical gaps
- `/lavra:lavra-work lab-mqd6f` → `/lavra:lavra-work-multi` — set up wave plan, created branch, launched 2 parallel implementation agents in isolated worktrees
- `bd` (beads) — created epic `lab-mqd6f` with 5 children, set file-conflict deps (`.4` after `.1`, `.5` after `.2`), created `lab-9ycyb`, registered swarm `lab-bqy36`
- **lab-mqd6f.1 agent** (`isolation: worktree`) — completed; worktree at `.claude/worktrees/agent-a9fd9eb30cf205a78`
- **lab-mqd6f.2 agent** (`isolation: worktree`) — rejected by user mid-execution; no changes applied

**Issues encountered:**
- lab-mqd6f.2 agent tool use rejected by user — changes were not applied; worktree left at `.claude/worktrees/agent-a4f481395a8d89d63` but empty/uncommitted

## Commands Executed

```bash
# Branch setup
git pull origin main && git checkout -b bd-work/scout-security-fixes

# Conflict detection (forced sequential deps)
bd dep add lab-mqd6f.4 lab-mqd6f.1
bd dep add lab-mqd6f.5 lab-mqd6f.2

# Build verification (by lab-mqd6f.1 agent in worktree)
cargo check --all-features -p lab   # → 0 errors, clean
cargo test --all-features -p lab -- tool_search_scope  # → all pass
```

## Behavior Changes (Before/After)

| Surface | Before | After |
|---------|--------|-------|
| `scout` MCP tool | Any authenticated session could call `scout` and enumerate full upstream tool catalog | Requires `lab:read`, `lab`, or `lab:admin` scope; forbidden envelope returned otherwise |
| `scout include_schema=true` | `lab:read` callers received full `input_schema` for admin-only tools | `lab:read`-only callers receive `include_schema=false` (names/descriptions only) |
| stdio transport | Unaffected (no AuthContext) | Unaffected — `is_none_or` preserves trusted stdio behavior |

(Changes are in an uncommitted worktree at `server.rs` — not yet on the branch HEAD.)

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --all-features -p lab` | 0 errors | 0 errors | ✅ |
| `tool_search_scope_allowed_permits_all_expected_scopes` | all assertions pass | pass | ✅ |
| `scout_allows_lab_read_but_invoke_requires_lab` | lab:read passes scout, fails invoke | pass | ✅ |
| `scout_include_schema_suppressed_for_read_only_callers` | lab:read returns false for schema visibility | pass | ✅ |

## Risks and Rollback

- The scope change to `scout` is additive — existing `lab`/`lab:admin` callers are unaffected. Only zero-scope or `lab:read`-only OAuth tokens are newly denied.
- The worktree changes are not yet merged to `bd-work/scout-security-fixes`. They exist at `.claude/worktrees/agent-a9fd9eb30cf205a78/crates/lab/src/mcp/server.rs`.
- Rollback: delete branch `bd-work/scout-security-fixes`; worktrees auto-clean.

## Decisions Not Taken

- **Option A for CA1** (migrate scout/invoke to shared dispatch): Rejected — incompatible parameter shapes, rejection guard was intentional, 5+ new files for one MCP consumer.
- **`resolved_semantic_urls()` new method**: Rejected — over-engineering; inline `let-else` is simpler and already the established pattern.
- **Reusing `tool_execute_scope_allowed` for scout**: Rejected — it excludes `lab:read`, which is a real production scope needed for discovery.
- **Deferring lab-mqd6f.3 wiremock tests**: Not deferred — kept P0, but blocked on `lab-9ycyb` (auth headers) so tests can assert auth header presence.

## Open Questions

- Should `lab-mqd6f.2` be retried without worktree isolation so each step can be reviewed interactively?
- The worktree changes for `.1` need to be merged into `bd-work/scout-security-fixes` before Wave 2 (`.4` doc + `.5` panics) can run.
- `lab-9ycyb` (Qdrant/TEI auth) is not yet implemented — `lab-mqd6f.3` (wiremock tests) remains blocked.

## Next Steps

**Unfinished (started this session, not completed):**
- Apply lab-mqd6f.1 worktree changes to `bd-work/scout-security-fixes` branch (merge or cherry-pick from `.claude/worktrees/agent-a9fd9eb30cf205a78`)
- Implement lab-mqd6f.2 (priority=0 suppression bypass + invoke path priority check + config lock consolidation) — was rejected; re-run without worktree isolation

**Follow-on (not yet started):**
- lab-mqd6f.4: Add fourth exception category to `mcp/CLAUDE.md` + 2-line comment at `server.rs:~1079` (bundle with .1 commit)
- lab-mqd6f.5: Remove `.expect()` panics — fix all 3 call sites with inline `let-else` (after .2 lands)
- lab-9ycyb: Add auth headers to `QdrantClient` and `TeiClient` in `lab-apis`
- lab-mqd6f.3: Write wiremock tests for Qdrant/TEI clients (after lab-9ycyb)
- Create PR from `bd-work/scout-security-fixes` to `main`
