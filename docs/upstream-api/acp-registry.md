# ACP Registry Upstream Contract

**Last updated:** 2026-05-07
**Status:** SDK client exists; runtime exposure is through Marketplace `agent.*`

The ACP Agent Registry is a read-only CDN manifest used to discover
ACP-compatible coding agents. Lab keeps the upstream SDK in
`lab-apis::acp_registry`, but the public CLI/MCP/API surface is the always-on
`marketplace` service.

## Source

Marketplace resolves the runtime base URL from:

```env
LAB_ACP_REGISTRY_URL=https://cdn.agentclientprotocol.com
```

If unset, Marketplace falls back to the SDK default:

```text
https://cdn.agentclientprotocol.com
```

The SDK metadata still lists `ACP_REGISTRY_URL` for the SDK-only
`acp_registry` service entry. Do not document `ACP_REGISTRY_URL` as the
Marketplace runtime override; use `LAB_ACP_REGISTRY_URL` for `agent.*`.

## Upstream Shape

The SDK fetches:

```text
GET /registry/v1/latest/registry.json
```

The response is a registry wrapper containing `agents[]`. Marketplace currently
filters that full manifest client-side for `agent.get`.

All reads are unauthenticated. The HTTP client uses bounded timeouts and does
not follow redirects.

## Lab Runtime Surface

Runtime actions live under the single Marketplace tool/service:

| Action | Destructive | Return | Notes |
| --- | --- | --- | --- |
| `agent.list` | No | `Agent[]` | Lists all registry agents. |
| `agent.get` | No | `Agent` | Requires `id`, for example `openai/codex-cli`. |
| `agent.install` | Yes | `InstallResults` | Requires `id`, `node_ids`, and confirmation. |
| `agent.uninstall` | Yes | `UninstallResult` | Requires `id` and confirmation. |

The generated service catalog must continue to show:

- `marketplace`: `available`, exposed on CLI/MCP/API/web
- `acp_registry`: `sdk_only`, no CLI/MCP/API/web exposure

## Install Contract

`agent.install` writes provider configuration, not arbitrary agent state:

- local target: `node_ids` may contain `local` or the controller hostname
- local binary target: install under `~/.lab/bin/<agent_id>/`, write an entry to
  `~/.lab/acp-providers.json`, and record the computed SHA-256
- local `npx` / `uvx` target: write command, args, distribution, and version
  metadata to `~/.lab/acp-providers.json`
- remote target: send a fleet RPC `agent.install`; remote binary and `uvx`
  installs are not supported yet and return structured per-node errors

`agent.uninstall` removes the local provider entry from
`~/.lab/acp-providers.json`.

## Confirmation And Errors

Destructive `agent.*` actions use the shared Lab confirmation gate:

- CLI requires `-y` / `--yes`
- MCP uses elicitation or `params.confirm: true`
- HTTP requires `params.confirm: true`; missing or false confirmation returns
  `kind: "confirmation_required"` with HTTP `422`

SDK and dispatch errors use the canonical envelope from `docs/dev/ERRORS.md`.
Registry misses return `not_found`; malformed input returns `invalid_param` or
`missing_param`; unsupported distribution/target combinations return a
structured SDK-style error kind.

## Binary And Package Integrity

Binary distribution installs are allowed only after hardening checks:

- archive URL must use HTTPS
- loopback, private, link-local, unspecified, and common local-only hostnames
  are rejected
- redirects are not followed
- downloads stream to a temp archive, are hashed with SHA-256, flushed and
  fsynced, then extracted into a temp directory
- extraction rejects symlinks, path escapes, and partial extraction
- installed binary permissions are set explicitly to `0755` on Unix

Package distributions (`npx`, `uvx`) are config-only provider entries. Lab
records the package command and argv; the package manager resolves and verifies
the package according to its own rules when the agent command runs.

## Documentation Ownership

This file documents the upstream ACP registry and its Marketplace projection.
The broader unified Marketplace contract lives in
[../services/MARKETPLACE.md](../services/MARKETPLACE.md). Coverage status lives
in [../coverage/acp_registry.md](../coverage/acp_registry.md).
