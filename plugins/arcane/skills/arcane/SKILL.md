---
name: arcane
description: This skill should be used when the user asks to manage Docker containers, images, volumes, networks, stacks/projects, or environments, or mentions Arcane, Docker management, container operations, image updates, or GitOps workflows. Talks directly to the Arcane REST API.
---

# Arcane

Docker management via the [Arcane](https://arcane.ofkm.dev) REST API. Resources are **environment-scoped**: pick an environment, then operate on its containers/images/volumes/networks/projects.

## How to call it

Read the base URL and API key from `~/.lab/.env`:

```bash
ARCANE_URL=$(grep -E '^ARCANE_URL='     ~/.lab/.env | cut -d= -f2-)
ARCANE_API_KEY=$(grep -E '^ARCANE_API_KEY=' ~/.lab/.env | cut -d= -f2-)
api() { curl -sS -H "X-API-Key: $ARCANE_API_KEY" "$@"; }   # add -X POST/DELETE as needed
```

Auth is the `X-API-Key: <key>` header on every request. Never echo the key. Responses are JSON (paginated lists carry a `data` array).

## Discover the environment first

```bash
api "$ARCANE_URL/api/environments"     # grab an environment id (envId) from data[].id
EID=<envId>
```

## Common operations

| Intent | Request |
|---|---|
| System health | `api "$ARCANE_URL/api/health"` |
| Version | `api "$ARCANE_URL/api/version"` |
| List environments | `api "$ARCANE_URL/api/environments"` |
| List containers | `api "$ARCANE_URL/api/environments/$EID/containers"` |
| Container details | `api "$ARCANE_URL/api/environments/$EID/containers/<id>"` |
| Start / stop / restart (**destructive**) | `api -X POST "$ARCANE_URL/api/environments/$EID/containers/<id>/start"` (or `/stop`, `/restart`) |
| Delete container (**destructive**) | `api -X DELETE "$ARCANE_URL/api/environments/$EID/containers/<id>"` |
| List images | `api "$ARCANE_URL/api/environments/$EID/images"` |
| List volumes | `api "$ARCANE_URL/api/environments/$EID/volumes"` |
| List networks | `api "$ARCANE_URL/api/environments/$EID/networks"` |
| List projects (compose stacks) | `api "$ARCANE_URL/api/environments/$EID/projects"` |

Paths follow `/api/environments/{envId}/{resource}[/{id}[/{verb}]]`. Sub-resources mirror Docker: images support `pull`/`delete`/`prune`/`scan`; volumes/networks support `create`/`delete`/`prune`; projects support `up`/`down`/`restart`/`pull`/`redeploy`/`destroy`.

> Container **logs** are not exposed by the Arcane REST API — use `docker logs <container>` on the host or the Arcane web UI.

## Destructive actions

Stopping/restarting/deleting containers, deleting or pruning images/volumes/networks, and `project` `down`/`destroy`/`restart`/`redeploy` all change running state or remove data. Confirm with the user before running them.

## Configuration

`ARCANE_URL` and `ARCANE_API_KEY` live in `~/.lab/.env`. Verify connectivity:

```bash
api "$ARCANE_URL/api/health" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants container **logs** — use `docker logs` / the Arcane UI, not this API.
