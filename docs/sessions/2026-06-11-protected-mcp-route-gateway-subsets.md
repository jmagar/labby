---
date: 2026-06-11 01:48:53 EST
repo: git@github.com:jmagar/lab.git
branch: codex/protected-mcp-route-gateway-subsets
head: 1ca595bea7210ccadfbd60679c7df7eb3fbe7c37
plan: docs/superpowers/plans/2026-06-11-protected-mcp-route-gateway-subsets.md
working directory: /home/jmagar/workspace/lab/.worktrees/protected-mcp-route-gateway-subsets
worktree: /home/jmagar/workspace/lab/.worktrees/protected-mcp-route-gateway-subsets
pr: "#111 Fix protected MCP route gateway subsets https://github.com/jmagar/lab/pull/111"
---

# Protected MCP route gateway subsets

## User Request

Work the plan file `2026-06-11-protected-mcp-route-gateway-subsets.md` using the `vibin:work-it` workflow.

## Session Overview

Implemented protected MCP routes that can expose a gateway-subset target as an in-process scoped MCP route. The work added config parsing, CLI/gateway CRUD support, scoped MCP discovery and dispatch filtering, protected route HTTP wiring, docs, tests, and follow-up review fixes. PR #111 is open and mergeable with unresolved review threads at zero after the final CodeRabbit thread was answered and resolved.

## Sequence of Events

1. Created the isolated worktree and branch `codex/protected-mcp-route-gateway-subsets`.
2. Wrote the superpowers implementation plan at `docs/superpowers/plans/2026-06-11-protected-mcp-route-gateway-subsets.md`.
3. Implemented `gateway_subset` protected route config, CLI flags, route scope propagation, scoped MCP handlers, scoped upstream pool filtering, and protected router mounting.
4. Reviewed the implementation through local inspection, CodeRabbit/PR comments, and targeted tests, then patched direct code-mode bypasses, synthetic resource leaks, unhealthy-upstream masking, lifecycle messaging, duplicate route validation, structured backend errors, and host dedupe.
5. Addressed the final middleware-stack review by moving protected virtual-host interception inside the shared HTTP middleware stack and adding a request-id propagation regression assertion.
6. Refreshed PR review state, verified there were zero unresolved non-outdated review threads, and saved this session artifact.

## Key Findings

- `protected_mcp_intercept` originally returned the scoped router response outside the shared timeout/trace/CORS/request-id stack; this was fixed in `crates/lab/src/api/router.rs` by mounting the intercept as inner middleware.
- Gateway-subset routes need a distinct `McpRouteScope` so `tools/list`, `resources/list`, `prompts/list`, `call_tool`, upstream raw tool resolution, and code-mode catalog calls share one allowlist boundary.
- Runtime gateway protected-route CRUD cannot hot-swap already-mounted scoped MCP services safely, so gateway-subset protected route changes now return a restart-required result instead of implying live remount.
- Subject-scoped OAuth discovery has to apply the route scope before surfacing allowed resources and capabilities, otherwise disallowed upstreams can be visible before filtering.
- Duplicate gateway-subset `public_path` values across hosts are ambiguous for the single scoped router mount and are rejected during config validation.

## Technical Decisions

- Reused the existing one-tool-per-service MCP shape and added route-scoped filtering rather than minting per-upstream tools for protected subsets.
- Kept `expose_code_mode` authoritative: hidden code-mode routes deny direct `search` and `execute` calls instead of only hiding them from discovery.
- Made route-scope denial return structured MCP errors with stable `kind` fields so agents can distinguish scope denial from missing upstreams.
- Left protected route runtime add/update/delete available for normal proxy routes, but documented and enforced restart semantics for gateway-subset route lifecycle changes.
- Kept the final middleware fix in the router layer, because in-process gateway-subset traffic should behave like the normal `/mcp` surface rather than bypass shared HTTP middleware.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `crates/lab/src/api/error.rs` | - | Added protected route API error mapping. | `git diff --name-status origin/main...HEAD` |
| modified | `crates/lab/src/api/router.rs` | - | Added protected route gateway-subset dispatch, structured invalid-backend handling, middleware-stack fix, and regression tests. | Commits `db421742`, `36f64998`, `1ca595be` |
| modified | `crates/lab/src/api/state.rs` | - | Stored the scoped protected MCP router in shared app state. | Commit `db421742` |
| modified | `crates/lab/src/cli/gateway.rs` | - | Added gateway protected-route CLI fields and target handling. | Commits `db421742`, `36f64998` |
| modified | `crates/lab/src/cli/serve.rs` | - | Built route-scoped MCP services and deduped allowed hosts. | Commits `db421742`, `296e0aa2` |
| modified | `crates/lab/src/config.rs` | - | Parsed and validated protected route `gateway_subset` targets. | Commits `db421742`, `ee2830e7`, `e5340f39`, `00125690` |
| modified | `crates/lab/src/dispatch/gateway/**` | - | Added protected route gateway actions, lifecycle checks, code-mode scope resolution, and tests. | `git diff --name-status origin/main...HEAD` |
| modified | `crates/lab/src/dispatch/upstream/pool/**` | - | Applied route-scoped filtering to tools, prompts, resources, raw reads, and discovery. | `git diff --name-status origin/main...HEAD` |
| modified | `crates/lab/src/mcp/**` | - | Added route scope model and applied it across MCP handlers, context, call_tool, resources, prompts, catalog, and server state. | `git diff --name-status origin/main...HEAD` |
| created | `crates/lab/src/mcp/route_scope.rs` | - | Defines `McpRouteScope` for protected gateway subsets. | `git diff --name-status origin/main...HEAD` |
| modified | `docs/dev/ERRORS.md` | - | Documented new stable error kind. | Commit `db421742` |
| created | `docs/superpowers/plans/2026-06-11-protected-mcp-route-gateway-subsets.md` | - | Implementation plan used for the work. | Commit `db421742` |
| modified | `docs/surfaces/MCP.md` | - | Documented protected gateway-subset behavior and restart semantics. | Commits `db421742`, `ee2830e7`, `00125690` |

## Beads Activity

No bead activity observed for this session. Evidence: `bd list --all --sort updated --reverse --limit 100 --json` was run during closeout; it returned existing historical beads but no new or directly session-specific bead was created, claimed, edited, or closed in this worktree.

## Repository Maintenance

Plans: `find docs/plans docs/superpowers/plans -maxdepth 2 -type f` showed the active plan under `docs/superpowers/plans/2026-06-11-protected-mcp-route-gateway-subsets.md`. It was not moved because PR #111 is still open.

Beads: no bead changes were made. The session is tracked by GitHub issue/PR #111 and this session note.

Worktrees and branches: `git worktree list --porcelain` showed the main checkout, this active PR worktree, `.claude/worktrees/objective-ardinghelli-203310`, and `.worktrees/settings-page-revamp`. No cleanup was attempted because the other worktrees are active or unclear ownership.

Stale docs: docs directly affected by this feature were updated in `docs/surfaces/MCP.md` and `docs/dev/ERRORS.md`. No broader docs cleanup was attempted beyond the feature surface.

Transparency: local worktree was clean before writing this session file. PR #111 was open, mergeable, and CI had several jobs still in progress when metadata was collected.

## Tools and Skills Used

- `vibin:work-it`: drove the branch-oriented implementation and closeout workflow.
- `superpowers:writing-plans`: produced the implementation plan before coding.
- `vibin:save-to-md`: used for this session artifact and path-limited session-file commit rules.
- Shell and Git: inspected status, diffs, worktrees, commits, PR checks, and ran verification.
- GitHub CLI and GraphQL API: created/refreshed PR state, fetched comments, replied to and resolved the final review thread.
- Code review agents and PR reviewers: review comments from CodeRabbit and prior review passes shaped several hardening commits.

## Commands Executed

| command | result |
|---|---|
| `cargo fmt --all` | Passed. |
| `CARGO_BUILD_JOBS=1 cargo test -p labby --lib resolve_raw_upstream_tool_scoped_hides_priority_zero_upstreams --all-features` | Passed. |
| `CARGO_BUILD_JOBS=1 cargo test -p labby --lib protected_route_invalid_backend_url_returns_structured_error --all-features` | Passed. |
| `CARGO_BUILD_JOBS=1 cargo test -p labby --lib protected_gateway_subset_dispatches_to_scoped_router_after_auth --all-features` | Passed. |
| `CARGO_BUILD_JOBS=1 cargo test -p labby --lib protected_route --all-features` | Passed: 19 tests. |
| `CARGO_BUILD_JOBS=1 cargo check --workspace --all-features` | Passed. |
| `CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-features -- -D warnings` | Passed. |
| `gh-fetch-comments --pr 111 --repo jmagar/lab --no-beads -o /tmp/pr111-comments.json` | Refreshed PR review comments; final unresolved non-outdated thread count was 0. |
| `gh pr view 111 --json number,title,url,headRefName,baseRefName,state,isDraft,mergeable,statusCheckRollup` | PR #111 was open, not draft, mergeable; some CI jobs were still in progress at closeout metadata time. |

## Errors Encountered

- Review found protected code-mode bypasses where direct `search` and `execute` remained callable when `expose_code_mode=false`; fixed by enforcing route-scope denial in direct code-mode dispatch.
- Review found scoped discovery/resource paths could reveal or fan out to disallowed upstreams; fixed by applying route-scope filtering before upstream dispatch and synthetic resource exposure.
- Review found scoped raw tool resolution could skip priority-zero routability and unhealthy allowed upstreams could be masked by healthy disallowed ones; fixed in the gateway manager and upstream pool tests.
- Review found invalid backend URLs returned an unstructured error; fixed with a structured response and targeted test.
- Review found protected virtual-host gateway-subset dispatch skipped the shared HTTP middleware stack; fixed in `1ca595be` with a request-id propagation regression assertion.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Protected MCP route target | Routes only proxied to a backend URL or named upstream. | Routes can target a scoped gateway subset and mount an in-process MCP service at the protected public path. |
| MCP discovery | Route-scoped callers could see broader catalog/resource surfaces in some paths. | Tools, prompts, resources, raw upstream calls, and code-mode catalogs are filtered by allowed upstreams. |
| Code mode | Hidden code-mode helpers could still be called directly. | Hidden code-mode helpers deny direct calls for scoped routes. |
| Runtime protected-route CRUD | Gateway-subset changes could appear to succeed as live changes. | Gateway-subset route changes report restart-required semantics. |
| HTTP middleware | Virtual-host scoped responses bypassed request-id/trace/timeout/compression/CORS. | Scoped protected responses pass through the shared HTTP middleware stack. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo fmt --all` | Formatting clean. | Passed. | pass |
| `cargo test -p labby --lib protected_route --all-features` | Protected-route tests pass. | 19 passed, 0 failed. | pass |
| `cargo test -p labby --lib protected_gateway_subset_dispatches_to_scoped_router_after_auth --all-features` | Scoped router dispatch works and propagates `x-request-id`. | Passed. | pass |
| `cargo test -p labby --lib resolve_raw_upstream_tool_scoped_hides_priority_zero_upstreams --all-features` | Scoped raw tool resolution hides priority-zero/disallowed upstreams. | Passed. | pass |
| `cargo test -p labby --lib protected_route_invalid_backend_url_returns_structured_error --all-features` | Invalid backend URL returns structured error. | Passed. | pass |
| `cargo check --workspace --all-features` | All-features check passes. | Passed. | pass |
| `cargo clippy --workspace --all-features -- -D warnings` | Clippy passes with warnings denied. | Passed. | pass |
| `gh-fetch-comments --pr 111 --repo jmagar/lab --no-beads -o /tmp/pr111-comments.json` | No unresolved current review threads remain. | `unresolved_not_outdated 0`. | pass |

## Risks and Rollback

Risk: protected route middleware order changed for virtual-host protected MCP routes. The regression test covers request-id propagation for gateway subsets, but operators should still watch long-lived remote proxy behavior after merge.

Rollback: revert PR #111 or specifically revert `1ca595be` to restore the previous protected virtual-host middleware order. Reverting the full branch removes `gateway_subset` protected route support.

## Decisions Not Taken

- Did not hot-reload scoped MCP routers for gateway-subset protected route CRUD. Restart-required behavior is safer because the scoped router captures mounted service state at server startup.
- Did not add per-upstream MCP tools. Route scoping preserves the existing single-tool service model.
- Did not move the active plan to a completed plans directory because the PR is still open and CI was not fully settled at closeout.

## References

- PR #111: https://github.com/jmagar/lab/pull/111
- Plan: `docs/superpowers/plans/2026-06-11-protected-mcp-route-gateway-subsets.md`
- MCP surface docs: `docs/surfaces/MCP.md`
- Error contract docs: `docs/dev/ERRORS.md`

## Open Questions

- Final CI jobs were still running when closeout metadata was collected, although local all-features tests/check/clippy passed and CodeRabbit status had turned success.

## Next Steps

1. Wait for remaining CI jobs on PR #111 to finish.
2. Merge PR #111 if CI remains green.
3. After merge, remove the active worktree and branch only after verifying the PR branch is merged into `main`.
