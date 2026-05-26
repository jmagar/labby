# Crate Extraction Open Questions

Status: draft
Related: `docs/crate-extract/spec.md`, `docs/crate-extract/research.md`

## Purpose

This document keeps unresolved decisions out of the spec and contract until they
are ready to lock.

## Repository and Distribution

### Should extracted Rust crates live in one repo or many?

Options:

- stay in the Lab workspace,
- move to one multi-crate platform repo,
- move to one repo per major product,
- hybrid: shared platform repo plus product repos.

Current leaning:

- prove boundaries inside the Lab workspace first.

### Should versions be synchronized?

Options:

- one synchronized Lab platform version,
- independent crate/package versions,
- shared crates synchronized, product crates independent.

Current leaning:

- semver everywhere, but delay release policy until packages are real.

### Public packages or git-only/private?

Options:

- publish all,
- publish only stable shared packages,
- keep product packages git-only,
- private registry.

Current leaning:

- git tags first.

## Backend Shape

### When should `lab-oauth` get a full runtime builder?

Known:

- `lab-auth` is the auth library.
- `lab-oauth` is the target OAuth runtime/server boundary.
- OAuth runtime/server routes need to be composable by future products.

Open:

- whether `lab-oauth` should own a full runtime builder in the first extraction
  wave or start as a thin binary/composition wrapper.

### How much MCP code belongs in `lab-surface`?

Risk:

- `lab-surface` could become too broad.

Current leaning:

- keep protocol primitives/shared envelopes in `lab-surface`,
- keep product MCP behavior in product crates.

### What is the first product extraction?

Options:

- Gateway first: highest value, highest coupling.
- Doctor/Stash first: lower value, lower coupling.

Current leaning:

- Gateway is the proof that matters, but a smaller product may be useful as a
  pattern spike.

### Resolved: `extract` is retired

Known:

- It primarily discovers credentials from existing homelab appdata and writes
  Lab `.env` values.
- Some supported parsers target older service integrations.

Decision:

- `extract` is removed from the current product surface.
- Do not create a `lab-extract` crate in this extraction plan.
- Rebuild credential discovery only if a future product requirement justifies a
  new design.

## REST and Client Generation

### Should generated client wrappers be generated or hand-written?

Options:

- raw `openapi-fetch` only,
- generated product-friendly wrappers,
- hand-written wrappers over generated types.

Current leaning:

- start with raw typed client plus a few hand-written wrappers for Gateway;
  generate wrappers later if the shape repeats.

### How do we version REST and MCP contracts together?

Known:

- REST/OpenAPI and MCP/action contracts are separate surfaces.

Open:

- whether one package version should cover both or whether manifests should
  version independently.

## Frontend

### How Next-specific should `@jmagar/lab-web` be?

Options:

- React-only core plus `/next` adapters,
- explicitly Next-first package,
- split `lab-web-core` and `lab-web-next`.

Current leaning:

- React package with optional Next-specific exports.

### Should `lab-web-assets` be shared?

Options:

- shared Rust crate,
- generated code per product binary,
- no crate; each product owns asset serving.

Current leaning:

- defer until a standalone binary needs embedded UI.

### How does Aurora get consumed?

Options:

- npm package,
- git package,
- shadcn registry URL,
- workspace path during development.

Current leaning:

- workspace/git during development, package or registry for reuse.

## Security and Auth

### What is the minimum auth contract for generated clients?

Open:

- token getter shape,
- refresh behavior,
- scope checks,
- handling bearer dev mode.

### How should frontend auth expose scopes?

Open:

- generic `useRequireScope`,
- product-specific guards,
- route metadata-driven guards.

## Execution

### How strict should boundary enforcement be initially?

Options:

- advisory docs only,
- rg-based CI checks,
- cargo metadata/package dependency checks,
- full lint rules.

Current leaning:

- start with simple scripts once packages exist.

### When should external repos be created?

Current leaning:

- after in-repo crate/package boundaries pass tests and have at least one
  consumer fixture.
