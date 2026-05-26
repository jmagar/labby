# Crate Extraction Testing Strategy

Status: draft
Related: `docs/crate-extract/contract.md`,
`docs/crate-extract/execution-strategy.md`

## Purpose

This document expands the verification contract for crate/package extraction.

## Test Layers

### Shared Crate Tests

Each shared crate should have focused unit tests for its own public API.

Examples:

- `lab-catalog`: registry registration, duplicate handling, filtering,
  completion, catalog projection.
- `lab-surface`: error envelope serialization, status mapping, action request
  parsing.
- `lab-config`: config discovery order, env merge idempotence, secret masking.
- `lab-runtime`: bind address resolution, runtime dir resolution, shutdown
  helper behavior.

### Product Runtime Tests

Each product crate should test runtime/domain behavior independently where
possible.

Required shape:

- builder creates expected registry/router fragments,
- domain functions work without full `labby`,
- product does not import sibling product internals,
- current behavior parity tests remain green.

### REST/MCP Parity Tests

For behavior exposed over both REST and MCP:

- test REST handler,
- test MCP action handler,
- assert both call or produce behavior from the same domain function,
- assert errors map to the correct surface-specific envelope.

This does not require byte-identical response envelopes across REST and MCP.

### OpenAPI Tests

Required:

- OpenAPI document builds,
- document is valid JSON,
- expected product paths are present,
- auth/security metadata is present where required,
- DTO schemas are present.

Current ActionSpec-derived OpenAPI should be treated as transitional. Product
REST OpenAPI tests should target resource-shaped routes.

### Generated Client Tests

Required:

```bash
labby internal export-openapi --products gateway --out packages/lab-api-client/generated/openapi.json
pnpm --dir packages/lab-api-client generate
pnpm --dir packages/lab-api-client typecheck
```

Also required:

- wrapper function type tests,
- one consumer fixture that imports the generated client,
- no generated file drift in CI.

### Frontend Package Tests

`@jmagar/lab-web`:

- typecheck,
- component tests for auth bootstrap/protected route behavior,
- shell rendering tests,
- async abort/race tests for shared hooks,
- consumer fixture build.

Template:

- Next build succeeds,
- Aurora wiring resolves,
- lab-web imports resolve,
- generated client imports resolve.

### Standalone Binary Tests

For each standalone binary:

```bash
cargo build -p lab --bin <binary> --all-features
<binary> --help
```

Gateway binary should eventually add:

- config load smoke,
- serve startup smoke,
- gateway list smoke,
- schema resource smoke.

### Full Composition Tests

The full `labby` binary must continue to build and test:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
```

## Boundary Tests

Add boundary tests as soon as practical:

- shared crates do not import product crates,
- product crates do not import sibling product crates,
- frontend packages respect allowed dependency direction,
- Aurora does not import lab-web,
- lab-api-client does not import React/lab-web.

Implementation can be simple at first:

- `rg`-based scripts,
- cargo metadata checks,
- package.json dependency checks,
- TypeScript import lint.

## CI Expectations

Minimum extraction CI should include:

- Rust all-features check,
- Rust all-features nextest,
- generated OpenAPI check,
- generated client typecheck,
- lab-web typecheck/build,
- template build,
- boundary check script.

## Test Data Rules

- Use fixtures for OpenAPI/client generation.
- Do not require live homelab services for unit tests.
- Integration tests that hit real services must stay ignored or separately
  gated.
- Do not log or commit real tokens/config values.

## Known Gaps

- Product REST facades do not exist yet.
- Generated client package does not exist yet.
- lab-web package does not exist yet.
- Boundary enforcement scripts do not exist yet.
- Standalone binaries do not exist yet.
