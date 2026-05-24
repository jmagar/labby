---
date: 2026-05-24 16:56:26 EDT
repo: git@github.com:jmagar/lab.git
branch: fix/gateway-oauth-tool-gating
head: f97cf13a
session id: 019e584a-5353-7d50-84e1-52f357e4f744
transcript: /home/jmagar/.codex/sessions/2026/05/24/rollout-2026-05-24T00-42-06-019e584a-5353-7d50-84e1-52f357e4f744.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
---

# Stdio MCP / HTTP MCP Gateway Parity Quick Push

## User Request

Investigate and resolve the issue where the stdio MCP server did not expose the same gateway feature surface as the HTTP MCP server, then quick-push the completed fix.

## Session Overview

Found that `stdio_mode` was disabling too much startup wiring in `crates/lab/src/cli/serve.rs`: upstream OAuth runtime, upstream discovery, global gateway manager install, and auto-import. Changed startup so normal `labby mcp` follows the same gateway runtime path as HTTP MCP, while preserving upstream-spawn suppression only for recursive stdio children identified by `LAB_SPAWN_DEPTH > 0`. Bumped release metadata from `0.17.3` to `0.17.4`.

## Sequence of Events

1. Loaded the systematic-debugging workflow and inspected the Lab MCP startup path.
2. Compared the HTTP and stdio branches in `crates/lab/src/cli/serve.rs`.
3. Identified the root cause: stdio skipped gateway runtime setup unconditionally.
4. Patched stdio startup to suppress upstream runtime only when the recursion guard is active.
5. Added a focused regression test for the recursion guard behavior.
6. Verified formatting, focused tests, and all-features crate check.
7. Ran quick-push release preparation: version bump, changelog entry, and session capture.

## Key Findings

- `crates/lab/src/cli/serve.rs` used `stdio_mode` to skip upstream OAuth runtime, upstream discovery, and gateway manager installation.
- The MCP handler itself was shared, but stdio received a weaker gateway runtime state.
- The correct safety boundary is recursive stdio child detection, not stdio transport alone.
- `LAB_SPAWN_DEPTH > 0` remains the guard that prevents Lab from recursively spawning itself through a gateway upstream.

## Technical Decisions

- Preserve the recursion guard because upstream spawning from recursive Lab children can create process recursion.
- Install the gateway manager for stdio as well as HTTP so direct `gateway` actions use the same configured manager.
- Reuse the existing `resolve_lab_spawn_depth` helper and add `stdio_recursion_guard_active` for a small, testable decision point.
- Treat this as a patch release because the change fixes a transport parity bug.

## Files Changed

| status | path | previous path | purpose | evidence |
|--------|------|---------------|---------|----------|
| modified | `crates/lab/src/cli/serve.rs` | | Align normal stdio MCP gateway runtime setup with HTTP MCP while preserving recursive child suppression. | `cargo check --manifest-path crates/lab/Cargo.toml --all-features` passed. |
| modified | `Cargo.toml` | | Bump workspace version to `0.17.4`. | `cargo check` ran with Lab crates at `0.17.4`. |
| modified | `Cargo.lock` | | Record `lab-apis`, `lab-auth`, and `labby` package versions at `0.17.4`. | `cargo check` updated the lockfile. |
| modified | `apps/gateway-admin/package.json` | | Keep gateway admin package version in sync at `0.17.4`. | `git grep -F "0.17.3"` found no stale current package version field. |
| modified | `CHANGELOG.md` | | Add `0.17.4` release notes for stdio MCP gateway parity. | New `0.17.4` section added. |
| created | `docs/sessions/2026-05-24-stdio-mcp-http-parity-quick-push.md` | | Capture this debugging and quick-push session. | This file. |

## Beads Activity

No bead activity observed for this stdio MCP parity fix. `bd list --all --sort updated --reverse --limit 20 --json` was read during the save pass; the returned recent items were historical closed issues unrelated to this session.

## Repository Maintenance

- Plans: `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md` exist, but neither was clearly completed by this session, so no plan files were moved.
- Beads: read-only inspection only; no bead changes were made.
- Worktrees/branches: current worktree is `/home/jmagar/workspace/lab` on `fix/gateway-oauth-tool-gating`; another registered worktree at `.worktrees/code-mode-contract` owns `main`. No branch or worktree cleanup was performed during quick-push.
- Stale docs: `CHANGELOG.md` was updated because the release version changed. No broader docs were changed.
- Active PR: `gh pr view --json number,title,url` returned `none`.

## Tools and Skills Used

- Skills: `systematic-debugging`, `quick-push`, `save-to-md`.
- Shell commands: `rg`, `sed`, `git`, `cargo`, `bd`, `gh`, `date`, `find`, `ls`.
- File tools: `apply_patch` for code, changelog, version, and session-note edits.
- Memory: prior Lab notes were used to identify known stdio vs HTTP scout/catalog mismatch context and quick-push conventions.
- MCP/app tools: none used for this quick-push beyond local shell evidence.

## Commands Executed

- `cargo fmt --all --check`: passed.
- `cargo test --manifest-path crates/lab/Cargo.toml --all-features cli::serve::tests::stdio_recursion_guard_only_suppresses_child_spawns`: passed.
- `cargo test --manifest-path crates/lab/Cargo.toml --all-features cli::serve::tests::lab_spawn_depth_resolution_tolerates_bad_env`: passed.
- `cargo check --manifest-path crates/lab/Cargo.toml --all-features`: passed before and after the version bump.
- `git grep -F "0.17.3" -- '*.toml' '*.json' '*.md' '*.yml' '*.yaml'`: only found historical/changelog/session references, not stale current version fields.
- `git diff --check`: passed before quick-push staging.

## Errors Encountered

- Initial focused test invocation with `cargo test -p lab ... --all-features` failed because the package was outside the workspace invocation shape. Reran with `--manifest-path crates/lab/Cargo.toml --all-features`, which passed.
- Parallel focused test runs briefly contended on Cargo locks, then both completed successfully.

## Behavior Changes

| before | after |
|--------|-------|
| Normal `labby mcp` skipped upstream OAuth runtime, upstream discovery, and global gateway manager installation. | Normal `labby mcp` wires the same gateway manager/upstream discovery path as HTTP MCP. |
| Stdio MCP could have a weaker `scout`/`invoke`/gateway-resource surface than HTTP MCP. | Stdio MCP receives the same gateway runtime state unless it is a recursive Lab child. |
| All stdio startup suppressed upstream spawning. | Only `LAB_SPAWN_DEPTH > 0` stdio startup suppresses upstream spawning. |

## Verification Evidence

| command | expected | actual | status |
|---------|----------|--------|--------|
| `cargo fmt --all --check` | Formatting clean. | Passed. | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features cli::serve::tests::stdio_recursion_guard_only_suppresses_child_spawns` | Recursion guard behavior pinned. | Passed in lib and bin test targets. | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features cli::serve::tests::lab_spawn_depth_resolution_tolerates_bad_env` | Spawn-depth parsing remains tolerant. | Passed in lib and bin test targets. | pass |
| `cargo check --manifest-path crates/lab/Cargo.toml --all-features` | All-features Lab crate check clean. | Passed. | pass |
| `git diff --check` | No whitespace errors. | Passed. | pass |

## Risks and Rollback

- Risk: normal stdio MCP can now spawn configured upstreams, matching HTTP behavior but increasing startup work compared with the previous suppressed path.
- Mitigation: recursive child suppression remains active when `LAB_SPAWN_DEPTH > 0`.
- Rollback: revert the upcoming quick-push commit to restore the previous stdio suppression behavior and version metadata.

## Decisions Not Taken

- Did not make stdio serve the REST API or web UI. The fix targets HTTP MCP feature parity, not parity with the whole HTTP product server.
- Did not delete or move unrelated plan files because they were not clearly completed by this session.
- Did not perform branch/worktree cleanup during quick-push because the request was only to push the current fix.

## References

- `crates/lab/src/cli/serve.rs`
- `crates/lab/src/mcp/server.rs`
- `crates/lab/src/mcp/catalog.rs`
- `CHANGELOG.md`
- Prior Lab memory notes about stdio `scout` being weaker than HTTP MCP before this fix.

## Open Questions

- Live external contract parity with mcporter against stdio was not run during this quick-push pass; code-path parity and focused tests were verified locally.

## Next Steps

- Commit and push `fix/gateway-oauth-tool-gating`.
- After push, optionally run live mcporter checks against `labby mcp` and `labby-http` to compare `scout`, `invoke`, and gateway resource behavior against real configured upstreams.
