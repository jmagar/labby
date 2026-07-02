# Nodes Update Design

## Goal

Add a first-class `lab nodes update` command that builds the local release artifact, rolls it out to selected nodes, repairs node runtime state to the canonical nodes contract, verifies that each node reconnects to the controller, and updates the local controller last when the current machine is the controller.

## Problem

The current rollout path is split across:
- `lab deploy run`
- host-specific wrapper/service knowledge
- ad hoc config rewrites
- ad hoc cleanup of legacy runtime files
- manual verification that nodes reconnected

That split caused a real operational failure during the nodes clean-break migration:
- all node binaries were updated
- all nodes still had legacy `[device].master` config
- some nodes were not restarted cleanly
- controller logs showed websocket resets until manual config repair and restart

The product needs a canonical operator command that owns the full node rollout contract rather than leaving repair steps to manual intervention.

## Scope

`lab nodes update` will:
- build the local release artifact once
- target an explicit set of nodes or `--all`
- push the built artifact to each target node
- rewrite runtime config to canonical `[node].controller`
- remove stale legacy runtime files
- restart the node runtime using the known host service model
- verify local node health on-host
- verify that each node reconnects to the controller
- update the local controller last when the current machine is the controller
- verify controller health after the local controller update

## Preconditions

`nodes update` must not delete legacy runtime state until the runtime itself no
longer consumes it.

Current code reality:
- `serve` still resolves runtime role from `config.device.master`
- `serve` still opens `.labby/device-enrollments.json`

That means the implementation must either:
1. first migrate runtime/bootstrap to the canonical node paths, then enable
   legacy cleanup in `nodes update`, or
2. gate legacy cleanup behind a completed runtime-migration check

V1 should choose option 1. The update command should not remove active runtime
files from a tree that still reads them.

## Non-Goals

This command will not:
- redesign the entire deploy subsystem
- introduce backward compatibility for legacy config formats at runtime
- support arbitrary remote service managers beyond the patterns the current deploy/runtime system already knows
- update non-node services
- silently succeed without reconnect verification

## CLI Shape

Canonical surface:

```text
lab nodes update <targets...>
lab nodes update --all
```

Rules:
- `<targets...>` is one or more node host aliases from the deploy configuration
- `--all` targets every configured node host
- the local controller host is included automatically and updated last when the current machine is the controller
- machine-readable output uses staged per-host results similar to the current deploy flow

Likely flags:
- `--all`
- `-y, --yes`
- `--json`
- `--no-verify` should not exist in v1; verification is the contract

## Operational Contract

### Build

- Build the local release binary once.
- Use that exact artifact for all node targets and the local controller update.
- If the build fails, nothing is updated.

### Node update sequence

For each remote node:
1. Push artifact to the configured install path.
2. Rewrite config to:

```toml
[node]
controller = "<controller-host>"

[mcp]
host = "127.0.0.1"
port = 8765
```

3. Remove legacy runtime files if present:
- `device-token`
- `device-enrollments.json`

4. Preserve canonical runtime files:
- `node-token`
- `node-enrollments.json`

5. Restart the runtime with the host’s known service model.
6. Verify local node health from the host itself.
7. Verify that the controller sees the node as connected.

### Restart model

The command must support the fleet’s actual runtime restart patterns:
- `systemctl restart <unit>`
- `systemctl --user restart <unit>`
- wrapper/script restart for hosts that do not use a managed unit

The restart model must be explicit in configuration and must not be inferred
from ad hoc shell behavior during rollout.

### Local controller update sequence

If the current machine is the controller:
1. Install the same built artifact locally.
2. Restart or reload the local controller service using the existing controller service model.
3. Verify `http://127.0.0.1:8765/health` is OK.
4. Verify final node connectivity from the controller view.

Controller update is always last.

The local controller path is a distinct execution path, not a fake SSH target.
It must define:
- the local install path source of truth
- the local service/unit source of truth
- the privilege model used for local install and restart

## Failure Model

- Results are reported per host with staged progress.
- A node failure does not stop updates to other nodes.
- Overall success requires:
  - every requested node updated and verified connected
  - local controller updated and verified healthy when included
- Build failure aborts the entire operation before any host changes.
- Controller update still runs last after partial node failures so the local machine is not left behind the release artifact, but the final result remains failed if any requested node does not verify.

## Verification Contract

A remote node counts as successful only if all of the following are true:
1. artifact installed
2. config rewritten to `[node].controller`
3. legacy runtime files removed
4. node runtime restarted successfully
5. node local `/health` is OK
6. controller reports that node as connected

The local controller counts as successful only if:
1. local artifact installed
2. controller service restarted successfully
3. local controller `/health` is OK

## Data And Serialization Expectations

Per `docs/design/SERIALIZATION.md`:
- `lab` owns the operator-facing JSON envelope and staged result reporting
- public output must use `node_id`, never `device_id`
- human output may summarize per-host stages, but JSON output should expose the structured stage results

## Reuse Strategy

Implementation should reuse current deploy and node runtime primitives only where
they actually fit:
- artifact build logic
- remote preflight / transfer / install primitives
- host locking
- SSH inventory and per-host targeting
- existing controller client for node connectivity verification

Current deploy does not yet own:
- node config rewrite
- legacy runtime-state cleanup
- local `/health` verification
- controller-side connected verification
- wrapper-based restart models
- local-controller update orchestration

So `nodes update` should use a focused orchestration layer that composes deploy
building blocks rather than pretending the existing deploy runner already
implements the full node rollout contract.

## Output Contract

The command is target-oriented. The primary result identity is the update
target host.

Rules:
- host/alias remains the canonical rollout target identity
- `node_id` is included when controller verification resolves it
- JSON results must make the verification distinction explicit:
  - install target host
  - observed controller-side `node_id`
  - connected status

## Acceptance Criteria

`lab nodes update` is complete when:
1. operator can run `lab nodes update <targets...>` or `lab nodes update --all`
2. local release artifact is built once and pushed to all targets
3. each updated node is normalized to `[node].controller`
4. stale legacy runtime files are removed on each updated node
5. updated nodes reconnect and appear as connected in `lab nodes list`
6. local controller is updated last on controller machines
7. all-features verification remains green
8. docs clearly describe `nodes update` as the canonical node rollout path
