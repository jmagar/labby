---
date: 2026-05-26 17:58:09 EST
repo: git@github.com:jmagar/lab.git
branch: feat/workspace-runtime-builder
head: 05020b2b
plan: docs/superpowers/plans/2026-05-26-lab-workspace-runtime-builder.md
working directory: /home/jmagar/workspace/lab/.worktrees/workspace-runtime-builder
worktree: /home/jmagar/workspace/lab/.worktrees/workspace-runtime-builder
pr: "#76 Extract workspace runtime builder seam https://github.com/jmagar/lab/pull/76"
---

# Workspace runtime builder session

## User Request

Read the crate-extraction docs, decide where to start, write a plan, then work it through end to end.

## Session Overview

Implemented the first small runtime-builder slice for the workspace filesystem service. The branch adds a `workspace` module, a surface-neutral `WorkspaceRuntimeBuilder`, registry helper cleanup, route-mount policy centralization, and a plan artifact updated after review.

## Sequence of Events

1. Read `docs/crate-extract/` and related ADRs to understand the extraction direction.
2. Selected the workspace filesystem service as the smallest practical runtime-builder candidate.
3. Wrote the implementation plan in `docs/superpowers/plans/2026-05-26-lab-workspace-runtime-builder.md`.
4. Created worktree `/home/jmagar/workspace/lab/.worktrees/workspace-runtime-builder` on `feat/workspace-runtime-builder`.
5. Implemented the builder, registry constructor, CLI startup wiring, and HTTP route policy wiring.
6. Ran review agents and addressed the architecture finding by making the runtime surface-neutral.
7. Fixed a reviewer-identified regression where `workspace.root = "~/..."` no longer expanded.
8. Reran focused checks and full nextest.

## Key Findings

- `fs` was a good first slice because its runtime state is small: workspace root resolution plus API mount policy.
- The initial runtime design imported MCP/API adapters, which contradicted the extraction boundary. The final runtime owns state and policy only.
- Existing `workspace.root` behavior expands `~` and `~/...`; preserving that required adding `WorkspaceRuntimeConfig.home`.
- MCP catalog registration still belongs in `registry.rs` because it is a surface adapter concern.

## Technical Decisions

- `WorkspaceRuntimeConfig` takes `root` and `home` instead of the full `LabConfig`, keeping the runtime builder easier to extract later.
- `WorkspaceRuntime` stores `Result<PathBuf, String>` so startup can attach the root on success and log the concrete error on failure.
- `/v1/fs` route creation remains in `api/router.rs`; `WorkspaceRuntime::should_mount_http_routes` only owns the boolean security policy.
- `RegisteredService::bootstrap_operator` replaced a more generic `bootstrap` name to match the exact service kind it constructs.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `crates/lab/src/api/router.rs` | - | Delegate `/v1/fs` auth-disabled mount policy to workspace runtime. | `WorkspaceRuntime::should_mount_http_routes(...)` |
| modified | `crates/lab/src/cli/serve.rs` | - | Resolve and attach workspace root via `WorkspaceRuntimeBuilder`. | `WorkspaceRuntimeConfig { root, home }` |
| modified | `crates/lab/src/dispatch/fs.rs` | - | Remove now-unused public re-export. | `resolve_workspace_root` remains internal to fs client use |
| modified | `crates/lab/src/lib.rs` | - | Expose feature-gated workspace module. | `#[cfg(feature = "fs")] pub mod workspace;` |
| modified | `crates/lab/src/main.rs` | - | Add binary-side workspace module declaration. | `#[cfg(feature = "fs")] mod workspace;` |
| modified | `crates/lab/src/registry.rs` | - | Add `bootstrap_operator` and preserve MCP-filtered fs registration. | `default_registry_uses_mcp_filtered_fs_actions` |
| created | `crates/lab/src/workspace.rs` | - | Workspace runtime module entry point. | Re-exports runtime types |
| created | `crates/lab/src/workspace/runtime.rs` | - | Surface-neutral runtime builder and tests. | `WorkspaceRuntimeBuilder`, `WorkspaceRuntimeConfig` |
| created | `docs/superpowers/plans/2026-05-26-lab-workspace-runtime-builder.md` | - | Implementation plan and acceptance criteria. | Updated after architecture review |

## Beads Activity

No bead activity observed for this specific workspace-runtime task. `bd list --all --sort updated --reverse --limit 50 --json` returned existing historical closed items, but none were directly tied to PR #76.

## Repository Maintenance

- Plans: inspected `docs/plans` and `docs/superpowers/plans`. No plan files were moved because the repository has many historical plans and only this branch's plan was clearly in scope.
- Beads: inspected recent bead state; no directly relevant bead was found or changed.
- Worktrees and branches: inspected worktrees and branches. Left `/home/jmagar/workspace/lab` on `main` and the active PR worktree on `feat/workspace-runtime-builder`; no stale worktree was safe to remove.
- Stale docs: updated the active plan artifact to remove stale `http_routes` and `registered_service` design text after review.
- PR state: PR #76 existed with prior CI checks passing on the older remote commit and one CodeRabbit review comment against stale plan text; the local plan rewrite addressed that stale snippet before push.

## Tools and Skills Used

- Skills: `superpowers:writing-plans`, `work-it`, `superpowers:executing-plans`, and `save-to-md`.
- Shell commands: `git`, `cargo`, `cargo nextest`, `gh`, `rg`, `ps`, `kill`, `find`, and `bd`.
- File tools: `apply_patch` for edits and normal shell reads for inspection.
- Subagents: architecture and simplification reviewers, plus PR review agents. One review caught the `~` expansion regression.
- External services: GitHub CLI for PR metadata and check state.

## Commands Executed

| command | result |
|---|---|
| `cargo check -p labby --all-features` | Passed after local web assets were available. |
| `cargo test -p labby workspace --all-features` | Passed. |
| `cargo test -p labby registry --all-features` | Passed. |
| `cargo test -p labby router --all-features` | Passed. |
| `cargo fmt --all --check` | Passed. |
| `git diff --check` | Passed. |
| `cargo check --workspace --all-features` | Passed. |
| `cargo nextest run --workspace --all-features` | Passed: 1549 passed, 25 skipped. |

## Errors Encountered

- Used `cargo check -p lab --all-features` initially; package name is `labby`. The plan was corrected.
- `include_dir!` needed `apps/gateway-admin/out` in the worktree for all-features checks; copied the existing ignored asset output locally.
- A verification chain held Cargo locks after interruption; stale worktree Cargo processes were killed.
- Review found `~` expansion was lost in the refactor; fixed with `expand_home_path` and a regression test.
- Warnings surfaced for an unnecessary `PathBuf` qualification and an unused `resolve_workspace_root` re-export; both were removed.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| workspace startup | `cli::serve` called `dispatch::fs::resolve_workspace_root` directly. | `cli::serve` builds `WorkspaceRuntime` and attaches the resolved root. |
| route mount policy | Router owned the raw disabled-auth boolean condition. | Router asks `WorkspaceRuntime::should_mount_http_routes`. |
| registry helper | `fs` used inline `RegisteredService` construction. | `fs` uses `RegisteredService::bootstrap_operator`. |
| configured tilde roots | Existing config resolver expanded `~`. | Runtime builder preserves `~` expansion with explicit `home`. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo test -p labby workspace --all-features` | workspace runtime tests pass | passed | pass |
| `cargo test -p labby registry --all-features` | registry tests pass | passed | pass |
| `cargo test -p labby router --all-features` | router tests pass | passed | pass |
| `cargo fmt --all --check && git diff --check` | no formatting or whitespace issues | passed | pass |
| `cargo check --workspace --all-features` | workspace checks clean | passed | pass |
| `cargo nextest run --workspace --all-features` | full suite clean | 1549 passed, 25 skipped | pass |

## Risks and Rollback

Risk is concentrated around workspace-root startup behavior and `/v1/fs` mount gating. Rollback path is to revert PR #76 or specifically revert the workspace runtime wiring commits; `fs` dispatch logic itself was not moved.

## Decisions Not Taken

- Did not extract a separate crate yet; this slice intentionally proves the runtime-builder shape inside `lab`.
- Did not move MCP/API adapters into the runtime after review; surface ownership stays with registry/router.
- Did not broadly clean historical plan files because ownership and completion state were ambiguous.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/adr/`
- `docs/superpowers/plans/2026-05-26-lab-workspace-runtime-builder.md`
- PR #76: https://github.com/jmagar/lab/pull/76

## Open Questions

- Whether future runtime builders should keep `pub mod workspace` public in the library or narrow it to crate visibility once extraction scaffolding matures.

## Next Steps

- Push the session artifact and review-fix commit.
- Recheck PR #76 after CI reruns on `05020b2b` plus this session-note commit.
- Merge once checks are green and no current review comments remain actionable.
