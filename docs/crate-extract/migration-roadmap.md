# Crate Extraction Migration Roadmap

Status: draft
Related: `docs/crate-extract/execution-strategy.md`

## Purpose

This document summarizes migration phases at a higher level than the execution
strategy. It describes milestone outcomes, not per-task instructions.

## Milestone 0: Architecture Lock

Inputs:

- `spec.md`
- `contract.md`
- `research.md`
- `execution-strategy.md`
- `inventory.md`
- `dependency-map.md`
- `package-manifest.md`

Exit criteria:

- package boundaries are named,
- contract rules are explicit,
- lane ownership is clear,
- open questions are tracked.

## Milestone 1: Shared Platform Seams

Goal:

Create in-repo workspace crate/module boundaries for shared platform primitives.

Outcomes:

- `lab-catalog` shape established,
- `lab-surface` shape established,
- `lab-config` primitives identified,
- `lab-runtime` helpers identified,
- full `labby` binary still builds.

Risk:

- over-extracting shared crates before product needs are proven.

Mitigation:

- start with narrow APIs used by one product plus full `labby`.

## Milestone 2: First Product Runtime Builder

Recommended first proof:

- `lab-gateway` if we want highest-value proof,
- or `lab-doctor`/`lab-stash` if we want lowest coupling first.

Outcomes:

- product has local runtime builder,
- product exposes registry/router fragments,
- product behavior still works through full `labby`,
- no sibling product dependency introduced.

## Milestone 3: Gateway REST/OpenAPI Facade

Goal:

Create conventional REST/admin routes for Gateway while preserving compact MCP
action exposure.

Outcomes:

- Gateway REST resource routes exist,
- Gateway DTOs derive `utoipa::ToSchema`,
- product OpenAPI includes Gateway routes,
- TypeScript client can be generated for Gateway,
- REST and MCP share runtime/domain functions.

## Milestone 4: Generated API Client

Goal:

Create `@jmagar/lab-api-client`.

Outcomes:

- OpenAPI export command exists,
- client types generate reproducibly,
- product wrapper functions typecheck,
- at least one consumer fixture typechecks.

## Milestone 5: Lab Web Package

Goal:

Create `@jmagar/lab-web` from shared admin shell/auth patterns.

Outcomes:

- auth bootstrap and protected route wrapper extracted,
- admin shell primitives extracted,
- package uses Aurora,
- package does not own product pages,
- local consumer fixture builds.

## Milestone 6: Remaining Product Runtimes

Goal:

Apply the runtime builder pattern to remaining products.

Products:

- `lab-acp`
- `lab-marketplace`
- `lab-fleet`
- `lab-stash`
- `lab-logs`
- `lab-workspace`
- `lab-setup`
- `lab-doctor`
- `lab-oauth`

Outcomes:

- each product has owned runtime API,
- global `registry.rs`, `api/router.rs`, `api/state.rs`, and `serve.rs` become
  composition layers instead of product owners.

## Milestone 7: Standalone Binaries

Goal:

Add thin standalone binaries over product crates.

Initial binaries:

- `lab-gateway`
- `lab-acp`
- `lab-marketplace`
- `lab-fleet`

Outcomes:

- binaries build,
- binaries expose `--help`,
- at least `lab-gateway` runs end to end,
- no binary owns product business logic.

## Milestone 8: Externalization Decision

Goal:

Decide distribution shape after boundaries are proven in-repo.

Options:

- keep as Lab workspace crates,
- split into one multi-crate platform repo,
- split into one repo per major product,
- publish selected crates/packages,
- use git tags only.

Exit criteria:

- package versioning policy selected,
- release process selected,
- consumer fixtures updated to use the chosen dependency mode.

## Roadmap Risks

- Gateway is high value but highly coupled.
- REST/OpenAPI client generation requires DTO work.
- `lab-config` can become too broad if product config is not separated.
- `lab-web` can become too broad if product pages leak into it.
- parallel branches can still collide at integration choke points.
