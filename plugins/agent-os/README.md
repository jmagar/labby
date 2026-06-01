# agent-os

Drive the **agent-os** Windows 11 sandbox VM through its [Windows-MCP](https://github.com/CursorTouch/Windows-MCP) server. This plugin self-registers the `windows-mcp` MCP from your configured URL + bearer token, ships the `agent-os` skill, a `/agent-os` status command, and a SessionStart health check.

## What's inside

| Component | Purpose |
|---|---|
| `.mcp.json` | Registers the `windows-mcp` HTTP MCP server from userConfig (`${user_config.agent_os_mcp_url}` + bearer). |
| `skills/agent-os/` | The full agent-os skill: tool surface, recipes, Tailscale maintenance, Troubleshooting. |
| `commands/agent-os.md` | `/agent-os status` — runs the health ladder (MCP → container → guest Tailscale → serve mapping → gateway). |
| `hooks/hooks.json` + `scripts/setup.sh` | SessionStart / ConfigChange read-only health check; warns if unconfigured, unreachable, or the token is rejected. |

## Configuration

Set these in the plugin's settings (Claude Code prompts on enable). Values reach two surfaces with two syntaxes — see the skill's Configuration section:

- **config files** (`.mcp.json`, hook/command lines) use `${user_config.<key>}` (literal lowercase key).
- **subprocess scripts** get `$CLAUDE_PLUGIN_OPTION_<KEY>` (uppercased key).

| Key | Required | Default | What it is |
|---|---|---|---|
| `agent_os_mcp_url` | ✅ | `https://agent-os.manatee-triceratops.ts.net/mcp` | Windows-MCP `/mcp` endpoint (prefer the VM's own Tailscale/MagicDNS name). |
| `agent_os_mcp_token` | ✅ (sensitive) | — | Bearer token Windows-MCP expects. |
| `agent_os_vm_tailscale_ip` | | `100.109.125.128` | Guest's own Tailscale IP for `ssh agent-os` (port 22). |
| `agent_os_vm_host` | | `tootie` | Docker host running the container. |
| `agent_os_host_forward_ssh` | | `docker@100.120.242.29` | Host-forward SSH target (Tailscale maintenance — survives `tailscale down`). |
| `agent_os_host_forward_port` | | `2222` | Host-forward port → guest `:22`. |
| `agent_os_compose_file` | | `/mnt/cache/compose/windows/docker-compose.yml` | Compose file on the Docker host to bring the VM up. |
| `agent_os_container_name` | | `agent-os-win11` | dockur/windows container name. |
| `agent_os_novnc_url` | | `http://tootie:8006` | dockur/windows web (noVNC) UI for visual debugging. |
| `agent_os_autostart` | | `false` | `"true"` lets the SessionStart hook SSH to the Docker host and `docker compose up -d` the VM when the MCP endpoint is unreachable (requires `agent_os_vm_host` + `agent_os_compose_file`). Starts only, never stops; doesn't block on Windows boot. |

### Auto-recovery (opt-in)

By default the SessionStart hook is read-only — it probes the MCP endpoint and prints status. Set `agent_os_autostart` to `"true"` and, when the endpoint is unreachable, the hook will SSH to `agent_os_vm_host` and `docker compose -f <agent_os_compose_file> up -d` to bring the VM up (fire-and-forget; the Windows boot takes a few minutes, so re-run `/agent-os status` shortly after). It only ever *starts* the VM, and only when the container isn't already running. It does **not** install Windows-MCP — that's a one-time in-VM provisioning step.

The defaults are Jacob's homelab values — override for any other deployment. The VM is portable across Docker hosts; point `agent_os_mcp_url` at the VM's own Tailscale name so it follows the VM.

## Verifying it works

After enabling + configuring:

1. The SessionStart hook prints a status line (configured/reachable, or a warning).
2. `/mcp` should list `windows-mcp` as connected.
3. `/agent-os status` runs the full health ladder on demand.

If `windows-mcp` doesn't appear in `/mcp`, the most likely cause is the URL/token not being set — an unset `${user_config.*}` value leaves the server unregistered. See the skill's **Troubleshooting** section.

## Notes

- This skill was moved out of the `vibin` plugin into its own plugin so it can carry userConfig and self-register the MCP.
- It does **not** fire on the user's personal Windows (steamy-wsl) — that's the `nircmd` skill.
