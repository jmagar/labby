# Crate Extraction Dependency Map

Status: draft
Related: `docs/crate-extract/spec.md`, `docs/crate-extract/contract.md`

## Purpose

This document records the target dependency graph and the current coupling
points that must be unwound during extraction.

## Target Backend Graph

```text
application binaries
  -> product runtime crates
  -> shared platform crates
  -> external crates
```

Shared platform crates:

```text
lab-auth
lab-config
lab-runtime
lab-catalog
lab-surface
lab-observability
```

Product crates:

```text
lab-gateway
lab-marketplace
lab-acp
lab-fleet
lab-stash
lab-oauth
lab-logs
lab-workspace
lab-setup
lab-doctor
```

Allowed product-to-product dependencies:

```text
none
```

Cross-product orchestration belongs in binaries or composition crates.

## Target Frontend Graph

```text
product web app/template
  -> @jmagar/lab-web
  -> @jmagar/lab-api-client
  -> @jmagar/aurora
  -> external npm packages
```

Package-to-package frontend dependencies:

```text
@jmagar/lab-web -> @jmagar/lab-api-client
@jmagar/lab-web -> @jmagar/aurora
@jmagar/lab-api-client -> external fetch/runtime dependencies only
@jmagar/aurora -> external UI dependencies only
```

Disallowed:

```text
@jmagar/aurora -> @jmagar/lab-web
@jmagar/lab-api-client -> @jmagar/lab-web
@jmagar/lab-web -> product app route files
```

## Current Coupling Points

### Global Registry

Current file:

- `crates/lab/src/registry.rs`

Current role:

- owns service registration for many products,
- imports product dispatch modules,
- builds global default/docs registries,
- defines service visibility and filtering.

Target:

- generic registry/catalog types move toward `lab-catalog`,
- product crates expose local registry fragments,
- integration layer composes fragments.

### Global Router

Current file:

- `crates/lab/src/api/router.rs`

Current role:

- mounts OAuth,
- mounts `/v1`,
- mounts node/fleet routes,
- mounts gateway/upstream OAuth,
- mounts MCP and protected MCP routes,
- mounts OpenAPI docs,
- owns broad auth middleware behavior.

Target:

- shared router/auth helpers move toward `lab-surface`/`lab-runtime`,
- product crates expose route groups,
- integration layer composes route groups.

### Global App State

Current file:

- `crates/lab/src/api/state.rs`

Current role:

- contains catalog/registry,
- service clients,
- gateway manager,
- node store,
- enrollment store,
- logs system,
- ACP registry,
- auth config/state,
- workspace root,
- web assets.

Target:

- split into shared state primitives plus product runtime state,
- product route groups receive their own state,
- integration state only composes product states.

### Global Serve Orchestrator

Current file:

- `crates/lab/src/cli/serve.rs`

Current role:

- resolves runtime role,
- builds registry,
- starts node runtime,
- opens stores,
- builds gateway runtime,
- initializes OAuth,
- installs global managers,
- starts logs/ACP/web/MCP/HTTP.

Target:

- startup helpers move toward `lab-runtime`,
- product initialization moves behind product runtime builders,
- binary stays a composition wrapper.

### Global Runtime Handles

Current examples:

- `dispatch/gateway/client.rs` installs current gateway manager globally.
- `dispatch/acp/client.rs` installs current ACP registry globally.

Target:

- prefer explicit runtime handles in builders/routes,
- keep globals only as temporary compatibility shims,
- remove or isolate globals before crate extraction.

## Known Direction Risks

- `gateway` depends on config, OAuth, upstream proxying, semantic search, MCP
  resources, and virtual Lab services.
- `marketplace` has historical coupling to ACP and stash metadata.
- `api/router.rs` couples OAuth protected routes to gateway manager state.
- `gateway-admin` imports visual primitives from product-specific files in some
  areas; shared visuals should move toward Aurora/lab-web.
- OpenAPI is currently generated from action metadata, not REST resources.

## Boundary Enforcement Targets

Backend:

- no shared crate imports `crate::dispatch::<product>`,
- no product crate imports sibling product internals,
- no product runtime reads global state unless documented as compatibility shim,
- binary crates are allowed to import multiple product crates.

Frontend:

- Aurora imports no Lab web/package code,
- lab-web imports no product route pages,
- lab-api-client imports no React components,
- product apps import lab-web and generated clients, not the reverse.

## Target Composition Examples

Gateway binary:

```text
lab-gateway-bin
  -> lab-gateway
  -> lab-oauth
  -> lab-auth
  -> lab-config
  -> lab-runtime
  -> lab-catalog
  -> lab-surface
  -> lab-observability
```

The arrows above are direct binary/composition dependencies, not a chain where
`lab-gateway` imports `lab-oauth`. Gateway and OAuth are sibling product crates.
The binary composes both with shared auth/config/runtime dependencies.

Full Lab binary:

```text
labby
  -> all product runtime crates
  -> shared platform crates
```

Frontend gateway app:

```text
gateway-admin
  -> @jmagar/lab-web
  -> @jmagar/lab-api-client
  -> @jmagar/aurora
```
