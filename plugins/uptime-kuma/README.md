# uptime-kuma

Read-only monitoring of a self-hosted [Uptime Kuma](https://github.com/louislam/uptime-kuma) instance via direct HTTP — no MCP server, no `lab` CLI dependency.

## What it does

- **`/metrics`** (Prometheus, API-key Basic auth): current up/down state, response times, TLS cert expiry per monitor.
- **Status-page JSON** (public, no auth): published status page config + live heartbeats/uptime.

## What it does NOT do

Monitor management — create / edit / pause / delete / acknowledge — is **not possible over plain HTTP**. Uptime Kuma exposes those only through its socket.io API (what the web UI uses). Use the web UI or a socket.io client (e.g. the `uptime-kuma-api` Python library) for management. This plugin is deliberately read-only.

## Configuration

Set when the plugin is enabled (the API key goes to secure OS storage, not `settings.json`):

| Setting | Sensitive | Description |
|---|---|---|
| Uptime Kuma URL | no | Base URL, e.g. `https://status.example.com` |
| Metrics API key | yes | For `/metrics` HTTP Basic auth (Settings → API Keys) |
| Status page slug | no | Optional; enables public status-page JSON reads |

A `SessionStart` hook checks reachability of `/metrics` and/or the status page and prints a one-line status; it changes nothing.

See the skill at `skills/uptime-kuma/SKILL.md` and the reference at `skills/uptime-kuma/references/api.md`.
