---
date: 2026-05-23 21:49:10 EDT
repo: git@github.com:jmagar/lab.git
branch: fix/gateway-oauth-tool-gating
head: 4e0570c5
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
---

# Gateway OAuth Tool Gating Quick Push

## User Request

The user asked to debug why Lab kept gating tools despite using the admin email user, then asked for Lavra design/research/review, and finally asked for `quick-push`.

## Session Overview

Implemented and pushed a branch that routes admin and trusted MCP upstream OAuth calls through the shared gateway credential subject, refreshed related gateway docs/generated catalogs/skill references, bumped the release surfaces to `0.17.2`, and pushed `fix/gateway-oauth-tool-gating`.

## Sequence of Events

1. Diagnosed the live gating symptom as an upstream OAuth subject mismatch: credentials existed under shared subject `gateway`, while MCP request paths were looking under request subjects such as `static-bearer`.
2. Designed the fix around separating actor identity from upstream OAuth credential-subject routing.
3. Implemented `oauth_upstream_subject_for_request` in `crates/lab/src/mcp/server.rs` and wired it through subject-scoped upstream tools, prompts, and resource discovery/read paths.
4. Ran focused MCP server tests and later ran `cargo check` during quick-push.
5. Created branch `fix/gateway-oauth-tool-gating`, bumped versions from `0.17.1` to `0.17.2`, updated `CHANGELOG.md`, committed, and pushed.

## Key Findings

- Admin scope mint/refresh was not the root problem; current tokens already carried `lab:admin`.
- Existing upstream OAuth credentials were stored under shared gateway subject `gateway`, matching the documented operator model.
- MCP upstream subject-scoped paths were using request subjects, causing admin/static-bearer calls to miss shared credentials.
- Lavra review found one remaining risk: `read_resource` for `lab://upstream/...` can return through the generic upstream branch before the subject-scoped OAuth branch is considered.

## Technical Decisions

- Admin callers and trusted callers use the shared gateway subject for upstream OAuth credential lookup.
- Non-admin callers keep their own request subject, preserving per-user isolation.
- Request subject remains the actor/logging identity; `oauth_subject` is logged separately when routing subject-scoped upstream calls.
- The changelog uses the existing `*(this)*` convention for the current commit row because a commit cannot contain its own stable hash.

## Files Changed

| status | path | purpose | evidence |
|---|---|---|---|
| modified | `CHANGELOG.md` | add `0.17.2` release notes | `git show --name-status HEAD` |
| modified | `Cargo.toml` | bump workspace version to `0.17.2` | `cargo check` updated workspace package versions |
| modified | `Cargo.lock` | sync Rust package versions | `cargo check` |
| modified | `apps/gateway-admin/package.json` | bump gateway admin package to `0.17.2` | version-sync grep |
| modified | `crates/lab/src/mcp/server.rs` | route admin/trusted OAuth upstream calls through gateway subject | focused MCP tests |
| modified | `crates/lab/src/dispatch/gateway/{catalog.rs,dispatch.rs,manager.rs}` | gateway catalog/dispatch/manager updates included in pushed worktree | `git show --name-status HEAD` |
| modified | `crates/lab/src/cli.rs`, `crates/lab/src/cli/gateway.rs`, `crates/lab/src/docs/render.rs` | CLI/docs rendering updates included in pushed worktree | `git show --name-status HEAD` |
| modified | `apps/gateway-admin/lib/**/*.ts` | gateway admin OAuth adapter/client test updates | `git show --name-status HEAD` |
| modified | `docs/generated/*` | refreshed generated catalog/API/MCP docs | `git show --name-status HEAD` |
| modified | `docs/runtime/CONFIG.md`, `docs/services/GATEWAY.md` | document shared gateway OAuth subject behavior | `git show --name-status HEAD` |
| modified | `plugins/lab/skills/using-lab-cli/**` | refreshed Lab CLI skill references and added OpenAI agent config | `git show --name-status HEAD` |
| created | `docs/sessions/2026-05-23-beads-full-audit.md` | session/audit artifact already present in worktree | `git show --name-status HEAD` |
| created | `docs/superpowers/plans/2026-05-23-lab-cli-surface-completion.md` | plan artifact already present in worktree | `git show --name-status HEAD` |
| created | `docs/superpowers/plans/2026-05-23-upstream-proxy-hardening.md` | plan artifact already present in worktree | `git show --name-status HEAD` |

## Beads Activity

No bead changes were made in this quick-push turn. `bd list --all --sort updated --reverse --limit 20 --json` was run for session context; it returned historical closed issues, not current session edits.

## Repository Maintenance

- Plans: `find docs/plans -maxdepth 2 -type f` found `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md`; neither was moved because neither was proven completed in this session.
- Beads: read-only bead inspection only; no bead state changes were made.
- Worktrees/branches: `git worktree list --porcelain` showed active worktrees for `fix/lab-cli-surface-completion` and `fix/upstream-proxy-hardening`; no cleanup was attempted because they are active registered worktrees.
- Stale docs: docs and generated catalogs included in the pushed commit were treated as part of the worktree being pushed; no additional stale-doc sweep was attempted after push.
- PR: `gh pr view --json number,title,url` returned `none`; no PR existed at save time.

## Tools and Skills Used

- Skills: `systematic-debugging`, `lavra-plan`, `lavra-research`, `lavra-review`, `quick-push`, and `save-to-md`.
- Shell/Git: used `git status`, `git diff`, `git switch`, `git add`, `git commit`, `git push`, `cargo check`, `git grep`, and repository inspection commands.
- MCP/Lab: used Lab MCP scout during diagnosis to confirm the gateway search surface still worked.
- Web: used browser search during Lavra research for MCP authorization and OAuth resource-indicator references.
- File editing: used `apply_patch` for manual edits.

## Commands Executed

| command | result |
|---|---|
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features mcp::server::tests::oauth_upstream_subject` | passed |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features mcp::server::tests::` | passed |
| `cargo fmt --all --check` | passed after formatting |
| `cargo check` | passed with existing unused/dead-code warnings in the `fs` surface |
| `git push -u origin fix/gateway-oauth-tool-gating` | pushed branch and set upstream |

## Errors Encountered

- An initial Cargo test invocation used multiple filters; Cargo accepts only one test filter. It was rerun with a valid module filter.
- Changelog self-hash insertion was attempted after commit, but each amend changed the commit hash. The changelog was returned to the repo's existing `*(this)*` convention.

## Behavior Changes (Before/After)

- Before: admin/static-bearer MCP upstream OAuth paths could look for credentials under the request subject and fail even when shared gateway credentials existed.
- After: admin and trusted callers resolve upstream OAuth credentials through the shared `gateway` subject, while non-admin callers remain subject-scoped.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo fmt --all --check` | formatting clean | passed after `cargo fmt --all` | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features mcp::server::tests::oauth_upstream_subject` | resolver tests pass | passed | pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features mcp::server::tests::` | MCP server tests pass | passed | pass |
| `cargo check` | workspace check succeeds | passed with existing warnings | pass |
| `git grep -F "0.17.1" -- '*.toml' '*.json' '*.md' '*.yml' '*.yaml'` | no current version fields remain at `0.17.1` | only changelog/history/reference hits remained | pass |

## Risks and Rollback

- Risk: Lavra review identified that subject-scoped resource reads may still be shadowed by the generic `lab://upstream/...` resource branch.
- Risk: full workspace `just test` / `cargo nextest run --workspace --all-features` was not run.
- Rollback: revert commit `4e0570c5` from branch `fix/gateway-oauth-tool-gating`, or reset the branch before merge.

## Decisions Not Taken

- Did not force-push or delete any existing branches/worktrees.
- Did not move plan files because completion was not established from current-session evidence.
- Did not run full all-features nextest because quick-push verification used `cargo check` plus previously run focused MCP tests.

## References

- MCP Authorization specification pages consulted during research.
- OAuth 2.0 Resource Indicators RFC 8707 consulted during research.
- Repo docs: `docs/services/UPSTREAM.md`, `docs/services/GATEWAY.md`, and `docs/runtime/CONFIG.md`.

## Open Questions

- Should subject-scoped OAuth resource reads be moved ahead of the generic upstream resource branch, or should generic resource routing become OAuth-subject aware?

## Next Steps

1. Open a PR from `fix/gateway-oauth-tool-gating`.
2. Fix or explicitly defer the subject-scoped resource-read routing issue found during review.
3. Run the full all-features test path before merge.
