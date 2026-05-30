---
name: dozzle
description: Direct Dozzle API workflow for the real-time Docker container log viewer. Use when the user wants to inspect Dozzle health, version, containers, logs, authentication failures, session-cookie refresh, auth setup, or Dozzle MCP. Uses `DOZZLE_URL` and optional `DOZZLE_SESSION_COOKIE`; do not route through Lab MCP or `lab dozzle`.
---

# Dozzle

Real-time Docker container log viewer.

## How to call it

Use Dozzle's HTTP API directly. Do not use Lab MCP, `mcp__lab__dozzle`,
`lab dozzle`, or `labby dozzle`; those paths are stale for this service.

### Configuration

Read connection values from the environment when available:

```bash
DOZZLE_URL                 # base URL, for example http://host:8080
DOZZLE_SESSION_COOKIE      # optional raw Cookie header value
```

If the shell environment is not already populated, `~/.lab/.env` may contain
these values. Source it only inside a subshell and suppress source output.

### Security Model

Dozzle's local web API routes are implementation details, not a stable public
REST contract. Use them as best-effort operational probes and keep calls
read-only unless the user explicitly asks for a mutating action.

Check the root `config__json` for `authProvider`, enabled shell/actions, and
host inventory. If `authProvider` is `none`, access control is provided only by
the surrounding network/proxy boundary. Treat direct Tailnet or LAN URLs as
bypasses around Authelia/forward-proxy auth unless ACLs restrict them.

Dozzle can reach Docker hosts, and Docker socket access is highly privileged.
If shell/actions/download are enabled, prefer an authenticated/proxied path and
do not invoke shell or action endpoints without explicit user intent.

### Auth

Dozzle may run with no auth or with a browser session cookie. Probe without a
cookie first when `DOZZLE_SESSION_COOKIE` is unset. When the cookie is set, pass
it as a `Cookie` header through stdin config, not as a command-line argument:

```bash
(
  set +x
  set -a; . ~/.lab/.env >/dev/null 2>&1 || true; set +a
  /usr/bin/curl -fsS --config - "$DOZZLE_URL/api/version" <<EOF
header = "Cookie: ${DOZZLE_SESSION_COOKIE}"
EOF
)
```

Never echo the cookie or include it in logs. If a request returns `401` or
`403`, help the user refresh the browser session cookie without pasting it into
chat.

## Common API Checks

Use `/usr/bin/curl` if shell startup or sourced env files alter `PATH`.

```bash
(
  set +x
  set -a; . ~/.lab/.env >/dev/null 2>&1 || true; set +a
  /usr/bin/curl -fsS "$DOZZLE_URL/api/version"
  /usr/bin/curl -fsS "$DOZZLE_URL/" | sed -n '/config__json/,/<\\/script>/p'
)
```

For endpoint details, safe cookie helpers, and container/log workflows, read
[`references/api.md`](references/api.md).

For configuring Dozzle auth providers or MCP, read
[`references/auth-mcp.md`](references/auth-mcp.md).

## When NOT to use this skill

- The user is asking about a different lab service - load that service's skill instead.
- The user is asking about Lab MCP, Lab CLI internals, or gateway behavior - use the Lab/operator skill instead.
