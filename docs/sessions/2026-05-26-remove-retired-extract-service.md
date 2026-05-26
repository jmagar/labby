---
date: 2026-05-26 14:19:14 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 263102f9
agent: Codex
session id: 88d7387f-3aa2-4a16-bad4-52fe10310abd
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/88d7387f-3aa2-4a16-bad4-52fe10310abd.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 263102f9 [main]
---

# Remove Retired Extract Service

## User Request

Review the crate-extract docs, resolve the reported findings, then remove the dead `extract` feature entirely and push the result straight to `main`.

## Session Overview

- Reviewed and updated the crate extraction docs around generated OpenAPI verification, standalone binaries, OAuth ownership, current-vs-target REST routes, retired services, and resolved research recommendations.
- Removed the retired `extract` service from `lab-apis`, `lab`, generated docs, and user-facing service docs.
- Fixed the host sccache wrapper so cargo builds keep the workspace toolchain when Cargo invokes rustc from dependency directories.
- Verified the workspace with all-features check, clippy, nextest, generated-doc checks, and CLI smoke checks.
- Bumped the project from `0.17.6` to `0.17.7`, updated the changelog, committed `263102f9`, and pushed to `origin/main`.

## Sequence of Events

1. Audited `docs/crate-extract` against the live code and generated route metadata.
2. Removed `extract` source modules, feature flags, dependencies, CLI/API/dispatch registration, generated artifacts, and service documentation.
3. Regenerated docs and validated the generated docs were fresh.
4. Investigated mixed-rustc cache failures in the configured `sccache-wrapper`.
5. Updated `/home/jmagar/.local/bin/sccache-wrapper`, cleared the poisoned cache, restarted the user sccache service, and confirmed all wrapper invocations used the workspace rustc.
6. Fixed one websocket disconnect test race and two ACP test warnings exposed by the full test build.
7. Ran verification, bumped versions, updated `CHANGELOG.md`, committed, and pushed to `main`.

## Key Findings

- `extract` was a synthetic credential scanner tied to retired homelab service credential extraction and was no longer part of the intended product surface.
- The crate extraction docs referenced `labby internal export-openapi`, but the live path is generated docs/OpenAPI through `labby docs generate/check`.
- The docs referenced a current standalone `lab-gateway` binary, but current Cargo metadata only declares the `labby` binary.
- The sccache wrapper re-resolved rustc based on the current directory, so dependency builds under cargo registry paths could use a different rustup default than the workspace toolchain.
- `node::ws_client::tests::flush_queue_once_drains_into_real_fleet_websocket_handler` assumed server disconnect cleanup was synchronous with the client test helper returning.

## Technical Decisions

- Removed `extract` rather than disabling it, because the feature was explicitly declared dead and should not remain in generated surfaces or feature matrices.
- Kept shared SSH/config helpers that are still used outside `extract`, including core SSH parsing and device/node discovery paths.
- Modeled `lab-oauth` as reusable and independently extractable, with gateway integration through composition/shared auth contracts rather than a product-crate dependency.
- Fixed sccache at the wrapper layer instead of disabling `RUSTC_WRAPPER`, preserving the host build setup.
- Bumped patch version to `0.17.7` because the work retires dead code and refreshes docs/verification without adding a new product surface.

## Files Modified

- Version/changelog: `Cargo.toml`, `Cargo.lock`, `apps/gateway-admin/package.json`, `CHANGELOG.md`.
- Extract removals: `crates/lab-apis/src/extract.rs`, `crates/lab-apis/src/extract/**`, `crates/lab/src/cli/extract.rs`, `crates/lab/src/dispatch/extract.rs`, `crates/lab/src/api/services/extract.rs`, `docs/services/EXTRACT.md`.
- Feature/dependency wiring: workspace and crate `Cargo.toml` files, `crates/lab-apis/src/lib.rs`, `crates/lab/src/cli.rs`, `crates/lab/src/dispatch.rs`, `crates/lab/src/api/services.rs`, `crates/lab/src/api/router.rs`, `crates/lab/src/registry.rs`.
- Config and gateway credential plumbing: `crates/lab/src/config.rs`, `crates/lab/src/config/env_merge.rs`, `crates/lab/src/dispatch/gateway/config_mutation.rs`, `crates/lab/src/dispatch/gateway/index.rs`, `crates/lab/src/dispatch/gateway/manager.rs`.
- Generated docs and metadata: `docs/generated/*`, `crates/lab/src/docs/projection.rs`, `crates/lab/src/docs/routes.rs`, `crates/lab/src/api/openapi.rs`.
- Planning docs: `docs/crate-extract/*`, `docs/dev/ERRORS.md`, `docs/dev/SERVICES.md`, `docs/dev/SERVICE_LAYER_MIGRATION.md`, `docs/features/*`, root/crate README files.
- Verification fixes: `crates/lab/src/node/ws_client.rs`, `crates/lab/src/acp/runtime.rs`.
- Host tool outside repo: `/home/jmagar/.local/bin/sccache-wrapper`, with backup `/home/jmagar/.local/bin/sccache-wrapper.bak-20260526112756`.

## Commands Executed

| Command | Result |
| --- | --- |
| `cargo run -p labby --all-features -- docs generate` | Regenerated docs after extract removal. |
| `cargo run -p labby --all-features -- docs check` | Passed, `checked 15 docs artifacts: fresh`. |
| `cargo check --workspace --all-features` | Passed with `RUSTC_WRAPPER` enabled. |
| `cargo clippy --workspace --all-features -- -D warnings` | Passed with `RUSTC_WRAPPER` enabled. |
| `cargo nextest run --workspace --all-features` | Passed, `1540 tests run: 1540 passed, 25 skipped`. |
| `cargo run -p labby --all-features -- --help \| rg extract \|\| true` | No `extract` command shown. |
| `cargo run -p labby --all-features -- extract --help` | Failed with `unrecognized subcommand 'extract'`, expected after removal. |
| `SCCACHE_SERVER_UDS=/tmp/sccache-jmagar.sock sccache --show-stats` | Reported zero cache read/write errors after the wrapper fix. |
| `git commit -m "chore: remove retired extract service"` | Created `263102f9`. |
| `git push origin main` | Pushed `0ae9a1e1..263102f9` to `origin/main`. |

## Errors Encountered

- `cargo nextest` initially failed with mixed-rustc metadata errors because sccache cached artifacts compiled by different Rust toolchains.
- The wrapper root cause was resolving rustc after Cargo had changed working directories; the wrapper now trusts Cargo's explicit rustup toolchain rustc path and exports `RUSTUP_TOOLCHAIN` from it.
- Full nextest then exposed one real test race in the node websocket disconnect path; the test now waits briefly for the server-side disconnected snapshot.
- Nextest also surfaced two `unused_qualifications` warnings in ACP tests; the tests now call the local helper directly.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| CLI | `extract` was documented and wired as an always-on command. | `labby extract` is not recognized. |
| HTTP/API docs | Generated route docs included extract surfaces. | Extract routes are gone from generated docs and OpenAPI. |
| Features | `extract` feature pulled SSH/SFTP/XML dependencies. | Feature and dependencies are removed. |
| Crate extraction docs | Mixed current and target claims for several surfaces. | Docs distinguish current vs target and mark `extract` retired. |
| Host builds | Wrapper could mix workspace and default rustup toolchains. | Wrapper keeps Cargo's explicit toolchain path through sccache. |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `cargo check --workspace --all-features` | Workspace compiles | Passed | Passed |
| `cargo clippy --workspace --all-features -- -D warnings` | No warnings/errors | Passed | Passed |
| `cargo nextest run --workspace --all-features` | Full test pass | `1540 passed, 25 skipped` | Passed |
| `cargo run -p labby --all-features -- docs check` | Generated docs fresh | `checked 15 docs artifacts: fresh` | Passed |
| `labby --help \| rg extract` | No extract output | No output | Passed |
| `labby extract --help` | Unknown command | `unrecognized subcommand 'extract'` | Passed |

## Risks and Rollback

- Removing `extract` is intentionally breaking for anyone still calling that dead command or feature.
- Rollback path is `git revert 263102f9`, plus restoring the previous sccache wrapper backup if the host wrapper change needs to be undone.
- The sccache wrapper is outside the repo, so future host migrations should carry that change separately.

## Decisions Not Taken

- Did not disable `RUSTC_WRAPPER`; the wrapper was fixed as requested.
- Did not preserve an inert `extract` compatibility shim; the user explicitly asked to rip the feature out.
- Did not create a feature branch; the user explicitly requested pushing straight to `main`.

## Open Questions

- Whether the retired generated `onboarding-audit` docs should be replaced by a new generated artifact under a different command later.

## Next Steps

- No unfinished implementation tasks from this session.
- Optional follow-up: document the host sccache wrapper fix in a checked-in developer note if this workstation setup should be reproducible across machines.
