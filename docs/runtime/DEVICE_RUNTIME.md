# Node Runtime

`labby serve` is the always-on node runtime for every Linux `x86_64` machine that participates in a `lab` fleet.

One machine is the configured controller. It owns the operator control plane:

- Web UI
- MCP
- `/v1/{service}` REST routes
- `/v1/gateway`
- node inventory
- node log ingestion and search

Every other machine runs as a non-controller node. Non-controller nodes keep only the local runtime and the `/v1/nodes/*` namespace.

## Role Resolution

Node role is resolved from:

1. local hostname
2. `[node].controller` in `config.toml`, when present

If `[node].controller` is missing, the local host resolves itself as the controller. Legacy `[device].master` remains a compatibility fallback.

Container deployments must resolve the host machine identity, not Docker's
ephemeral container hostname. The bundled Compose file mounts host
`/etc/hostname` at `/run/host/hostname`; `labby serve` checks that file before
falling back to the process `HOSTNAME`. Operators can also set
`LAB_HOST_HOSTNAME` explicitly for runtimes that cannot mount the host hostname
file.

Example:

```toml
[node]
controller = "tootie"
```

On `tootie`, `labby serve` runs as the controller. On any other host, it runs as a non-controller node and reports to `tootie`.

## Startup Behavior

When `labby serve` starts, it resolves the local hostname and node role, creates the in-process node runtime, and then:

- on the controller:
  - creates the shared fleet state store
  - creates the durable enrollment store at `~/.lab/device-enrollments.json` (legacy path; controller-owned)
  - mounts the full operator control plane plus `/v1/nodes/*`
  - exposes the node websocket endpoint at `GET /v1/nodes/ws`
- on a non-controller node:
  - mounts `/health`, `/ready`, and `/v1/nodes/*`
  - disables the Web UI, MCP, gateway management, and the service REST surface
  - scans local MCP config inventory and queues metadata
  - collects bootstrap logs and queues them locally
  - starts a long-lived websocket session to the controller and drains metadata, status, and log envelopes over that connection

## Fleet Transport

Non-controller nodes now use a websocket-first fleet transport:

1. derive `ws://` or `wss://` from the configured controller base URL
2. connect to `GET /v1/nodes/ws`
3. send `initialize` with:
   - `lab.device_id` (wire field — do not rename)
   - `lab.device_token` (wire field — do not rename)
   - `lab.tailnet_identity`
4. wait for enrollment approval
5. once approved, keep the socket open and send:
   - `nodes/metadata.push`
   - `nodes/status.push`
   - `nodes/log.event`

Unknown nodes are rejected at `initialize` and recorded as pending enrollments on the controller. Operators approve or deny them through the controller API, CLI, or MCP surface.

## Node API Namespace

The node runtime still exposes `/v1/nodes/*`, but websocket fleet delivery is now the canonical node-to-controller path.

Write-oriented routes:

- `POST /v1/nodes/oauth/relay/start`
- `POST /v1/nodes/enrollments/{node_id}/approve`
- `POST /v1/nodes/enrollments/{node_id}/deny`

Read-oriented routes (controller-only):

- `GET /v1/nodes/enrollments`
- `GET /v1/nodes`
- `GET /v1/nodes/{node_id}`
- `POST /v1/nodes/logs/search`

Fleet read routes are controller-only. On a non-controller node they return a structured `not_found` error rather than exposing an empty or partial local view.

## Metadata Inventory

Non-controller nodes scan the current home directory for MCP config inventory from:

- `~/.claude.json`
- `~/.codex/config.toml`
- `~/.gemini/settings.json`

Each discovered file is reported with:

- source name
- path
- modified timestamp
- SHA-256 content hash
- parsed MCP server map

This is inventory only. The controller stores the uploaded metadata in memory for fleet inspection.

## Log Buffering

Node outbound delivery uses a durable local queue rooted at:

```text
~/.lab/node-runtime-queue.jsonl
```

Rules:

- non-controller nodes append metadata, status, and log envelopes locally first
- the queue is acknowledged only after the controller accepts each websocket request
- failed uploads remain on disk for the next flush attempt
- the controller stores ingested normalized log events in `~/.lab/node-logs.sqlite`

The current bootstrap collector is intentionally minimal. It normalizes into the shared `DeviceLogEvent` shape (Rust type name — do not rename) and is expected to grow without changing the node API contract.

## Enrollment

Enrollment is controller-owned and durable.

- pending, approved, and denied enrollment records are stored in `~/.lab/device-enrollments.json` (legacy path name; controller-owned)
- approval pins an exact `(device_id, device_token)` pair (wire field names — do not rename)
- denied nodes remain blocked until explicitly re-approved with a new pending record

Operator surfaces:

- CLI:
  - `labby nodes enrollments list`
  - `labby nodes enrollments approve <node_id> [--note ...]`
  - `labby nodes enrollments deny <node_id> [--reason ...]`
- HTTP:
  - `GET /v1/nodes/enrollments`
  - `POST /v1/nodes/enrollments/{node_id}/approve`
  - `POST /v1/nodes/enrollments/{node_id}/deny`
- MCP:
  - `node.enrollments.list`
  - `node.enrollments.approve`
  - `node.enrollments.deny`

## OAuth Relay Capability

The node runtime exposes the existing local OAuth relay helper through:

```http
POST /v1/nodes/oauth/relay/start
```

Request body:

```json
{
  "bind_addr": "127.0.0.1:38935",
  "target_url": "http://node.internal.example:38935/callback/dookie",
  "default_port": 38935,
  "request_timeout_ms": 30000
}
```

This starts the same local loopback forwarder used by `labby oauth relay-local`, but initiated through the node runtime on the target machine.

## Auth Expectations

When `LAB_MCP_HTTP_TOKEN` is configured, the controller still protects operator `/v1/*` routes and controller-routed CLI traffic with that bearer token.

Node-to-controller fleet delivery does not depend on bearer auth. It is authenticated inside websocket `initialize` using the node token (`device_token` wire field) pinned in the enrollment store.

OAuth mode still protects the public operator surface. Node sessions do not mint or refresh OAuth credentials on their own.
