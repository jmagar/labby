# Research: Lab Extractable Platform Boundaries

Date: 2026-05-26

Input spec: `docs/crate-extract/spec.md`

Method: adapted `lavra-research` for a spec document instead of a bead epic. Findings are evidence only; the spec was not revised during this research pass.

## Domain Profile

Languages:

- Rust
- TypeScript
- TSX/React

Frameworks and tools:

- Axum
- utoipa
- OpenAPI 3.1
- openapi-typescript
- openapi-fetch
- Next.js
- shadcn registry
- npm packages
- Cargo workspaces

Concerns:

- crate boundaries
- product runtime composition
- REST vs MCP surface separation
- generated TypeScript clients
- frontend package reuse
- OAuth/auth boundary placement
- versioning and compatibility
- avoiding cross-product dependencies

## Sources Checked

- Cargo Workspaces: https://doc.rust-lang.org/cargo/reference/workspaces.html
- utoipa docs: https://docs.rs/crate/utoipa/latest
- schemars derive docs: https://docs.rs/schemars/latest/schemars/derive.JsonSchema.html
- openapi-typescript docs: https://openapi-ts.dev/introduction
- openapi-fetch docs: https://openapi-ts.dev/openapi-fetch/
- npm package/module docs: https://docs.npmjs.com/about-packages-and-modules/
- Next.js `transpilePackages`: https://nextjs.org/docs/app/api-reference/config/next-config-js/transpilePackages
- shadcn registry docs: https://ui.shadcn.com/docs/registry
- Local Lab code: `Cargo.toml`, `crates/lab/Cargo.toml`, `crates/lab/src/api/openapi.rs`, `docs/crate-extract/spec.md`
- Lavra memory recall for Lab setup/schema/frontend/gateway prior findings

## Findings

### 1. The spec's OpenAPI direction is right, but Lab already has a legacy OpenAPI path

Severity: high

The spec says web/admin clients should be generated from REST/OpenAPI, while MCP should keep compact action-dispatch exposure. That is the right direction.

Local code already has `utoipa` and `utoipa-scalar` in the workspace and `crates/lab/src/api/openapi.rs` builds OpenAPI 3.1. The important catch: the current implementation explicitly says it is built programmatically from `ActionSpec` and uses no `#[utoipa::path]` annotations on handlers.

Evidence:

- `Cargo.toml` already declares `utoipa = { version = "5", features = ["axum_extras"] }`.
- `crates/lab/src/api/openapi.rs` says all utoipa coupling is confined there and the spec is built from `ActionSpec`.
- utoipa itself supports code-first OpenAPI for Rust REST APIs, including `ToSchema`, `#[utoipa::path]`, OpenAPI 3.1, and axum support.

Research conclusion:

The spec should treat the current OpenAPI implementation as a transitional artifact. The target should be product REST route OpenAPI, but migration will require replacing or augmenting the ActionSpec-derived `/v1/<service>` OpenAPI with resource-shaped REST route contracts.

### 2. Prefer `utoipa::ToSchema` for REST DTOs, not `schemars`, unless we need schema reuse outside OpenAPI

Severity: medium

The spec currently says the web client generation stack is:

```text
Rust REST route DTOs
  -> schemars JSON Schema
  -> utoipa/OpenAPI document
```

That layering is probably too indirect for this repo. Lab already uses `utoipa::ToSchema` in `crates/lab/src/api/openapi.rs`, and utoipa docs position `ToSchema` plus `#[utoipa::path]` as the normal code-first REST API path.

Schemars is still useful where Lab needs standalone JSON Schema outside OpenAPI, especially MCP/action schemas. But for REST/OpenAPI DTOs, the simpler rule should be:

```text
REST/admin DTOs derive serde + utoipa::ToSchema
MCP/action schemas may use schemars or ActionSpec-derived projection
```

Research conclusion:

Update the spec to distinguish REST DTO schema generation from MCP/action schema generation. Do not require every REST DTO to derive `schemars::JsonSchema` unless a second schema consumer needs it.

### 3. Cargo workspace-first extraction is strongly supported

Severity: medium

The spec says extraction should start as internal workspace crates before moving packages into separate repos. Cargo's official workspace model supports this approach: a workspace manages multiple packages together, shares `Cargo.lock`, has shared target output, and supports shared package/dependency metadata.

Research conclusion:

The spec's extraction strategy is sound. It should also state that initial crates should be workspace members with explicit package boundaries and narrow public APIs, and external publishing should wait until dependency direction and tests are stable.

### 4. Git dependencies are valid for npm packages, but npm does not install git workspaces

Severity: high

The spec shows frontend products consuming `@jmagar/lab-web`, `@jmagar/lab-api-client`, and `@jmagar/aurora` through git URLs. npm supports git URL dependencies with tags/branches/SHAs.

Important caveat from npm docs: installing a package directly from git does not install git submodules or workspaces. That matters if `@jmagar/aurora` or `@jmagar/lab-web` lives inside a monorepo and is referenced as a git dependency.

Research conclusion:

If frontend packages stay in a monorepo, use a package manager workspace during development and publish packages, or use a package-aware release tool. If using git dependencies directly, each dependency should resolve to a repository root with the package's own `package.json`, not an unpublished subpackage buried inside a workspace.

### 5. `@jmagar/lab-web` should assume React/Next adapters, but keep the core shell package Next-light

Severity: medium

Next.js can transpile local packages and external dependencies through `transpilePackages`, which supports monorepo/local package development. That reduces friction for `@jmagar/lab-web` during extraction.

The spec's split is good:

- `@jmagar/lab-web`: reusable package
- `create-lab-web-app` or template: full Next.js starter
- `lab-web-assets`: optional Rust-side static asset serving helper

Research conclusion:

Keep `@jmagar/lab-web` as a React package with optional Next-specific exports, for example:

```text
@jmagar/lab-web
@jmagar/lab-web/next
@jmagar/lab-web/auth
@jmagar/lab-web/shell
```

The package should not require a full Next app inside itself. The template should own app router files, `next.config`, Tailwind/PostCSS config, and starter routes.

### 6. shadcn registry supports Aurora-as-registry, not Rust-crate Aurora

Severity: low

The shadcn registry docs explicitly support running a custom registry to distribute components, hooks, pages, config, rules, and other files, and note the registry is not limited to React.

Research conclusion:

The spec is correct that Aurora should primarily be a frontend package/registry, not a Rust crate. If Aurora needs Rust integration, that should be an asset-serving or token-export helper, not the primary design-system distribution.

### 7. `openapi-typescript` + `openapi-fetch` is a good fit for the generated client

Severity: low

Official openapi-typescript docs state it supports OpenAPI 3.0 and 3.1, generates runtime-free TypeScript types, and can package API types into client SDKs. openapi-fetch is a small type-safe fetch client that infers request/response shapes from OpenAPI-generated types and avoids manual generics.

Research conclusion:

The spec's client generation tooling choice is good. The strongest implementation detail to add later is a contract test:

```text
generate OpenAPI -> generate TS types -> typecheck lab-api-client wrappers -> typecheck one consumer fixture
```

### 8. Prior Lab learnings support the boundary rules

Severity: high

Lavra recall surfaced several recurring Lab lessons relevant to this spec:

- Cross-product imports create coupling and should be avoided; use a public API exposed by the owning runtime instead.
- Visual primitives should live in Aurora/shared token packages, not product-specific theme files imported across products.
- Async frontend handlers need race/abort discipline when they become shared package code.
- Surface-specific action visibility should be structural, not duplicated catalogs.
- OpenAPI/schema work should avoid mixing SDK identity, validation, and UI concerns.

Research conclusion:

The spec's dependency-direction section should be treated as load-bearing, not stylistic. When implementation starts, add enforcement tests or tooling:

- no product crate imports sibling product crates directly,
- no frontend package imports product-specific app routes,
- `@jmagar/aurora` does not import `@jmagar/lab-web`,
- MCP/action visibility is derived from one canonical metadata source.

## Recommended Spec Updates

1. Replace the REST client generation stack from `schemars -> utoipa` with `utoipa::ToSchema -> OpenAPI` for REST DTOs, and reserve `schemars` for MCP/action schemas where needed.

2. Add a note that current Lab OpenAPI is ActionSpec-derived and must be treated as transitional for frontend client generation.

3. Add a packaging caveat: npm git dependencies must point at an actual package root; direct git install does not install workspaces/subpackages as independent packages.

4. Add package export guidance for `@jmagar/lab-web`:

   ```text
   @jmagar/lab-web
   @jmagar/lab-web/auth
   @jmagar/lab-web/shell
   @jmagar/lab-web/next
   ```

5. Add enforcement criteria to extraction readiness:

   - product crate import boundaries are checked,
   - generated OpenAPI and TypeScript client typecheck in CI,
   - REST and MCP routes share runtime/domain functions,
   - frontend shared package has at least one consumer fixture.

6. Add an explicit "transitional OpenAPI" section before implementation planning so future work does not accidentally build the generated client on top of the current loose `/v1/<service>` action-dispatch OpenAPI.

## High-Risk Areas For Future Planning

- Gateway REST facade design: a conventional REST shape needs DTOs and resource routes that do not yet exist.
- `lab-config` extraction: existing config code includes product-specific fields and env behavior; it may need generic primitives plus product config structs.
- `lab-surface` scope: avoid making it a dumping ground for all HTTP/MCP/CLI code.
- `lab-web` scope: avoid putting product pages into the shared package.
- npm distribution: git dependencies against monorepo subpackages are likely to disappoint unless package publishing or workspace-aware tooling is planned.

## Bottom Line

The spec direction is sound. The main correction is schema generation: REST/admin APIs should lean into utoipa's `ToSchema` and route documentation path, while ActionSpec/schemars should stay on the MCP/action side. The second correction is packaging: Node packages are real npm packages with their own `package.json`; direct git dependencies should resolve to package roots or be replaced by published packages/tags.
