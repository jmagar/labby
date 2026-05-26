# ADR 0002: Split Shared Platform Crates from Product Runtime Crates

Date: 2026-05-26

Status: Accepted

## Context

Lab has reusable infrastructure concerns and reusable product capabilities mixed
inside the current binary crate. If shared infrastructure imports product code,
future products inherit unnecessary dependencies and the extracted platform
cannot stay small.

## Decision

Use two backend crate classes.

Shared platform crates provide reusable infrastructure:

- `lab-auth`
- `lab-config`
- `lab-runtime`
- `lab-catalog`
- `lab-surface`
- `lab-observability`

Product runtime crates own reusable product capabilities:

- `lab-gateway`
- `lab-marketplace`
- `lab-acp`
- `lab-fleet`
- `lab-stash`
- `lab-oauth`
- `lab-logs`
- `lab-workspace`
- `lab-setup`
- `lab-doctor`

Allowed backend dependency direction is:

```text
application binary
  -> product runtime crates
  -> shared platform crates
  -> external crates
```

Shared crates must not depend on product runtime crates. Product crates must not
depend on sibling product crates unless an explicit exception is added to the
crate extraction contract.

## Consequences

- Cross-product orchestration belongs in application composition layers or small
  shared interfaces.
- Shared crates must expose narrow public APIs and avoid broad internal
  re-exports.
- Product crates can evolve independently without dragging sibling runtime code
  into consumers.

## References

- `docs/crate-extract/contract.md`
- `docs/crate-extract/dependency-map.md`
- `docs/crate-extract/package-manifest.md`
