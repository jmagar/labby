# Crate Extraction Package Manifest

Status: draft
Related: `docs/crate-extract/spec.md`, `docs/crate-extract/contract.md`

## Purpose

This document defines the intended reusable packages and the minimum API each
must provide before it is considered extractable.

## Shared Platform Crates

### `lab-auth`

Type: Rust library

Owns:

- token validation and issuance primitives,
- scopes,
- auth middleware,
- auth/session state,
- protected resource metadata contracts.

Must not own:

- product-specific route handlers,
- gateway-specific OAuth behavior,
- frontend auth UX.

First consumers:

- `lab-oauth`
- `lab-gateway`
- `lab-acp`
- full `labby` binary.

### `lab-config`

Type: Rust library

Owns:

- config discovery,
- env loading,
- env merge,
- secret masking,
- public URL helpers,
- generic config traits.

Must not own:

- product-specific runtime behavior,
- service business logic.

First consumers:

- `lab-gateway`
- `lab-runtime`
- full `labby` binary.

### `lab-runtime`

Type: Rust library

Owns:

- runtime directories,
- bind helpers,
- graceful shutdown,
- process lifecycle helpers,
- server bootstrap primitives.

Must not own:

- product registry contents,
- product route definitions.

### `lab-catalog`

Type: Rust library

Owns:

- registry structs,
- service/action metadata,
- catalog generation,
- filtering/completion helpers.

Must not own:

- product registration side effects,
- product dispatch handlers.

### `lab-surface`

Type: Rust library

Owns:

- action request/response envelopes,
- shared error envelope contracts,
- REST status mapping helpers,
- MCP/action helper contracts,
- OpenAPI helper primitives.

Must not own:

- all API/router code,
- product-specific route handlers.

### `lab-observability`

Type: Rust library

Owns:

- dispatch log field conventions,
- redaction helpers,
- request ID helpers,
- actor-key derivation interfaces,
- activity event interfaces.

Must not own:

- logs storage/search implementation.

## Product Runtime Crates

### `lab-gateway`

Type: Rust library + optional binary

Owns:

- gateway manager,
- upstream pools,
- MCP proxying,
- gateway config mutation,
- import/tombstone state,
- exposure filters,
- `scout`/`invoke`,
- schema resources,
- gateway REST facade,
- gateway-specific OAuth/protected route integration.

Required API:

- gateway runtime builder,
- gateway registry fragment,
- gateway REST router,
- gateway MCP/action dispatch adapter.

Readiness:

- gateway standalone binary builds,
- current gateway schema resource tests pass,
- REST and MCP gateway paths share runtime functions.

### `lab-marketplace`

Type: Rust library + optional binary

Owns:

- plugin/agent/MCP marketplace catalog,
- sync,
- package metadata,
- install/update planning,
- marketplace REST/MCP actions.

Required API:

- marketplace runtime builder,
- marketplace registry fragment,
- marketplace REST router.

### `lab-acp`

Type: Rust library + optional binary

Owns:

- ACP provider config,
- adapter process runtime,
- sessions,
- events,
- model discovery,
- ACP persistence.

Required API:

- ACP runtime builder,
- session registry API,
- REST router for sessions/providers,
- MCP/action adapter where needed.

### `lab-fleet`

Type: Rust library + optional binary

Owns:

- node runtime,
- controller/node role resolution,
- enrollment,
- node WebSocket admission,
- device/fleet inventory.

### `lab-stash`

Type: Rust library + optional binary

Owns:

- artifact storage,
- component snapshots,
- revisions,
- stash providers.

### `lab-oauth`

Type: Rust library + optional binary

Owns:

- OAuth runtime/server surface,
- metadata routes,
- callback handling,
- token administration,
- development auth flows.

Depends on:

- `lab-auth`

### `lab-logs`

Type: Rust library + optional binary

Owns:

- log ingestion,
- log storage,
- search,
- stream/tail,
- log stats.

### `lab-workspace`

Type: Rust library + optional binary

Owns:

- workspace filesystem browsing,
- file preview contracts,
- workspace root resolution.

### `lab-setup`

Type: Rust library + optional binary

Owns:

- setup checks,
- repair flows,
- plugin hook behavior,
- local environment preparation.

### `lab-doctor`

Type: Rust library + optional binary

Owns:

- health checks,
- audits,
- reachability checks,
- diagnostic summaries.

## Frontend Packages

### `@jmagar/aurora`

Type: TypeScript/CSS/shadcn registry package

Owns:

- components,
- tokens,
- themes,
- shadcn registry output.

Source:

- `../aurora-design-system`

### `@jmagar/lab-api-client`

Type: TypeScript package

Owns:

- generated OpenAPI types,
- typed request client,
- product-friendly wrappers,
- optional action-dispatch helpers.

Required exports:

- root client factory,
- product modules,
- generated types.

### `@jmagar/lab-web`

Type: TypeScript/React package

Required exports:

- `@jmagar/lab-web`
- `@jmagar/lab-web/auth`
- `@jmagar/lab-web/shell`
- `@jmagar/lab-web/next`

Owns:

- auth bootstrap UX,
- protected route wrappers,
- session hooks,
- admin shell,
- nav primitives,
- error/loading/toast primitives,
- API provider wiring.

Must not own:

- product pages,
- backend authorization,
- full Next app scaffold.

### `create-lab-web-app` / `lab-web-template`

Type: starter/template

Owns:

- Next.js app scaffold,
- Aurora setup,
- app router files,
- starter layout,
- product placeholder routes.

## Manifest Maintenance

Update this file when:

- a package is added or removed,
- ownership changes,
- a new required export is introduced,
- a package gains a permitted dependency exception,
- readiness criteria change.
