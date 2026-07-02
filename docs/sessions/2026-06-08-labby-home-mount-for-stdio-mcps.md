---
date: 2026-06-08 11:00:28 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 5aafeeeb
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
---

# Labby home mount for local stdio MCPs

## User Request

Fix the Labby gateway failure where `synapse2` still reported that its stdio command could not start because a referenced file/path did not exist, then change the Docker runtime mount strategy to mount `/home/jmagar` so future local stdio MCPs such as `ytdl-mcp` do not each need separate per-repo mounts.

## Session Overview

The initial host-side gateway test passed, but the Labby UI and the `labby` container still failed because the stdio binary path existed on the host and not inside the container. The runtime fix was to update `docker-compose.prod.yml` so the Labby container mirrors `${HOME}:${HOME}`, then restart `labby-master`. After the restart, both `synapse2` and `ytdl-mcp` tested cleanly from inside the container and via gateway/API probes.

## Sequence of Events

1. Reproduced the user-visible failure from inside the `labby` container and confirmed the host path `/home/jmagar/workspace/synapse2/plugins/synapse2/bin/synapse` was missing inside the container.
2. Verified the same `synapse2` gateway succeeded on the host, separating host config correctness from container filesystem visibility.
3. First added a narrow Synapse workspace mount, restarted Labby, and proved `synapse2` recovered.
4. Broadened the approach to mount `/home/jmagar/workspace` so `ytdl-mcp` would also work without a dedicated mount.
5. Changed the final requested shape to mount `/home/jmagar` at the same absolute path inside the container, restarted Labby, and verified both target stdio MCPs.
6. Cleaned stale Synapse plugin source references in `/home/jmagar/workspace/synapse2`, including converting `AGENTS.md` and `GEMINI.md` back to symlinks to `CLAUDE.md`.

## Key Findings

- `docker-compose.prod.yml:89` now documents the local stdio MCP path issue; `docker-compose.prod.yml:92` mounts `${HOME}:${HOME}` into the container.
- The failure was not a bad Lab gateway config. It was a host/container path mismatch for stdio command paths stored as absolute host paths.
- `ytdl-mcp` was the correct gateway name; an intermediate probe used `youtube-dl` and correctly returned "gateway youtube-dl not found".
- The Synapse plugin source still had stale `synapse2` binary references even though the actual binary is `bin/synapse`.

## Technical Decisions

- Mounted `${HOME}:${HOME}` instead of only `${HOME}/workspace:${HOME}/workspace` because local stdio servers can reference binaries, shims, cache files, or plugin paths anywhere under the user's home directory.
- Kept the existing `/workspace/synapse2` alias for tools/scripts that expect the container-local path.
- Left pre-existing dirty Lab files untouched; the runtime fix only changed `docker-compose.prod.yml`.
- Did not move the open plan files because neither plan was clearly completed by this session.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `/home/jmagar/workspace/lab/docker-compose.prod.yml` | - | Mirror `/home/jmagar` into the Labby container for absolute-path stdio MCP commands. | `git diff -- docker-compose.prod.yml`; live mount showed `/home/jmagar -> /home/jmagar`. |
| modified | `/home/jmagar/workspace/synapse2/plugins/synapse2/CLAUDE.md` | - | Replace stale `synapse2` binary references with `synapse`. | `git -C /home/jmagar/workspace/synapse2 diff --stat`. |
| modified | `/home/jmagar/workspace/synapse2/plugins/synapse2/README.md` | - | Replace stale binary names in plugin documentation. | `git -C /home/jmagar/workspace/synapse2 diff --stat`. |
| modified | `/home/jmagar/workspace/synapse2/plugins/synapse2/hooks/plugin-setup.sh` | - | Use the real `synapse` binary in plugin setup logic. | `git -C /home/jmagar/workspace/synapse2 diff --stat`. |
| modified | `/home/jmagar/workspace/synapse2/plugins/synapse2/monitors/monitors.json` | - | Point monitor command at `bin/synapse`. | `git -C /home/jmagar/workspace/synapse2 diff --stat`. |
| modified | `/home/jmagar/workspace/synapse2/plugins/synapse2/AGENTS.md` | - | Convert to symlink to `CLAUDE.md` per agent memory convention. | `git -C /home/jmagar/workspace/synapse2 status --short --branch` showed typechange. |
| modified | `/home/jmagar/workspace/synapse2/plugins/synapse2/GEMINI.md` | - | Convert to symlink to `CLAUDE.md` per agent memory convention. | `git -C /home/jmagar/workspace/synapse2 status --short --branch` showed typechange. |
| created | `/home/jmagar/workspace/lab/docs/sessions/2026-06-08-labby-home-mount-for-stdio-mcps.md` | - | Save this session log. | Created by `vibin:save-to-md`. |

## Beads Activity

No bead changes were made during this stdio mount session.

Observed active, unrelated beads:

| bead | title | action | final status | why it mattered |
|---|---|---|---|---|
| `lab-3cxuj.1` | Code Mode trace params, redaction, and bounded history | Read-only inspection with `bd show`. | `IN_PROGRESS` | Confirmed it belongs to separate active Code Mode work and should not be closed or edited here. |
| `lab-3cxuj.2` | Shape Code Mode search/execute structured trace content | Read-only inspection with `bd show`. | `IN_PROGRESS` | Confirmed it belongs to separate active Code Mode work and should not be closed or edited here. |

## Repository Maintenance

### Plans

Checked `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md`. Both remain broad implementation plans unrelated to this container mount fix, so no plan files were moved to `docs/plans/complete/`.

### Beads

Read `lab-3cxuj.1` and `lab-3cxuj.2`; both are active Code Mode beads and were left unchanged. No follow-up bead was created because the requested runtime behavior was verified in the current session.

### Worktrees and branches

`git worktree list --porcelain` showed the main worktree plus `/home/jmagar/workspace/lab/.worktrees/codex/lab-3cxuj-code-mode-app`. `git branch -vv` showed both branches at `5aafeeeb`; the Codex worktree appears active/owned by the Code Mode beads, so it was not removed. Remote branch state showed `origin/main` at `5aafeeeb`.

### Stale docs

Updated the compose-file comments adjacent to the mount change. Broader documentation was not changed because the implementation is runtime compose wiring rather than a user-facing command or API contract.

### Dirty tree transparency

Pre-existing Lab dirty files were observed in `crates/lab/src/cli/gateway.rs`, `crates/lab/src/cli/serve.rs`, `crates/lab/src/config.rs`, gateway/oauth files, `plugins/labby/bin/labby`, and `docs/snippets/`. They were not part of this session's requested change and were not staged.

## Tools and Skills Used

- `superpowers:systematic-debugging`: used to separate host success from container failure before changing config.
- `vibin:save-to-md`: used to write this session artifact and commit only the generated log.
- Shell and Docker CLI: inspected mounts, tested gateway registrations, restarted the Labby container, and verified filesystem visibility.
- Git CLI: inspected dirty state, branch/worktree state, diffs, and cross-repo Synapse plugin changes.
- `bd`: read relevant recent beads for the repository maintenance pass.
- Labby CLI: ran `labby gateway test` inside the container for `synapse2` and `ytdl-mcp`.

## Commands Executed

| command | result |
|---|---|
| `docker exec labby labby gateway test --name synapse2` | Passed from inside the container; `tool_count: 2`, `prompt_count: 1`, `last_error: ∅`. |
| `docker exec labby labby gateway test --name ytdl-mcp` | Passed from inside the container; `tool_count: 2`, `prompt_count: 0`, `last_error: ∅`. |
| `docker inspect labby --format '{{range .Mounts}}{{println .Source "->" .Destination}}{{end}}' \| rg '/home/jmagar'` | Confirmed `/home/jmagar -> /home/jmagar` plus existing credential/plugin mounts. |
| `docker exec labby ls -ld /home/jmagar /home/jmagar/workspace /home/jmagar/workspace/synapse2 /home/jmagar/workspace/ytdl-mcp` | Confirmed all target directories exist inside the container. |
| `docker compose up -d labby-master` | Recreated/restarted the Labby service after compose mount edits. |
| `git diff -- docker-compose.prod.yml` | Confirmed the only Lab source diff from this fix is the mount replacement in `docker-compose.prod.yml`. |
| `just validate-plugin` in `/home/jmagar/workspace/synapse2` | Passed 41/41 after stale plugin reference cleanup. |

## Errors Encountered

- UI/gateway error: `synapse2` stdio command could not start because its referenced path did not exist. Root cause was the Labby Docker container lacking the absolute host path used by the stdio command. Resolved by mounting `${HOME}:${HOME}` and restarting Labby.
- Wrong probe name: `youtube-dl` was not a configured gateway. Corrected to `ytdl-mcp`.
- Synapse plugin drift: plugin files referenced `bin/synapse2` while the real binary was `bin/synapse`. Resolved in the Synapse plugin source and validated with the plugin validator.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| `synapse2` gateway in Labby container | Failed with missing stdio path. | Connects over stdio with 2 tools and 1 prompt. |
| `ytdl-mcp` gateway in Labby container | Would require a dedicated mount if only per-repo paths were mirrored. | Connects over stdio through the shared `/home/jmagar` mirror. |
| Compose mount strategy | Individual workspace mounts for local stdio servers. | Broad `${HOME}:${HOME}` mirror for local absolute-path stdio servers. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `docker exec labby labby gateway test --name synapse2` | `last_error` empty and tools discovered. | `tool_count: 2`, `prompt_count: 1`, `last_error: ∅`. | pass |
| `docker exec labby labby gateway test --name ytdl-mcp` | `last_error` empty and tools discovered. | `tool_count: 2`, `prompt_count: 0`, `last_error: ∅`. | pass |
| `docker inspect labby ... \| rg '/home/jmagar'` | Container includes a `/home/jmagar` mirror. | Output included `/home/jmagar -> /home/jmagar`. | pass |
| `docker exec labby ls -ld /home/jmagar/workspace/synapse2 /home/jmagar/workspace/ytdl-mcp` | Both paths visible inside container. | Both directories listed successfully. | pass |
| `just validate-plugin` in `/home/jmagar/workspace/synapse2` | Plugin validates after binary-name cleanup. | 41/41 checks passed. | pass |

## Risks and Rollback

- Risk: `${HOME}:${HOME}` gives the Labby container broader read/write visibility into the host user's home directory. This matches the user's requested operational tradeoff for local stdio MCPs, but it is broader than the previous per-repo mounts.
- Rollback: replace `- ${HOME}:${HOME}` in `docker-compose.prod.yml` with narrower per-repo mounts, restore the previous `open-design` and lab workspace mirrors if needed, then run `docker compose up -d labby-master` and retest affected gateways.

## Decisions Not Taken

- Did not solve this by changing `~/.labby/config.toml` paths to container-only aliases because each new local stdio MCP would still need a bespoke path rewrite.
- Did not leave the fix at `/home/jmagar/workspace` because the final user request explicitly changed the desired mount boundary to `/home/jmagar`.
- Did not commit Synapse plugin cleanup from the Lab repo because it belongs to `/home/jmagar/workspace/synapse2`, a separate repository.

## Open Questions

- Whether the broader home mount should later be tightened to read-only or split into read-only and read-write submounts. Current compose uses the default read-write mount.
- Whether the Synapse plugin cleanup should be committed and pushed separately from the Synapse repository.

## Next Steps

1. Commit and push this session log only from the Lab repo.
2. If desired, separately review, commit, and push the Synapse plugin cleanup in `/home/jmagar/workspace/synapse2`.
3. Re-run the Labby UI connection tests for `synapse2` and `ytdl-mcp` from the browser to confirm the visible UI state matches the container/API verification.
