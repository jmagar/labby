---
name: qdrant
description: "This skill should be used when the user asks about vector search, semantic search, or managing a Qdrant vector database. Triggers include: \"list collections\", \"create a collection\", \"search by vector\", \"upsert points\", \"run a similarity search\", \"find nearest neighbors\", \"check Qdrant health\", \"embedding storage\", or any mention of managing a Qdrant instance."
---

# Qdrant

Vector database for semantic search and embeddings. Talk to it directly over its HTTP REST API.

## How to call it

Read the base URL (and optional API key) from `~/.lab/.env`, then curl the Qdrant REST API:

```bash
QDRANT_URL=$(grep -E '^QDRANT_URL='     ~/.lab/.env | cut -d= -f2-)
QDRANT_API_KEY=$(grep -E '^QDRANT_API_KEY=' ~/.lab/.env | cut -d= -f2-)
AUTH=(); [ -n "$QDRANT_API_KEY" ] && AUTH=(-H "api-key: $QDRANT_API_KEY")
```

`QDRANT_API_KEY` is optional — include the `api-key` header only when it is set. Never echo the key.

## Common operations

| Intent | Request |
|---|---|
| Server health / version | `curl -sS "${AUTH[@]}" "$QDRANT_URL/"` |
| List collections | `curl -sS "${AUTH[@]}" "$QDRANT_URL/collections"` |
| Collection info | `curl -sS "${AUTH[@]}" "$QDRANT_URL/collections/<name>"` |
| Create collection | `curl -sS -X PUT "${AUTH[@]}" -H 'Content-Type: application/json' "$QDRANT_URL/collections/<name>" -d '{"vectors":{"size":<dim>,"distance":"Cosine"}}'` |
| Delete collection (**destructive**) | `curl -sS -X DELETE "${AUTH[@]}" "$QDRANT_URL/collections/<name>"` |
| Upsert points | `curl -sS -X PUT "${AUTH[@]}" -H 'Content-Type: application/json' "$QDRANT_URL/collections/<name>/points?wait=true" -d '{"points":[{"id":1,"vector":[...],"payload":{}}]}'` |
| Search by vector | `curl -sS -X POST "${AUTH[@]}" -H 'Content-Type: application/json' "$QDRANT_URL/collections/<name>/points/search" -d '{"vector":[...],"limit":10,"with_payload":true}'` |

Full REST reference: <https://api.qdrant.tech/>

## Destructive actions

Deleting a collection (`DELETE /collections/<name>`) removes all of its data and is irreversible. Confirm with the user before running it.

## Configuration

`QDRANT_URL` (required) and `QDRANT_API_KEY` (optional) live in `~/.lab/.env`. Verify connectivity:

```bash
curl -sS "$QDRANT_URL/" -w '\nHTTP %{http_code}\n'
```

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants to *generate* embeddings (not store/search them) — that's the `tei` skill.
