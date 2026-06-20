---
date: 2026-06-20
repo: git@github.com:jmagar/lab.git
branch: detached
head: 15852459
working_directory: /home/jmagar/.codex/worktrees/23788b92-b568-4d0b-acb7-8f57777bb09b/lab
worktree: /home/jmagar/.codex/worktrees/23788b92-b568-4d0b-acb7-8f57777bb09b/lab
pr: none_verified
beads:
  - lab-fe055
skills:
  - vibin:save-to-md
  - superpowers:systematic-debugging
  - superpowers:test-driven-development
  - superpowers:verification-before-completion
---

# MCP UI Resource Proxy Gate Session

## User Request

The user asked why the Lab connector exposed GitHub tools in ChatGPT when Cortex, YouTube, and Axon were expected MCP UI tools. After narrowing the behavior down, the user asked to make the existing server-level `proxy_resources` knob actually suppress GitHub MCP UI resource-backed tools instead of adding a separate allowlist first.

## Session Overview

The session investigated why some GitHub MCP tools were visible as top-level ChatGPT tools, confirmed they were MCP App UI tools advertised by the upstream GitHub MCP server, and implemented a regression-tested fix in Lab's upstream gateway code. The fix makes top-level MCP App tool promotion honor `UpstreamEntry.proxy_resources`, so disabling resource proxying for an upstream also suppresses promotion of its `ui://` backed tools.

## Sequence

1. Inspected the screenshot and mapped the visible tools to Lab synthetic tools and upstream MCP App tools.
2. Confirmed raw GitHub MCP metadata showed `_meta.ui.resourceUri` for pull request, issue, and user-profile tools.
3. Found that `proxy_resources` only controlled resource registration/proxying, while `healthy_ui_tools_allowed` still promoted UI tools from healthy upstreams.
4. Added a failing regression test proving an upstream with `proxy_resources=false` still exposed an MCP App UI tool before the fix.
5. Updated `UpstreamEntry` and upstream discovery/entry construction to carry the resource proxy flag.
6. Changed UI tool promotion to skip entries where `proxy_resources` is false.
7. Updated test fixtures for the new field and verified the positive and negative tool-list behavior.
8. Ran save-to-md maintenance: reviewed repo state, beads, worktrees, plans, and created a follow-up bead for rollout/config validation.

## Key Findings

- The visible GitHub entries were not ordinary raw GitHub tools. They were tools with MCP App UI metadata, including `ui://github-mcp-server/pr-write`, `ui://github-mcp-server/get-me`, `ui://github-mcp-server/issue-write`, and `ui://github-mcp-server/pr-edit`.
- Lab's root tool list promotes healthy upstream MCP App tools when raw upstream tools are hidden.
- The existing `proxy_resources` config flag did not participate in this promotion path, so disabling resource proxying was insufficient.
- The mismatch was specific to UI tool promotion; Code Mode synthetic tools should remain visible regardless of whether a specific upstream proxies resources.

## Technical Decisions

- Reused the existing server-level `proxy_resources` knob instead of introducing a new allowlist.
- Stored `proxy_resources` on `UpstreamEntry`, because the promotion path works from discovered entries and should not need to re-resolve original config.
- Treated MCP App tool promotion as dependent on resource proxying. If the gateway will not proxy the `ui://` resource, it should not advertise the tool as a top-level UI tool.
- Left in-process entries with `proxy_resources=true`, because synthetic/local services are not the problematic upstream server-resource case.

## Files Changed

Code changes remain uncommitted in this worktree:

- `crates/lab/src/dispatch/upstream/types.rs` adds `proxy_resources` to `UpstreamEntry`.
- `crates/lab/src/dispatch/upstream/pool/tools.rs` filters UI tool promotion by `entry.proxy_resources`.
- `crates/lab/src/dispatch/upstream/pool/entries.rs` carries `config.proxy_resources` into lazy upstream entries.
- `crates/lab/src/dispatch/upstream/pool/discover.rs` carries `proxy_resources` through discovery into remote upstream entries.
- `crates/lab/src/mcp/handlers_tools/tests.rs` adds the negative regression test and updates fixtures.
- Additional test fixtures were updated in gateway tests to set `proxy_resources: true`.
- This session artifact was added at `docs/sessions/2026-06-20-mcp-ui-resource-proxy-gate.md`.

## Beads Activity

- Read open beads with `bd list --status open --json`.
- Created `lab-fe055`: "Roll out proxy_resources gating for GitHub MCP UI tools".
- The bead tracks the remaining operational step: merge the code fix, set GitHub `proxy_resources=false` in the active Labby gateway config if the GitHub UI widgets should stay hidden, reload/restart Labby, and verify the ChatGPT Lab tool list.

## Repository Maintenance

- Plans: reviewed `docs/plans`. `docs/plans/complete/mcp-streamable-http-oauth-proxy.md` was already complete, and `docs/plans/fleet-ws-plan-lab-n07n.md` is an unrelated open fleet WebSocket plan. No plan files were moved.
- Beads: created one follow-up bead for the config/deploy side of this fix. No beads were closed because the code change is not yet committed, merged, or live-verified.
- Worktrees: inspected worktrees. Current worktree is detached at `15852459`; sibling worktrees exist for `main`, `marketplace-no-mcp`, and `fix/nonblocking-root-list-tools`. No cleanup was attempted because the current worktree is dirty and sibling branch ownership is unrelated to this save.
- Branches: inspected local and remote branches. No branch cleanup was attempted.
- Stale docs: no product docs were updated because the behavior is covered by tests and the remaining operator action is tracked in `lab-fe055`.

## Tools and Skills Used

- `vibin:save-to-md` for the session log and path-limited save commit workflow.
- `superpowers:systematic-debugging` to isolate the root cause instead of patching around symptoms.
- `superpowers:test-driven-development` to add a failing regression before the implementation.
- `superpowers:verification-before-completion` to rerun focused tests after formatting and fixture updates.
- Local shell commands for git, cargo, and bead inspection.

## Commands Executed

Key commands included:

```bash
git status --short --branch
git worktree list --porcelain
git branch -vv
git branch -r -vv
bd list --status open --json
bd create --title "Roll out proxy_resources gating for GitHub MCP UI tools" --description "..." --type task --priority 2 --labels gateway,mcp-ui,config,follow-up
cargo test -p labby mcp::handlers_tools::tests::list_tools_does_not_promote_upstream_mcp_app_tools_when_resources_are_not_proxied --all-features
cargo test -p labby mcp::handlers_tools::tests::list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden --all-features
cargo fmt --all
cargo test -p labby mcp::handlers_tools::tests::list_tools_ --all-features
```

## Errors Encountered

- The first regression test failed as expected before the implementation because `github_pr_ui` was still promoted when `proxy_resources=false`.
- After adding `proxy_resources` to `UpstreamEntry`, several test fixtures failed to compile until they were updated with `proxy_resources: true`.
- `mcp__lumen__semantic_search` failed earlier in the session with a timeout and then an LM Studio `413` length-limit error, so local search/read commands were used.
- `gh pr view` was blocked by local `mise` trust for this worktree's `.mise.toml`; PR state is recorded as not verified.
- Cargo emitted the known warning that `apps/gateway-admin/out` was missing and empty web assets were embedded for the test build.

## Behavior Changes

- Upstream UI tools backed by MCP App resources are no longer promoted when that upstream has `proxy_resources=false`.
- Code Mode's synthetic `codemode` tool remains visible in the same scenario.
- Positive behavior is preserved: upstream MCP App UI tools are still promoted when raw tools are hidden and resource proxying is enabled.

## Verification Evidence

Fresh focused verification passed:

```text
cargo test -p labby mcp::handlers_tools::tests::list_tools_ --all-features

running 4 tests
test mcp::handlers_tools::tests::list_tools_advertises_code_mode_output_schemas ... ok
test mcp::handlers_tools::tests::list_tools_skips_upstream_ui_tools_that_collide_with_synthetic_names ... ok
test mcp::handlers_tools::tests::list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden ... ok
test mcp::handlers_tools::tests::list_tools_does_not_promote_upstream_mcp_app_tools_when_resources_are_not_proxied ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 1742 filtered out
```

`git diff --stat` for the code change before this artifact:

```text
9 files changed, 66 insertions(+)
```

## Risks / Rollback

- The code fix is not yet committed in this worktree. Rollback before commit is a normal `git restore` of the touched code/test files.
- The operational behavior depends on the active Labby gateway config setting GitHub `proxy_resources=false`; the code change alone does not change existing config.
- If a user wants GitHub MCP App widgets, leaving `proxy_resources=true` preserves the old promotion behavior.
- The worktree is detached, so pushing code changes should be done from a named branch before opening a PR.

## Decisions Not Taken

- Did not add a granular allowlist. The existing server-level knob was enough and matches the user's stated preference.
- Did not remove GitHub from the gateway.
- Did not globally suppress MCP App UI tools.
- Did not clean up unrelated worktrees or branches.
- Did not move the unrelated fleet WebSocket plan to complete.

## References

- `crates/lab/src/mcp/handlers_tools.rs`
- `crates/lab/src/dispatch/upstream/pool/tools.rs`
- `crates/lab/src/dispatch/upstream/types.rs`
- `crates/lab/src/dispatch/upstream/pool/discover.rs`
- `crates/lab/src/dispatch/upstream/pool/entries.rs`
- `docs/plans/fleet-ws-plan-lab-n07n.md`
- Bead `lab-fe055`

## Open Questions

- Should the active GitHub upstream be set to `proxy_resources=false` permanently, or should GitHub UI widgets be available in some contexts?
- Should UI-tool promotion have a more explicit name in config later, or is `proxy_resources` intentionally the single server-level gate?

## Next Steps

1. Commit the code/test fix on a named branch.
2. Merge or deploy the fix.
3. Complete `lab-fe055`: set the active GitHub upstream `proxy_resources=false` if desired, reload Labby, and verify the ChatGPT Lab connector no longer shows the GitHub UI tools.
