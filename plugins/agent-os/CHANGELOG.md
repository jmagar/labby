# Changelog

## 0.1.0 — 2026-05-31

Initial release as a standalone plugin (extracted from `vibin`).

- **Self-registering MCP.** `.mcp.json` registers the `windows-mcp` HTTP server from userConfig via `${user_config.agent_os_mcp_url}` + bearer token — no `~/.claude.json` edit required.
- **userConfig** for all connection/host details: MCP URL + token (sensitive), VM Tailscale IP, Docker host, host-forward SSH/port, compose file, container name, noVNC URL.
- **`/agent-os` command** — runs the health ladder (MCP → container → guest Tailscale → serve mapping → gateway).
- **SessionStart / ConfigChange hook** (`scripts/setup.sh`) — health check that distinguishes unconfigured vs. token-rejected vs. unreachable. With `agent_os_autostart="true"` it also brings the VM up over SSH (`docker compose up -d`) when the endpoint is down — start-only, non-blocking, opt-in; never installs the server.
- **Skill** carried over from `vibin` with: webwright as the #1 browser/web-dev choice, a four-layer Connection model, Tailscale-maintenance guidance (don't bounce Tailscale over `ssh agent-os`), and a consolidated Troubleshooting table. Host-bound triggers removed in favor of sandbox-centric ones.
- Documents the Claude Code substitution split: `${user_config.<key>}` in `.mcp.json`/hooks/commands vs. `$CLAUDE_PLUGIN_OPTION_<KEY>` env vars inside subprocess scripts.
