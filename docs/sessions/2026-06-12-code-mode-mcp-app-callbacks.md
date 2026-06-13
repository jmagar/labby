---
date: 2026-06-12 22:07:09 EDT
repo: git@github.com:jmagar/lab.git
branch: codex/code-mode-mcp-app-callbacks
head: e2559d7b
plan: docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md
working directory: /home/jmagar/workspace/lab/.worktrees/code-mode-mcp-app-callbacks
worktree: /home/jmagar/workspace/lab/.worktrees/code-mode-mcp-app-callbacks e2559d7b [codex/code-mode-mcp-app-callbacks]
pr: "#118 Fix code mode MCP App sibling callbacks (https://github.com/jmagar/lab/pull/118)"
---

# Code mode MCP App callback session

## User Request

Create a plan to address the issue where code mode hides sibling upstream tools needed by rendered MCP Apps, then execute the plan with `$vibin:work-it`.

## Session Overview

Implemented and published PR #118 to let code mode MCP Apps call safe same-upstream sibling tools through `callServerTool` without re-exposing those tools in `tools/list`. The final pushed state blocks destructive direct UI, legacy-widget, and sibling callbacks, rejects ambiguous duplicate-name sibling matches, preserves route scope and execute-scope checks, and routes pre-resolved OAuth callbacks through subject-scoped routing.

## Sequence of Events

1. Wrote `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md` for the host-side fix.
2. Created clean worktree `/home/jmagar/workspace/lab/.worktrees/code-mode-mcp-app-callbacks` on branch `codex/code-mode-mcp-app-callbacks`.
3. Added upstream sibling lookup, code-mode callback gate behavior, docs, and tests.
4. Fixed unrelated all-features clippy drift already exposed by the branch verification gate.
5. Rebased onto current `origin/main`, verified, pushed, and opened PR #118.
6. Ran review waves, addressed findings around same-name upstream routing, execute scope, destructive callback bypasses, ambiguity ordering, OAuth subject-scoped routing, and missing handler tests.
7. Refreshed the PR body with final verification and saved this session note.

## Key Findings

- Code mode hid raw upstream sibling tools while still exposing MCP-App UI tools, making rendered apps able to display but unable to call their server-side helpers.
- `UpstreamPool::find_mcp_app_sibling_tool_candidates` now provides a narrow allowlist: same upstream, exposed target tool, routable upstream, and at least one exposed MCP App UI sibling (`crates/lab/src/dispatch/upstream/pool/tools.rs:150`).
- `call_tool_impl` now classifies callback attempts before raw upstream fallback, binding allowed callbacks to the selected upstream and returning `ambiguous_tool`, `confirmation_required`, or `forbidden` where appropriate (`crates/lab/src/mcp/call_tool.rs:237`).
- Pre-resolved OAuth callbacks now bypass the shared raw-pool call path and use subject-scoped routing (`crates/lab/src/mcp/call_tool_upstream.rs:66`, `crates/lab/src/mcp/call_tool_upstream.rs:126`, `crates/lab/src/mcp/call_tool_upstream.rs:308`).
- PR comments had no actionable review threads; Codex review and CodeRabbit both reported usage/rate limits on the final pushed state.

## Technical Decisions

- Keep model-facing `tools/list` collapsed in code mode; only callback dispatch gets the scoped exception.
- Require the sibling tool to be exposed by the upstream policy and paired with an exposed MCP-App UI tool on the same upstream.
- Require `lab` or `lab:admin` scope for hidden sibling callbacks, matching the execute-scope boundary.
- Reject destructive callbacks instead of trying to run confirmation elicitation through widget callbacks; the message points callers to `execute` with confirmation.
- Prefer ambiguity over destructive classification when duplicate sibling matches exist, because the caller has not identified which upstream should own the call.
- Add a `cfg(test)` server flag for the legacy widget callback branch instead of mutating process environment in Rust 2024 tests.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | crates/lab/src/acp/providers.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/acp/runtime.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/api/nodes/fleet.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/api/services/acp.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/cli.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/cli/doctor.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/cli/gateway.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/cli/oauth.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/cli/serve.rs | - | Test-only server field initialization and cleanup | `origin/main...HEAD` diff |
| modified | crates/lab/src/cli/setup.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/config.rs | - | Code mode/config cleanup | `origin/main...HEAD` diff |
| modified | crates/lab/src/config/env_merge.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/acp/persistence.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/doctor.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/doctor/proxy.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/doctor/system.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/fs/dispatch.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs | - | Code mode test adjustment | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/code_mode/tests_ids_schema.rs | - | Code mode test adjustment | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/code_mode/tests_runtime.rs | - | Code mode test adjustment | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/config.rs | - | Code mode config support | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/discovery/opencode.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/dispatch.rs | - | Code mode dispatch support | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/manager/tests/cleanup.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/manager/tests/code_mode.rs | - | Code mode test coverage | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/gateway/manager/tests/lifecycle.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/logs/metrics/tests.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/marketplace.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/marketplace/acp_dispatch.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/marketplace/backends/codex.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/marketplace/client.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/marketplace/dispatch.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/node/send.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/setup/bootstrap.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/setup/dispatch.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/stash/store.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/http_client.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/pool/ensure.rs | - | Upstream pool cleanup | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/pool/prompts_list.rs | - | Test-only server field initialization | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/pool/resources_read.rs | - | Test-only server field initialization and cleanup | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/pool/tools.rs | - | MCP App UI listing and sibling candidate lookup | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/process_guard.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/dispatch/upstream/types.rs | - | Upstream type cleanup | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/call_tool.rs | - | Callback gate, ambiguity/destructive/scope handling, selected-upstream binding | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/call_tool_codemode/tests.rs | - | Code mode test adjustments | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/call_tool_upstream.rs | - | Pre-resolved OAuth callback routing | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/handlers_prompts.rs | - | Test-only server field initialization | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/handlers_resources.rs | - | Test-only server field initialization | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/handlers_tools/tests.rs | - | Handler-level callback safety and routing tests | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/in_process_peer.rs | - | Test-only server field initialization | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/server.rs | - | Test-only server flag for legacy widget callback branch | `origin/main...HEAD` diff |
| modified | crates/lab/src/mcp/services/fs.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/node/runtime.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/node/ws_client.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/oauth/local_relay.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/oauth/upstream/encryption.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/src/output/render.rs | - | Clippy cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/acp_backend_contract.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/code_mode_runner.rs | - | Code mode test adjustment | `origin/main...HEAD` diff |
| modified | crates/lab/tests/deploy_runner.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/device_cli.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/device_runtime.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/device_scan.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/gateway_stdio_spawn.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/logs_api.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/nodes_cli.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/nodes_runtime.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | crates/lab/tests/upstream_oauth.rs | - | Test cleanup from all-features gate | `origin/main...HEAD` diff |
| modified | docs/dev/CODE_MODE.md | - | Document code-mode MCP App callback rules | `origin/main...HEAD` diff |
| created | docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md | - | Execution plan | `origin/main...HEAD` diff |
| modified | docs/surfaces/MCP.md | - | Document MCP surface callback behavior | `origin/main...HEAD` diff |

## Beads Activity

No bead activity observed. `bd list --all --sort updated --reverse --limit 100 --json` returned only historical closed issues, and `tail -200 .beads/interactions.jsonl` returned `none`.

## Repository Maintenance

### Plans

`find docs/plans docs/superpowers/plans -maxdepth 2 -type f` found the completed plan in `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md`. I did not move it because the save workflow only names `docs/plans/complete/`, and this repo currently has many active historical superpowers plans without a matching `docs/superpowers/plans/complete/` convention.

### Beads

No bead was created or closed during this session. There is no observed relevant open bead to close.

### Worktrees and branches

`git worktree list --porcelain` showed active worktrees for this branch plus unrelated branches: `codex/feature-slice-cleanup`, `codex/fix-code-mode-mcp-app-callbacks`, `codex/readme-rewrite`, `main`, and `codex/settings-page-config-plan`. I did not remove any worktree or branch because they are separate active contexts or protected base branches.

### Stale docs

`docs/dev/CODE_MODE.md` and `docs/surfaces/MCP.md` were updated in the implementation commits to reflect the new callback rule. No additional stale-doc update was found during the closeout pass.

### Transparency

The branch was clean before writing this session note. The PR body was updated through GitHub to reflect final verification; that did not change local files.

## Tools and Skills Used

- Shell commands: git, cargo, nextest, clippy, formatter, find, bead reads, and date/status inspection.
- File tools: `apply_patch` for the plan, implementation fixes, and this session artifact.
- GitHub connector: created and inspected PR #118, fetched comments/reviews/threads, and updated the PR body.
- Skills/plugins: `superpowers:writing-plans` for the plan, `vibin:work-it` for execution flow, and `vibin:save-to-md` for session closeout.
- Subagents/reviewers: implementation and review waves produced findings that drove the `a7ff3abb` and `e2559d7b` follow-up commits.
- External review bots: Codex review and CodeRabbit both reported usage/rate limits; no actionable review threads were present.

## Commands Executed

| command | result |
|---|---|
| `cargo fmt --all --check` | passed |
| `cargo test -p labby call_tool_ --all-features` | passed; 24 tests passed |
| `cargo test -p labby mcp_app_sibling_lookup --all-features` | passed; 2 tests passed |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | passed |
| `cargo nextest run --workspace --all-features` | passed; 1942 tests run, 1942 passed, 27 skipped |
| `git push` | pushed `e2559d7b` to `origin/codex/code-mode-mcp-app-callbacks` |
| GitHub PR comment/review/thread fetches | no actionable review threads; bot comments were rate/usage-limit notices |
| `bd list --all --sort updated --reverse --limit 100 --json` | historical closed issues only |
| `tail -200 .beads/interactions.jsonl` | `none` |

## Errors Encountered

- `cargo fmt --all --check` initially failed on formatting in `call_tool_upstream.rs` and an expected JSON array test assertion; `cargo fmt --all` fixed it.
- A first draft of the legacy widget callback test used unsafe process-env mutation, which is rejected by the crate's unsafe-code lint under tests. Replaced it with a `cfg(test)` per-server flag.
- Starting two cargo checks in parallel caused cargo build-lock waiting. The commands serialized and completed successfully.
- CodeRabbit and Codex review bots could not run final external reviews due usage/rate limits; direct PR thread inspection found no actionable review comments.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| MCP App in code mode | Rendered UI could be exposed while its safe sibling tools were hidden and unreachable. | Safe same-upstream sibling callbacks can dispatch while remaining hidden from `tools/list`. |
| Same-name upstream tools | A callback could be approved for one upstream but fall through to another raw same-name upstream. | Callback dispatch binds to the selected upstream or rejects ambiguity. |
| Destructive callbacks | Direct UI and legacy-widget callbacks could bypass confirmation. | Destructive callback paths return `confirmation_required`. |
| Hidden sibling callbacks | Review found missing execute-scope enforcement risk. | Hidden sibling callbacks require `lab` or `lab:admin` scope. |
| OAuth upstream callbacks | Pre-resolved OAuth callbacks could use the shared raw pool. | Pre-resolved OAuth callbacks use subject-scoped routing. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo fmt --all --check` | Rust formatting clean | exited 0 | pass |
| `cargo test -p labby call_tool_ --all-features` | callback handler tests pass | 24 passed | pass |
| `cargo test -p labby mcp_app_sibling_lookup --all-features` | sibling lookup tests pass | 2 passed | pass |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | no clippy warnings | exited 0 | pass |
| `cargo nextest run --workspace --all-features` | all-features workspace tests pass | 1942 passed, 27 skipped | pass |

## Risks and Rollback

Risk is concentrated in MCP upstream callback routing. Rollback is to revert PR #118 or specifically revert `e2559d7b` plus earlier callback commits if post-merge behavior differs from expected host routing. Destructive callbacks are intentionally conservative and can be loosened later only with a confirmed safe confirmation channel.

## Decisions Not Taken

- Did not expose sibling tools in `tools/list`; that would undo code mode's collapsed model-facing surface.
- Did not allow destructive widget callbacks through elicitation; widget-originated callbacks do not provide the same explicit confirmation path as `execute`.
- Did not move the superpowers plan into a new `complete/` directory because no matching convention was observed for `docs/superpowers/plans`.

## References

- PR #118: https://github.com/jmagar/lab/pull/118
- Downstream mitigation PR from the original issue: https://github.com/jmagar/ytdl-mcp/pull/4
- Plan: `docs/superpowers/plans/2026-06-12-code-mode-mcp-app-callbacks.md`
- Code mode docs: `docs/dev/CODE_MODE.md`
- MCP surface docs: `docs/surfaces/MCP.md`

## Open Questions

- External CodeRabbit/Codex bot review was rate-limited on the final pushed state, so there is no final third-party bot pass to summarize.
- CI status was not fetched after the final push in this session artifact; local verification passed all required Rust gates.

## Next Steps

1. Watch PR #118 CI on GitHub and merge when checks are green.
2. If desired after CodeRabbit limits reset, trigger `@coderabbitai review` for one more external pass.
3. After merge, validate a live `ytdl-mcp` panel in code mode: `youtube_search_ui` should render, safe Probe/Audio/Video sibling actions should dispatch, and destructive callbacks should be rejected unless run through `execute` with confirmation.
