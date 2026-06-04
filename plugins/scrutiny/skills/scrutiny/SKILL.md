---
name: scrutiny
description: "This skill should be used when the user asks about hard drive or SSD health, SMART data, disk failures, drive temperatures, or storage device status. Triggers include: \"check my drives\", \"are any disks failing\", \"show SMART errors\", \"what's the temperature on my drives\", \"is drive X healthy\", \"check Scrutiny\", or any question about disk health monitoring."
---

# Scrutiny

SMART hard-drive health monitoring. Talk to it directly over the Scrutiny web API (served under `/api`).

## How to call it

Read the base URL from `~/.lab/.env`:

```bash
SCRUTINY_URL=$(grep -E '^SCRUTINY_URL=' ~/.lab/.env | cut -d= -f2-)
```

Scrutiny's web API is unauthenticated by default.

> `SCRUTINY_URL` may be unset in `~/.lab/.env` — populate it before use.

## Common operations

| Intent | Request |
|---|---|
| Health | `curl -sS "$SCRUTINY_URL/api/health" -w '\nHTTP %{http_code}\n'` |
| Dashboard summary (all devices) | `curl -sS "$SCRUTINY_URL/api/summary"` |
| List monitored devices | `curl -sS "$SCRUTINY_URL/api/summary" \| python3 -c 'import sys,json;print(*json.load(sys.stdin)["data"]["summary"].keys(),sep="\n")'` |
| Device SMART details | `curl -sS "$SCRUTINY_URL/api/device/<wwn>/details"` |
| Temperature history | `curl -sS "$SCRUTINY_URL/api/summary/temp"` |

The device list comes from the `summary` payload — `GET /api/summary` returns `data.summary` keyed by device WWN, each with its latest SMART status. Use a WWN from there for the `device/<wwn>/details` call.

## Configuration

`SCRUTINY_URL` lives in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS "$SCRUTINY_URL/api/health" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants raw `smartctl` output on a specific host — that's an SSH/shell task, not Scrutiny.
