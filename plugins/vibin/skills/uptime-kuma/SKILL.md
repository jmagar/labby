---
name: uptime-kuma
description: Lab's Uptime Kuma integration — Self-hosted uptime and status page monitoring. Use when the user wants to manage their Uptime Kuma instance, or invokes `lab uptime-kuma` / `mcp__lab__uptime_kuma`. Calls the MCP tool first, falls back to the CLI if MCP is unavailable.
---

# Uptime Kuma

Self-hosted uptime and status page monitoring. Exposes **9 actions** via the `lab` homelab control plane.

## How to call it

**Prefer the MCP tool. Fall back to the CLI only when MCP is unavailable.**

### MCP (preferred)

One tool: `mcp__lab__uptime_kuma`. Dispatch shape: `{ "action": "<name>", "params": {...} }`.

Discover actions live:
```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "<name>" } }
```

Full action catalog: [`references/mcp.md`](references/mcp.md).

### CLI fallback

```bash
lab uptime-kuma --help
lab uptime-kuma <action> --help
labby --json uptime-kuma <action> ...
```

CLI mirrors MCP actions; dots become dashes (`server.health` → `server-health`). Full CLI surface: [`references/cli.md`](references/cli.md).

## Highlights

- `contract.status` — Get Uptime Kuma contract/API status
- `server.health` — Check Uptime Kuma server health
- `monitor.list` — List all monitors
- `monitor.get` — Get details for a specific monitor
- `monitor.create` — Create a new monitor
- `monitor.update` — Update an existing monitor
- `monitor.delete` — Delete a monitor
- `monitor.pause` — Pause a monitor
- `monitor.resume` — Resume a paused monitor

## Destructive actions

uptime-kuma exposes 5 destructive action(s): `monitor.create`, `monitor.update`, `monitor.delete`, `monitor.pause`, `monitor.resume`. These mutate state — confirm with the user before invoking. The full `Destructive` column is in `references/mcp.md`.

## Configuration

Credentials and base URLs live in `~/.lab/.env`. Onboard / re-extract with
`labby extract scan` and `labby extract apply`. Verify connectivity:

```bash
labby doctor service uptime-kuma
```

## When NOT to use this skill

- The user is asking about a different lab service — load that service's skill instead.
- The user is asking about `lab` itself (CLI internals, install, gateway, doctor across all services) — that's operator-tier, not `uptime-kuma`-specific.
