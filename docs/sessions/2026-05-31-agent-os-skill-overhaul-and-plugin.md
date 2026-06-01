---
date: 2026-05-31 14:47:32 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 7c8d727d
working directory: /home/jmagar
worktree: /home/jmagar/workspace/lab
session id: 15d84a51-8c02-4fa3-be6e-0b6bbce7a408
beads: No bead activity observed
---

# agent-os: VM migration cleanup, skill overhaul, and standalone plugin

## User Request

Started as "what device is `ssh agent-os` actually connecting to?" (the SSH config pointed at dookie's Tailscale IP but agent-os now runs on tootie), then expanded into: clean up the stale VM, fix the agent-os skill, and finally "create a full Claude Code plugin for this skill so we can use userConfig," placed in `~/workspace/lab/plugins`.

## Session Overview

Diagnosed and resolved a duplicate agent-os VM situation (two `agent-os-win11` containers sharing one Tailscale identity), removed the stale dookie copy, repointed SSH at the VM's own Tailscale IP, then did a deep overhaul of the `agent-os` skill (webwright as #1 browser tool, four-layer Connection model, Tailscale-maintenance guidance, consolidated Troubleshooting, generalized GPUI guidance, fixed stale host/path references after the move). Finally extracted the skill into a brand-new standalone **`agent-os` plugin** under `plugins/agent-os/` that self-registers the `windows-mcp` MCP from `userConfig` and ships a `/agent-os` command + an opt-in SessionStart auto-recovery hook.

Note on scope: the early infrastructure work (SSH config, Tailscale bounce, gateway reload, global `~/.claude/CLAUDE.md` and auto-memory edits) happened outside this repo. The in-repo deliverable is the `plugins/agent-os/` plugin (and the skill it carries).

## Sequence of Events

1. Probed `dookie:2222` and `tootie:2222`: both reached a Windows VM reporting hostname `agent-os` and the same Tailscale IP `100.109.125.128`, but different Docker bridge IPs — proving two distinct cloned containers sharing one Tailscale identity (a collision).
2. With user authorization, tore down the stale dookie container (`docker compose down`) and deleted its compose dir `/home/jmagar/compose/windows` (~41G freed). Confirmed the surviving VM runs on tootie.
3. Updated SSH config `Host agent-os` → `HostName 100.109.125.128`, `Port 22` (the VM's own Tailscale node, not a docker host port-forward); removed the redundant `agent-dos` alias. Updated global CLAUDE.md and auto-memory.
4. Overhauled the agent-os skill in `vibin`: corrected host references (dookie→tootie), made noVNC = `tootie:8006`, set webwright as the #1 browser/web-dev choice, made triggers sandbox-centric (dropped host-bound triggers), added a four-layer Connection model, a Tailscale-maintenance section, and a consolidated Troubleshooting table; generalized the "Axon Palette" war story into reusable GPUI-app guidance.
5. Investigated two flagged issues: `steamy-windows-mcp` showed `✗` in the gateway (intermittent — server up, token valid, a `list_tools` discovery timeout / desktop availability flake, not auth); `agent-os` Tailscale read "offline" while reachable on-LAN ("not in map poll"). Bounced the guest's tailscaled — initially severed the SSH session by running `tailscale down` over the guest's own Tailscale IP, recovered via the host-forward path, then a clean `tailscale up` cleared the offline state.
6. Built the standalone `agent-os` plugin: moved the skill out of `vibin`, added `plugin.json` (userConfig), `.mcp.json`, `hooks/hooks.json` + `scripts/setup.sh`, `commands/agent-os.md`, README, CHANGELOG.
7. Verified `userConfig` wiring against the official plugins-reference (local copy) + the Axon plugin's own wiring doc — corrected the substitution form twice (from inferred `${CLAUDE_PLUGIN_OPTION_*}` → wrong `${CLAUDE_PLUGIN_CONFIG_*}` → correct `${user_config.<key>}`).
8. Added opt-in SSH auto-recovery to `setup.sh` (`agent_os_autostart`): brings the VM up via `docker compose up -d` when the MCP endpoint is unreachable; start-only, non-blocking, gated.

## Key Findings

- **Plugin userConfig has two substitution surfaces with different syntaxes** (verified against the official `plugins-reference` and the Axon plugin): `${user_config.<key>}` in `.mcp.json` / hook commands / monitor commands, and `$CLAUDE_PLUGIN_OPTION_<KEY>` (uppercased) as env vars inside subprocess scripts. There is no `${CLAUDE_PLUGIN_CONFIG_*}` form.
- **HTTP-type MCP servers are registered, not installed, by a plugin.** `windows-mcp` is `type: http`; the plugin's `.mcp.json` only registers a client connection to the already-running in-VM server. A plugin cannot install/launch a remote HTTP MCP server (unlike a `stdio` server, which it spawns).
- The agent-os VM (`agent-os-win11`, dockur/windows) now lives only on tootie; compose at `/mnt/cache/compose/windows/docker-compose.yml`, VM disk at `/mnt/cache/appdata/windows/storage`, noVNC at `tootie:8006`, exposed via in-guest `tailscale serve` (`/ → http://localhost:8000`).
- `steamy-windows-mcp` `✗` was not an auth/token problem — the token is valid; the failure is an intermittent `list_tools` discovery timeout against a personal desktop.

## Technical Decisions

- **Standalone plugin over keeping the skill in vibin** — lets it carry `userConfig` and self-register the MCP; matches the in-progress migration of skills out of the monolithic `vibin` plugin.
- **`.mcp.json` HTTP registration via `${user_config.*}`** rather than editing `~/.claude.json` — the connection follows the plugin and is configured through plugin settings.
- **Auto-recovery is opt-in (`agent_os_autostart`, default `false`)** and start-only/non-blocking — never silently boots a deliberately-stopped VM, and respects the 30s SessionStart timeout.
- Documented the honest boundary: the plugin starts an already-provisioned VM; it does not install Windows-MCP from scratch.

## Files Changed

Only `plugins/agent-os/**` is the in-repo deliverable of this session. (The skill files were moved out of `plugins/vibin/skills/agent-os/`; that deletion is part of a broader, pre-existing uncommitted migration in the working tree — see Repository Maintenance.)

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| created | plugins/agent-os/.claude-plugin/plugin.json | — | manifest with 10 userConfig fields | `python3 -c json.load` OK; 10 keys |
| created | plugins/agent-os/.mcp.json | — | registers windows-mcp (http) via `${user_config.agent_os_mcp_url}` + bearer | JSON valid; `user_config` refs present, no stray `CLAUDE_PLUGIN_CONFIG` |
| created | plugins/agent-os/hooks/hooks.json | — | SessionStart + ConfigChange → setup.sh | JSON valid |
| created | plugins/agent-os/scripts/setup.sh | — | health probe + opt-in SSH auto-recovery | `bash -n` OK; executable (`-rwxrwxr-x`) |
| created | plugins/agent-os/commands/agent-os.md | — | `/agent-os` status health-ladder command | frontmatter present |
| created | plugins/agent-os/README.md | — | userConfig + setup + auto-recovery docs | written |
| created | plugins/agent-os/CHANGELOG.md | — | 0.1.0 entry | written |
| renamed | plugins/agent-os/skills/agent-os/SKILL.md | plugins/vibin/skills/agent-os/SKILL.md | the skill, parameterized to userConfig | moved + edited |
| renamed | plugins/agent-os/skills/agent-os/README.md | plugins/vibin/skills/agent-os/README.md | skill readme | moved |
| renamed | plugins/agent-os/skills/agent-os/CHANGELOG.md | plugins/vibin/skills/agent-os/CHANGELOG.md | skill changelog | moved |
| renamed | plugins/agent-os/skills/agent-os/agents/openai.yaml | plugins/vibin/skills/agent-os/agents/openai.yaml | agent def | moved |

## Beads Activity

No bead activity observed. No beads were created, claimed, edited, commented on, or closed during this session.

## Repository Maintenance

- **Plans**: Checked `docs/plans/` — `fleet-ws-plan-lab-n07n.md` and `mcp-streamable-http-oauth-proxy.md` remain; neither relates to this session and neither was completed here, so both were left in place (not moved to `docs/plans/complete/`).
- **Beads**: No bead activity this session; no tracker state changed.
- **Worktrees/branches**: `git worktree list` shows a single worktree on `main` (in sync with `origin/main`, 0 ahead / 0 behind). No branches or worktrees cleaned up — none were stale or owned by this session.
- **Working tree caveat (important)**: The tree has extensive **pre-existing uncommitted changes not made by this session** — mass deletions across `plugins/vibin/skills/*` (adguard, navidrome, plex, sonarr, …), the entire `plugins/tracearr/`, a modified `.claude-plugin/marketplace.json`, and new untracked `plugins/{arrs,navidrome,testing,uptime-kuma}/`. This looks like an in-progress extraction of skills into standalone plugins. This session deliberately did **not** stage or commit any of it.
- **Stale docs**: The agent-os skill's own stale host/path references (post VM-move) were corrected as part of the skill overhaul. No other repo docs were in scope.
- **Commit safety**: This session log is committed path-limited (only the artifact), so the unrelated dirty tree is not swept in.

## Tools and Skills Used

- **Shell (Bash)**: extensive — SSH probes to dookie/tootie/agent-os/steamy-wsl, `docker`/`docker compose`, `tailscale status/serve/ping`, `curl` MCP handshakes, `git`, JSON/bash linting. Issues: intermittent tool-result rendering (commands re-run to confirm); `powershell.exe` not on steamy-wsl's non-interactive PATH (Windows-side probes via that path returned empty until switched to encoded-command / direct approaches).
- **File tools (Read/Write/Edit)**: skill/plugin authoring and edits. A few Edits failed on stale `old_string` after the file changed mid-flight; re-read and reapplied.
- **MCP (labby gateway `search`/`execute`)**: used to attempt Axon `ask`; failed with `oauth_needs_reauth` for the `axon` upstream — fell back to a local copy of the official plugins-reference and the Axon plugin's wiring doc.
- **WebFetch**: official Claude Code plugins-reference (redirected docs.claude.com → code.claude.com); confirmed userConfig schema.
- **Skills**: `plugin-dev:plugin-structure`, `plugin-dev:mcp-integration` (layout + MCP config conventions); `vibin:save-to-md` (this log).
- **Memory**: wrote `reference_agent_os_vm.md`, `reference_plugin_userconfig.md` to auto-memory (outside this repo).

## Commands Executed

| command | result |
|---|---|
| `docker compose down` (dookie compose/windows) | removed stale `agent-os-win11`; ~41G freed after dir delete |
| `ssh -p 2222 docker@100.120.242.29 'tailscale up'` (after service restart) | restored guest Tailscale; control plane went `active; direct`, no longer "offline" |
| `lab gateway reload` | re-probed upstreams; agent-os `✓ 🔧 18` once VM reachable |
| `curl -X POST $URL initialize` (with token) | HTTP 200 in ~5ms — windows-mcp token valid, server healthy |
| `python3 -c json.load(...)` on plugin JSON | all OK |
| `bash -n scripts/setup.sh` | syntax OK; file executable |

## Errors Encountered

- **Self-severed SSH during Tailscale bounce**: ran `tailscale down` over `ssh agent-os` (the guest's own Tailscale IP), which killed the session before `tailscale up` ran, leaving Tailscale stopped. Resolved by reconnecting via the host-forward (`ssh -p 2222 docker@<tootie>`), restarting the Tailscale service, and `tailscale up`. Captured as a permanent warning in the skill's Tailscale-maintenance section.
- **userConfig substitution form wrong twice**: inferred `${CLAUDE_PLUGIN_OPTION_*}`, then `${CLAUDE_PLUGIN_CONFIG_*}`, both wrong for `.mcp.json`. Verified against the official reference + Axon plugin: correct form is `${user_config.<key>}`. Fixed across `.mcp.json`, skill, README, CHANGELOG, memory.
- **Axon MCP `ask` unavailable**: `oauth_needs_reauth` on the `axon` upstream; used local authoritative sources instead.

## Behavior Changes (Before/After)

| Area | Before | After |
|---|---|---|
| `ssh agent-os` | pointed at dookie:2222 (stale clone) | VM's own Tailscale IP `100.109.125.128:22` (single VM on tootie) |
| Duplicate VM | two `agent-os-win11` containers, one Tailscale identity | single VM on tootie; dookie clone removed (~41G freed) |
| agent-os skill | in `vibin`, stale dookie refs, hardcoded hosts | standalone plugin, parameterized to userConfig, current refs |
| windows-mcp registration | manual / global | self-registered by the plugin from userConfig (`.mcp.json`) |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `python3 -c json.load` × 3 (plugin.json/.mcp.json/hooks.json) | valid JSON | OK ×3 | pass |
| `grep user_config .mcp.json` / `grep CLAUDE_PLUGIN_CONFIG` | user_config present, no CONFIG | matched / none | pass |
| `bash -n scripts/setup.sh` + exec bit | syntax OK + executable | OK, `-rwxrwxr-x` | pass |
| `curl initialize` to windows-mcp w/ token | HTTP 200 | HTTP 200 (~5ms) | pass |
| `lab gateway list \| grep agent-os` | `✓ … 🔧 18` | `✓ … 🔧 18` | pass |

## Risks and Rollback

- **Auto-recovery SSHes out and starts a VM.** Gated behind `agent_os_autostart="true"` (default off), start-only, non-blocking. Rollback: leave the flag unset → hook is read-only.
- **`.mcp.json` substitution unproven end-to-end in this exact build.** Form matches the official reference and the working Axon plugin, but the only true confirmation is enabling the plugin and seeing `windows-mcp` connect in `/mcp`. Rollback: delete `plugins/agent-os/` (untracked) — nothing else depends on it.

## Decisions Not Taken

- **Did not commit the plugin or the broader migration.** The working tree holds an unrelated in-progress migration; a blanket commit would conflate it. Left for the user to scope.
- **Did not write a from-scratch in-VM installer** (`install-windows-mcp.ps1`) — offered, not requested.

## References

- Official Claude Code plugins reference (local copy): `…/plugins/cache/jmagar-lab/rust-bin-tools/*/skills/agent-config/references/claude/plugins-reference.md`
- Axon plugin userConfig→MCP wiring: `…/plugins/cache/jmagar-lab/axon/*/docs/sessions/2026-05-06-plugin-mcp-userconfig-wiring.md`
- Sibling reference plugins: `plugins/navidrome`, `plugins/uptime-kuma` (userConfig + setup.sh pattern)

## Open Questions

- The `save-to-md` context injected a transcript path under `-home-jmagar-workspace-lab` (`617c1932…`), but this conversation is session `15d84a51…` running from `/home/jmagar`. This log was written from the live conversation context, not that transcript, because they appear to be different sessions.
- Whether `${user_config.*}` substitution resolves correctly in this Claude Code build's `.mcp.json` HTTP url/headers — needs the live `/mcp` check.

## Next Steps

- **Verify the plugin**: enable + configure `agent-os` (set `agent_os_mcp_url` + `agent_os_mcp_token`), then confirm `windows-mcp` appears connected in `/mcp` and `/agent-os status` runs the health ladder.
- **Commit scoping** (user's call): scoped commit of just the agent-os move —
  `git add plugins/agent-os plugins/vibin/skills/agent-os && git commit -m "feat(agent-os): extract into standalone plugin with userConfig + self-registered windows-mcp"` — or fold it into the broader skills-migration commit the working tree is mid-way through.
- **Marketplace**: add the new `agent-os` plugin entry to `.claude-plugin/marketplace.json` if it should be published (not done this session).
- **Optional**: write `scripts/install-windows-mcp.ps1` for true from-scratch in-VM provisioning.
