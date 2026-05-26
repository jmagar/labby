# Crate Extraction API Surface

Status: draft
Related: `docs/crate-extract/spec.md`, `docs/crate-extract/contract.md`

## Purpose

This document describes the API shape split that extraction must preserve:

- REST/admin HTTP for web apps and generated TypeScript clients.
- MCP action dispatch for compact agent/tool exposure.
- CLI as a human/operator adapter over product runtime logic.

## Surface Rule

REST, MCP, and CLI are adapters. They must call shared product runtime/domain
functions rather than duplicate behavior.

```text
product runtime/domain logic
  -> REST/admin handlers
  -> MCP action handlers
  -> CLI handlers
```

## REST/Admin HTTP

REST/admin HTTP should be resource-shaped and represented in OpenAPI.

Gateway example target routes:

```http
GET    /v1/gateways
GET    /v1/gateways/{name}
POST   /v1/gateways
PATCH  /v1/gateways/{name}
DELETE /v1/gateways/{name}
GET    /v1/gateways/{name}/tools
GET    /v1/gateways/{name}/resources
GET    /v1/gateways/{name}/prompts
POST   /v1/gateways/{name}/reload
POST   /v1/gateways/{name}/oauth/start
DELETE /v1/gateways/{name}/oauth/token
```

REST route requirements:

- route has request/response DTOs,
- DTOs derive `serde` and `utoipa::ToSchema` where appropriate,
- route appears in product OpenAPI,
- route auth/scope policy is documented,
- route returns canonical error envelopes.

## MCP Action Dispatch

MCP keeps compact tool exposure.

Gateway example:

```text
gateway({ action, params })
```

Requirements:

- one compact tool per product/service where practical,
- `ActionSpec` remains the MCP discovery/help/schema source,
- destructive actions remain marked in action metadata,
- MCP action handlers call shared runtime/domain functions,
- protocol-specific behavior is documented as an exception.

## CLI

CLI commands may call product runtime/domain functions directly or go through
the dispatch layer.

Requirements:

- destructive CLI operations use the same destructive metadata as MCP,
- CLI formatting stays out of product runtime crates unless deliberately shared,
- CLI confirmation behavior remains consistent across products.

## OpenAPI Generation

Current Lab OpenAPI is generated from `ActionSpec` and is transitional for this
work.

Target:

```text
REST route DTOs + utoipa::ToSchema + route annotations/programmatic OpenAPI
  -> product OpenAPI document
  -> @jmagar/lab-api-client
```

Action contract generation remains separate:

```text
ActionSpec
  -> MCP docs/help/schema
  -> optional action-contract manifest
```

## Product Surface Checklist

For each product:

- [ ] Current MCP actions inventoried.
- [ ] Target REST resources named.
- [ ] DTOs identified.
- [ ] OpenAPI route coverage defined.
- [ ] Shared runtime/domain functions identified.
- [ ] CLI adapter path identified.
- [ ] Auth/scope policy documented.
- [ ] Destructive operations identified.

## Product Notes

### Gateway

High-priority REST facade because it is the first likely generated client.

Special concerns:

- upstream MCP tool schemas,
- protected routes,
- OAuth start/status/clear,
- import/tombstone state,
- semantic search/scout,
- virtual Lab-backed servers.

### ACP

Likely REST resources:

- providers,
- sessions,
- session events,
- prompts/messages,
- attachments,
- health/model metadata.

Special concerns:

- streaming/SSE,
- session ownership,
- adapter process lifecycle.

### Marketplace

Likely REST resources:

- sources,
- plugins,
- agents,
- MCP packages,
- sync status,
- install/update plans.

### Fleet

Likely REST resources:

- nodes,
- enrollment requests,
- node logs,
- node health,
- node websocket admission.

### Stash

Likely REST resources:

- artifacts,
- snapshots,
- revisions,
- providers.

### Logs

Likely REST resources:

- search,
- tail/stream,
- stats,
- retention/maintenance.

### Workspace

Likely REST resources:

- directory listing,
- file preview,
- workspace metadata.

Security note:

- workspace preview must remain carefully scoped and may not be exposed over MCP
  if it can leak arbitrary local files.
