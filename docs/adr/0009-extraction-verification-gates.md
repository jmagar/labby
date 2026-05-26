# ADR 0009: Require Boundary and Generated-Client Verification

Date: 2026-05-26

Status: Accepted

## Context

Extraction changes dependency direction, public APIs, generated clients, and
runtime composition. A successful narrow build can miss all-features coupling,
frontend package drift, or generated-client breakage.

## Decision

Require verification gates for backend composition, product boundaries,
REST/OpenAPI, generated clients, frontend packages, standalone binaries, and
boundary rules.

Minimum backend verification:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
```

Generated-client verification must export product OpenAPI, regenerate the
client, typecheck the client, and typecheck at least one consumer fixture before
the client is considered reusable.

Boundary checks should be added as soon as practical:

- shared crates do not import product crates,
- product crates do not import sibling product crates,
- frontend packages respect allowed dependency direction,
- Aurora does not import Lab web code,
- `lab-api-client` does not import React or `@jmagar/lab-web`.

Standalone binaries must build and expose `--help`; product smoke tests are
added as their runtime builders mature.

## Consequences

- All-features workspace verification remains the backend truth.
- Generated files cannot silently drift in CI.
- Boundary enforcement can start with simple scripts and become stricter over
  time.
- Live homelab services are not required for extraction unit tests.

## References

- `docs/crate-extract/contract.md`
- `docs/crate-extract/testing-strategy.md`
- `docs/crate-extract/execution-strategy.md`
