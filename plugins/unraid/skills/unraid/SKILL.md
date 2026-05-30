---
name: unraid
description: "This skill should be used when the user mentions Unraid, asks to check server health, monitor array or disk status, list or restart Docker containers, start or stop VMs, read system logs, check parity status, view notifications, manage API keys, check UPS or power status, get CPU or memory data, check disk temperatures, or perform any operation on an Unraid NAS server. Talks directly to the Unraid GraphQL API."
---

# Unraid

Operate an Unraid NAS through its **GraphQL API**. One endpoint (`/graphql`), authenticated with an API key; every operation is a GraphQL query or mutation.

## How to call it

Read the base URL and key from `~/.lab/.env`:

```bash
UNRAID_URL=$(grep -E '^UNRAID_URL='     ~/.lab/.env | cut -d= -f2-)
UNRAID_API_KEY=$(grep -E '^UNRAID_API_KEY=' ~/.lab/.env | cut -d= -f2-)

gql() {   # gql '<graphql query>'
  curl -sSk -H "x-api-key: $UNRAID_API_KEY" -H "Content-Type: application/json" \
    "$UNRAID_URL/graphql" -d "$(jq -nc --arg q "$1" '{query:$q}')"
}
```

Notes on the request:
- Endpoint is `$UNRAID_URL/graphql` (the URL in `~/.lab/.env` is the base host:port).
- `-k` is needed — Unraid serves a self-signed `*.myunraid.net` cert.
- Auth header is `x-api-key: <key>`. Never echo the key.
- A second instance may exist (`UNRAID_SHART_URL` / `UNRAID_SHART_API_KEY`) — swap the vars to target it.

## Common operations

```bash
# Server overview
gql '{ info { os { hostname uptime } versions { unraid } } }'

# Array status + disks
gql '{ array { state capacity { kilobytes { free used total } } disks { name status temp } } }'

# Docker containers
gql '{ docker { containers { names state status image } } }'

# VMs
gql '{ vms { domain { name state } } }'

# Parity check status
gql '{ parityHistory { date duration speed status errors } }'

# Notifications overview
gql '{ notifications { overview { unread { total } } } }'
```

Mutations follow the same shape (`gql 'mutation { ... }'`). The full schema — every query, mutation, and field — is in the reference files:

- [`references/schema.graphql`](references/schema.graphql) — full GraphQL SDL
- [`references/api-reference.md`](references/api-reference.md) — query/mutation catalog with examples
- [`references/endpoints.md`](references/endpoints.md) — endpoint and field map
- Use GraphQL introspection to explore live: `gql '{ __schema { queryType { fields { name } } } }'`

## Destructive actions

Mutations that stop the array, remove/clear disks, force-stop or reset VMs, delete notifications or API keys, or remove plugins/remotes change state irreversibly. Confirm with the user before running any mutation. Read queries (above) are safe.

## Logs

Container stdout/stderr are **not** available through the GraphQL API. For container logs, SSH to the host and run `docker logs <container>`. System log *files* are exposed via the schema's log fields (paths restricted to `/var/log/`, `/boot/logs/`, `/mnt/`).

## Configuration

`UNRAID_URL` and `UNRAID_API_KEY` (plus optional `UNRAID_SHART_*`) live in `~/.lab/.env`. Rate limit is ~100 requests / 10s. Verify connectivity:

```bash
gql '{ info { os { hostname } } }'
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants container logs — SSH + `docker logs`, not this API.
