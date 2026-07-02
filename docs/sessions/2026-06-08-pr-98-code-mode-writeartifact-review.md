---
date: 2026-06-08 18:07:36 EST
repo: git@github.com:jmagar/lab.git
branch: claude/heuristic-roentgen-5e827a
head: c7b0f741
working directory: /home/jmagar/workspace/lab/.claude/worktrees/heuristic-roentgen-5e827a
worktree: /home/jmagar/workspace/lab/.claude/worktrees/heuristic-roentgen-5e827a
pr: "#98 Add Code Mode artifact-first writeArtifact support — https://github.com/jmagar/lab/pull/98 (MERGED via squash a4d81c7b)"
beads: No bead activity observed
---

# PR #98 review, hardening, conflict resolution, and merge

## User Request

Run `/pr-review-toolkit:review-pr 98`, then (across follow-up turns) address all findings and follow-ups, re-review, create a follow-up issue, resolve the merge conflict with `main`, merge the PR, and clean up.

## Session Overview

Reviewed PR #98 (Code Mode artifact-first `writeArtifact`), found and empirically proved a **critical path-traversal sandbox escape**, fixed it with a containment backstop and regression tests, then addressed every review/re-review follow-up (silent-failure hardening of the new retention prune, an HTTP status-mapping bug, receipt encapsulation, and doc accuracy). Filed [lab#99](https://github.com/jmagar/lab/issues/99) for the deferred integration test, resolved a docs-only merge conflict with `main` (grafting the artifact-first pattern onto main's evolved snippet), and **squash-merged the PR** as `a4d81c7b`. Finished by cleaning up the PR's local/remote branches and a stale worktree. All gates green throughout (fmt, clippy `-D warnings`, 1740 tests) and corroborated by the full GitHub Actions CI matrix on the merge commit.

## Sequence of Events

1. **Review** (`/review-pr 98`) — read the PR's source surface, then manually found that `normalize_artifact_path` ran its lexical guards *before* `\`→`/` normalization. Wrote standalone Rust harnesses that **proved** a write escaping the artifact jail on Linux (both a relative `..` climb and an absolute-via-backslash variant). Dispatched 5 specialized review agents (code, silent-failure, type-design, tests, comments); all corroborated.
2. **Fix + tests** — reordered the normalization, added the post-join containment check (`reject_existing_symlink_ancestors`), switched I/O failures to the documented `internal_error`, added ULID-gated per-run retention pruning, tightened receipt field visibility, corrected docs, and added 9 regression tests. Verified green; committed `d8654e91`, pushed.
3. **Re-review** — re-ran the agent panel; they confirmed the escape was closed and surfaced minor issues in the new code (silent prune failures, a doc timing claim, an over-stated receipt comment, and a pre-existing `path_traversal`→500 status gap). Addressed all in `c51aefba` (+2 tests), pushed.
4. **Follow-up issue** — created GitHub issue [#99](https://github.com/jmagar/lab/issues/99) for the budget/retention/E2E integration test that lib unit tests cannot reach.
5. **Merge readiness** — found the PR was `CONFLICTING` (main advanced 4 docs-only commits). Resolved both conflicts and pushed the merge commit `d4bfc378`; GitHub recomputed `MERGEABLE / CLEAN`.
6. **Merge** — recommended and executed **squash and merge**; landed on `main` as `a4d81c7b`. Full CI matrix passed on it.
7. **Cleanup** — deleted local `pr-98`, deleted remote `codex/code-mode-artifacts`, removed scratch files, and (after a safety check) removed the stale codex worktree + its local branch. Explained that the active session worktree stays until the session ends.

## Key Findings

- **Critical escape** — `crates/lab/src/dispatch/gateway/code_mode/artifacts.rs` `normalize_artifact_path` checked `is_absolute()`/`reject_path_traversal()` on the raw string, then did `trimmed.replace('\\','/')`. On Linux a backslash is an ordinary byte, so `a\..\..\etc\evil` passed as one "normal" component and became real `../` separators; `\etc\x` became absolute `/etc/x`, where `root.join("/etc/x")` discards the base. Both proven to write outside `$LABBY_HOME` with standalone harnesses.
- **No second gate** — `handle_artifact_write` (`runner_drive.rs`) applies no caller/capability/destructive gate, so the path validator was the only barrier protecting the host filesystem.
- **The fix relied on existing primitives** — `reject_existing_symlink_ancestors` / `canonicalize_and_reject_write_path` already existed in `crates/lab/src/dispatch/path_safety.rs`; the original code simply didn't call them despite `reject_path_traversal`'s own docstring mandating it.
- **Pre-existing status bug** — bare `path_traversal` was unmapped in `crates/lab/src/api/error.rs`, falling through to 500 despite `docs/dev/ERRORS.md` documenting 422.
- **Harness limit** — `run_in_runner` spawns the runner via `std::env::current_exe()`, which in lib unit tests is the test binary (no `internal code-mode-runner` subcommand), so the budget/retention E2E is only reachable from `crates/lab/tests/code_mode_runner.rs` (CLI/subprocess) — captured in #99.

## Technical Decisions

- **Normalize `\`→`/` before the guards** (rather than rejecting backslashes outright) — preserves the intended Windows-path acceptance while closing the hole; add a post-join containment check as defense-in-depth.
- **Reuse `internal_error` for I/O write failures** instead of the undocumented `artifact_write_failed` — resolves the spec-sync violation with zero new error-kind surface; containment uses the already-documented `path_traversal`/`symlink_rejected`.
- **Lazy prune on first write only** — keeps search/no-write runs from touching `$LABBY_HOME` (also makes the test suite hermetic) and decouples retention from the search path.
- **ULID-only deletion in the prune** — guarantees an operator's stray file under the store can never be collected; `git worktree remove` (no `--force`) used as the safety net when removing the stale codex worktree.
- **Squash merge** — the branch contained review-iteration commits plus a mid-flight `Merge branch 'main'`; squash gives `main` one atomic, bisectable commit and avoids a merge-of-a-merge.

## Files Changed

All changes were authored on the PR branch and landed on `main` via squash `a4d81c7b`; this worktree (`c7b0f741`) predates them, so only the session note below is dirty here.

| status | path | purpose | evidence |
|---|---|---|---|
| modified | crates/lab/src/dispatch/gateway/code_mode/artifacts.rs | reorder normalization; post-join containment; I/O→internal_error; ULID-gated retention prune; receipt field visibility; observable env/read errors | commits d8654e91, c51aefba |
| modified | crates/lab/src/dispatch/gateway/code_mode/runner_drive.rs | lazy first-write prune wiring; `ensure_call_budget_for_test` seam | d8654e91 |
| modified | crates/lab/src/dispatch/gateway/code_mode/runner.rs | `next_runner_seq` uses `saturating_add` | d8654e91 |
| modified | crates/lab/src/dispatch/gateway/code_mode/protocol.rs | doc comment on `ArtifactWrite` variant | d8654e91 |
| modified | crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs | +11 regression tests (traversal, symlink, cap, content-type, I/O, budget gate, prune, env parse) | d8654e91, c51aefba |
| modified | crates/lab/src/api/error.rs | map bare `path_traversal` → 422 | c51aefba |
| modified | docs/dev/ERRORS.md | document artifact-write kinds; note new `path_traversal` emitter | d8654e91, c51aefba |
| modified | docs/runtime/CONFIG.md | 1 MiB cap, text/plain default, retention knob, lazy-prune wording | d8654e91, c51aefba |
| modified | docs/snippets/README.md | precise artifact-first guidance | d8654e91 |
| modified | docs/snippets/axon-artifact-smoke-output.md | clarify axon's upstream shape vs the lab receipt | d8654e91 |
| modified | docs/snippets/axon-fanout.md | conflict resolution: graft artifact-first `writeArtifact` onto main's evolved snippet | d4bfc378 |
| modified | docs/superpowers/plans/2026-06-08-code-mode-artifacts.md | corrected path-safety wording, receipt path example, acceptance criteria | d8654e91 |
| created | docs/sessions/2026-06-08-pr-98-code-mode-writeartifact-review.md | this session note | this commit |

## Beads Activity

No bead activity observed. This worktree has no `.beads` directory and the PR workflow used GitHub: PR #98 and follow-up issue #99. The remaining integration-test work is tracked in GitHub issue #99 rather than a bead to avoid duplicate trackers (see Open Questions).

## Repository Maintenance

- **Plans** — `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md` are unrelated to this session and not clearly complete; left untouched. `docs/plans/complete/` not created (nothing to move). The PR's own plan lives under `docs/superpowers/plans/` and was edited as part of the PR, not moved.
- **Beads** — no `.beads` dir in this worktree; no bead actions taken (follow-up captured in GitHub #99).
- **Worktrees/branches** — deleted local `pr-98` (`was d8654e91`, content in main); deleted remote `codex/code-mode-artifacts` (merged); removed stale worktree `.worktrees/codex/code-mode-artifacts` after a clean-status check and force-deleted its local branch `codex/code-mode-artifacts` (`was 39f3415f`, content in main via squash). The active session worktree (`claude/heuristic-roentgen-5e827a`, ancestor of main, clean) was left in place because it cannot be removed while the session runs.
- **Stale docs** — the doc updates this session targeted went out with PR #98 (CONFIG/ERRORS/README/plan/axon-fanout) and are on `main`; no additional stale-doc work identified.
- **Transparency** — every cleanup action above was verified with `git worktree list`, merge-ancestry checks (`git merge-base --is-ancestor`), and porcelain status before acting.

## Tools and Skills Used

- **Shell (Bash)** — git operations, `gh` (PR/issue/checks), `cargo` (fmt/clippy/nextest/check), `rustc` for standalone exploit-proof harnesses. Issue: `mise` initially refused the untrusted `.mise.toml`, shadowing `gh`; resolved with `mise trust`.
- **File tools** — Read/Edit/Write across the code_mode module, `api/error.rs`, and five docs.
- **Subagents (Task)** — 5 pr-review-toolkit agents (code-reviewer, silent-failure-hunter, type-design-analyzer, pr-test-analyzer, comment-analyzer) for the initial review and a 4-agent re-review pass.
- **Advisor** — consulted before committing to the fix approach (hardened the prune to ULID-only) and on the retention design.
- **Skills** — `vibin:save-to-md` (this note). No MCP servers or browser tools were used.

## Commands Executed

| command | result |
|---|---|
| `mise trust …/.mise.toml` | trusted; unshadowed `gh` |
| `rustc -O pathcheck.rs && ./pathcheck` | proved backslash bypass; later proved end-to-end write outside the jail |
| `cargo nextest run --workspace --all-features` | 1740 passed, 24 skipped (final) |
| `cargo clippy --workspace --all-features -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| `gh pr merge 98 --squash --subject … --body …` | MERGED as `a4d81c7b` |
| `git worktree remove …/.worktrees/codex/code-mode-artifacts` | removed (clean) |

## Errors Encountered

- **mise untrusted config** — `.mise.toml` not trusted blocked `gh`; fixed with `mise trust`.
- **Plain `git push` would mismatch** — the session branch's upstream is `origin/main` (name mismatch under `push.default=simple`); used `git push -u origin HEAD` for the session-note push to avoid any attempt against `main`.
- **Merge conflict (expected)** — two docs conflicted with main; resolved deterministically (plan doc → ours; axon-fanout → main base + artifact-first graft).

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| `writeArtifact` path validation | backslash-encoded `..`/absolute paths escaped the jail | normalized before guards + post-join containment/symlink check; rejected with `invalid_param`/`path_traversal`/`symlink_rejected` |
| Artifact I/O failure kind | undocumented `artifact_write_failed` | documented `internal_error` |
| `path_traversal` over HTTP | 500 (unmapped) | 422 |
| Artifact store growth | unbounded | pruned to newest `LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS` (default 200), lazy on first write |
| Retention failure visibility | silent | WARN on unreadable store / interrupted enumeration / unparseable env |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| standalone exploit harness (pre-fix logic) | escape reproduces | file written outside jail | pass (confirms bug) |
| `cargo nextest run --workspace --all-features` | all pass | 1740 passed, 24 skipped | pass |
| `cargo clippy --workspace --all-features -- -D warnings` | no warnings | clean | pass |
| `cargo fmt --all -- --check` | clean | clean | pass |
| GitHub Actions on `a4d81c7b` | green | Clippy/Test/Cargo Deny/Container build/Release smoke (×2)/Format/Generated docs/Actionlint all pass | pass |
| `gh pr view 98` | merged | state MERGED | pass |

## Risks and Rollback

- **New destructive code (retention prune)** — mitigated by ULID-only deletion, best-effort (never fails a run), and `0`-disables; tested for the no-op and operator-file-safety cases. Rollback: revert `a4d81c7b` on main.
- **Shared status-mapping change** (`path_traversal`→422) affects stash/marketplace import/export too — but aligns code with the existing `ERRORS.md` contract; full CI passed.

## Decisions Not Taken

- **Rebase/merge-commit** instead of squash — rejected: the branch contained an internal merge commit, making rebase awkward and a merge-commit produce a merge-of-a-merge.
- **A distinct `artifact_write_failed` error kind** — rejected in favor of reusing documented `internal_error` to avoid expanding the stable kind vocabulary.
- **Creating a beads issue for the follow-up** — rejected to avoid duplicating GitHub issue #99 (and no `.beads` dir present here).

## References

- PR: https://github.com/jmagar/lab/pull/98 (merged squash `a4d81c7b`)
- Follow-up issue: https://github.com/jmagar/lab/issues/99
- `docs/dev/ERRORS.md`, `docs/runtime/CONFIG.md`, `crates/lab/src/dispatch/path_safety.rs`, `crates/lab/tests/code_mode_runner.rs`

## Open Questions

- **Tracker of record** — the repo shows beads (`lab-*`) yet this PR workflow used GitHub PRs/issues; this worktree has no `.beads` dir. Confirm whether follow-ups like #99 should be mirrored into beads.

## Next Steps

- **Pick up [#99](https://github.com/jmagar/lab/issues/99)** — add the child-side `artifact_write` test in `crates/lab/tests/code_mode_runner.rs` (cheap) and a CLI-driven E2E (`labby gateway code exec`) covering receipt persistence, `max_tool_calls` enforcement, and lazy retention.
- **Optional cleanup** — after this session ends, from the main checkout: `git worktree remove .claude/worktrees/heuristic-roentgen-5e827a && git branch -d claude/heuristic-roentgen-5e827a` (branch is an ancestor of main).
