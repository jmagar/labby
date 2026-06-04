---
name: immich
description: "This skill should be used when the user mentions Immich or asks to browse their photo library, search for photos, find pictures from a trip or date range, list albums, check server stats, or view storage usage. Triggers include: \"find photos of\", \"show my albums\", \"search Immich\", \"how many photos do I have\", \"check my Immich server\", or any question about a self-hosted photo library."
---

# Immich

Self-hosted photo and video management. Talk to it directly over the Immich REST API (served under `/api`).

## How to call it

Read the base URL and API key from `~/.lab/.env`:

```bash
IMMICH_URL=$(grep -E '^IMMICH_URL='     ~/.lab/.env | cut -d= -f2-)
IMMICH_API_KEY=$(grep -E '^IMMICH_API_KEY=' ~/.lab/.env | cut -d= -f2-)
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
| List albums | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/albums"` |
| Get album assets | `curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/albums/<albumId>"` |

Full API reference: <https://api.immich.app/> (OpenAPI). The `/api/search/metadata` body accepts the full Immich search filter (album, person, type, date ranges, etc.).

## Configuration

`IMMICH_URL` and `IMMICH_API_KEY` live in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS -H "x-api-key: $IMMICH_API_KEY" "$IMMICH_URL/api/server/ping" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants to bulk-upload or sync photos — use the official `immich` CLI, not ad-hoc curl.
