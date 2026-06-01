#!/usr/bin/env bash
# SessionStart / ConfigChange hook for the agent-os plugin.
#
# By default this is READ-ONLY: it validates that the Windows-MCP endpoint is
# configured and reachable and prints a one-line status. The windows-mcp server
# is registered declaratively in ../.mcp.json from the same userConfig values
# (${user_config.agent_os_mcp_url} + token) — this script does not register it.
#
# OPT-IN RECOVERY: if `agent_os_autostart` is "true" AND the endpoint is
# unreachable, it will SSH to the Docker host and `docker compose up -d` the VM
# (fire-and-forget — a Windows boot takes minutes; it does not block on it).
# It never stops/removes anything. Default is off so a session never silently
# boots a VM you deliberately stopped.
#
# userConfig values arrive here as CLAUDE_PLUGIN_OPTION_* env vars (see the
# Claude Code plugins reference). In .mcp.json / hook commands the form is
# ${user_config.<key>} instead — different surface, different syntax.
set -euo pipefail

URL="${CLAUDE_PLUGIN_OPTION_AGENT_OS_MCP_URL:-}"
TOKEN="${CLAUDE_PLUGIN_OPTION_AGENT_OS_MCP_TOKEN:-}"
VM_HOST="${CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_HOST:-}"
CONTAINER="${CLAUDE_PLUGIN_OPTION_AGENT_OS_CONTAINER_NAME:-agent-os-win11}"
COMPOSE_FILE="${CLAUDE_PLUGIN_OPTION_AGENT_OS_COMPOSE_FILE:-}"
AUTOSTART="${CLAUDE_PLUGIN_OPTION_AGENT_OS_AUTOSTART:-false}"

# Reject control-character injection in configured values (these get interpolated
# into an ssh remote command below).
for v in "$URL" "$TOKEN" "$VM_HOST" "$CONTAINER" "$COMPOSE_FILE" "$AUTOSTART"; do
  case "$v" in
    *$'\n'* | *$'\r'*)
      echo "agent-os: config values must not contain newlines" >&2
      exit 2
      ;;
  esac
done

if [ -z "$URL" ] || [ -z "$TOKEN" ]; then
  echo "agent-os: not configured yet — set the Windows-MCP URL and bearer token in plugin settings. The windows-mcp server will not connect until both are set." >&2
  exit 0
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "agent-os: curl not found; skipping reachability check (windows-mcp registration still applies)." >&2
  exit 0
fi

probe() {
  # Echoes the HTTP status of an MCP initialize handshake (000 = no response).
  curl -s -o /dev/null -w '%{http_code}' \
    --connect-timeout 4 --max-time 10 \
    -X POST "$URL" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"agent-os-setup","version":"1"}}}' \
    2>/dev/null || echo "000"
}

# Opt-in: try to bring the VM up over SSH when the endpoint is unreachable.
# Returns 0 if it kicked off a start, 1 otherwise. Never blocks on Windows boot.
recover() {
  if [ "$AUTOSTART" != "true" ]; then
    echo "WARNING: agent-os windows-mcp at ${URL} is unreachable. Is the VM up?${VM_HOST:+ On ${VM_HOST}: docker ps | grep ${CONTAINER}.} Set agent_os_autostart=true to auto-start it, or run /agent-os status. See the skill's Troubleshooting section." >&2
    return 1
  fi
  if [ -z "$VM_HOST" ] || [ -z "$COMPOSE_FILE" ]; then
    echo "WARNING: agent-os autostart is on but agent_os_vm_host and/or agent_os_compose_file are not set — cannot bring the VM up. ${URL} is unreachable." >&2
    return 1
  fi
  if ! command -v ssh >/dev/null 2>&1; then
    echo "WARNING: agent-os autostart is on but ssh is not available on this host. ${URL} is unreachable." >&2
    return 1
  fi

  local ssh_opts=(-o ConnectTimeout=6 -o BatchMode=yes -o StrictHostKeyChecking=accept-new)

  # If the container is already running, starting it won't help — the guest is
  # likely still booting or its tailscale-serve mapping is down.
  if ssh "${ssh_opts[@]}" "$VM_HOST" "docker ps --format '{{.Names}}' | grep -qx '$CONTAINER'" 2>/dev/null; then
    echo "agent-os: container '$CONTAINER' is already running on ${VM_HOST}, but ${URL} isn't answering — the guest may still be booting, or its Tailscale/serve exposure is down. Run /agent-os status (see the skill's Tailscale maintenance section)." >&2
    return 1
  fi

  echo "agent-os: ${URL} unreachable and autostart is on — bringing the VM up on ${VM_HOST}…" >&2
  if ssh "${ssh_opts[@]}" "$VM_HOST" "docker compose -f '$COMPOSE_FILE' up -d" >/dev/null 2>&1; then
    echo "agent-os: started '$CONTAINER' on ${VM_HOST}. Windows boot + Windows-MCP startup takes a few minutes — rerun /agent-os status shortly; tools will appear once it's up." >&2
    return 0
  fi
  echo "WARNING: agent-os could not start '$CONTAINER' on ${VM_HOST} (ssh or 'docker compose up' failed). Bring it up manually: ssh ${VM_HOST} 'docker compose -f ${COMPOSE_FILE} up -d'." >&2
  return 1
}

code="$(probe)"

case "$code" in
  200|202)
    echo "agent-os: windows-mcp reachable and authenticated at ${URL}" ;;
  401|403)
    echo "WARNING: agent-os windows-mcp at ${URL} rejected the bearer token (HTTP ${code}) — check the token in plugin settings." >&2 ;;
  000)
    recover || true ;;
  *)
    echo "WARNING: agent-os windows-mcp at ${URL} returned HTTP ${code} (expected 200). The endpoint is reachable but may not be a healthy MCP server." >&2 ;;
esac

exit 0
