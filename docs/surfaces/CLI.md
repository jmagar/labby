---
title: "CLI Surface"
created: "2026-07-30"
updated: "2026-08-18"
---

# CLI Surface

The `labby` binary is the operator entrypoint for the supported gateway product.
Generated command help in [../generated/cli-help.md](../generated/cli-help.md) is
authoritative when this summary and the binary disagree.

## Top-Level Commands

- `labby serve` starts the hosted HTTP runtime, including the API, native MCP
  endpoint, auth routes, and exported web assets.
- `labby mcp` starts the local stdio MCP transport.
- `labby gateway` manages upstream MCP servers, protected routes, OAuth, reload,
  discovery, and Code Mode configuration.
- `labby setup` performs first-run setup, provisioning, plugin hooks, and host
  service operations.
- `labby doctor` audits supported configuration and runtime health.
- `labby snippets` manages executable Code Mode snippets.
- `labby skills` lists, searches, gets, and reads Agent Skills through the same canonical registry used by native Skills-over-MCP. With the gateway build it includes route-root proxied skills; a Skills-only build remains first-party-only.
- `labby docs` generates and verifies code-owned documentation artifacts.
- `labby health` performs a quick local health check.
- `labby oauth` runs local OAuth callback-relay helpers.
- `labby proxy` runs one explicitly selected stdio MCP child as a foreground
  Streamable HTTP endpoint. Labby flags precede the first child token; later
  tokens are child arguments and `--` is accepted but optional.
- `labby incus` manages the supported Incus gateway container.
- `labby update` installs a newer Labby release.
- `labby completions` emits shell completion scripts.

The runtime-conditional `lab_admin` service is exposed only when explicitly
enabled. The optional `fs` capability is an MCP/API/web service rather than a
standalone CLI command group.

`labby setup proxy` persists non-secret direct-proxy defaults and stores or
generates the separate bearer secret. `labby doctor proxy` with no route flags
runs the direct-proxy preflight; supplying the routed doctor URL/path flags
preserves the public Labby/protected-route diagnostic. See the
[stdio MCP proxy guide](../guides/STDIO_MCP_PROXY.md) for launcher inference,
auth/exposure modes, output, precedence, and cleanup.

## Shared Action Shape

Service-oriented commands ultimately dispatch the same action contract used by
MCP and HTTP:

```json
{ "action": "gateway.list", "params": {} }
```

Use `--json` for machine-readable output. Human output must remain a rendering
adapter over the same typed result, not a separate execution path.

## Destructive Operations

Actions marked destructive in the service catalog require explicit operator
confirmation. Non-interactive CLI use passes `-y` or `--yes`. Do not add
surface-local destructive classifications; `ActionSpec.destructive` is the
shared source of truth.

## Supported Product Boundary

The current CLI does not include ACP sessions, Registry browsing/installing,
Marketplace product commands, Fleet/node enrollment, Deploy-product commands, or
old Agent Artifact Manager workspaces. Historical command contracts are archived
under [../archive/retired-labby](../archive/retired-labby/).

File Stash has no CLI surface in v1. Its generic action dispatch is exposed by
authenticated API and MCP adapters, while file bytes use the HTTP
upload/download routes. No bespoke component/revision/provider/deploy command
tree is permitted.
