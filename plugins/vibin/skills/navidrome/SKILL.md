---
name: navidrome
description: Lab's Navidrome integration — Self-hosted music streaming server. Use when the user wants to manage their Navidrome instance, or invokes `lab navidrome` / `mcp__lab__navidrome`. Calls the MCP tool first, falls back to the CLI if MCP is unavailable.
---

# Navidrome

Self-hosted music streaming server. Exposes **6 actions** via the `lab` homelab control plane.

## How to call it

**Prefer the MCP tool. Fall back to the CLI only when MCP is unavailable.**

### MCP (preferred)

One tool: `mcp__lab__navidrome`. Dispatch shape: `{ "action": "<name>", "params": {...} }`.

Discover actions live:
```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "<name>" } }
```

Full action catalog: [`references/mcp.md`](references/mcp.md).

### CLI fallback

```bash
lab navidrome --help
lab navidrome <action> --help
labby --json navidrome <action> ...
```

CLI mirrors MCP actions; dots become dashes (`server.health` → `server-health`). Full CLI surface: [`references/cli.md`](references/cli.md).

## Highlights

- `server.ping` — Ping the Navidrome server
- `artist.list` — List all artists
- `album.list` — List albums with optional type/sort
- `album.get` — Get details for a specific album
- `search.query` — Search artists, albums, and tracks
- `playlist.list` — List all playlists

## Configuration

Credentials and base URLs live in `~/.lab/.env`. Onboard / re-extract with
`labby extract scan` and `labby extract apply`. Verify connectivity:

```bash
labby doctor service navidrome
```

## When NOT to use this skill

- The user is asking about a different lab service — load that service's skill instead.
- The user is asking about `lab` itself (CLI internals, install, gateway, doctor across all services) — that's operator-tier, not `navidrome`-specific.
