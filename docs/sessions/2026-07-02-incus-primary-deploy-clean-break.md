# Incus Primary Deploy Clean Break

## Metadata

- Date: 2026-07-02 00:29:42 EDT
- Repository: `git@github.com:jmagar/lab.git`
- Working directory: `/home/jmagar/workspace/lab`
- Branch: `codex/incus-primary-deploy-clean-break`
- Base commit: `ebff21f1`
- Release version prepared: `0.30.0`

## Request

Make Incus the primary supported Labby deployment path, remove the old `lab` naming and `~/.lab` runtime shape, migrate the live setup to the clean `labby` container/user/home naming, fix the repeated service-sync fallback, and quick-push the work.

## What Changed

- Added first-class Incus CLI paths: `labby incus setup`, `labby incus sync`, and `labby update`.
- Made `labby setup` favor the Incus path instead of pushing users through the web wizard first.
- Hard-broke old local state names: `~/.lab` moved to `~/.labby`, `LAB_HOME` became `LABBY_HOME`, and the Incus user/home became `labby` and `/home/labby`.
- Updated Incus bootstrap/profile/image configuration so the declarative container shape is in committed config and the host script validates/applies it.
- Baked the provisioning package/toolchain expectations into `config/incus/labby-image.yaml` and kept bare-metal package derivation tied back to that source.
- Added persistent Incus state/backup/snapshot plumbing and docs for supported storage drivers.
- Removed implicit artifact manifest syncing and replaced it with explicit persistent state/artifact mount semantics.
- Fixed Code Mode metrics so the overview reports child tool calls instead of top-level `gateway`, `logs`, and `code_mode` buckets.
- Fixed the MCP UI Code Mode trace clipping in `code_mode_app.html`.
- Fixed config TOML scalar updates so config mutation does not corrupt structured files.
- Updated docs, generated docs, plugin docs, examples, env references, session notes, and plans for the `labby` naming and primary Incus deployment model.
- Bumped the workspace/app version from `0.29.0` to `0.30.0` and added a changelog entry.

## Important Finding

The repeated `labby incus sync` force-restart fallback was not a Rust sync bug. It was an AppArmor signal mediation issue inside the Incus container.

Concrete proof from the live container:

- Root inside the container could not `kill -0 $$`.
- Root could not signal a child process it spawned.
- A throwaway systemd service left processes behind and logged permission-denied behavior during stop.
- The generated seccomp profile did not deny `kill`.
- Host labels showed stacked Incus AppArmor profiles ending in `//&unconfined`.

Fix:

- Added `raw.apparmor: signal peer=@{profile_name}//&unconfined,` to `config/incus/labby-gateway-profile.yaml`.
- Added bootstrap validation so new containers fail early if the profile is missing that signal rule.
- Restarted the live `labby` container after applying the profile.
- Re-ran `target/debug/labby incus sync`; it stopped the service, swapped the binary, restarted, and verified without using the force fallback.

## Files And Areas

Changed inventory at closeout:

- 225 tracked files changed.
- 2 new CLI modules: `crates/labby/src/cli/incus.rs` and `crates/labby/src/cli/update.rs`.
- Primary implementation files: `crates/labby/src/dispatch/setup/incus.rs`, `crates/labby/src/cli.rs`, `crates/labby/src/cli/setup.rs`, `crates/labby/src/dispatch/setup/provision.rs`, `crates/labby/src/config.rs`, `crates/labby/src/dispatch/logs/metrics.rs`, `crates/labby/src/mcp/call_tool_codemode.rs`.
- Primary Incus/config files: `config/incus/labby-gateway-profile.yaml`, `config/incus/labby-image.yaml`, `scripts/incus-bootstrap.sh`, `scripts/ci/smoke-incus-image.sh`.
- Primary docs: `docs/runtime/INCUS.md`, `docs/runtime/CONFIG.md`, `docs/runtime/ENV.md`, `docs/generated/cli-help.md`, `docs/generated/action-catalog.json`, `docs/generated/mcp-help.json`.
- Broad rename/documentation sweep: old `lab` state paths and URLs were moved to `labby` across docs, examples, Docker files, plugin docs, and historical session/planning notes.

## Verification

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | passed |
| `cargo check --workspace --all-features` | passed |
| `cargo run --package labby --all-features -- docs check` | passed, 15 docs artifacts fresh |
| `bash -n scripts/incus-bootstrap.sh` | passed |
| `scripts/incus-bootstrap.sh --dry-run --skip-install` | printed the AppArmor signal-rule validation step |
| `target/debug/labby incus sync` | passed after AppArmor profile fix, no force fallback |
| Live container `/ready` | ready |
| Public `https://labby.tootie.tv/ready` | ready |

## Follow-Ups

- Open a PR from `codex/incus-primary-deploy-clean-break`.
- Let CI build/test the Incus image and workspace.
- After review and green CI, merge to `main`.
- Cut the `v0.30.0` release once main is green.
