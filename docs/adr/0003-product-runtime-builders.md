# ADR 0003: Compose Products Through Runtime Builders

Date: 2026-05-26

Status: Accepted

## Context

The current Lab binary wires registries, routers, global state, service
managers, OAuth, logs, ACP, MCP, HTTP, and web assets in shared startup code.
That makes standalone product binaries and external product reuse difficult.

## Decision

Every product runtime crate must expose a library-level runtime builder or
equivalent composition API. Builders accept configuration and dependencies
explicitly, then return the product surface fragments needed by application
binaries.

The exact types may vary by product, but the contract is:

```rust
pub struct ProductRuntime {
    pub router: Option<axum::Router>,
    pub registry: Option<lab_catalog::ToolRegistry>,
    pub catalog: Option<lab_catalog::Catalog>,
}

pub struct ProductRuntimeBuilder {
    // explicit dependencies only
}

impl ProductRuntimeBuilder {
    pub async fn build(self) -> anyhow::Result<ProductRuntime>;
}
```

Standalone binaries must be thin wrappers over these library APIs and must not
own product business logic.

## Consequences

- Future products can compose only the product runtimes they need.
- `labby` becomes a composition binary instead of the only owner of product
  wiring.
- Global runtime handles are compatibility shims to remove or isolate during
  extraction.
- Product builder tests become the proof that a product can run outside the
  full Lab binary.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/migration-roadmap.md`
- `docs/crate-extract/inventory.md`
