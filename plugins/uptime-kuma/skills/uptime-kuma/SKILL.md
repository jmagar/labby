---
name: uptime-kuma
description: Self-hosted Uptime Kuma monitoring, read-only via direct HTTP. Use when the user wants to check monitor up/down state, uptime, response times, or a status page. Reads the Prometheus /metrics endpoint (API-key auth) and public status-page JSON. Does NOT create, edit, pause, or delete monitors — that requires Uptime Kuma's socket.io API, which is not reachable over plain HTTP.
---

# Uptime Kuma

Self-hosted uptime and status-page monitoring. Uptime Kuma has **no REST management API** — monitor configuration happens over a socket.io connection used by its own web UI. What *is* reachable with plain HTTP is **read-only**: the Prometheus `/metrics` endpoint and any published status page's JSON. This skill covers those.

## Configuration

The user sets URL / metrics API key / status-page slug in the plugin's user configuration. A `SessionStart` hook writes them into a 600-mode file:

```
${XDG_CONFIG_HOME:-$HOME/.config}/lab-uptime-kuma/config.env
```

> Why a file and not env vars: Claude Code injects `CLAUDE_PLUGIN_OPTION_*` only into plugin subprocesses (hooks/MCP/LSP), **not** into the Bash tool that runs these commands. The hook (a subprocess) reads them and materializes this file; the skill sources it.

Load it first; if missing, the plugin isn't configured yet:

```bash
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/lab-uptime-kuma/config.env"
[ -f "$CONFIG" ] || { echo "uptime-kuma not configured — set the URL/API key in plugin settings"; }
. "$CONFIG"   # exports UPTIME_KUMA_URL, UPTIME_KUMA_METRICS_API_KEY, UPTIME_KUMA_STATUS_SLUG
URL="${UPTIME_KUMA_URL%/}"
```

## Read monitor state via /metrics (needs the API key)

Uptime Kuma exposes Prometheus metrics. Auth is HTTP Basic with an **empty username** and the API key as the password (create the key under Settings → API Keys in the UI).

```bash
curl -sS -u ":$UPTIME_KUMA_METRICS_API_KEY" "$URL/metrics"
```

Key metrics (each labeled with `monitor_name`, `monitor_type`, `monitor_url`):

| Metric | Meaning |
|---|---|
| `monitor_status` | current state: `1` up, `0` down, `2` pending, `3` maintenance |
| `monitor_response_time` | last response time (ms) |
| `monitor_cert_days_remaining` | TLS cert days left |
| `monitor_cert_is_valid` | `1` valid, `0` not |

Examples:

```bash
# Which monitors are down right now?
curl -sS -u ":$UPTIME_KUMA_METRICS_API_KEY" "$URL/metrics" \
  | grep '^monitor_status' | grep ' 0$'

# Response time per monitor
curl -sS -u ":$UPTIME_KUMA_METRICS_API_KEY" "$URL/metrics" \
  | grep '^monitor_response_time'
```

> Never echo `$UPTIME_KUMA_METRICS_API_KEY`.

## Read a public status page (no auth)

If a status page is published, its config and live heartbeats are plain JSON:

```bash
SLUG="${UPTIME_KUMA_STATUS_SLUG}"
curl -sS "$URL/api/status-page/$SLUG"               # groups, monitors, incident, maintenance
curl -sS "$URL/api/status-page/heartbeat/$SLUG"     # per-monitor heartbeat list + uptime + avg ping
```

`heartbeat` returns `heartbeatList` (recent up/down samples) and `uptimeList` (rolling uptime ratios) keyed by monitor id. Pipe through `jq` to summarize.

Full endpoint reference: [`references/api.md`](references/api.md).

## What this skill CANNOT do

Creating, editing, pausing, resuming, or deleting monitors; reading the full monitor list with config; acknowledging incidents — **all require the socket.io API** (the protocol the web UI speaks), not plain HTTP. If the user asks for any of these:

- Say it's not available over HTTP and point them at the web UI, **or**
- Note that programmatic management needs a socket.io client (e.g. the `uptime-kuma-api` Python library), which is outside this skill's direct-curl scope.

Do not fabricate a REST call for management actions — there isn't one.

## When NOT to use this skill

- The user wants to manage monitors (see above) — this skill is read-only.
- The user is asking about a different homelab service — load that service's skill.
