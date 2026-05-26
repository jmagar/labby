# Spec: Lab Extractable Platform Boundaries

Status: draft
Owner: lab platform
Surfaces: Rust crates, Rust binaries, TypeScript packages, admin web apps

Contract: `docs/crate-extract/contract.md`
Execution strategy: `docs/crate-extract/execution-strategy.md`
Research findings: `docs/crate-extract/research.md`
Inventory: `docs/crate-extract/inventory.md`
Dependency map: `docs/crate-extract/dependency-map.md`
API surface: `docs/crate-extract/api-surface.md`
Package manifest: `docs/crate-extract/package-manifest.md`
Migration roadmap: `docs/crate-extract/migration-roadmap.md`
Testing strategy: `docs/crate-extract/testing-strategy.md`
Open questions: `docs/crate-extract/open-questions.md`

## Motivation

Lab has grown into several reusable product surfaces inside one binary and
one web app. Gateway, ACP, Marketplace, Stash, OAuth, Fleet/Nodes, logs,
setup, doctor, and the admin web shell are useful outside the current Lab
product.

The goal is to turn those surfaces into reusable packages with clear ownership
and dependency direction. Future products should be able to depend on a small
set of Lab packages the way an application depends on Rust crates or Node
packages, without copying Lab source into the product repo and without dragging
in the full `labby` application.

## Goals

- Define the reusable backend crates Lab should expose.
- Define the reusable frontend packages Lab should expose.
- Separate product runtimes from shared platform primitives.
- Make future standalone binaries possible without requiring an immediate repo
  extraction.
- Preserve the current full `labby` binary as a composition of the same
  reusable boundaries.
- Keep backend authorization enforced in Rust while moving reusable frontend
  auth UX into a frontend package.
- Keep Aurora as the design system dependency for Lab web products.

## Non-goals

- No immediate code movement is specified here.
- No implementation task list is specified here.
- No commitment to publish on `crates.io` or npm in the first phase.
- No requirement that every candidate boundary becomes a separate repository.
- No requirement that every product boundary has a standalone binary on day one.
- No frontend component rewrite is implied by this spec.

## Package Model

Future products should consume extracted Lab capabilities through package
dependencies, not by vendoring Lab source.

Rust products consume Rust crates:

```toml
[dependencies]
lab-auth = { git = "ssh://git@github.com/jmagar/lab-auth.git", tag = "v0.1.0" }
lab-gateway = { git = "ssh://git@github.com/jmagar/lab-gateway.git", tag = "v0.1.0" }
lab-runtime = { git = "ssh://git@github.com/jmagar/lab-runtime.git", tag = "v0.1.0" }
```

Frontend products consume Node/TypeScript packages:

```json
{
  "dependencies": {
    "@jmagar/lab-web": "git+ssh://git@github.com/jmagar/lab-web.git#v0.1.0",
    "@jmagar/lab-api-client": "git+ssh://git@github.com/jmagar/lab-api-client.git#v0.1.0",
    "@jmagar/aurora": "git+ssh://git@github.com/jmagar/aurora-design-system.git#v0.1.0"
  }
}
```

During active extraction, path dependencies are allowed inside a development
workspace. Product repos should eventually reference versioned git tags or
published packages.

## Backend Crate Set

### Shared Platform Crates

These crates are not products. They are the reusable substrate other product
crates depend on.

| Crate | Type | Owns |
|---|---|---|
| `lab-auth` | Rust library | Auth primitives, scopes, tokens, validation, auth middleware, session state, protected resource metadata contracts. |
| `lab-config` | Rust library | Config discovery, config parsing helpers, env loading, env merge, secret masking, public URL resolution, shared config traits. |
| `lab-runtime` | Rust library | Startup helpers, runtime directories, bind address helpers, graceful shutdown, process lifecycle helpers, server bootstrap primitives. |
| `lab-catalog` | Rust library | Tool/action registry, registered service metadata, catalog generation, action completion, service filtering. |
| `lab-surface` | Rust library | Shared surface primitives: MCP action envelopes, schema/help conventions, REST error mapping, OpenAPI helpers, status mapping, dispatch result contracts. |
| `lab-observability` | Rust library | Dispatch log fields, redaction helpers, request IDs, actor-key derivation interfaces, activity event interfaces. |

### Product Runtime Crates

These crates are reusable product capabilities. Each may expose a library API
and may later ship an optional binary.

| Crate | Type | Owns |
|---|---|---|
| `lab-gateway` | Rust library + optional binary | MCP upstream proxying, upstream pools, gateway config mutation, import/tombstone state, exposure filters, `scout`/`invoke`, schema resources, protected gateway routes, gateway-specific OAuth integration. |
| `lab-marketplace` | Rust library + optional binary | Unified plugin/agent/MCP marketplace catalog, sync, package metadata, install/update planning, marketplace API actions. |
| `lab-acp` | Rust library + optional binary | ACP provider configuration, adapter process runtime, sessions, model discovery, chat/session APIs, ACP persistence. |
| `lab-fleet` | Rust library + optional binary | Node runtime, controller/node role resolution, enrollment, node WebSocket admission, device/fleet inventory, node log ingestion interfaces. |
| `lab-stash` | Rust library + optional binary | Artifact storage, component snapshots, versioned local assets, stash providers and metadata. |
| `lab-oauth` | Rust library + optional binary | OAuth server/runtime surface built on `lab-auth`, auth metadata routes, callback handling, token administration, development auth flows. |
| `lab-logs` | Rust library + optional binary | Runtime log ingestion, log storage, log search, SSE streaming, activity/log views used by admin products. |
| `lab-workspace` | Rust library + optional binary | Workspace filesystem browser, file preview contracts, workspace root resolution, file API actions. |
| `lab-setup` | Rust library + optional binary | Setup checks, repair flows, plugin hook behavior, idempotent local environment preparation. |
| `lab-doctor` | Rust library + optional binary | Health checks, audits, service reachability checks, diagnostic summaries. |

## Frontend Package Set

Frontend packages are Node/TypeScript packages. They are not Rust crates.

| Package | Type | Owns |
|---|---|---|
| `@jmagar/aurora` | TypeScript/CSS/shadcn registry package | Design system components, tokens, themes, shadcn registry output, visual language. Source of truth lives in `jmagar/aurora-design-system`. |
| `@jmagar/lab-web` | TypeScript/React package | Reusable admin shell, auth bootstrap, frontend session state, protected route wrappers, login/logout UX, nav shell, common toasts/errors, API provider wiring. |
| `@jmagar/lab-api-client` | TypeScript package | Shared API client primitives, generated REST clients, response/error types, and optional MCP/action-dispatch helpers. |
| `create-lab-web-app` or `jmagar/lab-web-template` | Starter/template | Optional scaffold for a new Next.js admin app with Aurora, `@jmagar/lab-web`, and `@jmagar/lab-api-client` already wired. |

## Surface Strategy

The low-tool-count requirement is an MCP exposure concern. It does not require
the HTTP/admin API to use the same action-dispatch shape.

Lab should support two external API shapes over the same product runtime logic:

```text
Product runtime/domain logic
  -> REST/admin HTTP handlers for web apps and generated TypeScript clients
  -> MCP action-dispatch handlers for compact agent/tool exposure
```

For example, Gateway may expose REST routes such as:

```http
GET    /v1/gateways
GET    /v1/gateways/{name}
POST   /v1/gateways
PATCH  /v1/gateways/{name}
DELETE /v1/gateways/{name}
GET    /v1/gateways/{name}/tools
GET    /v1/gateways/{name}/resources
GET    /v1/gateways/{name}/prompts
POST   /v1/gateways/{name}/reload
POST   /v1/gateways/{name}/oauth/start
DELETE /v1/gateways/{name}/oauth/token
```

The MCP surface can still expose a single compact service tool:

```text
gateway({ action, params })
```

This preserves both goals:

- frontend/admin apps get conventional typed APIs,
- generated TypeScript clients can use OpenAPI,
- MCP keeps one tool per product/service to avoid context bloat,
- business logic remains in one shared runtime layer,
- destructive/action metadata remains available to MCP elicitation and CLI
  confirmation.

`ActionSpec` remains the source of truth for MCP action discovery, action help,
MCP schemas, and destructive-action metadata. REST/OpenAPI becomes the source
of truth for web API client generation.

REST routes are an admin/product API facade, not a second business-logic layer.
REST handlers and MCP action handlers must call the same product runtime/domain
functions. A behavior fix should not need to be made twice.

Destructive operation metadata still belongs in the product action metadata so
MCP elicitation and CLI confirmation stay consistent. REST routes should express
destructive behavior through normal HTTP method semantics and auth/scope policy,
while frontend confirmation UX should read product metadata where available.

## API Client Generation

`@jmagar/lab-api-client` should be generated primarily from Rust-emitted
OpenAPI for the REST/admin HTTP API.

The current `ActionSpec` metadata remains useful for MCP discovery, but it is
not sufficient as the primary source for a strongly typed frontend client.
Today it stores parameter types as string labels and return types as
informational names. A typed web client should be generated from explicit
request/response DTOs and OpenAPI route contracts.

The recommended generation stack for web/admin clients is:

```text
Rust REST route DTOs
  -> utoipa::ToSchema + route metadata
  -> product OpenAPI document
  -> OpenAPI TypeScript generator
  -> @jmagar/lab-api-client
```

### Rust Side

Use `utoipa::ToSchema` for REST/OpenAPI schema generation. Reserve
`schemars::JsonSchema` for standalone JSON Schema consumers such as MCP/action
schema projections.

Rust-owned REST request/response DTOs should derive `serde` and
`utoipa::ToSchema`:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct GatewaySchemaParams {
    pub name: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct GatewayServersResponse {
    pub servers: Vec<GatewayServerSummary>,
}
```

REST route metadata should point at real schemas:

```rust
#[utoipa::path(
    get,
    path = "/v1/gateways/{name}/schema",
    params(("name" = String, Path, description = "Gateway name")),
    responses((status = 200, body = GatewaySchemaResponse))
)]
async fn gateway_schema(/* ... */) -> Result<Json<GatewaySchemaResponse>, ApiError> {
    // call shared lab-gateway runtime/domain logic
}
```

The Lab binary or a dedicated generator crate should export OpenAPI documents:

```bash
labby internal export-openapi \
  --products gateway,acp,marketplace,fleet,stash \
  --out packages/lab-api-client/generated/openapi.json
```

The OpenAPI document should include:

- product name,
- endpoint path and method,
- request path/query/body schemas,
- response body schemas,
- auth requirements,
- error envelope schema,
- auth/scope hints when known,
- deprecation and stability metadata when known.

An additional action-contract manifest may still be generated for MCP tooling,
docs, and action-dispatch helpers:

```bash
labby internal export-action-contract \
  --products gateway,acp,marketplace,fleet,stash \
  --out packages/lab-api-client/generated/action-contract.json
```

### TypeScript Side

Use an OpenAPI TypeScript client generator for REST clients.

Preferred tools:

- `openapi-typescript` for route and schema types.
- `openapi-fetch` or a thin repo-owned wrapper for typed request execution.
- A small repo-owned generator only where product-friendly function names are
  better than raw path calls.

Generated output should look like:

```text
packages/lab-api-client/
├── src/
│   ├── core/
│   │   ├── client.ts
│   │   ├── errors.ts
│   │   └── transport.ts
│   ├── gateway/
│   │   ├── client.ts
│   │   └── types.ts
│   ├── acp/
│   │   ├── client.ts
│   │   └── types.ts
│   ├── action/
│   │   └── client.ts
│   └── index.ts
└── generated/
    ├── openapi.json
    ├── openapi-types.ts
    └── action-contract.json
```

Example generated REST client:

```ts
export async function gatewayServers(
  client: LabApiClient,
): Promise<GatewayServersResponse> {
  return client.get("/gateways")
}

export async function gatewaySchema(
  client: LabApiClient,
  name: string,
): Promise<GatewaySchemaResponse> {
  return client.get("/gateways/{name}/schema", {
    params: { path: { name } },
  })
}
```

Example consumer usage:

```ts
import { createLabClient } from "@jmagar/lab-api-client"
import { gatewayServers, gatewaySchema } from "@jmagar/lab-api-client/gateway"

const client = createLabClient({
  baseUrl: "/v1",
  getToken: () => session.accessToken,
})

const servers = await gatewayServers(client)
const schema = await gatewaySchema(client, "github")
```

### Alternatives Considered

`ts-rs` can generate TypeScript directly from Rust types, but it does not solve
REST route and client generation by itself. It is useful for DTOs, less useful
for producing route-aware product clients.

An action-contract-only generator can describe MCP and action-dispatch clients,
but it keeps the web API shaped around commands rather than conventional admin
resources.

The chosen approach should therefore be REST/OpenAPI for web clients and
ActionSpec/action contracts for MCP, CLI, docs, and optional action helpers.

## Extraction Strategy

Extraction should happen in layers:

1. Create internal workspace crates/packages inside the current Lab repository.
2. Move code behind crate/package APIs while keeping the full `labby` binary
   working.
3. Add standalone binaries as thin composition layers over product crates.
4. Add generated REST/OpenAPI clients for the product APIs that need frontend
   reuse.
5. Move crates/packages into separate repositories or publish them only after
   they satisfy the extraction readiness criteria.

The first extraction should not be a fresh rewrite of the backend. Existing
Gateway, ACP, Marketplace, Fleet, OAuth, and Stash behavior has accumulated
runtime edge cases that should be preserved. New frontend packages may be
greenfield when that produces a cleaner reusable shell, as long as the old app
remains a parity reference.

## Versioning and Compatibility

All extracted Rust crates and TypeScript packages should use semver.

Compatibility rules:

- REST APIs remain under explicit versions such as `/v1`.
- OpenAPI documents should include a package/API version.
- `@jmagar/lab-api-client` versions should identify the OpenAPI contract they
  were generated from.
- MCP action contracts should carry a separate manifest version because they
  are not the same surface as REST/OpenAPI.
- Breaking changes to REST routes, response shapes, auth requirements, or MCP
  action params require a major version bump or a compatibility alias.
- Product crates may evolve faster than shared platform crates, but shared
  platform crates should avoid product-specific dependencies.

During active extraction, git tags are enough. Publishing to `crates.io` or npm
is a distribution decision, not an architecture prerequisite.

## Web Asset Serving

`@jmagar/lab-web` is a Node/TypeScript package, not a Rust crate.

If Rust binaries need to embed or serve compiled frontend assets, that should be
a separate helper boundary, for example:

```text
lab-web-assets
  -> embed compiled frontend bundles
  -> serve static assets / SPA fallback
  -> no React component source
```

Product binaries may depend on `lab-web-assets` if they want self-contained
admin UI serving. They should not depend on frontend source packages directly.

### Frontend Auth Ownership

Frontend auth belongs in `@jmagar/lab-web`:

- session fetch
- auth bootstrap state
- protected route wrapper
- login/logout redirects
- bearer-token development support
- scope/role checks for UI gates
- auth-aware API client wiring

Backend enforcement remains in Rust:

- `lab-auth` validates tokens and scopes.
- `lab-oauth` serves OAuth runtime routes.
- product crates and routers enforce authorization on every protected backend
  operation.

Frontend auth must never be the source of authorization truth. It only improves
UX and prevents showing unavailable UI.

## Dependency Direction

Allowed dependency direction:

```text
future product binary/app
  -> product runtime crates
  -> shared platform crates
  -> external crates

future frontend app
  -> @jmagar/lab-web
  -> @jmagar/lab-api-client
  -> @jmagar/aurora
  -> external npm packages
```

Product runtime crates may depend on shared platform crates. Shared platform
crates must not depend on product runtime crates.

Frontend arrows above are app-level dependencies, not a package-to-package chain.
`@jmagar/lab-web` may depend on `@jmagar/lab-api-client` and `@jmagar/aurora`;
`@jmagar/lab-api-client` must stay UI-framework-free and must not depend on
Aurora or `@jmagar/lab-web`.

Allowed examples:

- `lab-gateway -> lab-auth`
- `lab-gateway -> lab-catalog`
- `lab-gateway -> lab-surface`
- `lab-acp -> lab-runtime`
- `lab-oauth -> lab-auth`
- `@jmagar/lab-web -> @jmagar/aurora`
- `@jmagar/lab-web -> @jmagar/lab-api-client`

Disallowed examples:

- `lab-auth -> lab-gateway`
- `lab-catalog -> lab-acp`
- `lab-runtime -> lab-marketplace`
- `lab-gateway -> lab-acp` for chat/session logic
- `@jmagar/aurora -> @jmagar/lab-web`

Cross-product integration must happen through small interfaces or composition
in the final application binary, not by product crates importing each other
directly.

## Runtime Composition

The current full Lab binary should become a composition of product runtimes.

Conceptually:

```text
labby =
  lab-gateway
  + lab-marketplace
  + lab-acp
  + lab-stash
  + lab-oauth
  + lab-fleet
  + lab-logs
  + lab-workspace
  + lab-setup
  + lab-doctor
```

Standalone binaries can compose smaller sets:

```text
lab-gateway =
  lab-runtime
  + lab-config
  + lab-auth
  + lab-oauth client/server pieces required for gateway routes
  + lab-catalog
  + lab-surface
  + lab-observability
  + lab-gateway

lab-acp =
  lab-runtime
  + lab-config
  + lab-auth
  + lab-catalog
  + lab-surface
  + lab-observability
  + lab-acp

lab-marketplace =
  lab-runtime
  + lab-config
  + lab-auth
  + lab-catalog
  + lab-surface
  + lab-observability
  + lab-marketplace
```

The full `labby` binary should not be the only place product runtime wiring
exists. Each product runtime must eventually expose a builder or similar API
that an application can call directly.

## Product Runtime API Shape

Each product runtime crate should expose a library API before or alongside any
standalone binary.

The exact type names may vary, but the architectural contract is:

```rust
pub struct ProductRuntime {
    pub router: Option<axum::Router>,
    pub registry: Option<lab_catalog::ToolRegistry>,
    pub catalog: Option<lab_catalog::Catalog>,
}

pub struct ProductRuntimeBuilder {
    // product-specific configuration and shared dependencies
}

impl ProductRuntimeBuilder {
    pub async fn build(self) -> anyhow::Result<ProductRuntime>;
}
```

Future products should be able to do this:

```rust
let gateway = lab_gateway::GatewayRuntime::builder()
    .config(gateway_config)
    .auth(auth_config)
    .build()
    .await?;

let app = my_product::router().merge(gateway.router.expect("gateway HTTP surface"));
```

The important property is explicit dependency injection. Product crates should
accept config, auth, storage, and runtime dependencies through constructors or
builders instead of reading global Lab state.

## Binaries

Standalone binaries are allowed but secondary to reusable library APIs.

Candidate binaries:

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

Binaries should be thin wrappers around product crates. They may own CLI flags,
process startup, tracing initialization, and final server bind behavior. They
should not contain product business logic that is unavailable to library
consumers.

## Aurora Design System

Aurora is a frontend design system and shadcn registry, not primarily a Rust
crate.

The preferred reuse model is:

```text
@jmagar/aurora
  -> React components
  -> CSS/tokens/themes
  -> shadcn registry JSON
  -> frontend documentation/examples
```

A Rust crate is only appropriate for Aurora-adjacent server concerns, such as:

- embedding a compiled Aurora asset bundle,
- serving registry JSON from a Rust service,
- exposing design tokens to Rust-side templates.

Those are optional helper crates and should not replace the Node/TypeScript
design-system package.

## Current Lab Mapping

Current code already has partial boundaries:

| Target boundary | Current Lab locations |
|---|---|
| Gateway | `crates/lab/src/dispatch/gateway.rs`, `crates/lab/src/dispatch/gateway/**`, `crates/lab/src/dispatch/upstream/**`, `crates/lab/src/api/services/gateway.rs`, `crates/lab/src/api/upstream_oauth.rs` |
| Marketplace | `crates/lab/src/dispatch/marketplace.rs`, `crates/lab/src/dispatch/marketplace/**`, marketplace frontend routes and clients |
| ACP | `crates/lab/src/acp/**`, `crates/lab/src/dispatch/acp/**`, ACP frontend routes and clients |
| Stash | `crates/lab/src/dispatch/stash/**` |
| OAuth | `crates/lab-auth/**`, `crates/lab/src/oauth/**`, auth routes in `crates/lab/src/api/router.rs` |
| Fleet/Nodes | `crates/lab/src/node/**`, `crates/lab/src/api/nodes.rs`, `crates/lab/src/api/nodes/**`, `crates/lab/src/dispatch/node/**` |
| Logs | `crates/lab/src/dispatch/logs/**`, logs frontend routes and clients |
| Workspace | `crates/lab/src/dispatch/fs/**`, filesystem frontend routes and clients |
| Setup | `crates/lab/src/dispatch/setup/**`, setup CLI/API/frontend pieces |
| Doctor | `crates/lab/src/dispatch/doctor/**` |
| Registry/catalog | `crates/lab/src/registry.rs`, `crates/lab/src/catalog.rs` |
| Surfaces | `crates/lab/src/mcp/**`, `crates/lab/src/api/**`, `crates/lab/src/cli/**` |
| Runtime/config | `crates/lab/src/config.rs`, `crates/lab/src/config/**`, `crates/lab/src/cli/serve.rs` |
| Observability | `crates/lab/src/observability/**`, dispatch/logging helpers |
| Lab web shell | `apps/gateway-admin/**` shared shell/auth/layout/API patterns |

The current missing layer is product runtime composition. Today global files
such as `crates/lab/src/registry.rs`, `crates/lab/src/api/router.rs`,
`crates/lab/src/api/state.rs`, and `crates/lab/src/cli/serve.rs` still wire
many product domains together.

## Extraction Readiness Criteria

A boundary is ready to become an independent crate or package when:

- It has a single owner and a clear public API.
- It can be used without importing the full `lab` crate.
- It does not read global Lab process state except through explicit config or
  injected dependencies.
- Its tests can run without building unrelated product runtimes.
- It does not depend on sibling product crates unless the dependency is part of
  the explicit architecture.
- It exposes enough library API for future products to compose it without using
  a Lab-specific binary.
- Its standalone binary, if any, is a thin wrapper around the library API.

## Open Questions

- Should extracted Rust crates live in one multi-crate repository or one repo
  per crate after they leave Lab?
- Should package versioning be synchronized across all Lab crates or allowed to
  evolve independently?
- Which packages should be public and which should remain git-only/private?
- Should `@jmagar/lab-api-client` expose mostly raw `openapi-fetch` calls,
  product-friendly wrapper functions, or both?
- Should `lab-web` ship only framework-neutral primitives where possible, or
  explicitly target Next.js/React as the admin app standard?
- When should `lab-oauth` get a full runtime builder versus starting as a thin
  binary/composition wrapper around `lab-auth` plus shared surface/runtime crates?
- Should `lab-web-assets` be a shared Rust crate, product-specific generated
  code, or just a pattern each product binary owns?

## Success Definition

This architecture succeeds when a new product can depend on only the packages
it needs, compose those packages into its own backend and frontend, and avoid
copying source from the full Lab repository.

For example, a future Gateway-only product should be able to use:

```text
Backend:
  lab-gateway
  lab-auth
  lab-oauth
  lab-config
  lab-runtime
  lab-catalog
  lab-surface
  lab-observability

Frontend:
  @jmagar/lab-web
  @jmagar/lab-api-client
  @jmagar/aurora
```

without depending on ACP, Marketplace, Stash, Fleet, Workspace, Setup, Doctor,
or the full `labby` application.
