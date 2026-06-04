---
date: 2026-06-04 02:55:43 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: ca834476
session id: 0ad6dc60-4bd5-4287-b8c0-ade216dccd20
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/0ad6dc60-4bd5-4287-b8c0-ade216dccd20.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab ca834476 [main]
pr: none
beads: none
---

# ACP PR Review — Security Hardening and Test Coverage

## User Request

Run the `/comprehensive-review:pr-enhance` skill scoped to ACP only, generate a full PR description for the ACP security hardening epic, then fix all identified issues.

## Session Overview

Generated a comprehensive PR description for the ACP review remediation epic (v0.12.0–v0.13.0+), audited all checklist items against the actual code, identified and fixed a missed constant-time HMAC vulnerability in `persistence.rs`, added five missing IDOR test cases for principal isolation, and verified the full ACP test suite (138/138 pass) with clippy and compile clean.

## Sequence of Events

1. **PR analysis.** Ran `git diff`, `git log`, `git show` across ACP-tagged commits to map the full scope: `dispatch.rs`, `persistence.rs`, `registry.rs`, `runtime.rs`, `params.rs`, `catalog.rs`.
2. **PR description generated.** Produced a structured PR description covering security (P0), reliability (P1/P2), new actions (`session.start_and_prompt`, `session.bulk_close`, stderr sanitization), infra, risk assessment, and a review checklist.
3. **Checklist verification.** Read every referenced file and matched each checklist item against actual code. 12 of 13 items were already correctly implemented.
4. **Security fix applied.** Found that `verify_permission_outcome_payload` in `persistence.rs` used non-constant-time string `==` for HMAC comparison. Fixed to use `mac.verify_slice(&hex::decode(expected_hex)?)` matching the pattern already used in `validate_subscribe_ticket`.
5. **Rustfmt cleanup committed.** Staged `registry.rs` and `persistence.rs` (rustfmt line-wrap + security fix) and committed.
6. **Next review phase.** Spawned an `Explore` agent to audit three classes: missing access guards, catalog/dispatch parity, and test coverage gaps.
7. **IDOR tests written.** Agent found 5 actions (`session.get`, `session.events`, `session.prompt`, `session.cancel`, `session.subscribe_ticket`) with no wrong-principal test. Wrote all 5 tests in the dispatch test block.
8. **Tests verified green.** All 5 new tests pass; full ACP suite 138/138 pass.
9. **Committed IDOR test suite.**

## Key Findings

- `persistence.rs:924–941` — `verify_permission_outcome_payload` compared HMAC hex strings with `==` (non-constant-time). The subscribe-ticket path (`validate_subscribe_ticket`) was already fixed but this codepath was missed.
- All 17 ACP catalog actions had matching dispatch arms — no parity gap.
- All session-access actions enforced principal isolation via `check_session_access` or internal `check_principal` — no missing guard.
- Five IDOR scenarios had no test: `session.get`, `session.events`, `session.prompt`, `session.cancel`, `session.subscribe_ticket` with wrong principal.
- `check_principal` correctly masks IDOR as `not_found` when principals mismatch, and returns `auth_failed` for empty principals.

## Technical Decisions

- **Constant-time verification pattern.** Used `hex::decode(expected_hex)` → `mac.verify_slice(&bytes)` rather than `subtle::ConstantTimeEq` to stay consistent with the existing pattern in `validate_subscribe_ticket`. No new dependencies needed.
- **IDOR tests at dispatch layer.** Wrote tests in `dispatch.rs` using `dispatch_with_registry` + `inject_fake_session` (the established test harness) rather than unit-testing `check_principal` directly, so tests exercise the full dispatch-to-registry path.
- **Plans left in place.** Both `fleet-ws-plan-lab-n07n.md` and `mcp-streamable-http-oauth-proxy.md` are active — no moves to `complete/`.

## Files Changed

| Status | Path | Purpose |
|---|---|---|
| modified | `crates/lab/src/dispatch/acp/persistence.rs` | Fix non-constant-time HMAC string comparison in `verify_permission_outcome_payload`; rustfmt cleanup |
| modified | `crates/lab/src/acp/registry.rs` | Rustfmt line-wrap cleanup in `.lock().expect()` chains |
| modified | `crates/lab/src/dispatch/acp/dispatch.rs` | Add 5 IDOR principal-isolation tests |

## Beads Activity

No bead activity observed. This session was a code review and hardening pass, not tracked as a discrete bead.

## Repository Maintenance

**Plans:** `docs/plans/fleet-ws-plan-lab-n07n.md` (WebSocket fleet transport, bead `lab-n07n`, status open) and `docs/plans/mcp-streamable-http-oauth-proxy.md` (streamable HTTP + OAuth + MCP proxy) are both active. Neither is complete — left in place.

**Worktrees and branches:** `git worktree list --porcelain` shows one worktree at `/home/jmagar/workspace/lab` on `main` at `ca834476`. `git branch -a` shows only `main` locally and `origin/main` remotely — they match. No stale worktrees or extra branches exist.

**Beads:** No bead state was changed. The work was a direct review/hardening pass without a dedicated issue.

**Stale docs:** No docs contradicted by this session's changes. The ACP README security section was already updated in the P0-P2 commit (`9d3729a2`).

**Dirty working tree:** 90+ files modified/deleted in the working tree are unrelated to this session (gateway, stash, upstream, MCP surface, docs, plugins). All are pre-existing unstaged changes from other in-progress work — not touched this session.

## Tools and Skills Used

- **Shell (Bash):** `git diff`, `git log`, `git show`, `git add`, `git commit`, `cargo check`, `cargo nextest run`, `cargo clippy`, `cargo fmt`. No issues.
- **File tools (Read/Edit/Write):** Read `dispatch.rs`, `persistence.rs`, `registry.rs`, `params.rs`, `runtime.rs`, `catalog.rs`, `deny.toml`. Edited `persistence.rs` and `dispatch.rs`. No issues.
- **Skills:** `/comprehensive-review:pr-enhance` — loaded and executed to drive the PR description and review workflow.
- **Subagents (Explore):** Spawned one `Explore` agent to audit three classes (access guards, catalog parity, test coverage). Returned accurate findings.
- **Background tasks:** Two `cargo nextest run` and one `cargo clippy` ran concurrently in background; all returned exit 0.

## Commands Executed

| Command | Result |
|---|---|
| `cargo check --workspace --all-features` | Exit 0, clean |
| `cargo clippy --workspace --all-features` | Exit 0, 2 pre-existing `dead_code` warnings unrelated to ACP |
| `cargo fmt --all` | Applied rustfmt to workspace (stash/dispatch.rs one-liner cleanup) |
| `cargo nextest run --workspace --all-features -E 'test(acp)'` | 133/133 pass |
| `cargo nextest run … -E 'test(session_get_rejects_wrong_principal) or …'` | 5/5 pass |
| `cargo nextest run --workspace --all-features -E 'test(acp)'` (post-test-add) | 138/138 pass |

## Behavior Changes (Before/After)

| Area | Before | After |
|---|---|---|
| `verify_permission_outcome_payload` | String `==` comparison of hex HMAC — timing oracle | `mac.verify_slice(&hex::decode(expected_hex)?)` — constant-time |
| IDOR test coverage | `session.get/events/prompt/cancel/subscribe_ticket` had no wrong-principal test | 5 tests added, all green |

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `cargo check --workspace --all-features` | exit 0 | exit 0 | pass |
| `cargo clippy --workspace --all-features` | no new errors | no new errors | pass |
| `cargo nextest run -E 'test(acp)'` (133 tests pre-add) | 133 pass | 133 pass | pass |
| 5 new IDOR tests | all pass | all pass | pass |
| `cargo nextest run -E 'test(acp)'` (138 tests post-add) | 138 pass | 138 pass | pass |

## Risks and Rollback

The HMAC fix changes verification behavior: any permission-outcome event signed by the old code whose stored hex was computed incorrectly (shouldn't happen — signing is unchanged) would now fail `hex::decode`. In practice this is zero risk because `hmac_tag` always produced valid lowercase hex. Rollback: revert commit `2c139784`.

The IDOR test commit (`ca834476`) is purely additive — no rollback concern.

## Decisions Not Taken

- **Adding `subtle::ConstantTimeEq` crate.** The `hmac` crate's `verify_slice` already provides constant-time comparison without a new dependency. Chosen over `subtle` to minimise dep surface.
- **Testing `check_principal` in isolation.** Decided to test at the dispatch layer instead, which provides more end-to-end coverage with the same setup cost.

## Open Questions

- **Deploy note:** SSE subscribe tickets issued before the `acp_hmac_key()` consolidation commit will fail validation after rollout. Browser clients reconnect automatically; long-lived non-browser SSE consumers (if any) need a re-subscribe. Scope unknown.
- **Existing orphan rows:** `PRAGMA foreign_keys = ON` was added in the P0 commit but retroactive enforcement does not clean up orphaned event rows from before. Unknown whether any exist in production.

## Next Steps

- **Answer to user question:** No open worktrees or branches. Only `main` exists locally and remotely, and a single worktree at `/home/jmagar/workspace/lab`.
- **Continue next review phase** (if desired): SSE subscribe lifecycle, HTTP surface ACP routes (`api/services/`), or the `session.events` backfill cap contract.
- **Stage and commit the large dirty working tree** when that work is ready — the gateway, stash, upstream, and MCP surface changes are all unstaged and need their own commit(s).
- **Address the two active plans** when prioritized: fleet WebSocket transport (`lab-n07n`) and streamable HTTP + OAuth proxy.
