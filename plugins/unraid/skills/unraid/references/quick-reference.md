# Unraid GraphQL — Quick Reference

All operations use the `gql` curl helper from [`SKILL.md`](../SKILL.md):

```bash
gql() { curl -sSk -H "x-api-key: $UNRAID_API_KEY" -H "Content-Type: application/json" \
  "$UNRAID_URL/graphql" -d "$(jq -nc --arg q "$1" '{query:$q}')"; }
```

## Read queries (verified)

```bash
# Server / health
gql '{ info { os { hostname uptime } versions { unraid } } }'

# Array status + disks (temps in °C, sizes in kilobytes)
gql '{ array { state capacity { kilobytes { free used total } } disks { name status temp } } }'

# Docker containers
gql '{ docker { containers { names state status image } } }'

# Virtual machines
gql '{ vms { domain { name state } } }'

# Parity check history
gql '{ parityHistory { date duration speed status errors } }'

# Notifications overview
gql '{ notifications { overview { unread { total } } } }'
```

## Exploring the schema

The full set of queries, mutations, and fields is large and verified live. Use these to discover the exact shape rather than guessing:

```bash
# Top-level query fields
gql '{ __schema { queryType { fields { name } } } }'
# Top-level mutation fields
gql '{ __schema { mutationType { fields { name } } } }'
# Fields of a given type
gql '{ __type(name:"Docker") { fields { name type { name kind } } } }'
```

Authoritative catalogs in this folder:
- [`schema.graphql`](schema.graphql) — full SDL
- [`api-reference.md`](api-reference.md) — query/mutation reference with examples
- [`endpoints.md`](endpoints.md) — endpoint and field map

## Mutations

Mutations (start/stop array, container/VM control, parity start/pause, notification/key management, plugin install/remove) execute immediately — there is no confirmation gate in raw GraphQL. **Confirm with the user before sending any mutation.** Look up the exact mutation name and arguments in `api-reference.md` / `schema.graphql` before composing the call.

## Notes

- Disk/array sizes are in **kilobytes**; `info.memory` / `metrics.memory` are in **bytes**; temperatures in **°C**.
- Container stdout/stderr logs are **not** exposed by the API — SSH to the host and use `docker logs`.
- Rate limit ~100 requests / 10s; poll no faster than every ~5 seconds.
