# Architecture

`lab` is a pluggable homelab CLI and MCP server implemented as a Rust workspace with a split between reusable upstream-facing SDK clients and product-facing dispatch and surface layers.

It also includes a product-local device runtime subsystem. That subsystem is separate from gateway and shared service dispatch code and owns fleet role resolution, device ingest, and master-only control-plane gating.

## Core Shape

- One workspace
- Reusable `labby-*` crates plus one product binary crate
- One `labby` binary
- A small set of feature-gated product slices
- One MCP tool per service

## Crate Split

### `crates/labby-apis`

`labby-apis` is the pure SDK layer. It owns:

- typed service clients
- request and response models
- auth handling
- shared HTTP behavior
- shared error taxonomy
- shared action metadata
- plugin metadata
- health-check contracts

It does not own CLI parsing, MCP transport, HTTP routing, `.env` file loading,
or shell-facing UX.

### `crates/labby-auth`

`labby-auth` is the auth middleware crate. It owns:

- OAuth 2.0 authorization server (Google OIDC provider)
- JWT signing and validation (RS256)
- SQLite-backed token and session storage
- axum middleware and route handlers
- upstream OAuth manager/cache/runtime helpers

It is separated from `labby-apis` because it depends on `axum`, which is
forbidden in the pure SDK crate. It does not own CLI parsing or MCP transport.

### `crates/labby-runtime`

`labby-runtime` owns surface-neutral contracts and helpers used across product
and extracted runtime crates:

- `ToolError`
- gateway config DTOs
- dispatch helper payloads
- redaction and path-safety helpers
- shared security/spawn guards
- feature-gated pure DTO dependencies

### `crates/labby-codemode`

`labby-codemode` is the client-neutral Code Mode execution kernel. It owns the
Javy/QuickJS runner protocol, warm runner pool, result shaping, snippet engine,
and TypeScript descriptor generation. Hosts inject tools through `CodeModeHost`.

### `crates/labby-gateway`

`labby-gateway` is the reusable gateway runtime. It owns upstream MCP proxy
pools, discovery/import orchestration, virtual servers, protected routes,
gateway OAuth lifecycle, manager state, and the Code Mode host adapter. It does
not own product config rendering or `.env` writes; those are injected by the
host through `GatewayConfigStore`.

### `crates/labby-web`

`labby-web` owns embedded and filesystem static asset serving for Labby web UI
exports, including symlink escape defense.

### `crates/labby-winjob`

`labby-winjob` is the small Windows Job Object helper crate. It contains the
platform FFI needed for process-tree reaping on Windows so the main workspace
can keep `unsafe_code = "forbid"` elsewhere.

### `crates/labby`

`labby` is the product binary. It owns:

- CLI commands
- MCP server registration and dispatch
- HTTP API route mounting
- config loading
- output rendering
- install/uninstall flows
- doctor and operator workflows
- the device runtime and fleet state store
- product-local dispatch and config-store adapters

It must stay thin at the surface boundary. Reusable gateway, Code Mode, auth,
web-serving, and runtime helpers stay in their extracted crates.

## Golden Rule

If behavior is shared across product surfaces, it belongs in one shared execution layer. Upstream API logic belongs in `labby-apis`; reusable gateway/runtime/code-mode behavior belongs in the extracted `labby-*` crates; product-surface dispatch belongs in `crates/labby/src/dispatch`. The CLI, MCP, HTTP, and web layers are adapters, not logic owners.

That rule is structural, not aspirational:

- `labby-apis` has no `clap`, `rmcp`, or `axum`
- `labby-auth` has no `clap` or `rmcp`
- `labby-runtime` has no product-surface transport dependencies
- `labby` depends on extracted crates rather than duplicating runtime logic

## Module Layout

The workspace uses modern Rust module layout:

- no `mod.rs`
- a module `foo` is declared in `foo.rs`
- its submodules live in `foo/`

Per-service layout in `labby-apis`:

- `<service>.rs`
- `<service>/client.rs`
- `<service>/types.rs`
- `<service>/error.rs`

Per-service layout in `labby` typically includes:

- `src/dispatch/<service>.rs` plus `src/dispatch/<service>/`
- `src/cli/<service>.rs`
- `src/api/services/<service>.rs` when the service is exposed over HTTP

## Shared Contracts

The architecture is anchored around a few cross-cutting contracts:

- `ServiceClient`: common health-check interface
- `ServiceStatus`: normalized health result
- service-specific ID newtypes
- `Auth`: shared auth model
- `ApiError`: normalized transport-layer error taxonomy
- `HttpClient`: shared request/retry/logging/error-mapping layer
- `ActionSpec` / `ParamSpec`: service action catalog schema
- `PluginMeta`: service metadata for generated docs, install/setup flows, and
  doctor checks

These contracts keep service modules consistent and make CLI, MCP, HTTP, web,
and operator tooling compose cleanly.

### `ServiceClient`

Every service client implements a common health surface:

- `name()`
- `service_type()`
- `health()`

That gives `labby health`, `labby doctor`, and MCP `status` surfaces a shared
model without forcing all other service operations into one trait.

### `ServiceStatus`

`ServiceStatus` is the normalized health result shape. Its important fields are:

- reachability
- auth state
- optional version
- latency
- optional detail message

Rules:

- unreachable implies auth is not OK
- health probes have a shorter timeout budget than ordinary requests
- transport failures become structured status data rather than panics

### ID Newtypes

Service identifiers must use service-local newtypes rather than raw integers everywhere. The goal is to prevent mixing:

- internal ids
- external provider ids
- ids from different services

## Runtime Surfaces

The same service logic is exposed through the product surfaces that the service
opts into:

- CLI: `labby <service-or-command> ...`
- MCP stdio: `labby mcp`
- MCP HTTP: `labby serve`
- HTTP API and Labby web UI: `labby serve`

All three consume the same service metadata and service clients.

The canonical ownership and dependency rules between `labby-apis`, extracted runtime crates, the shared dispatch layer, and the product surfaces live in [DISPATCH.md](./dev/DISPATCH.md).

## Logging Shape

Observability is a mandatory shared contract, not a per-service convention.

The canonical source of truth is [OBSERVABILITY.md](./OBSERVABILITY.md).

High-level ownership is:

- `labby` owns caller context and dispatch logging
- `labby-apis::core::HttpClient` owns outbound request logging and transport failure detail

Required boundary rules:

- CLI, MCP, and HTTP must emit one dispatch event per user-visible action
- `HttpClient` must emit `request.start` plus `request.finish` or `request.error` for every outbound call
- health probes must be distinguishable from normal actions
- destructive actions must log intent and outcome

Field-level requirements, redaction rules, and verification gates live in [OBSERVABILITY.md](./OBSERVABILITY.md). Do not redefine them piecemeal in service modules.

## Data Flow

Normal request flow:

1. Load config in `labby`
2. Construct the correct SDK client or product-local subsystem
3. Dispatch through the shared `crates/labby/src/dispatch` layer
4. Let `HttpClient` handle auth, retry, timeout, and error mapping for upstream-backed services
5. Return typed or surface-neutral data to the caller surface
6. Render via CLI, MCP envelope, API envelope, or web view

## Config Boundary

`labby-apis` never reads config files or ambient env on its own. Config loading lives in `labby`.

- secrets: `~/.lab/.env`
- preferences: `config.toml` (`./` → `~/.lab/` → `~/.config/lab/`)

The binary resolves those inputs, then constructs clients explicitly.

## Service Model

Feature-gated product slices currently are `gateway`, `marketplace`, `fs`,
`deploy`, and `acp_registry`. Base control-plane services such as `doctor`,
`setup`, `logs`, `device`, `stash`, and `acp` compile without an individual
feature flag.

For a first-class service or capability, add only the surfaces it actually
supports:

- a `labby-apis` module when the service needs pure data types, SDK clients, or
  shared metadata
- one shared dispatch entry in `crates/labby/src/dispatch`
- CLI, MCP, API, and web adapters only when the service exposes those surfaces
- one `PluginMeta` when it participates in generated env/service metadata
- one health-check implementation when it models a remotely configured service

Product-local surfaces are explicit. `crates/labby-apis::marketplace` exports pure
types while all dispatch and filesystem behavior lives under
`crates/labby/src/dispatch/marketplace/`; [`GATEWAY.md`](./services/GATEWAY.md)
documents the product-local management surface for runtime upstream
configuration; and [`DEVICE_RUNTIME.md`](./runtime/DEVICE_RUNTIME.md) describes
the device runtime that turns every `labby serve` process into either the fleet
controller or a reporting non-controller node.
