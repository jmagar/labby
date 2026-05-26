# ADR 0005: Generate TypeScript Clients from REST OpenAPI

Date: 2026-05-26

Status: Accepted

## Context

Lab needs reusable TypeScript clients for admin and product web apps. The
current OpenAPI document is generated from `ActionSpec`, which is useful for
docs but too command-shaped and weakly typed for product web clients.

Research confirmed that Lab already has `utoipa` available and that
`openapi-typescript` plus `openapi-fetch` is a good fit for generated clients.

## Decision

Generate `@jmagar/lab-api-client` primarily from REST/admin OpenAPI documents.

Rust REST request and response DTOs should derive `serde` and
`utoipa::ToSchema` where appropriate. Reserve `schemars::JsonSchema` for
standalone JSON Schema consumers such as MCP/action schema projections unless a
DTO has both REST and non-OpenAPI schema consumers.

The preferred generation path is:

```text
Rust REST route DTOs
  -> utoipa::ToSchema + route metadata
  -> product OpenAPI document
  -> openapi-typescript
  -> openapi-fetch or a thin typed wrapper
  -> @jmagar/lab-api-client
```

An action-contract manifest may still be generated from `ActionSpec` for MCP
tooling, docs, and optional action-dispatch helpers. It is a separate contract
from REST/OpenAPI and carries its own version.

## Consequences

- A product client is not contract-ready until product REST routes and DTOs
  exist.
- Generated client output must typecheck in CI.
- At least one consumer fixture must typecheck before a client is considered
  reusable.
- Raw typed clients can be wrapped with product-friendly functions when that
  improves ergonomics.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/research.md`
- `docs/crate-extract/testing-strategy.md`
