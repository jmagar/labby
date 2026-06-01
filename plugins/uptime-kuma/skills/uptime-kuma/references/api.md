# Uptime Kuma — HTTP read surfaces

Uptime Kuma's primary API is **socket.io** (used by its own UI) and is *not* covered here. Only the two plain-HTTP read surfaces below are usable with `curl`.

## 1. Prometheus metrics — `GET /metrics`

- **Auth:** HTTP Basic, empty username + API key as password (`curl -u ":$KEY"`). API keys are created under Settings → API Keys. If no key exists and metrics auth is disabled, it may be open — but assume auth.
- **Format:** Prometheus text exposition.

Metrics (labels: `monitor_name`, `monitor_type`, `monitor_url`, `monitor_hostname`, `monitor_port`):

| Metric | Values / meaning |
|---|---|
| `monitor_status` | `0` down, `1` up, `2` pending, `3` maintenance |
| `monitor_response_time` | milliseconds |
| `monitor_cert_days_remaining` | days until TLS cert expiry |
| `monitor_cert_is_valid` | `1` / `0` |

## 2. Status page JSON (public, no auth)

For a published status page with slug `<slug>`:

| Endpoint | Returns |
|---|---|
| `GET /api/status-page/<slug>` | status page config: `config`, `incident`, `publicGroupList` (groups → monitor list), `maintenanceList` |
| `GET /api/status-page/heartbeat/<slug>` | `heartbeatList` (recent samples per monitor id), `uptimeList` (rolling uptime ratios, keys like `<id>_24`) |

Heartbeat sample shape: `{ "status": 0|1, "time": "...", "msg": "...", "ping": <ms> }`.

## Not available over HTTP (socket.io only)

Monitor list with full config, create/update/delete/pause/resume monitor, add/edit notifications, acknowledge incidents, tags, maintenance windows. Programmatic access to these needs a socket.io client such as the community [`uptime-kuma-api`](https://github.com/lucasheld/uptime-kuma-api) Python library. Do not invent REST endpoints for them.
