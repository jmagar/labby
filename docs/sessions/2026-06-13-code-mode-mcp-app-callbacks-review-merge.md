---
date: 2026-06-13 00:40:17 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 09abb569
working directory: /home/jmagar/workspace/lab/.worktrees/session-log-main
worktree: /home/jmagar/workspace/lab/.worktrees/session-log-main 09abb569 [main]
pr: "#118 Fix code mode MCP App sibling callbacks https://github.com/jmagar/lab/pull/118"
---

# Code mode MCP App callback review, merge, and cleanup

## User Request

The session began from a handoff describing the `ytdl-mcp` mitigation PR and a blocked `jmagar/lab` host issue, then the user asked to create a plan, dispatch PR Review Toolkit agents to review the full Lab PR, address all findings, merge it, clean up, pull latest, and save the session to markdown.

## Session Overview

The active Lab PR for code mode MCP App callbacks was reviewed by six PR Review Toolkit agents. Their actionable findings were implemented, locally verified, pushed to PR #118, and documented in the PR body. The PR was squash-merged, the feature worktree and local feature branch were cleaned up, `main` was pulled in the `session-log-main` worktree, and this session artifact was created for the final record.

## Sequence of Events

1. Created and used the implementation plan for code mode MCP App callback behavior.
2. Published PR #118 for safe same-upstream MCP App sibling callbacks in code mode.
3. Dispatched PR Review Toolkit agents across code review, simplification, comment analysis, test analysis, silent failure hunting, and type design.
4. Addressed the material review findings in a follow-up commit and updated the PR body with the review and verification record.
5. Pushed the fix commit, enabled squash merge with branch deletion, and confirmed PR #118 merged.
6. Removed the feature worktree, deleted the local feature branch, pruned worktree metadata, and pulled latest `main` in the clean `session-log-main` worktree.
7. Ran the save-session maintenance pass, pruned the stale remote-tracking branch for the merged PR, and wrote this note.

## Key Findings

- Code mode hid raw upstream tools but still needed rendered MCP Apps to call safe sibling tools through `app.callServerTool`; otherwise UI panels rendered but their action buttons became dead ends.
- PR Review Toolkit found that callback pre-resolution trusted the cached upstream pool and could bypass gateway config eligibility; the fix moved callback candidate lookup through gateway policy in `crates/lab/src/dispatch/gateway/manager/code_mode_resolve.rs`.
- PR Review Toolkit found that protected-route `tools/list` advertised disallowed built-in services; the fix added route-scope filtering in `crates/lab/src/mcp/handlers_tools.rs`.
- A named pre-resolved handoff made callback routing easier to audit in `crates/lab/src/mcp/call_tool_upstream.rs`.
- The direct UI callback path is intentionally allowed with read scope as a render-entry callback, while hidden same-upstream sibling callbacks still require execute scope.

## Technical Decisions

- Callback lookup now uses a manager-level resolver so the same gateway configuration policy applies before a pre-resolved callback reaches upstream dispatch.
- Direct MCP-App UI tools and same-upstream sibling tools are classified separately with `CallbackToolLookup`, preserving the direct render-entry path while adding the sibling callback carve-out.
- Ambiguous same-name candidates are rejected before destructive classification, keeping duplicate upstream tool names from silently selecting an arbitrary target.
- Destructive direct UI, legacy widget, and sibling callbacks remain blocked with `confirmation_required`; operators must use the explicit execute path for destructive calls.
- Protected route discovery now filters built-ins by `McpRouteScope`, matching call-time authorization with advertised tool visibility.

## Files Changed

The implementation was squash-merged in `2d2e1534 Fix code mode MCP App sibling callbacks (#118)`. This table lists the files changed by that merge plus this generated session artifact.

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `crates/lab/src/acp/providers.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/acp/runtime.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/api/nodes/fleet.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/api/services/acp.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/cli.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/cli/doctor.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/cli/gateway.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/cli/oauth.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/cli/serve.rs` | - | Protected route and related MCP serving changes | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/cli/setup.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/config.rs` | - | Code mode and gateway config support | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/config/env_merge.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/acp/persistence.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/doctor.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/doctor/proxy.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/doctor/system.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/fs/dispatch.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs` | - | Code mode test adjustment | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/code_mode/tests_ids_schema.rs` | - | Code mode test adjustment | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs` | - | Code mode test adjustment | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/config.rs` | - | Gateway config support | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/discovery/opencode.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/dispatch.rs` | - | Related gateway dispatch changes | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/manager.rs` | - | Exposes `CallbackToolLookup` from code mode resolver | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/manager/code_mode_resolve.rs` | - | Adds policy-aware callback candidate resolution | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/manager/tests/cleanup.rs` | - | Gateway manager test cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/manager/tests/code_mode.rs` | - | Code mode manager test cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/gateway/manager/tests/lifecycle.rs` | - | Gateway manager test cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/logs/metrics/tests.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/marketplace.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/marketplace/acp_dispatch.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/marketplace/backends/codex.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/marketplace/client.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/marketplace/dispatch.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/node/send.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/setup/bootstrap.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/setup/dispatch.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/stash/store.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/http_client.rs` | - | Related upstream cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/pool/ensure.rs` | - | Related upstream cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/pool/prompts_list.rs` | - | Related upstream cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/pool/resources_read.rs` | - | Related upstream cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/pool/tools.rs` | - | Adds MCP App sibling lookup helpers and tests | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/process_guard.rs` | - | Related upstream cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/dispatch/upstream/types.rs` | - | Related upstream cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/call_tool.rs` | - | Implements and later extracts the widget callback gate | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/call_tool_codemode/tests.rs` | - | Code mode call tool test updates | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/call_tool_upstream.rs` | - | Adds `PreResolvedUpstreamTool` and subject-scoped pre-resolved routing | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/handlers_prompts.rs` | - | Related handler cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/handlers_resources.rs` | - | Related handler cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/handlers_tools.rs` | - | Filters built-ins through route scope in `tools/list` | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/handlers_tools/tests.rs` | - | Adds callback, protected-route, destructive, ambiguity, and policy regression tests | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/in_process_peer.rs` | - | Related MCP test support | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/server.rs` | - | Related MCP server support | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/mcp/services/fs.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/node/runtime.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/node/ws_client.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/oauth/local_relay.rs` | - | Related OAuth cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/oauth/upstream/encryption.rs` | - | Related OAuth cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/src/output/render.rs` | - | Related clippy/all-features cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/acp_backend_contract.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/code_mode_runner.rs` | - | Code mode runner test cleanup | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/deploy_runner.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/device_cli.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/device_runtime.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/device_scan.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/gateway_stdio_spawn.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/logs_api.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/nodes_cli.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/nodes_runtime.rs` | - | Test cleanup carried by PR #118 | `git show --name-status 2d2e1534` |
| modified | `crates/lab/tests/upstream_oauth.rs` | - | OAuth routing test cleanup | `git show --name-status 2d2e1534` |
| modified | `docs/dev/CODE_MODE.md` | - | Documents code mode MCP App callback rule | `git show --name-status 2d2e1534` |
| created | `docs/sessions/2026-06-12-code-mode-mcp-app-callbacks.md` | - | Prior implementation session note | `git show --name-status 2d2e1534` |
| created | `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md` | - | Implementation plan committed with PR #118 | `git show --name-status 2d2e1534` |
| modified | `docs/surfaces/MCP.md` | - | Documents MCP surface behavior for callback bypass | `git show --name-status 2d2e1534` |
| created | `docs/sessions/2026-06-13-code-mode-mcp-app-callbacks-review-merge.md` | - | This session artifact | `save-to-md` |

## Beads Activity

No bead activity observed for this session. The required `bd list --all --sort updated --reverse --limit 100 --json` read returned existing historical Lab beads, but none were created, edited, claimed, assigned, commented on, or closed for this PR review/merge/save flow. `.beads/interactions.jsonl` was absent or empty for the current worktree (`tail -200 .beads/interactions.jsonl` returned `none`).

## Repository Maintenance

### Plans

- Checked `docs/plans/` with `find docs/plans -maxdepth 2 -type f`; only `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` and `docs/plans/fleet-ws-plan-lab-n07n.md` were present.
- No plan under `docs/plans/` was moved: the complete plan was already under `complete/`, and `fleet-ws-plan-lab-n07n.md` was not proven completed in this session.
- Observed many `docs/superpowers/plans/` files, including `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md`; the skill maintenance rule only authorizes moving clearly completed plans under `docs/plans/`, so no superpowers plan files were moved.

### Beads

- Ran the required bead reads before changing tracker state.
- No directly relevant session bead was found, so no bead changes were made.
- No follow-up bead was created because no remaining code or review work was identified after the merge.

### Worktrees and branches

- Cleaned up the completed PR worktree before this note: removed `/home/jmagar/workspace/lab/.worktrees/code-mode-mcp-app-callbacks`, ran `git worktree prune`, and deleted local branch `codex/code-mode-mcp-app-callbacks`.
- Pulled `main` in `/home/jmagar/workspace/lab/.worktrees/session-log-main`; it fast-forwarded to `2d2e1534` and later observed `09abb569` after the subsequent `docs: add feature slice cleanup plan` commit was present on `origin/main`.
- Ran `git fetch --prune`; it removed stale remote-tracking ref `origin/codex/code-mode-mcp-app-callbacks`.
- Left all other worktrees alone because they are active named branches: `/home/jmagar/workspace/lab`, two `.claude/worktrees/*` worktrees, `readme-rewrite`, `settings-page-config-plan`, and `session-log-main`.

### Stale docs

- The implementation updated `docs/dev/CODE_MODE.md` and `docs/surfaces/MCP.md` for the behavior changed by PR #118.
- No further stale documentation was identified during the save-session pass.

### Transparency

- A `gh pr view` command run from `session-log-main` hit a `mise` trust error for the worktree-local `.mise.toml`; PR metadata was fetched successfully from the trusted primary checkout instead.
- No unrelated dirty files were staged or committed.

## Tools and Skills Used

- **Skills.** Used `superpowers:writing-plans`, `vibin:work-it`, `github:gh-pr`, `superpowers:finishing-a-development-branch`, and `vibin:save-to-md`.
- **Subagents.** Used six PR Review Toolkit agents: code reviewer, code simplifier, comment analyzer, PR test analyzer, silent failure hunter, and type design analyzer.
- **Shell commands.** Used `git`, `gh`, `cargo`, `bd`, `find`, `tail`, `sed`, `ls`, and `date` for repository state, PR state, verification, maintenance, and artifact creation.
- **File tools.** Used `apply_patch` to create this markdown session artifact.
- **GitHub CLI.** Used `gh pr view`, `gh pr checks`, `gh pr edit`, and `gh pr merge` to inspect, document, and merge PR #118.
- **External issue tracker.** Used `bd` read commands only; no bead mutations were needed.

## Commands Executed

| command | result |
|---|---|
| `cargo fmt --all --check` | Passed before merge. |
| `cargo test -p labby call_tool_ --all-features` | Passed: 30 tests. |
| `cargo test -p labby protected_ --all-features` | Passed: 39 tests. |
| `cargo test -p labby mcp_app_sibling_lookup --all-features` | Passed: 2 tests. |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | Passed. |
| `cargo nextest run --workspace --all-features` | Passed: 1950 tests run, 1950 passed, 27 skipped. |
| `git commit -m "fix: honor gateway policy for widget callbacks"` | Created review-fix commit `70fa7cae`. |
| `git push` | Pushed `codex/code-mode-mcp-app-callbacks` to origin. |
| `gh pr edit 118 --body-file -` | Updated PR #118 body with review findings and verification. |
| `gh pr merge 118 --repo jmagar/lab --squash --auto --delete-branch` | Squash-merged PR #118 and requested branch deletion. |
| `git worktree remove /home/jmagar/workspace/lab/.worktrees/code-mode-mcp-app-callbacks` | Removed the completed feature worktree. |
| `git branch -D codex/code-mode-mcp-app-callbacks` | Deleted the local feature branch at `70fa7cae`. |
| `git pull --ff-only` in `session-log-main` | Fast-forwarded `main` through the merge commit. |
| `git fetch --prune` | Pruned stale remote-tracking branch `origin/codex/code-mode-mcp-app-callbacks`. |

## Errors Encountered

- Creating the `jmagar/lab` issue from the earlier ytdl-mcp handoff was blocked because GitHub MCP scope only allowed `jmagar/ytdl-mcp`; the session proceeded by working directly in the local Lab repository instead.
- Several external PR review bots reported usage or rate limits; PR Review Toolkit subagents provided the actionable review pass instead.
- `gh pr view` from `session-log-main` failed once through the `mise` shim because `.mise.toml` in that worktree was not trusted. The workaround was to run the same GitHub read from the trusted primary checkout.
- `git checkout main` in the primary checkout failed because `main` was already checked out by `/home/jmagar/workspace/lab/.worktrees/session-log-main`; the cleanup flow pulled latest in that main worktree instead.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| MCP App callbacks in code mode | Rendered MCP Apps could expose UI while their sibling action tools were hidden, causing dead-end buttons. | Safe same-upstream non-destructive sibling callbacks can execute while raw sibling tools remain hidden from model-facing discovery. |
| Callback policy enforcement | Pre-resolved widget callbacks could trust cached upstream pool state. | Callback candidates are filtered through gateway config eligibility, including enabled state and routable priority. |
| Protected `tools/list` | Built-in services outside route scope could appear in discovery even though calls were denied. | Built-in tool discovery now respects route scope. |
| Destructive callback handling | Destructive callback cases needed explicit hardening across direct UI, legacy, and sibling paths. | Destructive callbacks return `confirmation_required` and must use explicit execute flow. |
| PR state | PR #118 was open. | PR #118 is merged into `main` with squash merge commit `2d2e1534e3f88b0cf1b5620e9f37e024b6c7181e`. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo fmt --all --check` | Rust formatting clean. | Passed. | pass |
| `cargo test -p labby call_tool_ --all-features` | Call tool tests pass. | 30 tests passed. | pass |
| `cargo test -p labby protected_ --all-features` | Protected route tests pass. | 39 tests passed. | pass |
| `cargo test -p labby mcp_app_sibling_lookup --all-features` | Sibling lookup tests pass. | 2 tests passed. | pass |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | Strict clippy clean. | Passed. | pass |
| `cargo nextest run --workspace --all-features` | Full all-features workspace tests pass. | 1950 passed, 27 skipped. | pass |
| `gh pr view 118 --json state,mergedAt,mergeCommit` | PR is merged. | `state=MERGED`, `mergedAt=2026-06-13T04:34:50Z`, merge commit `2d2e1534e3f88b0cf1b5620e9f37e024b6c7181e`. | pass |
| `git ls-remote --heads origin codex/code-mode-mcp-app-callbacks` | Deleted remote branch absent. | No output. | pass |
| `git fetch --prune` | Stale remote-tracking ref removed. | Reported deletion of `origin/codex/code-mode-mcp-app-callbacks`. | pass |

## Risks and Rollback

- The callback carve-out deliberately allows safe same-upstream callbacks from rendered MCP Apps; regression risk is concentrated in gateway policy and route-scope enforcement. Roll back by reverting squash merge `2d2e1534e3f88b0cf1b5620e9f37e024b6c7181e`.
- Some non-required CI jobs were still visible as in-progress in the PR rollup immediately after GitHub reported the PR merged. Local all-features verification and required merge gates passed before merge.
- Remaining active worktrees were not cleaned because ownership or active PR state was not proven during this session.

## Decisions Not Taken

- Did not attempt to create the `jmagar/lab` issue via the GitHub MCP after scope denial; the ready-to-paste issue text from the handoff remains the fallback.
- Did not move `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md` to another folder because the maintenance rule only authorizes moving completed plans under `docs/plans/`.
- Did not delete active branches or worktrees beyond the completed PR branch because several are tied to active named branches and were not proven obsolete.
- Did not run `mise trust` for the `session-log-main` worktree; avoided changing trust configuration just to fetch PR metadata.

## References

- PR #118: https://github.com/jmagar/lab/pull/118
- Merge commit: `2d2e1534e3f88b0cf1b5620e9f37e024b6c7181e`
- Review-fix commit before squash: `70fa7cae fix: honor gateway policy for widget callbacks`
- Prior implementation session note: `docs/sessions/2026-06-12-code-mode-mcp-app-callbacks.md`
- Implementation plan: `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md`

## Open Questions

- The `jmagar/lab` host issue from the initial ytdl-mcp handoff was not filed because repository scope was unavailable in the earlier session.
- Remaining worktrees and branches may have their own cleanup opportunities, but they were not proven stale in this save-session pass.

## Next Steps

- File the Lab host issue once `jmagar/lab` is available in the relevant GitHub MCP scope, or paste the prepared issue text manually.
- Continue any separate active branch work from the remaining worktrees; no action is needed for PR #118.
- If desired, separately audit the remaining active worktrees for stale branches, but treat that as a distinct cleanup task with per-branch evidence.
