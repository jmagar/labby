# ADR 0006: Package Reusable Admin UI as Lab Web

Date: 2026-05-26

Status: Accepted

## Context

The gateway admin app contains reusable admin shell, auth bootstrap, protected
route, API provider, loading, error, toast, and navigation behavior. Future Lab
products should not copy this code or import product-specific app routes.

Aurora is the design system and shadcn registry source of truth. Backend
authorization remains a Rust concern.

## Decision

Create `@jmagar/lab-web` as a reusable TypeScript/React package for Lab-style
admin products.

Required exports:

```text
@jmagar/lab-web
@jmagar/lab-web/auth
@jmagar/lab-web/shell
@jmagar/lab-web/next
```

`@jmagar/lab-web` owns frontend auth UX, protected route wrappers, session
hooks, admin shell primitives, common loading/error/toast primitives, and API
provider wiring.

It must not own product pages, backend authorization, or a full Next.js app
scaffold. The starter/template owns full app files and consumes
`@jmagar/lab-web`.

Aurora remains a frontend package/registry boundary, not a Rust crate. If Rust
binaries need compiled web assets, use a separate `lab-web-assets` helper
boundary that contains built assets, not React source.

## Consequences

- Frontend auth improves UX but is never the authorization source of truth.
- `@jmagar/lab-web` may depend on `@jmagar/aurora` and
  `@jmagar/lab-api-client`.
- `@jmagar/aurora` must not depend on `@jmagar/lab-web`.
- `@jmagar/lab-api-client` must remain UI-framework-free.
- Shared async UI utilities need abort/race cleanup tests when they cross await
  boundaries.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/package-manifest.md`
- `docs/crate-extract/inventory.md`
