# Crate Extraction Docs

This folder contains the architecture and execution documents for extracting
Lab into reusable Rust crates, TypeScript packages, and standalone binaries.

## Core Docs

- `spec.md` — target architecture and package model.
- `contract.md` — enforceable boundary, surface, package, and verification rules.
- `research.md` — evidence gathered against the spec.
- `execution-strategy.md` — worktree, lane, wave, merge, and verification strategy.

## Supporting Docs

- `inventory.md` — current file ownership by target boundary.
- `dependency-map.md` — target dependency graph and current coupling points.
- `api-surface.md` — REST/MCP/CLI surface split and target route shape.
- `package-manifest.md` — package/crate ownership and readiness notes.
- `migration-roadmap.md` — milestone-level migration path.
- `testing-strategy.md` — verification strategy and future CI gates.
- `open-questions.md` — unresolved decisions intentionally kept out of the spec.

## Decision Records

- `../adr/README.md` — accepted ADRs derived from this crate/package extraction
  plan.

## Reading Order

1. `spec.md`
2. `contract.md`
3. `research.md`
4. `execution-strategy.md`
5. `inventory.md`
6. `dependency-map.md`
7. `api-surface.md`
8. `package-manifest.md`
9. `migration-roadmap.md`
10. `testing-strategy.md`
11. `open-questions.md`
12. `../adr/README.md`
