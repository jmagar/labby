---
date: 2026-06-19 16:17:45 EST
repo: git@github.com:jmagar/lab.git
branch: main (session work done on claude/agitated-rubin-328e01, merged via PR #141)
head: 8f822b24 (main, merge commit) / df39567a (feature-branch tip, now detached worktree)
working directory: /home/jmagar/workspace/lab/.claude/worktrees/agitated-rubin-328e01
worktree: /home/jmagar/workspace/lab/.claude/worktrees/agitated-rubin-328e01 (detached HEAD after merge)
pr: "#141 fix(code-mode): review follow-ups + deadlock-safe runner writebacks — https://github.com/jmagar/lab/pull/141 (MERGED)"
beads: lab-6w2hz (closed), lab-b4l59 (closed), lab-zhnmr (open follow-up)
---

# Code Mode review, fixes, and merge

## User Request

"Conduct a thorough review of our new code mode implementation." This expanded over the session into: address the findings, fix the genuine ones, run the pr-review-toolkit review and address its issues, then merge PR #141 into `main` once CI was green and clean up.

## Session Overview

Reviewed the new Code Mode + snippets subsystem (~13k LOC) with parallel specialized agents, separated real bugs from by-design behavior, fixed the genuine issues, opened PR #141, then ran a second comprehensive pr-review-toolkit pass and addressed every actionable item (2 regression tests, a timeout-trace fix, doc accuracy). After full CI passed, merged PR #141 into `main` and cleaned up the branch/worktree plus a stale merged worktree.

## Sequence of Events

1. Initial review: four parallel agents (sandbox containment, snippets, runner/pool correctness, MCP surface/auth), then independent verification of the high-impact findings.
2. Reported findings; user pushed back on the `snippets.exec` finding ("shouldn't an admin be able to do that?") — conceded it is by-design (admin is root-equivalent).
3. Addressed the LOW findings + doc drift; began a marker-based fix for the error-kind "forgery" (#3), then reverted it on discovering an explicit test (`#2b`) proving the behavior is intentional.
4. Fixed the un-deadlined runner stdin writeback (two-pipe deadlock, #2).
5. Committed two batches, pushed branch, opened PR #141.
6. User asked if it was already merged — verified it was not; left PR open (closing would discard the work).
7. Ran the repo-status skill; removed a stale merged worktree (`codex/cloudflare-codemode-parity`); reported full repo status.
8. Ran pr-review-toolkit review (code, silent-failure, comments, tests) on PR #141; addressed all actionable items (2 tests + timeout trace + doc accuracy).
9. CI went fully green; merged PR #141 into `main`, deleted remote+local feature branch, fast-forwarded local `main`, created a follow-up bead.

## Key Findings

- The Code Mode sandbox is genuinely solid: no JS escape, no cross-caller state leak; `env_clear`, process-group reaping, fresh-runtime-per-execution, timeout→evict all hold on every spawn path.
- Real robustness bug (#2): `write_runner_input` (`runner_io.rs:21-28`) did `write_all`+`flush` with no timeout; the writebacks at `runner_drive.rs` tool-result/snippet paths were the one in-loop `await` not covered by the 30s deadline — a child flooding stdout while the parent writes a large `ToolResult` could two-pipe-deadlock and leak the pool slot.
- The widget-callback scope gate (`call_tool.rs`) was keyed to `hidden_sibling`; the legacy `LAB_CODE_MODE_WIDGET_CALLBACKS` path set it `false` while surfacing un-advertised tools — a scope-check bypass. Fixed by renaming to `requires_scope_check` and setting the legacy path `true`.
- Two findings were flagged but are by-design with no cross-trust impact: `snippets.exec` route/destructive surface (admin = root-equivalent per the trust model) and error-kind "forgery" (a caller setting its own execution's error kind, asserted intentional by `code_mode_runner.rs` test `#2b`).
- pr-review-toolkit confirmed the fixes are correct (no bugs); actionable items were coverage + doc accuracy, including a stale `env_clear()` citation (`runner_drive.rs:163` → `pool/runner_handle.rs`).

## Technical Decisions

- Reverted the #3 marker-based "forgery" fix rather than break a tested intentional contract; documented the rationale in `classify_rejection` instead.
- Fixed #2 with a `write_runner_input_by_deadline` helper wrapping the write in `timeout_at(deadline, …)` + child termination on expiry; later threaded `&state.calls` so the timeout error carries the partial call trace like every other timeout path.
- Left the snippet route/destructive consistency wart alone (admin already root-equivalent); did not weaken or special-case it.
- Did not add the write-deadline integration test (needs a flaky stalled-child fixture); filed as follow-up bead `lab-zhnmr` instead.

## Files Changed

All landed on `main` via PR #141 (commits `37b15c0c`, `b0e64661`, `df39567a`).

| status | path | purpose | evidence |
|---|---|---|---|
| modified | crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs | deadline-bound writeback helper + call-trace threading | full suite green |
| modified | crates/lab/src/dispatch/gateway/code_mode/runner_io.rs | drop dead `code_mode_fuel_exhausted` passthrough; fix Windows guard comment | clippy clean |
| modified | crates/lab/src/mcp/call_tool.rs | widget scope gate `requires_scope_check`; legacy path requires lab/lab:admin | new test |
| modified | crates/lab/src/dispatch/gateway/code_mode/runner.rs | `reset_execution_jail` chdir-back-to-base on failure; docstring fix | full suite green |
| modified | crates/lab/src/dispatch/snippets/store.rs | `atomic_write_snippet` `force` param; under-lock no-overwrite guard + test | new test |
| modified | crates/lab/src/mcp/handlers_tools/tests.rs | regression test for legacy widget-callback scope denial | 306 tests pass |
| modified | crates/lab/src/dispatch/gateway/code_mode/pool/runner_handle.rs | document `MAX_LINE_BYTES` pool multiplier | n/a (comment) |
| modified | crates/lab/src/dispatch/gateway/code_mode/CLAUDE.md | artifact-containment wording; wire-protocol shapes; env_clear citation | comment review |
| modified | crates/lab/src/dispatch/gateway/CLAUDE.md | env_clear citation → pool/runner_handle.rs | comment review |
| modified | docs/dev/CODE_MODE.md | promotion plaintext/secret persistence caveat | n/a (doc) |

## Beads Activity

| id | title | action(s) | status | why |
|---|---|---|---|---|
| lab-6w2hz | Code Mode review follow-ups (fuel-kind, widget gate, jail/TOCTOU, doc drift) | created, noted, closed | closed | tracked the LOW-finding fix batch |
| lab-b4l59 | Code Mode: deadline-bound parent→child stdin writebacks | created, noted, closed | closed | tracked the two-pipe deadlock fix |
| lab-zhnmr | Code Mode write-deadline expiry needs a deterministic test | created | open | follow-up for the deferred (flaky) write-deadline integration test |

One earlier `bd create` for the follow-up did not persist (no ID returned, absent from the open list); re-created successfully as `lab-zhnmr`.

## Repository Maintenance

- Plans: `docs/plans/fleet-ws-plan-lab-n07n.md` is active and unrelated to this session — left in place. No completed plans to move.
- Beads: see Beads Activity. Two closed (work verified), one follow-up created for known remaining work.
- Worktrees/branches: removed `codex/cloudflare-codemode-parity` (worktree + local branch) — proven merged into `main` (`merge-base --is-ancestor` = yes, 0 unique commits, clean, no remote ref). After merge, deleted remote + local `claude/agitated-rubin-328e01`. Left `marketplace-no-mcp` (protected long-lived ref) and the two other-session worktrees `claude/dreamy-mclean-661c82` (dirty — uncommitted CLAUDE.md edits) and `claude/optimistic-fermi-dd2d50` (live harness session) untouched.
- Stale docs: fixed the `env_clear()` citation and other doc drift as part of the PR; no remaining known stale Code Mode docs.
- Transparency: this session's worktree is now on a detached HEAD (its branch was merged + deleted); it was not removed because doing so from inside would kill the session CWD — the harness cleans up session worktrees at exit.

## Tools and Skills Used

- Shell/git/gh: branch/worktree inventory, merge-ancestry checks, commits, push, `gh pr create/checks/merge`, remote-branch deletion via `gh api`.
- File tools: Read/Edit/Write across the Code Mode tree and docs.
- Subagents: 4 general-purpose review agents (initial review) + 4 pr-review-toolkit agents (code-reviewer, silent-failure-hunter, comment-analyzer, pr-test-analyzer).
- Skills: `pr-review-toolkit:review-pr`, `vibin:repo-status` (collector script failed on CRLF line endings — gathered evidence via direct git/gh instead), `vibin:save-to-md`.
- Beads (`bd`): issue tracking; one create silently no-op'd and was retried.

## Commands Executed

| command | result |
|---|---|
| cargo nextest run --workspace --all-features | 2174 passed, 0 failed, 14 skipped |
| cargo clippy --all-features --tests / cargo fmt --check | clean |
| gh pr checks 141 --watch | all pass (Windows self-hosted, feature slices, container/release smoke, gitleaks/GitGuardian) |
| gh pr merge 141 --merge --delete-branch | merged on GitHub (local checkout step errored on `main` worktree; merge succeeded) |
| git -C ~/workspace/lab merge --ff-only origin/main | main → 8f822b24 |

## Errors Encountered

- `gh pr merge 141 --merge --delete-branch` printed `fatal: 'main' is already used by worktree` — gh's local post-merge checkout step, not the merge; GitHub reported the PR MERGED. Resolved by finishing cleanup manually (remote branch delete via `gh api`, local `main` fast-forward, branch delete).
- `vibin:repo-status` collector script `repo_context.sh` failed with `$'\r'` syntax errors (CRLF line endings). Worked around by running the documented git/gh evidence commands directly.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Runner writeback on a stalled child | could deadlock past the 30s backstop and leak the pool slot | bounded by the deadline; child killed + slot evicted, returns `timeout` |
| Legacy widget-callback (LAB_CODE_MODE_WIDGET_CALLBACKS) | hidden tool callable without scope check | requires `lab`/`lab:admin` scope |
| Upstream returning `code_mode_fuel_exhausted` | passed through verbatim (→ HTTP 408) | normalized to `internal_error` |
| Concurrent non-force snippet create | TOCTOU could both pass and overwrite | under-lock guard returns `Conflict` |
| Runner cwd after a jail-reset failure | left in a just-removed directory | falls back to the stable spawn base |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| cargo nextest run --workspace --all-features | all pass | 2174 passed / 0 failed | pass |
| new test: call_tool_requires_execute_scope_for_legacy_widget_callbacks | forbidden + scopes | pass | pass |
| new test: atomic_write_snippet_rejects_overwrite_without_force_under_lock | Conflict | pass | pass |
| gh pr checks 141 | all green | all pass (only cubic skipped) | pass |
| gh pr view 141 state | MERGED | MERGED (2026-06-19T17:00:06Z) | pass |

## Risks and Rollback

- Low risk: changes are additive hardening of a well-tested subsystem; full CI (incl. Windows self-hosted) passed. Rollback path: revert merge commit `8f822b24` on `main`.

## Decisions Not Taken

- Did not implement the error-kind marker fix (#3) — would break the intentional, tested `#2b` behavior.
- Did not change the `snippets.exec` surface — admin is already root-equivalent; no boundary is being violated.
- Did not add the flaky write-deadline integration test — deferred to `lab-zhnmr`.
- Did not remove the two other-session worktrees (one dirty, one live).

## References

- PR: https://github.com/jmagar/lab/pull/141
- docs/dev/CODE_MODE.md, crates/lab/src/dispatch/gateway/code_mode/CLAUDE.md (Code Mode source of truth)

## Next Steps

- Optional: implement the deterministic write-deadline expiry test (`lab-zhnmr`) when a non-flaky stalled-child fixture is feasible.
- Harness will clean up this detached session worktree at exit; to remove now: `git -C ~/workspace/lab worktree remove --force .claude/worktrees/agitated-rubin-328e01`.
