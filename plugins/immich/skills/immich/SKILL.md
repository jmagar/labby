---
name: immich
description: Immich — self-hosted photo and video management. Use when the user wants to check their Immich server, look up their profile, or search photos and videos. Talks directly to the Immich REST API.
---

# Immich

Self-hosted photo and video management. Talk to it directly over the Immich REST API (served under `/api`).

## How to call it

Read the base URL and API key from `~/.lab/.env`:

```bash
IMMICH_URL=$(grep -E '^IMMICH_URL='     ~/.lab/.env | cut -d= -f2-)
IMMICH_API_KEY=$(grep -E '^IMMICH_API_KEY=' ~/.lab/.env | cut -d= -f2-)
API=("$IMMICH_URL/api" -H "x-api-key: $IMMICH_API_KEY")
```

Authentication is the `x-api-key: <key>` header on every request. Never echo the key.

> `IMMICH_URL` / `IMMICH_API_KEY` may be unset in `~/.lab/.env` — populate them before use.

## Common operations

| Intent | Request |
|---|---|
| Health | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/server/ping"` → `{"res":"pong"}` |
| Version | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/server/version"` |
| Server about / info | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/server/about"` |
| Server statistics | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/server/statistics"` |
| Current user (me) | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/users/me"` |
| Search assets (metadata) | `curl -sS -X POST -H "x-api-key: $IMMICH_API_KEY" -H 'Content-Type: application/json' "$IMMICH_URL/api/search/metadata" -d '{"query":"beach"}'` |
| Get one asset | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/assets/<id>"` |

Full API reference: <https://api.immich.app/> (OpenAPI). The `/api/search/metadata` body accepts the full Immich search filter (album, person, type, date ranges, etc.).

## Configuration

`IMMICH_URL` and `IMMICH_API_KEY` live in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/server/ping" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants to bulk-upload or sync photos — use the official `immich` CLI, not ad-hoc curl.
