# Contract: Lab Crate and Package Extraction

Status: draft
Surfaces: Rust crates, Rust binaries, TypeScript packages, generated clients,
REST API, MCP API
Related: `docs/crate-extract/spec.md`,
`docs/crate-extract/research.md`,
`docs/crate-extract/execution-strategy.md`

This contract pins the rules for extracting Lab into reusable backend crates,
frontend packages, and standalone binaries. Any change to package ownership,
dependency direction, generated-client source of truth, or public surface shape
is a contract change and must update this file and the spec in the same PR.

## Package Classes

Lab extraction has three package classes.

### Shared Platform Crates

Shared platform crates provide reusable infrastructure and must not depend on
product runtime crates.

Required shared crates:

- `lab-auth`
- `lab-config`
- `lab-runtime`
- `lab-catalog`
- `lab-surface`
- `lab-observability`

Contract:

- Shared crates MAY depend on external crates.
- Shared crates MAY depend on other shared crates when the dependency direction
  is acyclic and documented.
- Shared crates MUST NOT depend on product runtime crates.
- Shared crates MUST NOT import product-specific modules, routes, config
  structs, UI code, or dispatch handlers.
- Shared crates MUST expose narrow public APIs. Re-exporting broad internal
  modules as convenience exports is a contract violation.

### Product Runtime Crates

Product runtime crates own reusable product capability.

Required product runtime crates:

- `lab-gateway`
- `lab-marketplace`
- `lab-acp`
- `lab-fleet`
- `lab-stash`
- `lab-oauth`
- `lab-logs`
- `lab-workspace`
- `lab-setup`
- `lab-doctor`

Contract:

- Product crates MAY depend on shared platform crates.
- Product crates MUST NOT depend on sibling product runtime crates unless the
  dependency is explicitly listed in this contract.
- Cross-product integration MUST happen in the final application/binary
  composition layer or through a small shared interface.
- Product crates MUST expose a library API usable without invoking a Lab binary.
- Product crates MUST keep business logic out of standalone binary wrappers.

Allowed product-to-product dependency exceptions:

- None at this time.

### Frontend Packages

Frontend packages are Node/TypeScript packages, not Rust crates.

Required frontend packages:

- `@jmagar/aurora`
- `@jmagar/lab-web`
- `@jmagar/lab-api-client`
- `create-lab-web-app` or `jmagar/lab-web-template`

Contract:

- `@jmagar/aurora` MUST NOT depend on `@jmagar/lab-web`.
- `@jmagar/lab-web` MAY depend on `@jmagar/aurora`.
- `@jmagar/lab-web` MAY depend on `@jmagar/lab-api-client`.
- `@jmagar/lab-web` MUST NOT import product app routes or product-specific
  pages.
- `@jmagar/lab-api-client` MUST NOT depend on `@jmagar/lab-web`.
- The starter/template MAY depend on all frontend packages.

## Dependency Direction

Allowed backend direction:

```text
application binary
  -> product runtime crates
  -> shared platform crates
  -> external crates
```

Allowed frontend direction:

```text
product web app or template
  -> @jmagar/lab-web
  -> @jmagar/lab-api-client
  -> @jmagar/aurora
  -> external npm packages
```

This is an app-level dependency set, not a transitive package chain. The allowed
frontend package graph is:

```text
@jmagar/lab-web -> @jmagar/lab-api-client
@jmagar/lab-web -> @jmagar/aurora
@jmagar/lab-api-client -> external fetch/runtime dependencies only
@jmagar/aurora -> external UI dependencies only
```

Disallowed examples:

- `lab-auth -> lab-gateway`
- `lab-catalog -> lab-acp`
- `lab-runtime -> lab-marketplace`
- `lab-gateway -> lab-acp`
- `lab-acp -> lab-gateway`
- `@jmagar/aurora -> @jmagar/lab-web`
- `@jmagar/lab-api-client -> @jmagar/lab-web`

## Runtime API Contract

Every product runtime crate MUST expose a library-level runtime builder or
equivalent composition API.

The exact type names are product-specific, but the contract is:

```rust
pub struct ProductRuntime {
    pub router: Option<axum::Router>,
    pub registry: Option<lab_catalog::ToolRegistry>,
    pub catalog: Option<lab_catalog::Catalog>,
}

pub struct ProductRuntimeBuilder {
    // explicit dependencies only
}

impl ProductRuntimeBuilder {
    pub async fn build(self) -> anyhow::Result<ProductRuntime>;
}
```

Rules:

- Builders MUST accept configuration and runtime dependencies explicitly.
- Builders MUST NOT read global Lab process state except through explicitly
  supplied config/runtime handles.
- Product runtime APIs MUST be usable by a future product without importing the
  full `lab` crate.
- Runtime builders MAY return optional surface pieces when a product does not
  expose every surface.
- Runtime builders MUST make it possible for the full `labby` binary to compose
  products without duplicating product business logic.

## Surface Contract

Lab supports two API shapes over the same product runtime/domain logic.

### REST/Admin HTTP

REST/admin HTTP is the primary surface for web apps and generated TypeScript
clients.

Contract:

- REST routes SHOULD be resource-shaped.
- REST handlers MUST call shared product runtime/domain functions.
- REST handlers MUST NOT duplicate business logic already implemented for MCP
  action handlers.
- REST routes MUST be represented in product OpenAPI documents before they are
  treated as client-generation ready.
- REST auth requirements MUST appear in the OpenAPI document when known.
- REST error responses MUST use the canonical Lab error envelope or a documented
  product-specific extension.

### MCP Action Dispatch

MCP action dispatch is the primary surface for compact agent/tool exposure.

Contract:

- MCP SHOULD expose one compact tool per product/service where practical.
- `ActionSpec` remains the source of truth for MCP action discovery, action
  help, MCP schemas, and destructive-action metadata.
- MCP action handlers MUST call the same product runtime/domain functions as
  REST handlers.
- MCP-only behavior MUST be documented as a protocol-specific exception.
- Surface-specific action visibility MUST be structural. Duplicating entire
  action catalogs for MCP and REST is a contract risk unless a test proves
  parity or intentional divergence.

### CLI

CLI commands MAY use product runtime/domain functions directly or call the same
dispatch layer as MCP.

Contract:

- Destructive CLI operations MUST respect the same destructive metadata as MCP.
- CLI confirmation flags MUST remain consistent with product action metadata.
- CLI-specific formatting MUST stay outside product runtime crates unless the
  product crate explicitly owns reusable CLI helpers.

## OpenAPI and Client Generation Contract

`@jmagar/lab-api-client` is generated primarily from REST/admin OpenAPI.

Current Lab OpenAPI is transitional:

- Existing Lab OpenAPI is generated from `ActionSpec` in
  `crates/lab/src/api/openapi.rs`.
- That current OpenAPI is useful for docs, but it MUST NOT be treated as the
  final source of truth for typed web/admin clients.
- Product REST routes and DTOs must be introduced before a product client is
  considered contract-ready.

REST DTO contract:

- REST request/response DTOs SHOULD derive `serde::Serialize` /
  `serde::Deserialize` as appropriate.
- REST request/response DTOs SHOULD derive `utoipa::ToSchema`.
- `schemars::JsonSchema` SHOULD be reserved for standalone JSON Schema needs,
  especially MCP/action schema projections, unless a DTO has both REST and
  non-OpenAPI schema consumers.

Generated client contract:

- `@jmagar/lab-api-client` MUST include generated OpenAPI types.
- `@jmagar/lab-api-client` SHOULD use `openapi-typescript` for route/schema
  types.
- `@jmagar/lab-api-client` SHOULD use `openapi-fetch` or a thin typed wrapper
  for request execution.
- Product-friendly wrapper functions MAY be generated or hand-written over the
  raw typed client.
- Generated clients MUST typecheck in CI.
- At least one consumer fixture MUST typecheck before a generated client is
  considered ready for reuse.

MCP/action contract generation:

- An action-contract manifest MAY be generated for MCP tooling, docs, and
  optional action-dispatch helpers.
- The action-contract manifest is separate from REST/OpenAPI and MUST carry its
  own version.

## Frontend Package Contract

### `@jmagar/lab-web`

`@jmagar/lab-web` is a reusable React package for Lab-style admin products.

Required export areas:

```text
@jmagar/lab-web
@jmagar/lab-web/auth
@jmagar/lab-web/shell
@jmagar/lab-web/next
```

Contract:

- `@jmagar/lab-web/auth` owns frontend auth UX only.
- Backend authorization remains in Rust (`lab-auth`, `lab-oauth`, and product
  route middleware).
- `@jmagar/lab-web/shell` owns reusable admin shell primitives.
- `@jmagar/lab-web/next` MAY expose Next.js-specific adapters.
- The base package MUST NOT require a full Next.js app inside itself.
- Shared async UI utilities MUST handle abort/race cleanup when crossing await
  boundaries.

### `@jmagar/aurora`

Aurora is the design system and shadcn registry source of truth.

Contract:

- Aurora owns visual primitives, tokens, themes, and shadcn registry output.
- Product-specific visual tokens MUST NOT be imported across products as shared
  primitives.
- Generic visual primitives should move to Aurora.

### Template App

The template/starter owns full Next.js app scaffolding.

Contract:

- The template MAY include app router files, `next.config`, Tailwind/PostCSS
  config, starter routes, and Aurora wiring.
- The template MUST consume `@jmagar/lab-web`; it MUST NOT fork its reusable
  shell/auth implementation.

## Rust Web Asset Serving Contract

`@jmagar/lab-web` is not a Rust crate.

If Rust binaries need to embed or serve compiled frontend bundles, the helper
boundary is:

```text
lab-web-assets
```

Contract:

- `lab-web-assets` MAY embed compiled assets.
- `lab-web-assets` MAY provide static serving and SPA fallback helpers.
- `lab-web-assets` MUST NOT contain React source components.
- Product binaries MAY depend on `lab-web-assets` for self-contained UI
  serving.

## Package Distribution Contract

Rust crates:

- Initial extraction SHOULD happen as workspace crates.
- External git dependencies SHOULD reference versioned tags.
- Publishing to `crates.io` is optional and not required for first reuse.

Node packages:

- Every reusable frontend package MUST have its own `package.json`.
- Direct git dependencies MUST point at a package root.
- Do not rely on direct npm git installs to install subpackages buried inside a
  monorepo workspace.
- If packages stay in a monorepo, use workspace-aware development and a release
  flow that publishes or packages each dependency explicitly.

Versioning:

- Extracted packages MUST use semver.
- REST APIs MUST remain under explicit versions such as `/v1`.
- OpenAPI documents MUST carry an API/package version.
- `@jmagar/lab-api-client` versions MUST identify the OpenAPI contract they
  were generated from.
- Breaking changes to REST routes, response shapes, auth requirements, MCP
  action params, package exports, or dependency direction require a major
  version bump or a compatibility alias.

## Extraction Readiness Contract

A boundary is ready to become an independent crate or package only when all of
these are true:

- It has a single owner.
- It has a narrow public API.
- It can be used without importing the full `lab` crate.
- It does not read global Lab process state except through explicit config or
  injected dependencies.
- Its tests can run without building unrelated product runtimes where practical.
- It does not depend on sibling product crates unless explicitly allowed.
- It exposes enough library API for future products to compose it without using
  a Lab-specific binary.
- Its standalone binary, if any, is a thin wrapper around the library API.
- Import boundary checks exist or are explicitly deferred with rationale.
- REST/OpenAPI and TypeScript client typecheck gates exist when the boundary
  exposes a frontend client.
- At least one consumer fixture exists when the boundary is intended for external
  reuse.

## Verification Contract

Minimum backend verification:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
```

Minimum frontend verification when packages exist:

```bash
pnpm --dir packages/lab-api-client build
pnpm --dir packages/lab-api-client typecheck
pnpm --dir packages/lab-web build
pnpm --dir packages/lab-web typecheck
pnpm --dir templates/lab-web-app build
```

Minimum generated-client verification:

```bash
labby internal export-openapi --products gateway --out packages/lab-api-client/generated/openapi.json
pnpm --dir packages/lab-api-client generate
pnpm --dir packages/lab-api-client typecheck
```

Minimum standalone-binary verification:

```bash
cargo build -p lab --bin lab-gateway --all-features
lab-gateway --help
```

When package names become ambiguous in the workspace, use version-qualified
Cargo package selectors.

## Non-Contractual

The following may change without a contract update:

- Internal module names during in-repo extraction.
- Worktree names.
- Exact implementation order.
- Whether a package is temporarily path-based, git-based, or unpublished during
  extraction.
- The exact names of private builder fields.
- Generated file formatting.

## Contract Change Rules

The following require a contract update:

- Adding or removing a required shared platform crate.
- Adding or removing a required product runtime crate.
- Adding a product-to-product dependency exception.
- Changing REST vs MCP source-of-truth rules.
- Changing generated-client source of truth.
- Changing frontend package ownership.
- Changing package distribution assumptions.
- Changing extraction readiness criteria.
