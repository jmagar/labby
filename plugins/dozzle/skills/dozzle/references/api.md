# Dozzle API Reference

Use the configured `DOZZLE_URL` as the base URL. Add the session cookie only
when `DOZZLE_SESSION_COOKIE` is set. Do not pass the expanded cookie in process
argv.

## Safe Curl Helper

Use this helper pattern for authenticated and unauthenticated calls. It keeps
secret values out of argv and disables shell tracing for the call.

```bash
dozzle_curl() (
  set +x
  set -a; . ~/.lab/.env >/dev/null 2>&1 || true; set +a
  url="$1"
  shift
  if [ -n "${DOZZLE_SESSION_COOKIE:-}" ]; then
    /usr/bin/curl -fsS --config - "$@" "$url" <<EOF
header = "Cookie: ${DOZZLE_SESSION_COOKIE}"
EOF
  else
    /usr/bin/curl -fsS "$@" "$url"
  fi
)
```

## Discovery

The root HTML includes a `config__json` script containing the active Dozzle
version, auth provider, host list, and feature flags.

```bash
dozzle_curl "$DOZZLE_URL/" | sed -n '/config__json/,/<\\/script>/p'
```

`/api/version` returns the server version and is the simplest health/auth
probe.

```bash
dozzle_curl "$DOZZLE_URL/api/version"
```

## API Stability

Dozzle's local `/api/*` routes are used by the web UI and are not documented as
a stable public REST API. Treat them as best-effort operational probes. Handle
unexpected status codes, changed response shapes, and empty log responses
gracefully.

## Session Refresh

When the API returns `401` or `403`:

1. Ask the user to open Dozzle in a browser and sign in.
2. Have them inspect a Dozzle request such as `/api/version` or `/` in DevTools.
3. Have them copy the request `Cookie` header into `DOZZLE_SESSION_COOKIE` in
   their local env or `~/.lab/.env`.
4. Do not ask them to paste the cookie into chat.
5. Rerun the version probe with `dozzle_curl`.

## Container and Log Workflow

Discover valid host IDs and container IDs before fetching logs. The root config
has host IDs, and the event stream emits `containers-changed` with container
objects containing `host`, `id`, and `name`.

```bash
dozzle_curl "$DOZZLE_URL/" | sed -n '/config__json/,/<\\/script>/p'
dozzle_curl "$DOZZLE_URL/api/events/stream" --max-time 5 |
  sed -n '/^event: containers-changed/,+1p'
```

Use the discovered `host` and `id` values when fetching logs:

```bash
host="<host-id>"
id="<container-id>"
dozzle_curl "$DOZZLE_URL/api/hosts/$host/containers/$id/logs?stdout=1&stderr=1&tail=200"
```

For streams, always bound the command:

```bash
dozzle_curl "$DOZZLE_URL/api/hosts/$host/containers/$id/logs/stream" --max-time 10
```

URL-encode path values if they come from names or labels rather than IDs.

## Common Paths

- `GET /api/version` - version probe.
- `GET /api/events/stream` - server-sent event stream for live updates; use a
  short timeout when probing.
- `GET /api/hosts/{host}/containers/{id}/logs?...` - fetch container logs.
- `GET /api/hosts/{host}/containers/{id}/logs/stream` - stream container logs.
- `GET /api/groups/{name}/logs/stream` - stream group logs.
- `GET /api/host-groups/{name}/logs/stream` - stream host-group logs.
- `GET /api/labels/{label}/logs/stream` - stream label-selected logs.

Treat mutating container action endpoints as unsafe unless the user explicitly
asks for that action.

Do not use shell, action, download, or token endpoints unless the user
explicitly asks for that workflow. Default to read-only health, discovery, and
log retrieval.

## Auth Handling

If an endpoint returns `401` or `403`, retry only if `DOZZLE_SESSION_COOKIE` is
available. Do not print the cookie. If the retry still fails, report that the
Dozzle session cookie appears missing or expired.
