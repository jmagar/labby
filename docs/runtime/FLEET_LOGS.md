# Fleet Logs

Fleet logs are normalized node log events ingested by the controller from non-controller node runtimes.

## Event Shape

Each log event uses the shared `DeviceLogEvent` schema:

- `node_id`
- `source`
- `timestamp_unix_ms`
- optional `level`
- `message`
- structured `fields`

This keeps fleet log ingestion independent from the raw source format.

## Ingestion Flow

1. a non-controller node collects bootstrap log events
2. it appends a `syslog_batch` envelope to `~/.labby/node-runtime-queue.jsonl`
3. it sends the envelope over the live websocket session as `nodes/log.event`
4. the controller stores the normalized events in the durable SQLite node log store
5. the local queue entry is acknowledged only after a successful websocket response

The queue exists to make early-runtime log upload resilient to temporary controller outages.

## Query Surfaces

Fleet log search is available on the controller through:

- CLI: `labby logs search <node> <query>`
- HTTP: `POST /v1/nodes/logs/search`

Example:

```bash
labby logs search node-a oauth
```

```json
POST /v1/nodes/logs/search
{
  "node_id": "node-a",
  "query": "oauth"
}
```

The current implementation performs a case-insensitive substring match against `message`.

## Fleet Node Queries

The controller also exposes:

- `labby nodes list`
- `labby nodes get <node_id>`
- `GET /v1/nodes`
- `GET /v1/nodes/{node_id}`

Those responses include per-node log counts so operators can quickly see whether a node is checking in and sending data.

## Current Limits

- fleet inventory/session state is in-process, but accepted node log events are persisted to `~/.labby/node-logs.sqlite`
- enrollment state is durable; node log retention is controlled by `[node].log_retention_days`
- log search currently matches `message` only
- the bootstrap collector is intentionally conservative and may return no events on hosts without supported sources
