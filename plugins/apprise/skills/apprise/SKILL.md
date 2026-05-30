---
name: apprise
description: Apprise — universal notification dispatcher (100+ services behind one URL scheme). Use when the user wants to send notifications or manage stored Apprise configs on their Apprise API instance. Talks directly to the Apprise API over HTTP.
---

# Apprise

Universal notification dispatcher — 100+ services behind one URL scheme. Talk to the [Apprise API](https://github.com/caronc/apprise-api) directly over HTTP.

## How to call it

Read the base URL (and optional token) from `~/.lab/.env`:

```bash
APPRISE_URL=$(grep -E '^APPRISE_URL='   ~/.lab/.env | cut -d= -f2-)
APPRISE_TOKEN=$(grep -E '^APPRISE_TOKEN=' ~/.lab/.env | cut -d= -f2-)
AUTH=(); [ -n "$APPRISE_TOKEN" ] && AUTH=(-H "Authorization: Bearer $APPRISE_TOKEN")
```

`APPRISE_TOKEN` is only needed if your deployment is behind a reverse-proxy that enforces it. Never echo the token.

## Common operations

| Intent | Request |
|---|---|
| Health | `curl -sS "${AUTH[@]}" "$APPRISE_URL/status" -w '\nHTTP %{http_code}\n'` |
| Server details (loaded plugins) | `curl -sS "${AUTH[@]}" -H 'Accept: application/json' "$APPRISE_URL/details"` |
| Send (stateless) | `curl -sS -X POST "${AUTH[@]}" -H 'Content-Type: application/json' "$APPRISE_URL/notify" -d '{"urls":"mailto://user:pass@host","title":"Hi","body":"hello","type":"info"}'` |
| Send via stored key | `curl -sS -X POST "${AUTH[@]}" -H 'Content-Type: application/json' "$APPRISE_URL/notify/<key>" -d '{"title":"Hi","body":"hello","tag":"all"}'` |
| Store a config blob | `curl -sS -X POST "${AUTH[@]}" "$APPRISE_URL/add/<key>" -d 'config=mailto://user:pass@host' -d 'format=text'` |
| Get a stored config | `curl -sS "${AUTH[@]}" "$APPRISE_URL/get/<key>"` |
| List URLs under a key | `curl -sS "${AUTH[@]}" "$APPRISE_URL/json/urls/<key>"` |
| Delete a stored config (**destructive**) | `curl -sS -X POST "${AUTH[@]}" "$APPRISE_URL/del/<key>"` |

`type` is one of `info`, `success`, `warning`, `failure`. `format` is `text`, `markdown`, or `html`. For stateless sends, `urls` is a comma-separated list of [Apprise URLs](https://github.com/caronc/apprise/wiki); for keyed sends, the URLs come from the stored config and `tag` selects which ones fire.

## Destructive actions

`POST /del/<key>` permanently removes a stored config — confirm with the user before running it. Sending a notification (`/notify`) is also outward-facing: confirm recipients/content before firing unless the user explicitly asked to send.

## Configuration

`APPRISE_URL` (required) and `APPRISE_TOKEN` (optional) live in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS "$APPRISE_URL/status" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user wants a different notification backend (e.g. Gotify directly) — load that service's skill.
- The user is asking about a different homelab service — load that service's skill instead.
