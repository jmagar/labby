---
name: adguard
description: AdGuard Home — DNS-level ad blocking and network filtering. Use when the user wants to check their AdGuard instance status, query stats, search the DNS query log, or check filtering. Talks directly to the AdGuard Home control API.
---

# AdGuard

DNS-level ad blocking and network filtering. Talk to it directly over the AdGuard Home control API (served under `/control`, HTTP Basic auth).

## How to call it

Read the base URL and credentials from `~/.lab/.env`:

```bash
ADGUARD_URL=$(grep -E '^ADGUARD_URL='      ~/.lab/.env | cut -d= -f2-)
ADGUARD_USERNAME=$(grep -E '^ADGUARD_USERNAME=' ~/.lab/.env | cut -d= -f2-)
ADGUARD_PASSWORD=$(grep -E '^ADGUARD_PASSWORD=' ~/.lab/.env | cut -d= -f2-)
AUTH=(-u "$ADGUARD_USERNAME:$ADGUARD_PASSWORD")
```

Auth is HTTP Basic (`-u user:pass`). Never echo the password.

> `ADGUARD_*` may be unset in `~/.lab/.env` — populate them before use.

## Common operations

| Intent | Request |
|---|---|
| Server status + version + running state | `curl -sS "${AUTH[@]}" "$ADGUARD_URL/control/status"` |
| DNS query statistics | `curl -sS "${AUTH[@]}" "$ADGUARD_URL/control/stats"` |
| Search the query log | `curl -sS "${AUTH[@]}" "$ADGUARD_URL/control/querylog?search=<term>&limit=50"` |
| Filtering status / rule lists | `curl -sS "${AUTH[@]}" "$ADGUARD_URL/control/filtering/status"` |
| Check whether a host is blocked | `curl -sS "${AUTH[@]}" "$ADGUARD_URL/control/filtering/check_host?name=<host>"` |

The version string and running state are fields inside `GET /control/status` (there is no separate version endpoint). Full API reference: <https://github.com/AdguardTeam/AdGuardHome/blob/master/openapi/openapi.yaml>.

## Configuration

`ADGUARD_URL`, `ADGUARD_USERNAME`, and `ADGUARD_PASSWORD` live in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS -o /dev/null -w 'HTTP %{http_code}\n' "${AUTH[@]}" "$ADGUARD_URL/control/status"
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants to change filtering rules or protection settings — those are mutating `POST /control/*` endpoints; confirm intent first and consult the OpenAPI reference for the exact body.
