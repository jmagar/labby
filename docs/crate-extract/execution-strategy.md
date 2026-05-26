# Crate Extraction Execution Strategy

Status: draft
Owner: lab platform
Related spec: `docs/crate-extract/spec.md`

## Purpose

This document describes how to execute the crate/package extraction work using
parallel isolated worktrees without turning integration into the real project.

The architecture target lives in `spec.md`. This file covers execution:

- worktree layout,
- agent lane ownership,
- write scopes,
- parallel waves,
- choke-point ownership,
- merge order,
- verification gates.

## Core Rule

Worktrees prevent live file contention. They do not remove merge contention.

Parallel lanes are useful only when each lane has a clear write scope and one
integration lane owns shared wiring files.

## Worktree Model

Use one branch/worktree per lane:

```text
lab main extraction branch
  ├── ../lab-catalog
  ├── ../lab-surface
  ├── ../lab-config
  ├── ../lab-runtime
  ├── ../lab-gateway
  ├── ../lab-acp
  ├── ../lab-marketplace
  ├── ../lab-fleet
  ├── ../lab-stash
  ├── ../lab-web
  └── ../lab-integration
```

Each lane should commit independently. The integration lane merges completed
branches one at a time.

## Choke-Point Ownership

Only the integration lane should edit these shared wiring files unless
explicitly reassigned:

- workspace/root `Cargo.toml`
- `crates/lab/Cargo.toml`
- `crates/lab/src/lib.rs`
- `crates/lab/src/main.rs`
- `crates/lab/src/registry.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/api/state.rs`
- `crates/lab/src/cli.rs`
- `crates/lab/src/cli/serve.rs`
- `apps/gateway-admin/package.json`
- root package-manager lockfiles, if introduced
- CI workflow files

Product lanes may add local builders, local state structs, local route modules,
local tests, and package-local manifests under their owned crate/package
directories. They should not wire themselves into the global binary/router or
workspace manifests unless the lane explicitly owns integration for that wave.

## Lane Ownership

### Shared Platform Lanes

`lab-catalog` lane owns:

- extracted registry/catalog data structures,
- action/service metadata types,
- catalog generation,
- action completion/filtering,
- tests for registry/catalog behavior.

`lab-surface` lane owns:

- shared HTTP/MCP action envelopes,
- API error envelope contracts,
- status mapping helpers,
- OpenAPI helper primitives,
- MCP/action helper contracts.

`lab-config` lane owns:

- config file discovery helpers,
- dotenv/env loading helpers,
- env merge primitives,
- secret masking helpers,
- generic public URL/path helpers.

`lab-runtime` lane owns:

- runtime directory helpers,
- bind address helpers,
- graceful shutdown/server bootstrap primitives,
- process lifecycle helpers.

### Product Runtime Lanes

`lab-gateway` lane owns:

- `crates/lab/src/dispatch/gateway.rs`
- `crates/lab/src/dispatch/gateway/**`
- `crates/lab/src/dispatch/upstream.rs`
- `crates/lab/src/dispatch/upstream/**`
- gateway-specific REST facade modules,
- gateway runtime builder and tests.

`lab-acp` lane owns:

- `crates/lab/src/acp/**`
- `crates/lab/src/dispatch/acp.rs`
- `crates/lab/src/dispatch/acp/**`
- ACP runtime builder and tests.

`lab-marketplace` lane owns:

- `crates/lab/src/dispatch/marketplace.rs`
- `crates/lab/src/dispatch/marketplace/**`
- marketplace runtime builder and tests.

`lab-fleet` lane owns:

- `crates/lab/src/node/**`
- `crates/lab/src/api/nodes.rs`
- `crates/lab/src/api/nodes/**`
- `crates/lab/src/dispatch/node.rs`
- `crates/lab/src/dispatch/node/**`
- fleet runtime builder and tests.

`lab-stash` lane owns:

- `crates/lab/src/dispatch/stash.rs`
- `crates/lab/src/dispatch/stash/**`
- stash runtime builder and tests.

`lab-oauth` lane owns:

- `crates/lab/src/oauth/**`
- auth/OAuth route fragments currently wired through `crates/lab/src/api/router.rs`
- development auth flow runtime pieces,
- OAuth metadata/callback/token administration route extraction,
- OAuth runtime builder and tests.

`lab-logs` lane owns:

- `crates/lab/src/dispatch/logs.rs`
- `crates/lab/src/dispatch/logs/**`
- logs runtime builder and tests.

`lab-workspace` lane owns:

- `crates/lab/src/dispatch/fs.rs`
- `crates/lab/src/dispatch/fs/**`
- workspace/filesystem runtime builder and tests.

`lab-setup` lane owns:

- `crates/lab/src/dispatch/setup.rs`
- `crates/lab/src/dispatch/setup/**`
- setup runtime builder and tests.

`lab-doctor` lane owns:

- `crates/lab/src/dispatch/doctor.rs`
- `crates/lab/src/dispatch/doctor/**`
- doctor runtime builder and tests.

### Frontend Lanes

`lab-api-client` lane owns:

- `packages/lab-api-client/**`, once created,
- OpenAPI client generation scripts,
- generated client typecheck tests,
- consumer fixture tests.

`lab-web` lane owns:

- `packages/lab-web/**`, once created,
- auth bootstrap components,
- admin shell components,
- shared frontend API provider wiring,
- shared UI error/loading/toast primitives.

`lab-web-template` lane owns:

- `templates/lab-web-app/**` or separate template package,
- Next.js app scaffold,
- Aurora wiring,
- starter routes and config.

Aurora source remains outside this repo unless that repository is explicitly
included in a separate coordinated change.

## Parallel Waves

### Wave 0: Specification and Evidence

Sequential.

- Keep `docs/crate-extract/spec.md` current.
- Keep `docs/crate-extract/research.md` current.
- Keep this execution strategy current.

Exit criteria:

- target package boundaries are stable enough for lane assignment,
- choke-point owner is assigned,
- each lane has a disjoint write scope.

### Wave 1: Shared Platform Seams

Mostly parallel, with light coordination.

Run these lanes in parallel:

- `lab-catalog`
- `lab-surface`
- `lab-config`
- `lab-runtime`

Exit criteria:

- shared crates/modules exist inside the workspace,
- global Lab still builds,
- no product runtime depends on another product runtime through the shared
  crates,
- integration lane can wire old behavior through new shared APIs.

### Wave 2: Product Runtime Builders

Highly parallel.

Run these lanes in parallel:

- `lab-gateway`
- `lab-acp`
- `lab-marketplace`
- `lab-fleet`
- `lab-stash`
- `lab-oauth`
- `lab-logs`
- `lab-workspace`
- `lab-setup`
- `lab-doctor`

Each lane should expose a local runtime builder or equivalent API, but leave
global wiring to the integration lane.

Exit criteria per lane:

- product runtime builder exists,
- product tests pass in isolation where possible,
- existing behavior remains reachable in the current `labby` composition after
  integration,
- public API does not import sibling product internals.

### Wave 3: Frontend Packages and Client Generation

Parallel with late Wave 2 work, but not before REST/OpenAPI direction is stable
for at least one product.

Run these lanes in parallel:

- `lab-api-client`
- `lab-web`
- `lab-web-template`

Exit criteria:

- generated client typechecks,
- `lab-web` has one local consumer fixture,
- template app builds,
- package exports are explicit and stable enough for first consumers.

### Wave 4: Standalone Binaries

Parallel per product after runtime builders exist.

Candidate binary lanes:

- `lab-gateway`
- `lab-acp`
- `lab-marketplace`
- `lab-fleet`
- `lab-stash`
- `lab-oauth`

Exit criteria:

- binary is a thin wrapper over product crate APIs,
- binary does not duplicate business logic,
- binary-specific CLI/startup tests exist,
- global `labby` still builds and behaves as the composed product.

### Wave 5: Integration Hardening

Sequential.

- Merge completed lanes one at a time.
- Resolve choke-point conflicts centrally.
- Run full workspace verification.
- Add missing boundary tests.
- Update docs.

Exit criteria:

- `cargo check --workspace --all-features` passes,
- `cargo nextest run --workspace --all-features` passes or failures are
  documented and unrelated,
- frontend packages build/typecheck,
- generated client is reproducible,
- `labby` full runtime still works,
- at least one standalone binary works end to end.

## Merge Strategy

1. Start from the integration branch.
2. Merge shared platform lanes first.
3. Merge product lanes from smallest/least coupled to largest when possible:
   `lab-doctor`, `lab-stash`, `lab-workspace`, `lab-logs`, `lab-oauth`,
   `lab-fleet`, `lab-marketplace`, `lab-acp`, `lab-gateway`.
4. Merge frontend lanes after the first product REST/OpenAPI contract is stable.
5. Merge standalone binaries last.

For each branch:

```bash
git merge --no-ff <lane-branch>
cargo check --workspace --all-features
```

Run broader tests after batches rather than after every tiny merge if the batch
is low-risk.

## Verification Gates

Minimum backend gate:

```bash
cargo check --workspace --all-features
cargo nextest run --workspace --all-features
```

Minimum frontend gate when packages are present:

```bash
pnpm --dir packages/lab-api-client build
pnpm --dir packages/lab-api-client typecheck
pnpm --dir packages/lab-web build
pnpm --dir packages/lab-web typecheck
pnpm --dir templates/lab-web-app build
```

Minimum generated-client gate:

```bash
cargo run -p labby --all-features -- docs generate
cargo run -p labby --all-features -- docs check
cp docs/generated/openapi.json packages/lab-api-client/generated/openapi.json
pnpm --dir packages/lab-api-client generate
pnpm --dir packages/lab-api-client typecheck
```

This is the transitional gate while OpenAPI is still generated through
generated-docs. Replace it with a product REST/OpenAPI exporter once that command
exists.

Minimum gateway standalone gate:

```bash
cargo build -p <package-that-declares-binary> --bin lab-gateway --all-features
lab-gateway --help
```

This gate starts only after a `lab-gateway` binary target exists.

## Boundary Enforcement Ideas

Add enforcement as soon as it becomes cheap:

- Rust import boundary tests: product crates may depend on shared crates, not
  sibling product crates.
- Frontend import boundary tests: `@jmagar/aurora` must not import
  `@jmagar/lab-web`; `@jmagar/lab-web` must not import product app routes.
- OpenAPI/client contract test: generated OpenAPI produces TypeScript that
  typechecks.
- Runtime parity tests: REST handlers and MCP action handlers call shared domain
  functions rather than duplicating behavior.

## Rollback Strategy

Each lane should land as small commits that can be reverted independently.

If a product extraction destabilizes the full binary:

1. Revert only that lane merge.
2. Keep shared platform lane merges if they are already green.
3. Record the blocker in `docs/crate-extract/research.md` or a follow-up issue.
4. Re-run the product lane from the latest integration branch.

## Notes

- Isolated worktrees allow aggressive parallel implementation, but they do not
  remove the need for disjoint ownership.
- Global wiring files are integration files. Treat them as merge points, not as
  product-lane implementation files.
- The first successful product runtime builder sets the pattern. Gateway is the
  highest-value proof, but also the most coupled. A smaller product can be used
  as the first pattern if integration risk is too high.
