# Lab Docs

This directory is the documentation entrypoint for `lab`.

The docs are split by topic so contributors do not have to recover architecture, protocol rules, product behavior, and operator workflows from one large design document.

## Start Here

- Read [ARCH.md](./ARCH.md) to understand the crate split, runtime surfaces, and shared contracts.
- Use [crate-extract/README.md](./crate-extract/README.md) for the reusable crate/package extraction spec, contract, execution strategy, and open questions.
- Use [adr/README.md](./adr/README.md) for accepted architecture decision records.
- Read [CONVENTIONS.md](./CONVENTIONS.md) before changing implementation patterns or core APIs.
- Use [SERVICES.md](./dev/SERVICES.md), [CLI.md](./surfaces/CLI.md), and [MCP.md](./surfaces/MCP.md) for current surface-specific behavior. [TUI.md](./surfaces/TUI.md) records deferred TUI status.
- Use [design/CLI_DESIGN_SYSTEM.md](./design/CLI_DESIGN_SYSTEM.md) for the human-readable CLI output language and shared color policy.
- Use [design/component-development.md](./design/component-development.md) and [design/design-system-contract.md](./design/design-system-contract.md) when building or revising Labby web UI components.
- Use [CONFIG.md](./runtime/CONFIG.md), [HOST_GATEWAY.md](./runtime/HOST_GATEWAY.md), and [OPERATIONS.md](./OPERATIONS.md) for setup, Incus gateway deployment, and operator workflows.
- Refer to [OAUTH.md](./runtime/OAUTH.md) for bearer vs OAuth mode selection, Google-backed authorization flow, lab-issued JWT behavior, and callback-forwarding constraints.
- Use [GATEWAY.md](./services/GATEWAY.md) when managing upstream MCP gateways over CLI, MCP, `/v1/gateway`, or Gateway-managed OAuth protected MCP routes.
- Use [acp/README.md](./acp/README.md) for ACP service architecture, the `acp` vs `chat` boundary, and gateway integration direction.
- Use [acp/design.md](./acp/design.md) for ACP design details and [acp/research-findings.md](./acp/research-findings.md) for the supporting research notes.
- Use [coverage/README.md](./coverage/README.md), [upstream-api/README.md](./upstream-api/README.md), [generated/README.md](./generated/README.md), and [features/README.md](./features/README.md) for directory-level indexes.
- Use [MCPREGISTRY_METADATA.md](./services/MCPREGISTRY_METADATA.md) for Lab-owned registry metadata layered onto the mirrored MCP Registry surface.
- Use [DEVICE_RUNTIME.md](./runtime/DEVICE_RUNTIME.md), [FLEET_LOGS.md](./runtime/FLEET_LOGS.md), and [DEPLOY.md](./runtime/DEPLOY.md) for the master/non-master fleet runtime, device inventory, and deployment model.
- Use [MONITORS.md](./services/MONITORS.md) for Claude Code monitor definitions (`plugins/monitors/monitors.json`) and the `labby deploy monitor` command.
- Use [LOCAL_LOGS.md](./services/LOCAL_LOGS.md) for the local-master runtime log store, `/v1/logs`, SSE streaming, and gateway-admin `/logs`.
- See [UPSTREAM.md](./services/UPSTREAM.md) for upstream MCP gateway setup, configuration, tool merging, circuit breaker behavior, and resource proxying.
- Consult [TRANSPORT.md](./surfaces/TRANSPORT.md) for stdio and streamable HTTP transport configuration, middleware stack, and session management.
- Use [OBSERVABILITY.md](./dev/OBSERVABILITY.md) for the mandatory logging, correlation, redaction, and verification contract.
- Use [ERRORS.md](./dev/ERRORS.md) for the shared error taxonomy, envelope shapes, and status mapping contract.
- Use [design/SERIALIZATION.md](./design/SERIALIZATION.md) for the shared serde, envelope, and output-boundary contract.
- Use [DISPATCH.md](./dev/DISPATCH.md) for the shared surface-neutral dispatch-layer contract and dependency rules.
- Use [SERVICE_LAYER_MIGRATION.md](./dev/SERVICE_LAYER_MIGRATION.md) for the current status of the older service-layer migration plan.
- Use [SERVICE_ONBOARDING.md](./dev/SERVICE_ONBOARDING.md) when you are bringing a new service online end to end.
- Use [SCAFFOLD_AND_AUDIT.md](./dev/SCAFFOLD_AND_AUDIT.md) for the deferred scaffold/audit command contract, [DEPLOY_SERVICE.md](./runtime/DEPLOY_SERVICE.md) for deploy-service actions, and [FLEET_METHODS.md](./runtime/FLEET_METHODS.md) for fleet WebSocket methods.

## Reading Paths

### If You Are Adding or Refactoring Code

1. [ARCH.md](./ARCH.md)
2. [CONVENTIONS.md](./CONVENTIONS.md)
3. [SERVICES.md](./dev/SERVICES.md)
4. Then the surface doc you are touching:
   [CLI.md](./surfaces/CLI.md), [MCP.md](./surfaces/MCP.md), or the relevant HTTP/web docs

### If You Are Working on Product Behavior

1. [CLI.md](./surfaces/CLI.md) for command behavior
2. [design/CLI_DESIGN_SYSTEM.md](./design/CLI_DESIGN_SYSTEM.md) for human-readable output language
3. [MCP.md](./surfaces/MCP.md) for tool and envelope behavior
4. [TRANSPORT.md](./surfaces/TRANSPORT.md) and the service docs for HTTP/web behavior
5. [CONFIG.md](./runtime/CONFIG.md) for config and env implications
6. [OBSERVABILITY.md](./dev/OBSERVABILITY.md) for logging, request tracing, and redaction rules
7. [ERRORS.md](./dev/ERRORS.md) for stable kinds and structured error behavior
8. [design/SERIALIZATION.md](./design/SERIALIZATION.md) for serde and output-boundary rules
9. [DISPATCH.md](./dev/DISPATCH.md) for layer ownership and adapter direction
10. [SERVICE_LAYER_MIGRATION.md](./dev/SERVICE_LAYER_MIGRATION.md) for the current status of the older migration plan

### If You Are Working on a Service Integration

1. [SERVICES.md](./dev/SERVICES.md)
2. [ARCH.md](./ARCH.md)
3. [CONVENTIONS.md](./CONVENTIONS.md)
4. [MCP.md](./surfaces/MCP.md) and [CLI.md](./surfaces/CLI.md) for the public surfaces
5. [OBSERVABILITY.md](./dev/OBSERVABILITY.md) for instrumentation and verification requirements
6. [ERRORS.md](./dev/ERRORS.md) and [design/SERIALIZATION.md](./design/SERIALIZATION.md) for transport and envelope consistency
7. [DISPATCH.md](./dev/DISPATCH.md) for shared operation ownership across CLI, MCP, and API
8. [SERVICE_LAYER_MIGRATION.md](./dev/SERVICE_LAYER_MIGRATION.md) for the refactor sequence if you are migrating existing services

### If You Are Operating the Project

1. [CONFIG.md](./runtime/CONFIG.md)
2. [HOST_GATEWAY.md](./runtime/HOST_GATEWAY.md)
3. [TRANSPORT.md](./surfaces/TRANSPORT.md)
4. [OAUTH.md](./runtime/OAUTH.md) (if deploying with OAuth)
5. [GATEWAY.md](./services/GATEWAY.md) (if managing upstream MCP gateways)
6. [UPSTREAM.md](./services/UPSTREAM.md) (if proxying upstream MCP servers)
7. [DEVICE_RUNTIME.md](./runtime/DEVICE_RUNTIME.md)
8. [NODE_RUNTIME_CONTRACT.md](./runtime/NODE_RUNTIME_CONTRACT.md)
9. [DEPLOY.md](./runtime/DEPLOY.md)
10. [OPERATIONS.md](./OPERATIONS.md)
11. [CLI.md](./surfaces/CLI.md)

## Topic Map

- [ARCH.md](./ARCH.md)
  System shape, crate boundaries, shared contracts, and runtime flow.
- [TECH.md](./TECH.md)
  Stack choices, toolchain, feature posture, verification surfaces, and release tooling.
- [crate-extract/README.md](./crate-extract/README.md)
  Architecture, contract, dependency map, execution strategy, and verification plan for extracting Lab into reusable Rust crates, TypeScript packages, and standalone binaries.
- [adr/README.md](./adr/README.md)
  Accepted architecture decision records.
- [MCP.md](./surfaces/MCP.md)
  Transport model, prompts/completions/logging capabilities, one-tool-per-service design, discovery, envelopes, and destructive-op elicitation.
- [RMCP.md](./surfaces/RMCP.md)
  RMCP SDK integration contract: transports, feature posture, handler patterns, auth ownership, and capability rules.
- [OAUTH.md](./runtime/OAUTH.md)
  HTTP auth modes: static bearer compatibility, internal Google-backed OAuth, lab-issued JWTs, JWKS, RFC 9728 metadata, and redirect/callback forwarding rules.
- [GATEWAY.md](./services/GATEWAY.md)
  Gateway control plane: CRUD, reload/test flows, runtime views, tool exposure policy, and Gateway-managed OAuth protected MCP routes.
- [acp/README.md](./acp/README.md)
  ACP service entrypoint, first-class service design, and the browser `chat` relationship.
- [acp/design.md](./acp/design.md)
  ACP detailed design notes.
- [acp/research-findings.md](./acp/research-findings.md)
  ACP supporting research findings.
- [MCPREGISTRY_METADATA.md](./services/MCPREGISTRY_METADATA.md)
  Lab-owned metadata layered onto mirrored MCP Registry entries: contract, validation, audit fields, filters, CLI, and UI behavior.
- [DEVICE_RUNTIME.md](./runtime/DEVICE_RUNTIME.md)
  Master/non-master runtime roles, `/v1/nodes/*`, AI CLI inventory upload, queueing, and device OAuth relay.
- [NODES.md](./runtime/NODES.md)
  Node-facing CLI/API behavior and controller interactions.
- [NODE_RUNTIME_CONTRACT.md](./runtime/NODE_RUNTIME_CONTRACT.md)
  Controller/node runtime split, node-only artifact rules, HTTP surface boundaries, and rollout verification requirements.
- [FLEET_METHODS.md](./runtime/FLEET_METHODS.md)
  Fleet WebSocket JSON-RPC method contract and enrollment/session behavior.
- [FLEET_LOGS.md](./runtime/FLEET_LOGS.md)
  Fleet log ingestion, queueing, search, and current storage limits.
- [LOCAL_LOGS.md](./services/LOCAL_LOGS.md)
  Local-master runtime logging: shared store, bounded search/tail actions, SSE streaming, retention, and future fleet/syslog seams.
- [DEPLOY.md](./runtime/DEPLOY.md)
  Device-runtime deployment model for master and non-master machines.
- [DEPLOY_SERVICE.md](./runtime/DEPLOY_SERVICE.md)
  Deploy service action/API contract.
- [MONITORS.md](./services/MONITORS.md)
  Claude Code monitor definitions and `labby deploy monitor`.
- [UPSTREAM.md](./services/UPSTREAM.md)
  Upstream MCP proxy gateway: config, discovery, tool collision handling, circuit breaker, resource proxying.
- [TRANSPORT.md](./surfaces/TRANSPORT.md)
  Stdio and streamable HTTP transport: middleware stack, session management, DNS rebinding protection, CORS.
- `apps/gateway-admin/README.md`
  Labby admin UI: local frontend workflow, static export, and same-origin deployment model.
- [design/component-development.md](./design/component-development.md)
  Web UI component workflow: feature specs, `/dev/*` live read-only previews, render iteration, design-system review, and browser verification.
- [design/design-system-contract.md](./design/design-system-contract.md)
  Labby web UI design-system contract: Aurora tokens, typography, surfaces, components, page patterns, accessibility, and approval rules.
- [SERVICES.md](./dev/SERVICES.md)
  Service inventory, feature gates, plugin metadata, multi-instance support, coverage docs, and add-a-service workflow.
- [coverage/README.md](./coverage/README.md)
  Service coverage doc index.
- [upstream-api/README.md](./upstream-api/README.md)
  Upstream API/spec reference index.
- [generated/README.md](./generated/README.md)
  Generated CLI/MCP catalog docs and refresh notes.
- [features/README.md](./features/README.md)
  Focused feature docs and implementation artifacts.
- [design/README.md](./design/README.md)
  Design contract and artifact index.
- [SERVICE_ONBOARDING.md](./dev/SERVICE_ONBOARDING.md)
  End-to-end checklist for adding a new service, from upstream spec to verification.
- [SCAFFOLD_AND_AUDIT.md](./dev/SCAFFOLD_AND_AUDIT.md)
  Deferred scaffold/audit command contract.
- [CLI.md](./surfaces/CLI.md)
  Command structure, output rules, confirmation rules, setup/install surfaces, operator commands, and `labby oauth relay-local`.
- [design/CLI_DESIGN_SYSTEM.md](./design/CLI_DESIGN_SYSTEM.md)
  Human-readable CLI output language, semantic tokens, status hierarchy, and pipe-safe color policy.
- [design/CLI_OUTPUT_THEME_API.md](./design/CLI_OUTPUT_THEME_API.md)
  Proposed Rust API for CLI semantic styling, color policy resolution, and renderer integration.
- [TUI.md](./surfaces/TUI.md)
  Deferred TUI status.
- [CONFIG.md](./runtime/CONFIG.md)
  Env and TOML config ownership, load order, secrets handling, and instance naming.
- [HOST_GATEWAY.md](./runtime/HOST_GATEWAY.md)
  Primary amd64 Debian 13 Incus gateway runtime, in-box provisioning, hardened system service, Tailscale TUN passthrough, Docker smoke path, and rollback.
- [ENV.md](./runtime/ENV.md)
  Deployment-ready env examples and auth-mode variables.
- [OBSERVABILITY.md](./dev/OBSERVABILITY.md)
  Mandatory logging boundaries, required fields, correlation rules, redaction, and verification gates.
- [ERRORS.md](./dev/ERRORS.md)
  Shared error taxonomy, stable `kind` values, MCP and HTTP error envelopes, and status mapping.
- [design/SERIALIZATION.md](./design/SERIALIZATION.md)
  Serde ownership, stable envelope shapes, CLI output boundaries, and naming rules.
- [DISPATCH.md](./dev/DISPATCH.md)
  Surface-neutral dispatch ownership, dependency direction, operation metadata, and adapter responsibilities.
- [SERVICE_LAYER_MIGRATION.md](./dev/SERVICE_LAYER_MIGRATION.md)
  Phase-by-phase guide and checklist for moving existing services into the shared dispatch layer.
- [CONVENTIONS.md](./CONVENTIONS.md)
  Locked engineering rules around async, HTTP, testing, docs, API surface, and privacy.
- [OPERATIONS.md](./OPERATIONS.md)
  Repo helpers, doctor/health workflows, CI expectations, release behavior, and update rules.
- [CICD.md](./runtime/CICD.md)
  GitHub Actions check matrix and release behavior.
- [TESTING.md](./dev/TESTING.md)
  Test runner contract and verification expectations.
- [MARKETPLACE.md](./services/MARKETPLACE.md)
  Marketplace service, plugin workspace mirrors, save/deploy flows.

## Canonical Source Policy

These topic docs are the source of truth for the project.

When updating behavior or decisions:

- edit the topic doc that owns that concern
- do not recreate a monolithic “master design” file
- update multiple docs only when a decision genuinely crosses boundaries

## Edit Guide

Use the smallest correct doc:

- architecture or boundaries: [ARCH.md](./ARCH.md)
- implementation rules: [CONVENTIONS.md](./CONVENTIONS.md)
- service model or inventory: [SERVICES.md](./dev/SERVICES.md)
- CLI UX or command behavior: [CLI.md](./surfaces/CLI.md)
- CLI output language or color policy: [design/CLI_DESIGN_SYSTEM.md](./design/CLI_DESIGN_SYSTEM.md)
- MCP tool, discovery, or envelope behavior: [MCP.md](./surfaces/MCP.md)
- RMCP SDK integration, feature posture, and server-shape rules: [RMCP.md](./surfaces/RMCP.md)
- HTTP auth modes, JWKS, and JWT validation: [OAUTH.md](./runtime/OAUTH.md)
- gateway control plane, exposure policy, and protected MCP routes: [GATEWAY.md](./services/GATEWAY.md)
- ACP service architecture and chat/backend boundary: [acp/README.md](./acp/README.md)
- mirrored MCP Registry metadata contract: [MCPREGISTRY_METADATA.md](./services/MCPREGISTRY_METADATA.md)
- node runtime roles, fleet ingest, and master gating: [DEVICE_RUNTIME.md](./runtime/DEVICE_RUNTIME.md)
- controller/node runtime split and node artifact contract: [NODE_RUNTIME_CONTRACT.md](./runtime/NODE_RUNTIME_CONTRACT.md)
- fleet log ingestion and search: [FLEET_LOGS.md](./runtime/FLEET_LOGS.md)
- local-master runtime log store and SSE console: [LOCAL_LOGS.md](./services/LOCAL_LOGS.md)
- deployment topology and rollout guidance: [DEPLOY.md](./runtime/DEPLOY.md)
- upstream MCP proxy, circuit breaker, resource proxying: [UPSTREAM.md](./services/UPSTREAM.md)
- transport configuration, middleware, sessions: [TRANSPORT.md](./surfaces/TRANSPORT.md)
- deferred TUI status: [TUI.md](./surfaces/TUI.md)
- config, env, secrets, instance naming: [CONFIG.md](./runtime/CONFIG.md)
- observability, request tracing, redaction: [OBSERVABILITY.md](./dev/OBSERVABILITY.md)
- error taxonomy and envelope rules: [ERRORS.md](./dev/ERRORS.md)
- serialization and output-shape rules: [design/SERIALIZATION.md](./design/SERIALIZATION.md)
- dispatch-layer ownership and adapter rules: [DISPATCH.md](./dev/DISPATCH.md)
- service-layer migration execution plan: [SERVICE_LAYER_MIGRATION.md](./dev/SERVICE_LAYER_MIGRATION.md)
- stash versioning service and provider sync model: [STASH.md](./services/STASH.md)
- marketplace service and plugin workspace flows: [MARKETPLACE.md](./services/MARKETPLACE.md)
- deploy-service actions: [DEPLOY_SERVICE.md](./runtime/DEPLOY_SERVICE.md)
- node CLI/API behavior: [NODES.md](./runtime/NODES.md)
- fleet WebSocket methods: [FLEET_METHODS.md](./runtime/FLEET_METHODS.md)
- env examples: [ENV.md](./runtime/ENV.md)
- testing contract: [TESTING.md](./dev/TESTING.md)
- CI/CD behavior: [CICD.md](./runtime/CICD.md)
- operator workflows, CI, releases: [OPERATIONS.md](./OPERATIONS.md)
- stack and toolchain choices: [TECH.md](./TECH.md)

## Common Questions

- “Where does business logic belong?”
  See [ARCH.md](./ARCH.md).
- “What is the canonical MCP response/error shape?”
  See [MCP.md](./surfaces/MCP.md).
- “How should `lab` use the RMCP SDK itself?”
  See [RMCP.md](./surfaces/RMCP.md).
- “How do multi-instance services work?”
  See [CONFIG.md](./runtime/CONFIG.md) and [SERVICES.md](./dev/SERVICES.md).
- “How should a new service be added?”
  See [SERVICES.md](./dev/SERVICES.md).
- “What rules are locked and review-enforced?”
  See [CONVENTIONS.md](./CONVENTIONS.md).
- “What is the expected CI and release behavior?”
  See [OPERATIONS.md](./OPERATIONS.md) and [TECH.md](./TECH.md).
- “How do we extract Lab into reusable crates/packages?”
  See [crate-extract/README.md](./crate-extract/README.md).
