# Labby Product Documentation

This directory is the canonical documentation entrypoint for the current Labby product.

The live Rust/TypeScript implementation and the generated catalogs under [generated/](./generated/README.md) are the ground truth for what is compiled, registered, and exposed. Product prose should explain that implementation rather than preserve old product shapes.

Historical material that still has durable value lives under `docs/archive/` and is explicitly non-canonical. Transient research caches, session logs, generated smoke output, and completed implementation plans are not committed as product documentation; Git history retains them when historical archaeology is needed.

## Start Here

- [Architecture](./ARCH.md) — workspace boundaries, runtime flow, and product surfaces.
- [Technology](./TECH.md) — toolchain, dependencies, build posture, Rustdoc, and release model.
- [Conventions](./CONVENTIONS.md) — engineering rules that current code is expected to follow.
- [Service model](./dev/SERVICES.md) — the current registered service inventory and onboarding rules.
- [CLI](./surfaces/CLI.md), [MCP](./surfaces/MCP.md), [MCP conformance](./surfaces/MCP_CONFORMANCE.md), and [Transport](./surfaces/TRANSPORT.md) — public surface behavior and protocol contracts.
- [Skills and Loadouts](./guides/SKILLS_AND_LOADOUTS.md) — Agent Skills trust/exposure and route Loadout projections.
- [Local access bootstrap](./guides/LOCAL_ACCESS_BOOTSTRAP.md) — offline proof preparation, direct-local consume, recovery, revocation, and cleanup.
- [Access Control, Workspaces, and Artifact Distribution](./access-control/README.md) — active specification/contract for organizations, groups, projects, effective workspaces, scoped assets/capabilities, and Personal Labby Artifact sync/fork flows.
- [Skills-over-MCP compatibility](./plans/skills-over-mcp-compat/README.md) — active specification, contract, implementation plan, and progress tracker for universal Skills access.
- [Configuration](./runtime/CONFIG.md) and [Environment](./runtime/ENV.md) — runtime configuration and environment variables.
- [Operations](./OPERATIONS.md) — build, doctor, deployment, CI, release, and operator workflows.

## Current Product Services

The generated [service catalog](./generated/service-catalog.md) is authoritative. The current product documentation is split by service:

| Service | Product doc | Notes |
| --- | --- | --- |
| `browser` | [services/BROWSER.md](./services/BROWSER.md) | Rust-native WebMCP browser bridge, pairing, discovery, consent, and bounded invocation |
| `doctor` | [services/DOCTOR.md](./services/DOCTOR.md) | Always-on system, auth, OAuth relay, and proxy diagnostics |
| `gateway` | [services/GATEWAY.md](./services/GATEWAY.md) | Upstream catalog, protected routes, virtual servers, OAuth, Code Mode host |
| upstream proxy runtime | [services/UPSTREAM.md](./services/UPSTREAM.md) | HTTP/Unix/stdio upstream MCP connections, discovery, filtering, health, OAuth, skills |
| `setup` | [services/SETUP.md](./services/SETUP.md) | Bootstrap, settings, repair, plugin lifecycle, proxy setup, host provisioning |
| `server_logs` | [services/SERVER_LOGS.md](./services/SERVER_LOGS.md) | Labby's own server-process log query and journal tail |
| `fs` | [services/FILESYSTEM.md](./services/FILESYSTEM.md) | Optional jailed read-only workspace browsing and preview |
| `stash` | [services/STASH.md](./services/STASH.md) | Linux principal-scoped file upload, download, sharing, and bounded MCP reads |
| `snippets` | [services/SNIPPETS.md](./services/SNIPPETS.md) | Reusable Code Mode workflow storage, validation, execution, testing, promotion |
| `artifacts`, `bundles`, `jobs`, `sources`, `uploads` | [services/SKILLS.md](./services/SKILLS.md) and [artifacts/](./artifacts/) | Durable Artifact library, provider-backed control-plane projections, and native Agent Skills projection |
| `lab_admin` | [services/LAB_ADMIN.md](./services/LAB_ADMIN.md) | Runtime-conditional onboarding audit surface |
| access owner bootstrap | [services/ACCESS.md](./services/ACCESS.md) | Browser-only explicit creation of the first access-control owner |
| direct stdio proxy | [guides/STDIO_MCP_PROXY.md](./guides/STDIO_MCP_PROXY.md) | One selected stdio MCP server exposed over Streamable HTTP |

Do not hand-maintain a duplicate action inventory in prose. Use the generated [action catalog](./generated/action-catalog.md) for exact action names, parameters, scopes, destructive classification, and surfaces.

The browser-only [access owner bootstrap workflow](./services/ACCESS.md) is an
HTTP route, not a registered multi-surface service.

## Public Surfaces

- [CLI](./surfaces/CLI.md) — command grammar, output modes, confirmation behavior, and operator commands.
- [MCP](./surfaces/MCP.md) — tool/resource/prompt behavior, Code Mode, MCP Apps, and capability exposure.
- [MCP conformance](./surfaces/MCP_CONFORMANCE.md) — current protocol-version and conformance contract.
- [RMCP](./surfaces/RMCP.md) — how Labby integrates the Rust MCP SDK.
- [Transport](./surfaces/TRANSPORT.md) — stdio, Streamable HTTP, Unix socket, middleware, CORS, DNS-rebinding protection, and subscriptions.

## Runtime And Operations

- [Configuration](./runtime/CONFIG.md)
- [Environment](./runtime/ENV.md)
- [OAuth](./runtime/OAUTH.md)
- [OAuth callback relay](./runtime/CALLBACK_RELAY.md)
- [Reverse proxy](./runtime/REVERSE_PROXY.md)
- [Host gateway runtime](./runtime/HOST_GATEWAY.md)
- [Incus](./runtime/INCUS.md)
- [Unraid plugin](./runtime/UNRAID.md)
- [GitHub Actions runner](./runtime/ACTIONS_RUNNER.md)
- [CI/CD](./runtime/CICD.md)
- [Container runtime](./runtime/CONTAINERS.md)
- [Durable-state disaster recovery](./runtime/DISASTER_RECOVERY.md)
- [Operations](./OPERATIONS.md)
- [Technology and Rust build](./TECH.md)

## Developer Contracts

- [Dispatch](./dev/DISPATCH.md) — surface-neutral operation ownership and dependency direction.
- [Service model](./dev/SERVICES.md) — service inventory and registration rules.
- [Service onboarding](./dev/SERVICE_ONBOARDING.md) — end-to-end checklist for a new first-class capability.
- [Code Mode](./dev/CODE_MODE.md) — Code Mode runtime and host integration.
- [Errors](./dev/ERRORS.md) — stable error taxonomy and surface mapping.
- [Observability](./dev/OBSERVABILITY.md) — required fields, correlation, redaction, and verification.
- [Testing](./dev/TESTING.md) — local and CI verification expectations.
- [Rustdoc](./dev/RUSTDOC.md) — comprehensive Rust API documentation, doctest, and CI artifact contract.
- [Serialization](./design/SERIALIZATION.md) — output and wire-shape ownership.

Normative cross-surface contracts live under [contracts/](./contracts/):

- [Agent error contract](./contracts/agent-error-contract.md)
- [Integration identity](./contracts/integration-identity-v1.md) — authenticated installation and mounted-service discovery without credential-cache authority.
- [Code Mode tool errors](./contracts/code-mode-tool-errors.md)
- [MCP tool output](./contracts/mcp-tool-output.md)
- [Gateway schema resources](./contracts/gateway-schema-resources.md)
- [Skills extension](./contracts/skills-extension.md)
- [Stdio MCP proxy](./contracts/stdio-mcp-proxy.md)
- [Unraid Core integration](./contracts/unraid-core-integration-v1.md) —
  implemented unbundled appliance boundary; not an authorization to package
  Labby.
- [Core provider protocol](./contracts/core-provider-protocol-v1.md) —
  implemented private Core capability boundary for Labby Code Mode.

## Product Design

- [Design index](./design/README.md)
- [Phabby shared control plane](./design/phabby-control-plane.md) — accepted Phoenix/OTP target and Rust/BEAM ownership boundary.
- [Phabby migration ledger](./design/phabby-migration-ledger.md) — staged route, packaging, and ownership migration gates.
- [Web design-system contract](./design/design-system-contract.md)
- [Component development](./design/component-development.md)
- [CLI design system](./design/CLI_DESIGN_SYSTEM.md)
- [Claude Code Aurora theme](./design/CLAUDE_CODE_AURORA_THEME.md)
- [Google credential broker](./design/GOOGLE_CREDENTIAL_BROKER.md)
- [Remote gateway target](./design/REMOTE_GATEWAY_TARGET.md)
- [Brand assets](./assets/brand/README.md)

## Plugins And Snippets

- [Plugins](./PLUGINS.md) — checked-in Labby plugin, distribution boundary, and setup lifecycle.
- [Snippet authoring](./snippets/README.md) — executable Code Mode snippet format and workflow.

## Generated Product References

Run:

```bash
just docs-generate
just docs-check
```

Generated artifacts include:

- [service catalog](./generated/service-catalog.md)
- [action catalog](./generated/action-catalog.md)
- [environment reference](./generated/env-reference.md)
- [proxy configuration reference](./generated/proxy-config-reference.md)
- [API routes](./generated/api-routes.md)
- [MCP help](./generated/mcp-help.md)
- [CLI help](./generated/cli-help.md)
- [feature matrix](./generated/feature-matrix.md)
- `openapi.json`

Never edit generated artifacts by hand.

## Source-Of-Truth Rules

When documentation and implementation disagree:

1. verify the current implementation and generated catalogs;
2. fix the canonical product doc that owns the concern;
3. regenerate code-owned docs when code metadata changed;
4. update cross-links only where the behavior crosses product boundaries.

Avoid creating duplicate top-level summaries for a topic that already has a canonical service, runtime, surface, design, or developer doc.
