# Crate Extraction Inventory

Status: draft
Related: `docs/crate-extract/spec.md`, `docs/crate-extract/contract.md`

## Purpose

This document maps current Lab files to target extraction boundaries. It is a
working inventory, not a delete list.

Use it to assign ownership before implementation and to avoid two lanes editing
the same files in parallel.

## Shared Choke Points

These files currently compose multiple product domains and should be owned by
the integration lane during extraction:

- workspace/root `Cargo.toml`
- `crates/lab/Cargo.toml`
- `crates/lab/src/lib.rs`
- `crates/lab/src/main.rs`
- `crates/lab/src/registry.rs`
- `crates/lab/src/catalog.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/api/state.rs`
- `crates/lab/src/cli.rs`
- `crates/lab/src/cli/serve.rs`
- `crates/lab/src/config.rs`
- `crates/lab/src/config/**`
- `crates/lab/src/mcp/**`
- `crates/lab/src/output/**`
- `apps/gateway-admin/package.json`
- frontend package-manager lockfiles, if introduced

## Shared Platform Candidates

### `lab-auth`

Current files:

- `crates/lab-auth/**`
- auth route usage in `crates/lab/src/api/router.rs`
- auth config usage in `crates/lab/src/config.rs`

Notes:

- `crates/lab-auth` already exists and is the strongest existing crate boundary.
- Product route wiring should remain outside `lab-auth`.

### `lab-config`

Current files:

- `crates/lab/src/config.rs`
- `crates/lab/src/config/**`

Ambiguity:

- Current config contains both generic mechanics and product-specific fields.
- Extraction likely needs generic config primitives plus product config structs.

### `lab-runtime`

Current files:

- `crates/lab/src/cli/serve.rs`
- `crates/lab/src/process/**`
- `crates/lab/src/net/**`

Ambiguity:

- `serve.rs` mixes runtime bootstrapping with product startup.
- Extract helpers first; do not move all serve logic wholesale.

### `lab-catalog`

Current files:

- `crates/lab/src/registry.rs`
- `crates/lab/src/catalog.rs`
- catalog consumers in `crates/lab/src/mcp/**`, `crates/lab/src/cli/help.rs`,
  and docs generation.

### `lab-surface`

Current files:

- `crates/lab/src/api.rs`
- `crates/lab/src/api/error.rs`
- `crates/lab/src/api/openapi.rs`
- `crates/lab/src/dispatch/error.rs`
- `crates/lab/src/dispatch/helpers.rs`
- shared MCP action/envelope helpers in `crates/lab/src/mcp/**`

Ambiguity:

- Keep `lab-surface` narrow. It should not become a bucket for all API, MCP,
  and CLI code.

### `lab-observability`

Current files:

- `crates/lab/src/observability/**`
- `crates/lab/src/dispatch/logs/ingest.rs`
- dispatch logging helpers and conventions referenced by `docs/dev/OBSERVABILITY.md`

## Product Runtime Candidates

### `lab-gateway`

Current files:

- `crates/lab/src/dispatch/gateway.rs`
- `crates/lab/src/dispatch/gateway/**`
- `crates/lab/src/dispatch/upstream.rs`
- `crates/lab/src/dispatch/upstream/**`
- `crates/lab/src/api/services/gateway.rs`
- `crates/lab/src/api/upstream_oauth.rs`
- `crates/lab/src/oauth/upstream.rs`
- `crates/lab/src/oauth/upstream/**`
- `crates/lab/tests/gateway_schema_resources.rs`

Frontend:

- `apps/gateway-admin/app/(admin)/gateway/**`
- `apps/gateway-admin/app/(admin)/gateways/**`
- `apps/gateway-admin/components/gateway/**`
- `apps/gateway-admin/components/upstream-oauth/**`
- `apps/gateway-admin/lib/api/gateway-*`
- `apps/gateway-admin/lib/api/upstream-oauth-client.ts`
- `apps/gateway-admin/lib/types/gateway.ts`
- `apps/gateway-admin/lib/types/upstream-oauth.ts`
- `apps/gateway-admin/lib/hooks/use-gateways.ts`
- `apps/gateway-admin/lib/hooks/use-upstream-oauth.ts`
- `apps/gateway-admin/lib/gateway-env.ts`
- `apps/gateway-admin/lib/gateway-protected-route.ts`
- `apps/gateway-admin/lib/server/gateway-*`

Do not accidentally trim:

- `crates/lab/src/dispatch/gateway/semantic.rs`
- `crates/lab-apis/src/qdrant.rs`
- `crates/lab-apis/src/qdrant/**`
- `crates/lab-apis/src/tei.rs`
- `crates/lab-apis/src/tei/**`

These back gateway search/indexing behavior.

### `lab-marketplace`

Current files:

- `crates/lab/src/dispatch/marketplace.rs`
- `crates/lab/src/dispatch/marketplace/**`
- `crates/lab/src/cli/marketplace.rs`
- `crates/lab/src/cli/marketplace/**`

Frontend:

- `apps/gateway-admin/app/(admin)/marketplace/**`
- `apps/gateway-admin/components/marketplace/**`
- `apps/gateway-admin/lib/marketplace/**`
- `apps/gateway-admin/lib/api/marketplace-*`
- `apps/gateway-admin/lib/api/mcpregistry-*`

### `lab-acp`

Current files:

- `crates/lab/src/acp/**`
- `crates/lab/src/dispatch/acp.rs`
- `crates/lab/src/dispatch/acp/**`

Frontend:

- `apps/gateway-admin/app/(admin)/chat/**`
- `apps/gateway-admin/lib/acp/**`
- `apps/gateway-admin/lib/chat/**`
- ACP-related API clients and types.

### `lab-fleet`

Current files:

- `crates/lab/src/node/**`
- `crates/lab/src/api/nodes.rs`
- `crates/lab/src/api/nodes/**`
- `crates/lab/src/dispatch/node.rs`
- `crates/lab/src/dispatch/node/**`
- `crates/lab/src/cli/nodes.rs`

Frontend:

- `apps/gateway-admin/app/(admin)/nodes/**`
- `apps/gateway-admin/components/nodes/**`
- node/device API clients and types.

### `lab-stash`

Current files:

- `crates/lab/src/dispatch/stash.rs`
- `crates/lab/src/dispatch/stash/**`
- `crates/lab/src/cli/stash.rs`

### `lab-oauth`

Shared auth library source:

- `crates/lab-auth/**`

Product OAuth runtime source candidates:

- `crates/lab/src/oauth/**`
- auth/OAuth route wiring in `crates/lab/src/api/router.rs`
- gateway upstream OAuth usage in `crates/lab/src/api/upstream_oauth.rs`

Notes:

- `lab-auth` should remain the library for auth primitives.
- `lab-oauth` should be the runtime/server product wrapper around OAuth flows.

### `lab-logs`

Current files:

- `crates/lab/src/dispatch/logs.rs`
- `crates/lab/src/dispatch/logs/**`
- `crates/lab/src/cli/logs.rs`
- logs API/frontend clients and routes.

### `lab-workspace`

Current files:

- `crates/lab/src/dispatch/fs.rs`
- `crates/lab/src/dispatch/fs/**`
- filesystem/workspace frontend clients and routes.

### `lab-setup`

Current files:

- `crates/lab/src/dispatch/setup.rs`
- `crates/lab/src/dispatch/setup/**`
- setup frontend routes and API clients.

### `lab-doctor`

Current files:

- `crates/lab/src/dispatch/doctor.rs`
- `crates/lab/src/dispatch/doctor/**`
- `crates/lab/src/cli/doctor.rs`

## Frontend Package Candidates

### `@jmagar/lab-api-client`

Target files once created:

- `packages/lab-api-client/**`

Source material:

- `apps/gateway-admin/lib/api/**`
- `apps/gateway-admin/lib/types/**`
- future product OpenAPI documents.

### `@jmagar/lab-web`

Target files once created:

- `packages/lab-web/**`

Source material:

- `apps/gateway-admin/components/app-sidebar.tsx`
- shared layout/shell components
- auth bootstrap and protected route components
- shared toast/error/loading primitives
- API provider wiring.

### `lab-web-template`

Target files once created:

- `templates/lab-web-app/**`

Source material:

- `apps/gateway-admin/next.config.mjs`
- `apps/gateway-admin/postcss.config.mjs`
- `apps/gateway-admin/tsconfig.json`
- `apps/gateway-admin/components.json`
- Aurora setup from `../aurora-design-system`.

## Generated and Local Artifacts

These are not source boundaries:

- `target/**`
- `node_modules/**`
- `apps/gateway-admin/.next/**`
- `apps/gateway-admin/out/**`
- `.cache/**`
- `.full-review/**`
- `.worktrees/**`

## Open Inventory Questions

- Which exact frontend files become `@jmagar/lab-web` versus product app code?
- When should `lab-oauth` get a full runtime builder versus starting as a thin
  binary/composition wrapper around `lab-auth` until later?
- How much of `crates/lab/src/mcp/**` belongs in `lab-surface` versus product
  crates?
- Does `lab-logs` own activity event types, or does `lab-observability` own the
  interface and `lab-logs` own storage/search?
