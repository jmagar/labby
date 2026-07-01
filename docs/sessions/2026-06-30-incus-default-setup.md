---
date: 2026-06-30 22:24:19 EDT
repo: git@github.com:jmagar/lab.git
branch: codex/incus-default-setup
head: 82144a91
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
beads: lab-en26c
---

# Incus default setup and public installer

## User Request

Make `labby setup` the low-friction Incus bootstrap path, keep the web setup wizard as a later explicit step, publish the branch, review it, and merge once checks are green.

## Session Overview

This session moved the first-run operator flow toward `curl https://labby.tootie.tv/install.sh | sh` followed by bare `labby setup`. The old web setup behavior remains available as `labby setup wizard`, while the explicit `labby setup incus` subcommand keeps advanced bootstrap flags.

## Sequence of Events

1. Reviewed the dirty tree containing Incus bootstrap, public installer, static asset, and documentation changes.
2. Changed `labby setup` so bare setup invokes the Incus bootstrap with `latest` by default.
3. Added `labby setup wizard` for the existing web setup flow and hid legacy wizard flags from top-level setup help.
4. Updated docs, generated CLI help, plugin docs, and installer messaging to point users at `labby setup`.
5. Bumped the workspace and gateway-admin package version from `0.28.0` to `0.29.0`.
6. Ran focused local verification before preparing the branch for PR review.

## Key Findings

- `labby setup --help` still described the command as the web wizard after the behavior change; this was fixed in `crates/labby/src/cli.rs`.
- The old skip message still told users to rerun bare `labby setup`; it now points to `labby setup wizard` in `crates/labby/src/cli/setup.rs`.
- `docs/runtime/INCUS.md` already contained the right Incus operating model but needed to promote bare `labby setup` as the supported operator entry point.
- The public installer path needs the generated Next static export to carry `install.sh`, so `apps/gateway-admin/scripts/sync-install-script.mjs` and `apps/gateway-admin/public/install.sh` are part of the branch.

## Technical Decisions

- Bare `labby setup` now defaults to the Incus bootstrap because the primary supported deployment method is the Incus gateway container.
- `labby setup incus` remains for advanced flags such as `--local-binary`, `--skip-install`, storage overrides, and pinned versions.
- Top-level legacy wizard flags are hidden rather than removed so existing smoke paths keep parsing while new users see the Incus-first help.
- The web wizard remains explicit as `labby setup wizard` instead of being removed.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `Cargo.toml` | - | bump workspace version to `0.29.0` | `cargo check --workspace --all-features` compiled workspace crates as `0.29.0` |
| modified | `Cargo.lock` | - | sync package versions after the Cargo bump | `cargo check --workspace --all-features` passed |
| modified | `apps/gateway-admin/package.json` | - | bump app package version and add install-script sync hook | dirty tree and package manifest diff |
| created | `apps/gateway-admin/scripts/sync-install-script.mjs` | - | sync repo installer into the Next public directory | dirty tree includes untracked script directory |
| created | `apps/gateway-admin/scripts/sync-install-script.test.mjs` | - | test install-script sync behavior | dirty tree includes untracked script directory |
| created | `apps/gateway-admin/public/install.sh` | - | serve installer from the Next app export | dirty tree includes untracked public installer |
| modified | `crates/labby/src/cli.rs` | - | update `setup` command summary to Incus-first | `labby setup --help` prints the new summary |
| modified | `crates/labby/src/cli/setup.rs` | - | make bare setup run Incus and add `setup wizard` | setup tests and command smokes passed |
| modified | `crates/labby/src/dispatch/setup/incus.rs` | - | support binary-owned Incus bootstrap artifacts and options | setup tests passed |
| modified | `crates/labby-web/src/*` | - | serve `.sh` assets with proper file behavior/content type | labby-web tests were run during the implementation work |
| modified | `docs/generated/cli-help.md` | - | regenerate CLI help for new setup surface | `just docs-check` reported 15 fresh artifacts |
| modified | `docs/runtime/INCUS.md` | - | document bare `labby setup` as supported operator entry point | updated Incus runtime docs |
| modified | `plugins/labby/*` | - | default plugin docs/config to `labby.tootie.tv` and `labby setup` | plugin README and manifest diffs |
| modified | `scripts/incus-bootstrap.sh` | - | align shell bootstrap with the binary-owned flow | dirty tree includes bootstrap script changes |
| modified | `scripts/install.sh` | - | advertise the new first-run command | installer header now points at `labby setup` |
| modified | `CHANGELOG.md` | - | document the `0.29.0` feature release | new `0.29.0` section |

## Beads Activity

| bead | title | action | final status | why it mattered |
|---|---|---|---|---|
| lab-en26c | Validate one-line install flow | referenced as the active relevant work item | in_progress | This branch directly supports the one-line install/setup path |

## Repository Maintenance

- Plans: no plan files were moved during quick-push; moving or pruning plans is out of scope for this constrained save step.
- Beads: read current in-progress beads and identified `lab-en26c` as the relevant active task; no bead state was changed.
- Worktrees and branches: inspected worktrees and branches. The current worktree is `codex/incus-default-setup`; `marketplace-no-mcp` is a separate long-lived worktree and was left untouched.
- Stale docs: updated `docs/runtime/INCUS.md` and regenerated `docs/generated/cli-help.md` because they were directly contradicted by the new setup behavior.
- Transparency: no destructive cleanup was performed.

## Tools and Skills Used

- Shell and git commands: inspected status, branch, version fields, changelog, worktrees, and verification output.
- `apply_patch`: edited Rust, docs, manifests, and this session artifact.
- `vibin:quick-push`: used for the branch, version bump, session save, commit, and push workflow.
- `lavra:lavra-review`: loaded for the upcoming PR review phase; review has not run yet in this saved pre-commit context.
- Lumen semantic search: requested by developer instruction, but `tool_search` returned no callable `mcp__lumen__semantic_search` tool in this thread.

## Commands Executed

| command | result |
|---|---|
| `git switch -c codex/incus-default-setup` | created the feature branch from `main` |
| `cargo check --workspace --all-features` | passed; workspace crates compiled as `0.29.0` |
| `git grep -n -F '0.28.0' -- '*.toml' '*.json' '*.md' '*.yml' '*.yaml'` | only historical/reference/example hits remained |
| `just docs-generate` | generated 15 docs artifacts |
| `just docs-check` | checked 15 docs artifacts as fresh |
| `cargo test -p labby --all-features setup -- --nocapture` | passed 105 setup-related tests plus filtered integration checks |
| `cargo fmt --all --check` | passed |
| `cargo run -p labby --all-features -- setup --dry-run` | printed Incus bootstrap commands for bare setup |
| `cargo run -p labby --all-features -- setup wizard --no-setup` | printed the explicit wizard rerun message |

## Errors Encountered

- A shell search pattern containing backticks accidentally invoked `labby setup` through command substitution. The spawned `incus profile edit labby-gateway` process was killed, and a read-only `incus profile show labby-gateway` check showed the expected existing profile state.
- `mcp__lumen__semantic_search` was requested by developer instruction but is not exposed in this thread; `tool_search` found no matching tool.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| First-run CLI | `labby setup` opened or described the web wizard | `labby setup` bootstraps the Incus Labby gateway container |
| Web wizard | implicit top-level setup behavior | explicit `labby setup wizard` subcommand |
| Release selection | users could be pushed toward a pinned version | default Incus bootstrap uses `latest`; pinning is optional via `labby setup incus --version` |
| Public installer | install script lived primarily in repo checkout context | Next app can serve `/install.sh` from the generated public artifact |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo check --workspace --all-features` | workspace compiles after version bump | finished successfully | pass |
| `cargo test -p labby --all-features setup -- --nocapture` | setup CLI/dispatch tests pass | 105 setup tests passed | pass |
| `just docs-check` | generated CLI docs are fresh | checked 15 artifacts as fresh | pass |
| `cargo run -p labby --all-features -- setup --dry-run` | bare setup prints Incus bootstrap plan | printed Incus storage/profile/launch/provision/Tailscale plan | pass |
| `cargo run -p labby --all-features -- setup wizard --no-setup` | wizard remains explicit | printed `labby setup wizard` rerun guidance | pass |

## Risks and Rollback

- Risk: changing bare `labby setup` is a user-visible CLI behavior change. Rollback is to restore bare setup to the wizard and keep Incus under `labby setup incus`.
- Risk: `/install.sh` serving relies on Next public artifact sync. Rollback is to remove the public script copy/hook and serve the installer only from GitHub raw release paths.

## Decisions Not Taken

- Did not remove legacy top-level wizard flags; they remain hidden for compatibility.
- Did not make users provide `--version`; defaulting to `latest` keeps the install path low-friction.
- Did not make the web wizard the first step; it remains later/explicit because container deployment is now the supported default.

## References

- `docs/runtime/INCUS.md`
- `scripts/install.sh`
- `config/incus/labby-gateway-profile.yaml`
- `config/incus/labby-backup.yaml`

## Open Questions

- PR review and external review-toolkit agents still need to run after the implementation commit and PR creation.
- CI status is not known yet for this branch.
- A fresh release-artifact container proof still needs to be run after CI/release artifacts are available.

## Next Steps

1. Commit and push the implementation changes on `codex/incus-default-setup`.
2. Create a PR and run Lavra review against it.
3. Address all review findings and push fixes.
4. Dispatch the PR review toolkit agents and address their findings.
5. Wait for CI to pass, then merge the PR into `main`.
