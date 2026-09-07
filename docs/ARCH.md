---
title: "Architecture"
created: "2026-07-30"
updated: "2026-07-30"
---

# Architecture

`labby` is a Rust MCP gateway implemented as a workspace split between reusable gateway/auth/runtime crates and product-facing dispatch and surface adapters. The supported product boundary is gateway, Code Mode, authentication, protected routes, the direct stdio MCP proxy, setup, doctor, server logs, snippets, and the optional filesystem browser.

## Core Shape

- One workspace
- Reusable `labby-*` crates plus one product binary crate
- One `labby` binary
- A small set of feature-gated product slices
- One MCP tool per service

## Crate Split

### `crates/labby-primitives`

`labby-primitives` is a dependency-free leaf crate: `ActionSpec`/`ParamSpec`
(action metadata), `PluginMeta`/`EnvVar`/`Category` (plugin metadata),
`UiSchema` (Bootstrap wizard field schemas), and the static SSRF preflight
checks. These types are shared by both `labby-apis` (which re-exports them
from `core/`) and the gateway-extraction crates (`labby-gateway` depends on it
directly), so they live below both rather than in either — avoiding a choice
between forcing the gateway crates to pull in the full SDK, or forcing SDK
service modules to pull in gateway/runtime machinery just to declare
`pub const META: PluginMeta`.

### `crates/labby-apis`

`labby-apis` is the pure SDK layer. It owns:

- typed service clients
- request and response models
- auth handling
- shared HTTP behavior
- shared error taxonomy
- health-check contracts

Action and plugin metadata (`ActionSpec`, `PluginMeta`, etc.) are re-exported
from `labby-primitives` rather than owned here — see above.

It does not own CLI parsing, MCP transport, HTTP routing, `.env` file loading,
or shell-facing UX.

### `crates/labby-auth`

`labby-auth` is the auth middleware crate. It owns:

- OAuth 2.0 authorization server (Google OIDC provider)
- JWT signing and validation (Ed25519 / EdDSA; Google ID-token verification remains RS256)
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
- redaction and path-safety helpers
- backoff/jitter helpers
- feature-gated pure DTO dependencies

Dispatch-helper payloads and the stdio spawn-guard/SSRF security checks live in
`labby-gateway` instead — they are gateway-only concerns, and keeping them here
would pull `labby-primitives` into `labby-auth` and `labby-codemode`'s
dependency graph even though neither ever calls into them.

### `crates/labby-codemode`

`labby-codemode` is the client-neutral Code Mode execution kernel. It owns the
Javy/QuickJS runner protocol, warm runner pool, result shaping, snippet engine,
and TypeScript descriptor generation. Hosts inject tools through `CodeModeHost`.

### `crates/labby-gateway`

`labby-gateway` is the reusable gateway runtime. It owns upstream MCP proxy
pools, discovery/import orchestration, virtual servers, protected routes,
gateway OAuth lifecycle, manager state, the Code Mode host adapter, its own
`action`/`params` dispatch helpers, and the stdio spawn-guard/SSRF security
checks. Its public direct-stdio connector gives the product proxy the same
environment scrubbing, stderr draining, lifecycle negotiation, Unix process
group, and Windows Job Object ownership without routing through the aggregate
gateway catalog. It does not own product config rendering or `.env` writes;
those are injected by the host through `GatewayConfigStore`.

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
- foreground direct stdio proxy orchestration, loopback HTTP, Tailscale Serve,
  and ephemeral OAuth lease supervision
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

`labby proxy` is deliberately different: it is a CLI-only foreground product
runtime for one explicitly selected child. Its HTTP endpoint exposes the
child's MCP surface directly and does not register a `proxy` MCP tool or
`/v1/proxy` action route. OAuth lease management goes through the existing
admin-authenticated `gateway` action surface on a live daemon.

All three consume the same service metadata and service clients.

The canonical ownership and dependency rules between `labby-apis`, extracted runtime crates, the shared dispatch layer, and the product surfaces live in [DISPATCH.md](./dev/DISPATCH.md).

## Logging Shape

Observability is a mandatory shared contract, not a per-service convention.

The canonical source of truth is [OBSERVABILITY.md](./dev/OBSERVABILITY.md).

High-level ownership is:

- `labby` owns caller context and dispatch logging
- `labby-apis::core::HttpClient` owns outbound request logging and transport failure detail

Required boundary rules:

- CLI, MCP, and HTTP must emit one dispatch event per user-visible action
- `HttpClient` must emit `request.start` plus `request.finish` or `request.error` for every outbound call
- health probes must be distinguishable from normal actions
- destructive actions must log intent and outcome

Field-level requirements, redaction rules, and verification gates live in [OBSERVABILITY.md](./dev/OBSERVABILITY.md). Do not redefine them piecemeal in service modules.

## Data Flow

Normal request flow:

1. Load config in `labby`
2. Construct the correct SDK client or product-local subsystem
3. Dispatch through the shared `crates/labby/src/dispatch` layer
4. Let `HttpClient` handle auth, retry, timeout, and error mapping for upstream-backed services
5. Return typed or surface-neutral data to the caller surface
6. Render via CLI, MCP envelope, API envelope, or web view

Direct proxy flow:

1. Resolve and spawn one child through the reusable direct-stdio connector.
2. Bind a Streamable HTTP router to loopback with exact Host/Origin policy.
3. Apply tailnet, bearer, OAuth, or explicit no-auth policy.
4. For OAuth, lease the exact public resource through the live daemon.
5. Publish and supervise one exact Tailscale Serve mapping when selected.
6. On Ctrl+C or component failure, clean owned HTTP, Serve, lease, and process
   resources without touching aggregate gateway state.

See [guides/STDIO_MCP_PROXY.md](./guides/STDIO_MCP_PROXY.md) for the operator
contract and [contracts/stdio-mcp-proxy.md](./contracts/stdio-mcp-proxy.md) for
the stable wire and CLI vocabulary.

## Config Boundary

`labby-apis` never reads config files or ambient env on its own. Config loading lives in `labby`.

- secrets: `$LABBY_HOME/.env` (normally `~/.labby/.env`)
- preferences: exactly `$LABBY_HOME/config.toml` when the absolute override is
  set, otherwise `~/.labby/config.toml`

The binary resolves those inputs, then constructs clients explicitly.
See [Runtime Configuration](./runtime/CONFIG.md) for the authoritative
precedence and path contract.

## Service Model

Feature-gated product slices are `gateway` and `fs`. The supported always-on
operator services are `doctor`, `server_logs`, `setup`, and `snippets`;
`lab_admin` is runtime-conditional. The approved principal-scoped File Stash
contract is current default `gateway-host` functionality on Linux. It is
runtime-conditional, not a separate Cargo feature, and unsupported platforms
omit it from registration and routing.
Retired ACP, Registry-browser, Marketplace, Fleet/device runtime, Deploy-product,
and Agent Artifact Manager implementations are deleted rather than retained as
sleeping aliases. File Stash must not restore their component, revision,
workspace, provider, deploy-target, Marketplace-fork, or drift semantics.

For a first-class service or capability, add only the surfaces it actually
supports:

- a `labby-apis` module when the service needs pure data types, SDK clients, or
  shared metadata
- one shared dispatch entry in `crates/labby/src/dispatch`
- CLI, MCP, API, and web adapters only when the service exposes those surfaces
- one `PluginMeta` when it participates in generated env/service metadata
- one health-check implementation when it models a remotely configured service

Product-local surfaces are explicit. [`GATEWAY.md`](./services/GATEWAY.md)
documents the product-local management surface for runtime upstream
configuration. SDK-only or extracted service modules must not be documented as
current Labby CLI/MCP/API services unless they are registered by the current
`labby` crate feature table.
