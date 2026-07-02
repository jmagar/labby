# Nodes Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-class `lab nodes update` command that builds the local release artifact, rolls it out to selected nodes, repairs node runtime state, verifies node reconnection, and updates the local controller last.

**Architecture:** Extend the existing node CLI and deploy primitives instead of creating a parallel rollout system. Treat `nodes update` as an orchestration layer over artifact build, remote install, node config normalization, legacy file cleanup, runtime restart, and controller-side verification.

**Tech Stack:** Rust, Clap, existing `lab` deploy/node runtime primitives, controller HTTP API, SSH/system service rollout helpers, `cargo build --release`

---

## File Map

### Likely files to modify
- `crates/lab/src/cli/nodes.rs` — add `update` subcommand and output path
- `crates/lab/src/node/master_client.rs` — expose any missing controller-side verification helpers used by `nodes update`
- `crates/lab/src/dispatch/deploy/*.rs` — reuse artifact build/install/locking primitives where they fit
- `crates/lab/src/config.rs` — ensure controller-host resolution, local-controller detection, and restart-model config helpers are available
- `crates/lab/src/cli/serve.rs` — complete runtime/bootstrap migration off legacy `device` paths before legacy cleanup is enabled in rollout
- `crates/lab/src/api/state.rs` — remove remaining runtime state naming/reads that can keep legacy device-role wiring alive
- `crates/lab/src/api/web.rs` — align runtime-role checks with the completed node runtime contract if needed
- `crates/lab/tests/nodes_cli.rs` — CLI coverage for `nodes update`
- `crates/lab/tests/nodes_api.rs` or other existing integration-style tests — controller verification coverage if needed
- `docs/CLI.md` — document `lab nodes update`
- `docs/DEVICE_RUNTIME.md` — document node rollout and normalization behavior

### Likely new files
- Prefer no new top-level rollout subsystem unless existing deploy code cannot absorb the orchestration cleanly.
- If needed, add one focused orchestration module such as `crates/lab/src/node/update.rs`.

## Task 1: Finish the runtime clean break before enabling destructive cleanup

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/web.rs` if runtime-role checks depend on legacy naming
- Test: existing runtime/config tests

- [ ] Remove remaining runtime/bootstrap reads of legacy `config.device.master`.
- [ ] Remove remaining runtime/bootstrap reads of `.labby/device-enrollments.json`.
- [ ] Keep runtime/bootstrap on canonical node paths only.
- [ ] Add or update tests proving serve/bootstrap uses `[node].controller` and `.labby/node-enrollments.json`.
- [ ] Meet all Task 1 acceptance gates before any rollout cleanup work begins:
  - no `config.device` reads in runtime/bootstrap path
  - no `.labby/device-enrollments.json` reads in runtime/bootstrap path
  - runtime startup tests pass using only `[node].controller`
  - runtime startup tests pass using only `.labby/node-enrollments.json`

## Task 2: Map existing rollout primitives and lock the reuse boundary

**Files:**
- Read: `crates/lab/src/cli/nodes.rs`
- Read: `crates/lab/src/dispatch/deploy/` relevant modules
- Read: `crates/lab/src/node/master_client.rs`
- Read: `crates/lab/src/config.rs`

- [ ] Identify the exact build/install/lock/result types already used by `lab deploy`.
- [ ] Explicitly record which deploy pieces are reusable and which are not.
- [ ] Treat wrapper restarts, node config rewrite, local `/health`, controller verification, and local-controller rollout as new orchestration responsibilities.
- [ ] Record the final file list before writing code.

## Task 3: Add the CLI surface for `lab nodes update`

**Files:**
- Modify: `crates/lab/src/cli/nodes.rs`
- Test: `crates/lab/tests/nodes_cli.rs`

- [ ] Add `Update(UpdateArgs)` to `NodesCommand`.
- [ ] Define CLI arguments for explicit targets and `--all`.
- [ ] Keep output behavior consistent with existing nodes commands and shared JSON formatting.
- [ ] Add CLI tests for:
  - explicit targets
  - `--all`
  - invalid argument combinations

## Task 4: Implement target resolution and local-controller inclusion rules

**Files:**
- Modify: `crates/lab/src/cli/nodes.rs`
- Modify: `crates/lab/src/config.rs` if helper support is missing
- Test: `crates/lab/tests/nodes_cli.rs`

- [ ] Resolve remote node targets from current deploy/node configuration.
- [ ] Detect when the current machine is the controller.
- [ ] Ensure the local controller is added automatically and scheduled last when applicable.
- [ ] Add tests for ordering and inclusion rules.

## Task 5: Build the artifact once and reuse it for the full rollout

**Files:**
- Modify: existing deploy/build orchestration module(s)
- Modify: `crates/lab/src/cli/nodes.rs` or new `crates/lab/src/node/update.rs`
- Test: targeted unit tests around rollout planning/result shaping

- [ ] Reuse the release build path from the deploy workflow.
- [ ] Ensure build failure aborts before any host mutation.
- [ ] Pass a single built artifact path through the rest of the update flow.
- [ ] Add tests for build-once behavior and early-abort on build failure.

## Task 6: Define restart models explicitly and implement remote node normalization

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: deploy/remote execution module(s)
- Modify: `crates/lab/src/node/update.rs` if introduced
- Test: unit tests for generated remote actions or script content

- [ ] Add an explicit restart-model representation for:
  - system service
  - user service
  - wrapper/script restart
- [ ] Choose a single config source of truth for restart behavior.
- [ ] Extend the existing deploy host config rather than inventing a second parallel host-target config surface unless a concrete blocker appears.
- [ ] Define the exact structs to extend in `config.rs` before writing rollout logic:
  - `DeployHostOverride`
  - `DeployDefaults` if a default restart model is needed
- [ ] Make restart selection data-driven rather than inferred from ad hoc shell commands.
- [ ] Define the remote normalization sequence:
  - install artifact
  - rewrite `config.toml` to `[node].controller`
  - remove `device-token`
  - remove `device-enrollments.json`
  - preserve `node-token`
  - preserve `node-enrollments.json`
  - restart runtime
- [ ] Keep the remote commands deterministic and idempotent.
- [ ] Add tests that assert the normalization commands contain the canonical node config shape, legacy cleanup actions, and the correct restart behavior for each restart model.

## Task 7: Verify node phone-home as part of success criteria

**Files:**
- Modify: `crates/lab/src/node/master_client.rs`
- Modify: orchestration module used by `nodes update`
- Test: controller-client tests and/or CLI orchestration tests

- [ ] Add or reuse a controller query that can verify a specific host resulted in a connected node.
- [ ] Define remote success as both local node health OK and controller-side connected status.
- [ ] Return host-target identity plus resolved `node_id` verification metadata when available.
- [ ] Make this verification mandatory in the command path.
- [ ] Add tests for connected vs not-connected result handling.

## Task 8: Implement the local-controller execution path and run it last

**Files:**
- Modify: orchestration module used by `nodes update`
- Modify: local install/service restart helper code if needed
- Modify: `crates/lab/src/config.rs` if local install/service settings need to be resolved from config
- Test: rollout-order tests

- [ ] Implement a distinct local-controller job instead of pretending the controller is a remote SSH target.
- [ ] Resolve the local install path, service name, and privilege model from a single source of truth.
- [ ] Reuse local artifact install logic for the controller.
- [ ] Restart the controller service after remote node rollout completes.
- [ ] Verify local controller health on `127.0.0.1:8765`.
- [ ] Ensure controller update ordering is always last.
- [ ] Add tests for controller-last sequencing.

## Task 9: Shape staged results and errors for operator use

**Files:**
- Modify: orchestration/result types
- Modify: `crates/lab/src/cli/nodes.rs`
- Test: JSON-output tests

- [ ] Reuse current staged host result patterns where practical.
- [ ] Keep host/alias as the rollout target identity in results.
- [ ] Include resolved `node_id` and connected verification state as explicit metadata fields.
- [ ] Ensure human-readable output clearly identifies which stage failed per target.
- [ ] Add tests covering success, partial failure, and controller-failure output.

## Task 10: Document `nodes update` as the canonical rollout path

**Files:**
- Modify: `docs/CLI.md`
- Modify: `docs/DEVICE_RUNTIME.md`

- [ ] Document command usage, target selection, controller-last behavior, and verification contract.
- [ ] Document that `nodes update` rewrites node runtime config to `[node].controller` and removes stale legacy runtime files.
- [ ] Remove or downgrade any doc guidance that suggests manual repair as the standard rollout path.

## Task 11: Verify with repo-accurate commands

**Files:**
- Test: `crates/lab/tests/nodes_cli.rs`
- Test: any rollout/controller tests added above

- [ ] Run targeted tests for the new nodes CLI/update flow.
- [ ] Run full repo verification:
  - `just check`
  - `just build`
  - `just test`
- [ ] Confirm docs and code match the clean-break nodes contract.
