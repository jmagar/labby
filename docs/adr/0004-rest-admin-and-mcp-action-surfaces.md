# ADR 0004: Separate REST Admin APIs from MCP Action Dispatch

Date: 2026-05-26

Status: Accepted

## Context

Lab currently benefits from compact MCP exposure: one tool per service with an
`action` plus `params` shape. That keeps agent tool lists small. Web/admin apps
and generated clients need a different shape: resource-oriented HTTP routes
with typed request and response DTOs.

Using one action-dispatch API shape for every surface would make web clients
less conventional. Duplicating business logic per surface would create drift.

## Decision

Support two external API shapes over shared product runtime/domain logic:

```text
product runtime/domain logic
  -> REST/admin handlers
  -> MCP action handlers
  -> CLI handlers
```

REST/admin HTTP is the primary surface for web apps and generated TypeScript
clients. It should use resource-shaped routes and product OpenAPI documents.

MCP action dispatch remains the primary compact agent/tool surface. `ActionSpec`
remains the source of truth for MCP discovery, action help, action schemas, and
destructive-action metadata.

CLI remains an operator adapter over the same runtime/domain logic or shared
dispatch layer. Destructive CLI operations must respect the same destructive
metadata as MCP.

## Consequences

- REST routes and MCP actions must call the same product runtime/domain
  functions.
- Current ActionSpec-derived OpenAPI is transitional and not the final typed web
  client contract.
- REST error responses use canonical Lab error envelopes or documented product
  extensions.
- MCP-only behavior must be documented as a protocol-specific exception.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/api-surface.md`
