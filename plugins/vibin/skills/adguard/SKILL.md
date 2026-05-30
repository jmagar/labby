---
name: adguard
description: Lab's Adguard integration — DNS-level ad blocking and network filtering. Use when the user wants to manage their Adguard instance, or invokes `lab adguard` / `mcp__lab__adguard`. Calls the MCP tool first, falls back to the CLI if MCP is unavailable.
---

# Adguard

Dns-level ad blocking and network filtering. Exposes **6 actions** via the `lab` homelab control plane.

## How to call it

**Prefer the MCP tool. Fall back to the CLI only when MCP is unavailable.**

### MCP (preferred)

One tool: `mcp__lab__adguard`. Dispatch shape: `{ "action": "<name>", "params": {...} }`.

Discover actions live:
```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "<name>" } }
```

Full action catalog: [`references/mcp.md`](references/mcp.md).

### CLI fallback

```bash
lab adguard --help
lab adguard <action> --help
labby --json adguard <action> ...
```

CLI mirrors MCP actions; dots become dashes (`server.health` → `server-health`). Full CLI surface: [`references/cli.md`](references/cli.md).

## Highlights

- `server.status` — Get AdGuard Home server status and running state
- `server.version` — Get AdGuard Home version info
- `stats.summary` — Get DNS query statistics summary
- `querylog.search` — Search the DNS query log
- `filtering.status` — Get filtering rules status and stats
- `filtering.check-host` — Check whether a host is blocked

## Configuration

Credentials and base URLs live in `~/.lab/.env`. Onboard / re-extract with
`labby extract scan` and `labby extract apply`. Verify connectivity:

```bash
labby doctor service adguard
```

## When NOT to use this skill

- The user is asking about a different lab service — load that service's skill instead.
- The user is asking about `lab` itself (CLI internals, install, gateway, doctor across all services) — that's operator-tier, not `adguard`-specific.
