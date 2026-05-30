#!/usr/bin/env bash
# Connectivity check for the Unraid GraphQL skill.
# Reads credentials from ~/.lab/.env and pings the GraphQL endpoint.
# No MCP server or extra dependencies required — just curl + jq.

set -euo pipefail

ENV_FILE="${LAB_ENV_FILE:-$HOME/.lab/.env}"

echo "=== Unraid GraphQL skill — connectivity check ==="
echo ""

for bin in curl jq; do
  command -v "$bin" >/dev/null 2>&1 || { echo "Error: '$bin' is required but not installed."; exit 1; }
done

[ -f "$ENV_FILE" ] || { echo "Error: $ENV_FILE not found. Create it and add UNRAID_URL / UNRAID_API_KEY."; exit 1; }

UNRAID_URL=$(grep -E '^UNRAID_URL='     "$ENV_FILE" | cut -d= -f2-)
UNRAID_API_KEY=$(grep -E '^UNRAID_API_KEY=' "$ENV_FILE" | cut -d= -f2-)

if [ -z "$UNRAID_URL" ] || [ -z "$UNRAID_API_KEY" ]; then
  echo "Error: UNRAID_URL and/or UNRAID_API_KEY are empty in $ENV_FILE."
  echo "Get a key in the Unraid UI: Settings -> Management Access -> API Keys -> Create."
  exit 1
fi

echo "Endpoint: $UNRAID_URL/graphql"
echo "Pinging..."
resp=$(curl -sSk --max-time 15 "$UNRAID_URL/graphql" \
  -H "x-api-key: $UNRAID_API_KEY" -H 'Content-Type: application/json' \
  -d '{"query":"{ info { os { hostname } } }"}')

host=$(printf '%s' "$resp" | jq -r '.data.info.os.hostname // empty' 2>/dev/null || true)
if [ -n "$host" ]; then
  echo "✓ Connected to Unraid host: $host"
else
  echo "✗ Unexpected response:"
  printf '%s\n' "$resp"
  exit 1
fi
