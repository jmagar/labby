# Services

`lab` is built around feature-gated service integrations plus a small number of product-local surfaces. Most integrations follow the same structural contract so the CLI, MCP server, API, and TUI can treat them uniformly.

## Per-Service Shape

Most service integrations provide:

- a `lab-apis` module
- a typed client
- request/response types
- a service-specific error type
- a shared dispatch entry
- a `PluginMeta`
- a health-check implementation
- a CLI shim
- an MCP dispatch shim
- an API shim when the service is exposed over HTTP

Product-local surfaces are split into two categories:

- product-local control-plane surfaces, which may live entirely in `lab` when
  they primarily coordinate runtime behavior inside the product
- product-local capability modules, whose core logic still belongs in
  `lab-apis` even when they do not wrap a conventional upstream HTTP API

`gateway` is the reference control-plane surface and is allowed to live
entirely in `lab`.

The ACP/chat work should follow the capability-module pattern for ACP itself:
- `acp` becomes the first-class capability/service
- `chat` remains the UI route and presentation layer over that service

## Feature Gates

Most upstream-backed service integrations are feature-gated in `lab-apis` and re-exported in `lab`.

Categories currently include:

- media
- Servarr
- indexers
- downloads
- notes and documents
- network and infrastructure
- notifications
- AI and inference

Default feature posture:

- `lab-apis` defaults to no optional services
- `lab` defaults to all service integrations
- `all` enables every service integration
- always-on exposed services are compiled into `lab` without feature flags
- always-on SDK capability modules remain available where they are used by those exposed services

## Generated Inventories

Do not maintain service, env, action, feature, or onboarding matrices by hand in
this file. The current code-owned inventories are generated under
[`docs/generated/`](../generated/README.md):

- [service catalog](../generated/service-catalog.md)
- [environment reference](../generated/env-reference.md)
- [action catalog](../generated/action-catalog.md)
- [feature matrix](../generated/feature-matrix.md)

The generated service catalog distinguishes always-on, feature-gated,
runtime-conditional, synthetic, and SDK-only entries. `device_runtime` remains
an always-on SDK capability module, but the exposed registry service is
`device`.

## Service Sources

### Deferred Capability Boundaries

- Radicale
- Beads write operations, raw SQL, Dolt push/pull/commit, and direct Dolt database access
- LoggiFly Docker socket access, raw logs, labels, notification sends/tests, and container/OliveTin actions
- Uptime Kuma status-page mutation, maintenance windows, and fuller supervised socket actor lifecycle

Upstream source coverage lives in [`docs/upstream-api/`](../upstream-api/README.md).
Implementation coverage lives in [`docs/coverage/`](../coverage/README.md).

## Plugin Metadata

Every service publishes `PluginMeta` alongside the service module.

That metadata drives:

- TUI grouping
- install/uninstall prompts
- required env validation
- doctor checks
- docs and presentation

Metadata includes:

- canonical service name
- display name
- short description
- category
- docs URL
- required env vars
- optional env vars
- default port

Categories are part of the product model:

- `Media`
- `Servarr`
- `Indexer`
- `Download`
- `Notes`
- `Documents`
- `Network`
- `Notifications`
- `Ai`
- `Bootstrap`

## Multi-Instance Support

Multi-instance support is generic rather than hardcoded per service.

The config layer recognizes:

- `SERVICE_URL` as the default instance
- `SERVICE_<LABEL>_URL` as named instances

This is especially relevant for:

- Unraid
- Jellyfin
- OpenACP
- Plex
- qBittorrent
- any user who runs multiple copies of the same service

The service library layer stays unaware of instance naming. Instance lookup is a binary-level config concern.

OpenACP is registered as `openacp` and represents the upstream OpenACP daemon,
not Lab's internal `acp` service. Its actions intentionally stay
non-destructive in Lab's action catalog, so Lab CLI/MCP/API confirmation gates
do not apply to prompt/session, config, topic, tunnel, notify, or restart
actions.

## Adding a New Service

Use [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md) as the authoritative end-to-end checklist.

At a high level:

1. start from the upstream spec in `docs/upstream-api/`
2. build the `lab-apis` client and types
3. wire CLI, MCP, and HTTP shims
4. register the service in feature flags, discovery, dispatch, and metadata
5. update the coverage doc under `docs/coverage/`
6. test locally and verify against a real instance when possible

The important rule is that the service client owns logic. CLI, MCP, and HTTP layers only adapt inputs and outputs.

## Service Inventory Direction

The project is intentionally broad but follows one rule: one binary, one consistent control plane, many integrations.

The service set is grouped conceptually, not implemented as unrelated one-offs.

Run `just docs-generate` after changing registry entries, `PluginMeta`,
`ActionSpec`, API route metadata, Cargo features, or onboarding checks. Run
`just docs-check` before handing off generated-docs changes.

## Product-Local Services

[`GATEWAY.md`](../services/GATEWAY.md) documents a product-local management surface that
edits and reloads `[[upstream]]` config and therefore does not fit the usual
`lab-apis` service shape. [`acp/README.md`](../acp/README.md) documents ACP as a
product-local capability service whose core logic belongs in `lab-apis` while
its adapters and registration live in `lab`.

## Chat / ACP Surface

The `/chat` experience is currently a product-local UI surface rather than a first-class service integration:

- it is wired to the gateway backend and ACP bridge endpoints
- its behavior lives in `apps/gateway-admin` plus supporting Rust API routes
- it does not yet have the full first-class `acp` service shape
- the intended promotion path is `acp` as the first-class service and `chat` as
  the UI over it

If we promote chat to a service later, it should follow `SERVICE_ONBOARDING.md` and `DISPATCH.md` like any other first-class integration.
