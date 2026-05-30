# Unraid GraphQL — Troubleshooting Guide

## Credentials Not Configured

**Symptom:** `gql` returns an auth error, or `UNRAID_URL` / `UNRAID_API_KEY` are empty.

**Fix:** Set them in `~/.lab/.env`:

```bash
UNRAID_URL=https://your-unraid-host:port      # base host; the helper appends /graphql
UNRAID_API_KEY=your-api-key
```

Re-read them in your shell as shown in [`SKILL.md`](../SKILL.md). A second instance can be targeted via `UNRAID_SHART_URL` / `UNRAID_SHART_API_KEY`.

---

## Connection Failed / API Unreachable

**Symptoms:** timeout, connection refused, TLS error.

1. Confirm the endpoint responds (note `-k` — Unraid uses a self-signed `*.myunraid.net` cert):

   ```bash
   curl -sSk "$UNRAID_URL/graphql" -H "x-api-key: $UNRAID_API_KEY" \
     -H 'Content-Type: application/json' -d '{"query":"{ info { os { hostname } } }"}' \
     -w '\nHTTP %{http_code}\n'
   ```

2. Check that `UNRAID_URL` points at the GraphQL host:port (the helper appends `/graphql`).
3. Verify the API key has the required roles: **Unraid UI → Settings → Management Access → API Keys → Create** ("Viewer" for read-only, broader roles for mutations).

---

## GraphQL Errors

**Symptom:** response contains an `errors` array instead of (or alongside) `data`.

- **Unknown field / type:** verify field names against [`introspection-schema.md`](introspection-schema.md) or live introspection: `gql '{ __schema { queryType { fields { name } } } }'`.
- **Permission denied:** the API key lacks the role for that query/mutation — issue a key with the needed roles.

---

## Destructive Mutations

Raw GraphQL executes mutations immediately — there is no confirmation gate. Confirm intent with the user **before** sending any mutation that stops the array, removes/clears disks, force-stops/resets VMs, or deletes notifications/keys/plugins/remotes. Read queries are always safe.

---

## Rate Limit Exceeded

**Limit:** ~100 requests / 10 seconds. **Symptoms:** HTTP 429.

**Fix:** space out requests; avoid tight polling loops. Poll no faster than every ~5 seconds.

---

## Log Path Rejected

**Valid log path prefixes:** `/var/log/`, `/boot/logs/`, `/mnt/`. Query the schema's log-file fields to list available logs before reading one.

---

## Container Logs Not Available

Docker container stdout/stderr are **not accessible via the Unraid GraphQL API**. SSH to the Unraid server and use `docker logs <container>` directly.
