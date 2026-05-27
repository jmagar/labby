---
date: 2026-05-26 23:37:39 EST
repo: git@github.com:jmagar/lab.git
branch: lazy-upstream-runtime
head: c40031b2
plan: docs/superpowers/plans/2026-05-26-gateway-lazy-upstream-runtime.md
working directory: /home/jmagar/workspace/lab/.worktrees/lazy-upstream-runtime
worktree: /home/jmagar/workspace/lab/.worktrees/lazy-upstream-runtime
pr: "#77 Lazy load gateway upstream runtimes (https://github.com/jmagar/lab/pull/77)"
---

# Gateway lazy upstream runtime session

## User Request

Implement the plan to stop `labby serve` from starting every configured upstream MCP server for every connected agent, while preserving access through gateway `tool_search`, `scout`, `code_search`, and execution paths. Keep the work in the shared dispatch layer.

## Session Overview

Implemented lazy upstream runtime startup in PR #77. Startup now seeds upstream metadata without connecting every upstream process; shared gateway dispatch warms the required upstream on demand for search and execution paths, with owner and OAuth subject attribution preserved.

Review feedback was addressed through multiple commits, including concurrency hardening, cold-search failure surfacing, resource proxy registration preservation, discovery timeouts, and raw qualified upstream tool resolution.

## Sequence of Events

1. Created and worked in the `lazy-upstream-runtime` worktree and branch.
2. Added the lazy upstream runtime plan under `docs/superpowers/plans/`.
3. Changed gateway and upstream pool dispatch so startup seeds lazy catalog entries and specific search/execute paths warm configured upstreams on demand.
4. Ran review waves and addressed findings from Codex, CodeRabbit, and internal analyzer passes.
5. Added regression tests for reload seeding, read-scope cold-start prevention, singleflight lazy connects, lazy connect failure recording, resource upstream preservation, and raw qualified tool resolution.
6. Resolved all PR review threads; the read-only `code_search` cold-start suggestion was resolved with an explicit security rationale.

## Key Findings

- Lazy upstream connection must live in shared dispatch, not the MCP adapter, so CLI/MCP/API behavior stays consistent.
- OAuth-backed upstream discovery needs the request OAuth subject or the shared gateway subject; otherwise cold search cannot index OAuth upstream tools.
- `code_search` is read-accessible, but cold-starting stdio upstreams is a process-spawn side effect, so cold starts remain execute-scoped.
- Calling `seed_lazy_upstreams` with a one-element slice could overwrite `resource_upstreams`; a focused helper now preserves existing resource proxy upstreams.
- Raw MCP proxy resolution needed to honor `upstream::tool` names and preserve manager errors instead of dropping them with `.ok()`.

## Technical Decisions

- `GatewayManager` owns lazy runtime warm-up decisions for search and exact tool execution.
- `UpstreamPool` owns low-level lazy catalog registration, connect singleflight behavior, health/circuit updates, and bounded discovery timeouts.
- Read-only `code_search` can inspect cached already-connected upstream tools but cannot cold-start upstream processes.
- Lazy connect failures are surfaced as structured gateway errors when synchronous cold search has no usable index.
- Raw upstream tool resolution now searches only the requested upstream when the caller provides a qualified name.

## Files Changed

| status | path | previous path | purpose | evidence |
| --- | --- | --- | --- | --- |
| modified | `crates/lab/src/cli/serve.rs` | - | Wire serve startup to lazy gateway runtime behavior. | `git diff --name-status origin/main...HEAD` |
| modified | `crates/lab/src/dispatch/gateway/code_mode.rs` | - | Add code-mode lazy search/execute behavior and read-scope cold-start guard. | PR #77 commits `32708ceb..c40031b2` |
| modified | `crates/lab/src/dispatch/gateway/manager.rs` | - | Add lazy pool bootstrap, search/runtime warm-up, owner/OAuth threading, and raw tool resolution fixes. | PR #77 commits `32708ceb..c40031b2` |
| modified | `crates/lab/src/dispatch/upstream/pool.rs` | - | Add lazy upstream catalog seeding, singleflight connects, bounded lazy connect timeout, resource preservation, and tests. | PR #77 commits `32708ceb..c40031b2` |
| modified | `crates/lab/src/mcp/server.rs` | - | Keep MCP as a thin adapter while passing owner/OAuth data and preserving raw-resolution errors. | PR #77 commits `32708ceb..c40031b2` |
| modified | `docs/services/GATEWAY.md` | - | Document lazy gateway search and execution behavior. | `git diff --name-status origin/main...HEAD` |
| modified | `docs/services/UPSTREAM.md` | - | Document upstream lazy runtime behavior. | `git diff --name-status origin/main...HEAD` |
| created | `docs/superpowers/plans/2026-05-26-gateway-lazy-upstream-runtime.md` | - | Implementation plan and TDD execution record. | `git diff --name-status origin/main...HEAD` |
| created | `docs/sessions/2026-05-26-gateway-lazy-upstream-runtime.md` | - | Session log generated during closeout. | This save-to-md step |

## Beads Activity

No bead activity observed for this session. `bd list --all --sort updated --reverse --limit 20 --json` returned historical closed items unrelated to PR #77; no relevant bead was created, updated, claimed, or closed during this closeout.

## Repository Maintenance

Plans: checked `docs/plans` and found `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md`; neither was clearly part of this completed PR, so no plan files were moved.

Beads: checked recent beads with `bd list`; no directly relevant bead was observed, so no tracker changes were made.

Worktrees and branches: checked `git worktree list --porcelain`, local branches, and remote branches. Active worktrees are `main`, `lazy-upstream-runtime`, and `feat/workspace-runtime-builder`; no cleanup was safe because the current PR branch and another active feature worktree are still present.

Stale docs: updated gateway/upstream docs and the implementation plan during the PR. No additional stale-doc edits were made during this save step.

Transparency: `git status --short` was clean before writing this session artifact.

## Tools and Skills Used

- Shell commands: used `rg`, `sed`, `git`, `cargo fmt`, `cargo nextest`, `cargo check`, and `gh api graphql` for inspection, validation, commits, pushes, and PR thread management.
- File editing: used patch-based edits for Rust and markdown changes.
- GitHub CLI: used to inspect PR #77, add a review-thread rationale reply, and resolve review threads.
- Skills: used `work-it` workflow expectations and `save-to-md` session documentation workflow.
- MCP/app/browser tools: no MCP server tools or browser automation were used for the implementation.

## Commands Executed

| command | result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed earlier in the PR validation. |
| `cargo check -p labby --all-features` | Passed earlier in the PR validation. |
| `cargo nextest run -p labby dispatch::gateway::manager dispatch::gateway::code_mode dispatch::upstream::pool --all-features` | Passed 106 focused tests earlier in review hardening. |
| `cargo nextest run -p labby --all-features` | Passed 1356 tests, 25 skipped earlier in full validation. |
| `cargo fmt --all && cargo nextest run -p labby dispatch::upstream::pool::tests::ensure_tools_for_upstream_preserves_other_resource_upstreams dispatch::upstream::pool::tests::ensure_tools_for_upstream_singleflights_concurrent_connects dispatch::upstream::pool::tests::ensure_tools_for_upstream_records_lazy_connect_failures --all-features && git diff --check` | Passed 3 focused upstream tests and whitespace check. |
| `cargo fmt --all && cargo nextest run -p labby dispatch::gateway::manager::tests::resolve_raw_upstream_tool_honors_qualified_upstream_name dispatch::gateway::manager::tests::resolve_raw_upstream_tool_resolves_cached_tool_without_tool_search mcp::server::tests::server_reads_current_pool_from_gateway_manager --all-features && git diff --check` | Passed 3 focused manager/MCP tests and whitespace check. |
| `gh api graphql ... reviewThreads` | Confirmed all known PR review threads resolved after fixes and rationale reply. |

## Errors Encountered

- Review found that OAuth subject ownership was not preserved through lazy search warm-up. Fixed by threading owner and OAuth subject through gateway search/runtime readiness paths.
- Review found that lazy `ensure_tools_for_upstream` had no discovery timeout. Fixed by wrapping lazy connects in `DISCOVERY_TIMEOUT`.
- Review found `seed_lazy_upstreams([one])` could corrupt `resource_upstreams`. Fixed with `ensure_lazy_upstream_entry`.
- Review found raw qualified tool names could resolve the wrong upstream. Fixed by honoring `selector.upstream`.
- Review found raw-resolution errors were dropped with `.ok()`. Fixed by preserving the `Result` and returning structured errors for non-not-found failures.

## Behavior Changes (Before/After)

| area | before | after |
| --- | --- | --- |
| Gateway startup | Configured upstream MCP servers started eagerly for each connected agent. | Startup seeds lazy metadata; upstreams connect on demand. |
| Search warm-up | Cold `tool_search`/`code_search` could miss OAuth upstreams or fail silently. | Warm-up carries OAuth subject/owner and surfaces cold failures when no usable index exists. |
| Read-only `code_search` | Could have been allowed to spawn upstream processes if cold warming were read-scoped. | Read-only callers see cached connected tools only; execute-scoped callers can cold-start. |
| Upstream resource proxy list | Single-upstream lazy seeding could drop other resource upstreams. | Lazy entry registration preserves the existing resource upstream list. |
| Raw MCP proxy | Qualified names and resolution errors could be mishandled. | Qualified names route to the named upstream and structured errors are preserved. |

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `cargo fmt --all -- --check` | Formatting clean. | Passed earlier in PR validation. | pass |
| `cargo check -p labby --all-features` | All-features check clean. | Passed earlier in PR validation. | pass |
| `cargo nextest run -p labby --all-features` | Full labby test suite passes. | Passed earlier: 1356 passed, 25 skipped. | pass |
| focused upstream pool nextest command | New resource/timeout/failure regressions pass. | 3 passed, 1379 skipped. | pass |
| focused manager/MCP nextest command | Raw resolution and MCP compile path pass. | 3 passed, 1380 skipped. | pass |
| PR review thread GraphQL query | No unresolved actionable review threads. | All known threads resolved; read-only cold-start comment resolved with rationale. | pass |

## Risks and Rollback

Risk: first use of an upstream now pays connection and discovery latency, and broken upstreams surface on the first dependent search/execute call instead of startup.

Risk: read-only callers may see empty upstream search results until an execute-scoped caller warms the configured upstreams; this is intentional to avoid process-spawn side effects for `lab:read`.

Rollback path: revert PR #77 branch commits from `32708ceb` through `c40031b2` to restore eager upstream discovery behavior.

## Decisions Not Taken

- Did not allow `lab:read` `code_search` calls to cold-start upstream processes, because spawning configured stdio servers is an execution side effect.
- Did not move unrelated `docs/plans` files to complete, because they were not proven completed by this session.
- Did not clean other worktrees or branches, because they are active or tied to open branch work.

## References

- PR #77: https://github.com/jmagar/lab/pull/77
- Plan: `docs/superpowers/plans/2026-05-26-gateway-lazy-upstream-runtime.md`
- Dispatch ownership guide: `docs/dev/DISPATCH.md`
- Gateway docs: `docs/services/GATEWAY.md`
- Upstream docs: `docs/services/UPSTREAM.md`

## Open Questions

- No open implementation questions observed at closeout.

## Next Steps

Run final closeout validation after this session artifact commit: `cargo fmt --all -- --check`, `cargo check -p labby --all-features`, `cargo nextest run -p labby --all-features`, and `git status --short`.
