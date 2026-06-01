---
name: agent-os
description: Health/status check for the agent-os Windows sandbox VM and its windows-mcp connection.
argument-hint: "[status]"
---

# /agent-os $ARGUMENTS

Run a health check of the agent-os sandbox, working **outermost → innermost** so the
first failing layer is the diagnosis. Use the configured values (do not hardcode):

- MCP URL:        `$CLAUDE_PLUGIN_OPTION_AGENT_OS_MCP_URL`
- VM host:        `$CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_HOST` (Docker host)
- Container:      `$CLAUDE_PLUGIN_OPTION_AGENT_OS_CONTAINER_NAME`
- Host-forward:   `$CLAUDE_PLUGIN_OPTION_AGENT_OS_HOST_FORWARD_SSH` port `$CLAUDE_PLUGIN_OPTION_AGENT_OS_HOST_FORWARD_PORT`
- VM Tailscale:   `$CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_TAILSCALE_IP`
- noVNC:          `$CLAUDE_PLUGIN_OPTION_AGENT_OS_NOVNC_URL`

Steps (stop and report at the first hard failure, but try to gather all of them):

1. **MCP endpoint** — POST an `initialize` to `$CLAUDE_PLUGIN_OPTION_AGENT_OS_MCP_URL` with
   `Authorization: Bearer $CLAUDE_PLUGIN_OPTION_AGENT_OS_MCP_TOKEN` and
   `Accept: application/json, text/event-stream`. Expect HTTP 200.
   - 401/403 → bearer token wrong (fix in plugin settings).
   - no response → VM or its `tailscale serve` exposure is down; continue to step 2.
   - **Never print the token** — only report the status code.
2. **Container** — on the Docker host: `ssh $CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_HOST 'docker ps --format "{{.Names}}\t{{.Status}}" | grep $CLAUDE_PLUGIN_OPTION_AGENT_OS_CONTAINER_NAME'`.
   If absent: `ssh $CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_HOST 'docker compose -f $CLAUDE_PLUGIN_OPTION_AGENT_OS_COMPOSE_FILE up -d'` (only if the user confirms).
3. **Guest Tailscale** — via the host-forward (NOT `ssh agent-os`):
   `ssh -p $CLAUDE_PLUGIN_OPTION_AGENT_OS_HOST_FORWARD_PORT $CLAUDE_PLUGIN_OPTION_AGENT_OS_HOST_FORWARD_SSH 'tailscale status'`.
   If stopped, see the skill's **Tailscale maintenance** section before bouncing it.
4. **tailscale serve mapping** — in the guest: `tailscale serve status` should show `/ proxy http://localhost:8000`.
5. **Gateway (if used)** — `lab gateway list | grep agent-os` should show `✓ … 🔧 18`. If stale, `lab gateway reload`.

Report a short per-layer ✓/✗ summary and, on any ✗, the single most likely fix. For deeper
issues, point at the agent-os skill's **Troubleshooting** section.
