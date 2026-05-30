---
name: neo4j
description: Neo4j — graph database (nodes, relationships, Cypher queries). Use when the user wants to run Cypher, inspect the schema, or check their Neo4j instance. Talks to Neo4j over its HTTP transactional Cypher API.
---

# Neo4j

Graph database — nodes, relationships, Cypher queries. Drive it with Cypher over Neo4j's **HTTP transactional API**.

## How to call it

```bash
NEO4J_USER=$(grep -E '^NEO4J_USER='     ~/.lab/.env | cut -d= -f2-)
NEO4J_PASSWORD=$(grep -E '^NEO4J_PASSWORD=' ~/.lab/.env | cut -d= -f2-)
NEO4J_DB=$(grep -E '^NEO4J_DB='         ~/.lab/.env | cut -d= -f2-); NEO4J_DB=${NEO4J_DB:-neo4j}
# NOTE: ~/.lab/.env stores NEO4J_URL as a bolt:// endpoint, which curl cannot speak.
# The HTTP API listens separately (default port 7474). Set NEO4J_HTTP_URL to it:
NEO4J_HTTP_URL="http://tootie:7474"        # adjust host/port to your HTTP listener
AUTH=(-u "$NEO4J_USER:$NEO4J_PASSWORD")
```

Auth is HTTP Basic. The `bolt://` URL in `~/.lab/.env` is for binary Bolt clients (`cypher-shell`, drivers) — for curl you need the HTTP listener URL. Never echo the password.

## Running Cypher

All queries go through `POST /db/<database>/tx/commit` with a `statements` array:

```bash
cypher() {
  curl -sS "${AUTH[@]}" -H 'Content-Type: application/json' \
    "$NEO4J_HTTP_URL/db/$NEO4J_DB/tx/commit" \
    -d "{\"statements\":[{\"statement\":\"$1\"}]}"
}
```

| Intent | Cypher (pass to `cypher`) |
|---|---|
| Read query | `MATCH (n) RETURN n LIMIT 25` |
| Write query (**destructive**) | `CREATE (n:Person {name:'Ada'}) RETURN n` |
| List node labels | `CALL db.labels()` |
| List relationship types | `CALL db.relationshipTypes()` |
| List constraints | `SHOW CONSTRAINTS` |
| List indexes | `SHOW INDEXES` |
| List databases | `SHOW DATABASES` |
| Server / components info | `CALL dbms.components()` |

Multi-statement transaction: pass several objects in the `statements` array of a single `tx/commit` call. Server discovery (available endpoints) is `GET $NEO4J_HTTP_URL/`.

## Destructive actions

Any write Cypher (`CREATE`, `MERGE`, `SET`, `DELETE`, `DROP`, etc.) mutates the graph. Confirm with the user before running writes or multi-statement transactions that include them.

## Configuration

`NEO4J_USER`, `NEO4J_PASSWORD`, `NEO4J_DB`, and `NEO4J_URL` (bolt) live in `~/.lab/.env`. For curl you additionally need the HTTP listener URL (`NEO4J_HTTP_URL`), which is not stored there by default. Verify connectivity:

```bash
curl -sS "${AUTH[@]}" "$NEO4J_HTTP_URL/" -w '\nHTTP %{http_code}\n'
```

If only Bolt is exposed, use `cypher-shell -a "$NEO4J_URL" -u "$NEO4J_USER" -p "$NEO4J_PASSWORD"` instead of curl.

## When NOT to use this skill

- The user is asking about a different homelab service — load that service's skill instead.
- The user wants vector search — that's the `qdrant` skill.
