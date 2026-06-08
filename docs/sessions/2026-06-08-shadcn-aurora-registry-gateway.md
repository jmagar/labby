---
date: 2026-06-08 14:08:15 EDT
repo: git@github.com:jmagar/lab.git
branch: main
head: 70f39074
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 70f3907492d292b90c4c0629f09bc9f51d866fbd [main]
beads: lab-efwj2
---

# Shadcn Aurora registry gateway wiring

## User Request

Wire the existing Labby gateway `shadcn` MCP upstream so it can connect to the user's shadcn-compatible Aurora registry.

## Session Overview

The existing `shadcn` gateway upstream was updated to launch from `/home/jmagar/workspace/aurora-design-system`, where `components.json` defines the `@aurora` registry namespace. The working gateway path now sees both `@shadcn` and `@aurora`, and a Code Mode call can search the Aurora registry.

## Sequence of Events

1. Confirmed the current `shadcn` gateway upstream existed and was connected as stdio with `npx shadcn@latest mcp`.
2. Checked official shadcn MCP documentation and confirmed custom registries are read from the consuming project's `components.json`.
3. Verified `/home/jmagar/workspace/aurora-design-system/components.json` already includes `"@aurora": "https://aurora.tootie.tv/r/{name}.json"`.
4. Updated the Labby upstream first with a shell `cd` wrapper, then tested shadcn's native `--cwd` option.
5. Reproduced that native `--cwd` was not reliable through the MCP launch path, while the shell wrapper returned both registries.
6. Restored the saved upstream to the proven shell wrapper and verified the registry through Labby Code Mode.
7. Created follow-up bead `lab-efwj2` for the remaining stale runtime snapshot issue.

## Key Findings

- Official shadcn MCP behavior: registries are configured in project `components.json`; no special registry-side MCP mode is required.
- The Aurora project has the required namespace in `/home/jmagar/workspace/aurora-design-system/components.json`.
- `npx shadcn@latest mcp --cwd /home/jmagar/workspace/aurora-design-system` looked cleaner, but `mcporter` showed it could be misparsed as a positional argument in the MCP launch path.
- The shell wrapper `bash -lc 'cd /home/jmagar/workspace/aurora-design-system && exec npx shadcn@latest mcp'` correctly reported `@shadcn` and `@aurora`.
- `labby gateway get shadcn --json` still reports runtime counts as zero even though `gateway test` and Code Mode calls work.

## Technical Decisions

- Kept the existing `shadcn` upstream instead of adding a second gateway entry, preserving the existing protected route and name.
- Used the shell `cd` wrapper because it was empirically reliable across Labby and `mcporter`.
- Left `proxy_resources` and `proxy_prompts` unchanged because the shadcn server exposed tools and the session goal was registry access, not resource or prompt proxying.
- Created a follow-up bead for the stale operator runtime view instead of changing Lab code during this config-wiring session.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `/home/jmagar/.lab/config.toml` | — | Updated the existing `shadcn` upstream to run from the Aurora project directory. | `sed -n '120,129p' /home/jmagar/.lab/config.toml` showed `command = "bash"` and the `cd /home/jmagar/workspace/aurora-design-system && exec npx shadcn@latest mcp` wrapper. |
| created | `docs/sessions/2026-06-08-shadcn-aurora-registry-gateway.md` | — | Captured the session, maintenance pass, verification, and follow-up work. | This session artifact. |

## Beads Activity

| id | title | action(s) | final status | why it mattered |
|---|---|---|---|---|
| lab-efwj2 | Fix gateway runtime snapshot for stdio upstream tool counts | Created; attempted description correction after shell quoting issue. | open | Tracks the remaining mismatch where `gateway get` reports zero tool counts while `gateway test` and Code Mode can use the upstream. |

Note: `bd update lab-efwj2 --description ... --json` returned the corrected description, but a later `bd show lab-efwj2 --json` still displayed the noisier created description. This inconsistency was observed and not hidden.

## Repository Maintenance

### Plans

Checked `docs/plans/` and found `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md`. Both contain active or broad implementation planning content and were not clearly completed by this session, so nothing was moved to `docs/plans/complete/`.

### Beads

Read recent beads with `bd list --all --sort updated --reverse --limit 100 --json`. Created `lab-efwj2` for the only known remaining repo issue from this session. No bead was closed because the runtime snapshot mismatch remains open.

### Worktrees and branches

Inspected `git worktree list --porcelain`, local branches, and remote branches. Left all worktrees and branches untouched: `/home/jmagar/workspace/lab` is active `main`, and the two `codex/*` worktrees are separate active branches with unclear ownership for this closeout.

### Stale docs

No in-repo documentation was updated. The primary changed artifact is operator config in `/home/jmagar/.lab/config.toml`; broader Lab docs for the runtime-count mismatch should wait for investigation under `lab-efwj2`.

### Dirty state

Observed unrelated dirty files before writing the session note: `docs/snippets/axon-fanout.md`, several `docs/snippets/axon-research-brief-*` files, and `docs/superpowers/plans/2026-06-08-code-mode-artifacts.md`. They were left untouched and were not staged.

## Tools and Skills Used

- **Skills.** Used `build-web-apps:shadcn` for shadcn registry/MCP conventions, `superpowers:systematic-debugging` for the zero-tool and unknown-registry mismatch, and `vibin:save-to-md` for this session closeout.
- **Shell commands.** Used `git`, `labby`, `npx shadcn@latest`, `mcporter`, `bd`, `sed`, `rg`, `jq`, `ps`, and `find` for evidence gathering and verification.
- **Web lookup.** Consulted official shadcn docs for MCP and `components.json` registry behavior.
- **Labby gateway.** Updated and tested the `shadcn` upstream, reloaded the gateway, and invoked Code Mode helpers.
- **mcporter.** Compared stdio launch forms and proved the shell wrapper saw both registries while the native `--cwd` form failed in the ad hoc MCP launch path.

## Commands Executed

| command | result |
|---|---|
| `labby gateway list` | Showed `shadcn` connected as stdio. |
| `labby gateway get shadcn --json` | Showed saved config and later the persistent runtime-count mismatch. |
| `labby gateway update shadcn --command npx --arg shadcn@latest --arg mcp --arg=--cwd --arg /home/jmagar/workspace/aurora-design-system` | Saved native `--cwd` form, but later tests showed that launch shape was not reliable. |
| `mcporter ... call shadcn.get_project_registries` | Native `--cwd` form failed with a positional argument error; shell wrapper returned `@shadcn` and `@aurora`. |
| `labby gateway update shadcn --command bash --arg=-lc --arg 'cd /home/jmagar/workspace/aurora-design-system && exec npx shadcn@latest mcp'` | Saved the final working upstream command. |
| `labby gateway reload && labby gateway test --name shadcn --json` | Reported `tool_count: 7` and `exposed_tool_count: 7`. |
| `labby gateway code exec --json --code 'async () => codemode.shadcn.get_project_registries({})'` | Returned `@shadcn` and `@aurora`. |
| `labby gateway code exec --json --code 'async () => codemode.shadcn.search_items_in_registries({ registries: ["@aurora"], query: "button", limit: 5 })'` | Returned 38 Aurora matches and showed the first 5. |

## Errors Encountered

- `labby gateway test shadcn --json` failed because this CLI expects `--name shadcn`, not a positional upstream name.
- `labby gateway update ... --arg --cwd` and `--arg -lc` failed because Clap parsed those as Labby flags; using `--arg=--cwd` or `--arg=-lc` fixed argument preservation.
- Native `npx shadcn@latest mcp --cwd ...` failed in the `mcporter` stdio probe with `too many arguments for 'mcp'`, so the shell `cd` wrapper was selected.
- Code Mode initially returned `Unknown registry "@aurora"` because the persistent gateway path was still effectively using the default project context.
- A bead description create command used double quotes with backticks, causing shell command substitution. A corrective `bd update` was run and reported success, but `bd show` later still displayed the created text.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| `shadcn` gateway upstream | Launched as `npx shadcn@latest mcp` without Aurora project context. | Launches via `bash -lc 'cd /home/jmagar/workspace/aurora-design-system && exec npx shadcn@latest mcp'`. |
| shadcn project registries through Code Mode | Reported only `@shadcn` during the failed native `--cwd` path. | Reports `@shadcn` and `@aurora`. |
| Aurora registry search through Code Mode | Failed with `Unknown registry "@aurora"`. | Returns 38 matches for `button` in `@aurora`. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cd /home/jmagar/workspace/aurora-design-system && npx shadcn@latest search @aurora -q button` | Direct CLI can reach Aurora registry. | Found 38 matching items. | pass |
| `labby gateway test --name shadcn --json` | Configured gateway upstream exposes shadcn tools. | `tool_count: 7`, `exposed_tool_count: 7`, `last_error: null`. | pass |
| `labby gateway code exec --json --code 'async () => codemode.shadcn.get_project_registries({})'` | Gateway Code Mode sees Aurora namespace. | Returned `@shadcn` and `@aurora`. | pass |
| `labby gateway code exec --json --code 'async () => codemode.shadcn.search_items_in_registries({ registries: ["@aurora"], query: "button", limit: 5 })'` | Gateway Code Mode can query Aurora registry. | Returned 38 matches and first 5 items. | pass |
| `labby gateway get shadcn --json` | Operator runtime view should match usable tool state. | Still reported `tool_count: 0` and `exposed_tool_count: 0`. | warn |

## Risks and Rollback

The working configuration depends on `/home/jmagar/workspace/aurora-design-system` existing on this host. Roll back by changing `/home/jmagar/.lab/config.toml` for `shadcn` back to `command = "npx"` and `args = ["shadcn@latest", "mcp"]`, then running `labby gateway reload`.

## Decisions Not Taken

- Did not keep the native `--cwd` form because ad hoc MCP probing showed it was not reliable in this launch path.
- Did not edit Lab source to fix the stale runtime snapshot during this session; a bead was created because the user request was registry wiring, and the registry path is now usable.
- Did not move old plan files because they were not proven completed by this session.

## References

- https://ui.shadcn.com/docs/mcp
- https://ui.shadcn.com/docs/registry/mcp
- https://ui.shadcn.com/docs/components-json

## Open Questions

- Why does `labby gateway get shadcn --json` still report zero runtime counts when `gateway test` and Code Mode calls succeed?
- Why did `bd update lab-efwj2 --description ... --json` report corrected content while `bd show lab-efwj2 --json` later displayed the original noisy description?

## Next Steps

- Work bead `lab-efwj2` to reconcile persistent runtime summaries with the working Code Mode/gateway test path.
- Re-run `labby gateway code exec --json --code 'async () => codemode.shadcn.search_items_in_registries({ registries: ["@aurora"], query: "button", limit: 5 })'` after any Lab gateway runtime changes.
- If this config needs to apply to other app projects, point the wrapper at that app's directory and add the `@aurora` registry entry to that app's `components.json`.
