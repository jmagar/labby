---
date: 2026-06-11 00:11:48 EST
repo: git@github.com:jmagar/lab.git
branch: feat/setup-wizard-consolidation
head: 47fab213
plan: docs/superpowers/plans/2026-06-10-setup-wizard-consolidation.md
working directory: /home/jmagar/workspace/lab/.worktrees/setup-wizard
worktree: /home/jmagar/workspace/lab/.worktrees/setup-wizard
pr: 112 — feat(setup): first-run self-bootstrap + token generation (setup-wizard consolidation) — https://github.com/jmagar/lab/pull/112
beads: No bead activity observed
---

# Setup-wizard consolidation — first-run self-bootstrap

## User Request

`/work-it docs/superpowers/plans/2026-06-10-setup-wizard-consolidation.md` — execute the setup-wizard consolidation plan via the `vibin:work-it` workflow: implement in an isolated `.worktrees/` checkout, create a PR immediately, run review waves (lavra-review, three code_simplifier passes, full pr-review-toolkit), resolve all PR comments, save-to-md, and final publish.

## Session Overview

Implemented first-run self-bootstrap for `labby serve`: when no MCP token is configured, OAuth is inactive, and the bind is loopback, the server now generates a 64-char hex bearer token, writes a minimal `~/.lab/.env` (mode 0600), reloads it into the process env, and prints the `/setup` URL — closing the headless "bootstrap circularity" so the web `/setup` wizard is reachable as a single config surface. Work was done by a dispatched implementation agent in the worktree, a PR was opened, review waves ran, and the two CodeRabbit findings (token-to-stderr leak; success log level) were fixed and the review thread resolved. Worktree is green (clippy 0 warnings; 46 targeted bootstrap/token/auth tests pass).

## Sequence of Events

1. **Plan executed by implementation agent** — `superpowers:executing-plans` inside the worktree produced the `token.rs`, `bootstrap.rs`, dispatch wiring, serve.rs first-run block, docs, and catalogs.
2. **`forbid(unsafe_code)` blocker resolved** — the plan's `unsafe { std::env::set_var }` is impossible under the workspace-wide `unsafe_code = "forbid"`; reload via `dotenvy::from_path` (which encapsulates its own unsafe) plus making the in-process `bearer_token` authoritative.
3. **PR #112 created** and review waves run; review-wave HIGH findings (loopback gate defeat; reload-failure starting unauthenticated) folded in via the typed `BootstrapOutcome` refactor.
4. **CodeRabbit review fixed** — stopped printing the raw token to stderr (print 0600 `.env` path + grep hint instead); changed the success log from WARN to INFO; removed now-dead `let _ = &token;`.
5. **Verified, committed (47fab213), pushed**; replied to and resolved the CodeRabbit review thread; confirmed 0 unresolved actionable threads; CI re-run on the new commit.

## Key Findings

- `crates/lab/src/cli/serve.rs:511-554` — first-run block is gated by `should_bootstrap(token_configured, oauth) && is_loopback_host(&host)`; the lab-319g non-loopback-no-auth bail at `serve.rs:561` is preserved downstream.
- `crates/lab/src/dispatch/setup/token.rs` — `generate_mcp_token()` uses `getrandom::fill` + `hex` (64 hex chars), fail-closed `.expect()` on RNG.
- `crates/lab/src/dispatch/setup/bootstrap.rs` — `bootstrap_at(&Path)` returns typed `BootstrapOutcome::{Created{env_path,token}, AlreadyPresent{env_path}}`; reuses `env_merge::merge` (owns `create_dir_all` + 0600 + atomic write) and `map_merge_err`.
- `node/master_client.rs:191` and `cli/logs.rs:227` read `LAB_MCP_HTTP_TOKEN` from env independently — this is why the `dotenvy` reload after write is required, not optional.

## Technical Decisions

- **Reload over `set_var`**: workspace forbids unsafe; `dotenvy::from_path` reloads the generated `.env` without `lab` itself invoking unsafe, and does not override already-set vars.
- **In-process token authoritative**: `bearer_token = Some(token.clone())` is set before the reload so a reload failure (logged at ERROR) cannot start the server unauthenticated.
- **Never print the secret**: stderr is commonly captured by systemd/journald/Docker; print the 0600 `.env` path + `grep` hint so the secret is only ever in the 0600 file and in-process.

## Files Changed

| status | path | purpose | evidence |
|---|---|---|---|
| modified | crates/lab/src/cli/serve.rs | first-run bootstrap block; token-not-to-stderr; INFO log | commits 285d9a37, 19da4b55, 47fab213 |
| created | crates/lab/src/dispatch/setup/token.rs | `generate_mcp_token()` + tests | commit 8fc16f4f |
| created | crates/lab/src/dispatch/setup/bootstrap.rs | `BootstrapOutcome`, `bootstrap[_at]`, `bootstrap_action`, `should_bootstrap` + tests | commits 8fc16f4f, 19da4b55 |
| modified | crates/lab/src/dispatch/setup.rs | mod decls + re-exports | commit 8fc16f4f |
| modified | crates/lab/src/dispatch/setup/catalog.rs | `bootstrap` ActionSpec | commit 8fc16f4f |
| modified | crates/lab/src/dispatch/setup/dispatch.rs | `"bootstrap" => bootstrap_action()`; `map_merge_err` pub(super) | commit 8fc16f4f |
| modified | crates/lab/Cargo.toml + Cargo.lock | add `getrandom` | commit 8fc16f4f |
| modified | crates/lab/tests/architecture_orchestrator.rs | allowlist cli/serve.rs to import dispatch::setup | commit 285d9a37 |
| modified | CHANGELOG.md, docs/runtime/CONFIG.md, docs/generated/{action-catalog,mcp-help}.{json,md} | document first-run; regenerate catalogs | commit a0103741 |
| modified | docs/superpowers/plans/2026-06-10-setup-wizard-consolidation.md | Task 3 updated for loopback gate + typed outcome + reload | commit a41c9714 |
| created | docs/sessions/2026-06-11-setup-wizard-consolidation.md | this session note | this commit |

## Beads Activity

No bead activity observed. `bd list --status=open` showed no open issues matching setup/wizard/bootstrap. The work was executed directly from the plan file under the work-it workflow.

## Repository Maintenance

- **Plans**: The executed plan lives under `docs/superpowers/plans/` (superpowers tree), not `docs/plans/`; superpowers plans are not moved to `docs/plans/complete/`. Left in place — it is the executed plan referenced by PR #112.
- **`docs/plans/` candidates**: `fleet-ws-plan-lab-n07n.md` and `mcp-streamable-http-oauth-proxy.md` were NOT moved — the latter is being archived on branch `feat/codemode-mcp-ui-passthrough` ("docs: archive completed mcp-streamable-http-oauth-proxy plan"); both are owned by other branches/sessions. Out of scope.
- **Beads**: read `bd list --status=open`; no relevant beads to create/close — no-op with evidence above.
- **Worktrees/branches**: `git worktree list` shows other active worktrees (objective-ardinghelli, protected-mcp-route-gateway-subsets, settings-page-revamp) owned by other sessions — left untouched. This worktree (`setup-wizard`) stays until PR #112 merges.
- **Stale docs**: `docs/runtime/CONFIG.md` updated with the first-run section as part of the implementation; no further stale-doc drift observed.

## Tools and Skills Used

- **Shell (Bash)**: git status/commit/push, `cargo fmt`/`clippy`/`nextest`, `gh` PR + GraphQL review-thread queries/mutations, `bd list`.
- **Skills**: `vibin:work-it` (coordinator workflow), `vibin:save-to-md` (this note). `superpowers:executing-plans` used by the implementation agent.
- **Agents**: one implementation agent (plan execution), parallel review-wave agents (lavra-review, 3× code_simplifier, pr-review-toolkit) — all findings fixed.
- **GitHub**: `gh` CLI + GraphQL API for review-thread resolution. No failures or degraded behavior observed in this segment.

## Commands Executed

| command | result |
|---|---|
| `cargo fmt --all && cargo clippy --workspace --all-features -- -D warnings` | clean (0 warnings) |
| `cargo nextest run -p labby --all-features -E 'test(bootstrap) or test(token) or test(should_bootstrap)'` | 46 passed, 0 failed |
| `git commit … && git push` | pushed 47fab213 to origin/feat/setup-wizard-consolidation |
| `gh api graphql resolveReviewThread` | thread PRRT_kwDOR8nC1M6Ir7M9 isResolved=true |

## Errors Encountered

- **`forbid(unsafe_code)` vs plan's `set_var`**: root cause — eng-review checked main.rs/lib.rs not `[workspace.lints]`. Resolved by `dotenvy::from_path` reload + authoritative in-process token (no unsafe).
- **Review-wave HIGH-1/HIGH-2**: bootstrap could defeat the loopback safety gate / a reload failure could start the server unauthenticated. Resolved via loopback gating and authoritative-token + ERROR-on-reload-fail.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| `labby serve` first run (loopback, no token, no OAuth) | refused/required manual `~/.lab/.env` + token | generates token, writes 0600 `~/.lab/.env`, reloads it, prints `/setup` URL |
| first-run stderr | (n/a) | prints `.env` path + `grep` hint, never the raw token |
| non-loopback bind, no auth | bails (lab-319g) | unchanged — bails; bootstrap does not fire off-loopback |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo clippy --workspace --all-features -- -D warnings` | no warnings | finished clean | pass |
| `cargo nextest run -p labby --all-features -E 'test(bootstrap)…'` | all pass | 46 passed, 0 failed | pass |
| `gh pr view 112` mergeable | MERGEABLE | MERGEABLE | pass |
| PR #112 unresolved review threads | 0 | 0 | pass |

## Risks and Rollback

- Risk: a future non-loopback default or refactor that drops the `is_loopback_host` gate would let bootstrap silently mint a token on a LAN-bound server. Mitigated by the comment block at `serve.rs:499-510` and the preserved lab-319g bail. Rollback: revert PR #112 commits; serve reverts to requiring a pre-existing token/OAuth.

## References

- PR #112 — https://github.com/jmagar/lab/pull/112
- Plan — docs/superpowers/plans/2026-06-10-setup-wizard-consolidation.md
- CodeRabbit thread reply — https://github.com/jmagar/lab/pull/112#discussion_r3393197243

## Next Steps

1. Wait for PR #112 CI to go fully green — especially `Test (windows self-hosted)` (this is a same-repo PR, so the gated Windows job runs).
2. Merge PR #112 once green.
3. Follow-on (not this PR): switch the web `/setup` wizard UI to drive the full setup surface now that the headless bootstrap circularity is closed.
