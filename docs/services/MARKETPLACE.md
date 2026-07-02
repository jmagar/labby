# Marketplace

`marketplace` is the unified Lab surface for installable agent tooling:

- Claude Code and Codex plugin marketplaces (`sources.*`, `plugins.*`, `plugin.*`, `artifact.*`)
- the official MCP Registry (`mcp.*`)
- the ACP Agent Registry (`agent.*`)

It is compiled by the `marketplace` feature and exposed through the normal Lab
dispatch paths: CLI, MCP, HTTP API, and the web UI. The generated service
catalog must show `marketplace` as feature-gated/available when compiled, while
`mcpregistry` and `acp_registry` remain SDK-only entries.

## Ownership

Marketplace owns orchestration, local state, install planning, and safety gates.
The SDK-only registry modules own protocol clients and metadata:

| Surface | Runtime owner | SDK/source owner | Notes |
| --- | --- | --- | --- |
| Claude/Codex plugins | `crates/lab/src/dispatch/marketplace/` | Claude/Codex marketplace files under `~/.claude/plugins/` | Reads installed/source state and shells out to `claude plugin ...` for plugin install/uninstall. |
| MCP Registry | `marketplace` `mcp.*` actions | `lab-apis::mcpregistry` plus `[mcpregistry].url` | `mcpregistry` is not a first-class CLI/MCP/API service. |
| ACP Agent Registry | `marketplace` `agent.*` actions | `lab-apis::acp_registry` plus `LAB_ACP_REGISTRY_URL` | `acp_registry` is not a first-class CLI/MCP/API service. |

Marketplace does not re-implement upstream registry semantics. Registry URL
validation, schema validation, SDK decode errors, and upstream request failures
must flow through the shared dispatch error envelope.

## Data Sources

| Path or source | Purpose |
| --- | --- |
| `~/.claude/plugins/known_marketplaces.json` | Claude/Codex marketplace registry. |
| `<installLocation>/.claude-plugin/marketplace.json` | Marketplace manifest. |
| `<installLocation>/marketplace.json` | Fallback manifest location. |
| `~/.claude/plugins/installed_plugins.json` | Installed Claude/Codex plugin state. |
| `<installPath>/**` | Installed plugin artifacts returned by `plugin.artifacts`. |
| `[mcpregistry].url` | Optional MCP Registry base URL, defaulting to `https://registry.modelcontextprotocol.io`. |
| `LAB_ACP_REGISTRY_URL` | Optional ACP Agent Registry CDN base URL, defaulting to `https://cdn.agentclientprotocol.com`. |
| `~/.labby/acp-providers.json` | Local ACP provider entries written by `agent.install` and removed by `agent.uninstall`. |

Missing Claude/Codex marketplace files are treated as empty so a fresh machine
returns zero plugin sources without error.

## Actions

The full action inventory is generated from `ActionSpec`:

- [generated action catalog](../generated/action-catalog.md)
- [generated action catalog JSON](../generated/action-catalog.json)

The handwritten contract is organized by action family:

| Family | Examples | Role |
| --- | --- | --- |
| `sources.*` | `sources.list`, `sources.add` | List or add Claude/Codex plugin marketplaces. |
| `plugins.*` | `plugins.list` | Search plugin manifests across configured plugin marketplaces. |
| `plugin.*` | `plugin.get`, `plugin.install`, `plugin.workspace`, `plugin.deploy` | Read, install, edit, and deploy whole plugins. |
| `artifact.*` | `artifact.fork`, `artifact.list`, `artifact.update.apply` | Fork, reset, and update individual plugin artifacts. `artifact.diff` and `artifact.patch` are reserved but **not yet implemented** (see below). |
| `mcp.*` | `mcp.config`, `mcp.list`, `mcp.get`, `mcp.versions`, `mcp.validate`, `mcp.meta.get`, `mcp.meta.set`, `mcp.meta.delete`, `mcp.sync`, `mcp.install`, `mcp.uninstall` | Discover, validate, mirror, annotate, install, and remove MCP Registry servers. |
| `agent.*` | `agent.list`, `agent.get`, `agent.install`, `agent.uninstall` | Discover and install ACP-compatible agents. |

`help` and `schema` are also available through the shared service dispatch
model.

### Artifact action status

Not every `artifact.*` action is fully implemented yet:

- `artifact.diff` and `artifact.patch` are **reserved but not yet implemented**.
  Calling either returns the `not_implemented` error kind. The action signatures
  and the shared git shell-out boundary exist so the implementation can land
  without changing the wire shape, but no diff/patch is produced today.
- `artifact.list` returns a per-fork drift `status` field that is currently a
  placeholder. Per-artifact drift detection is not yet wired, so `status` is
  reported as `"unknown"` for every fork unless drift detection is enabled.
  Treat `"unknown"` as "drift not computed", not as "no drift".

The full marketplace ↔ stash boundary, including these caveats, is specified in
[marketplace-stash-integration.md](../contracts/marketplace-stash-integration.md).

### Fork persistence

Forked artifacts are stored as first-class Stash components, not in a
marketplace-private store. Two representations exist:

- **Modern (canonical):** each fork is a Stash component record whose
  `origin_meta` is `StashOrigin::Marketplace`. Tracked content lives in the
  normal Stash workspace, and marketplace helper metadata (base snapshot,
  pending update, drift cache) lives in a Stash-owned sidecar at
  `<stash_root>/marketplace/<component_id>/`. This component-record + sidecar
  pair is the authoritative storage for all new forks.
- **Legacy (retired, read-only):** earlier forks wrote a `.stash.json` file
  under `<workspace.root>/plugins/<plugin_id>/`. This path is no longer
  authoritative — it is recognized only as a legacy discovery branch and is
  migrated on read into the modern component-record model. New forks are never
  written there, and `.stash.json`/`stash_meta` is not the canonical fork store.

See [STASH.md](./STASH.md#marketplace-origin-components) for the stash-side view.

### Git runtime requirement

`artifact.fork` and the `artifact.update.*` actions shell out to a `git` binary
on `PATH` (for example, `artifact.update.check` fetches upstream refs). If `git`
is absent, these actions fail closed with the `git_not_available` error kind
rather than degrading silently. Install `git` on the controller host to use the
fork/update workflows.

## Install Targets

Marketplace supports multiple installation targets. Callers must choose an
explicit target instead of relying on hidden global defaults:

| Action | Target selector | Effect |
| --- | --- | --- |
| `plugin.install` / `plugin.uninstall` | `id` in `name@marketplace` form | Delegates to `claude plugin install/uninstall` for the controller host. |
| `plugin.deploy` | `id` | Copies the editable workspace mirror into the configured local plugin target. |
| `plugin.cherry_pick` | `node_ids`, `scope`, `components`, optional `project_path` | Installs selected plugin components on fleet nodes. |
| `mcp.install` | `gateway_ids` and/or `client_targets` | Adds remote HTTP servers to Lab gateway upstreams, or writes stdio command configs to Claude/Codex clients on fleet devices. At least one target set is required. |
| `mcp.uninstall` | `gateway_name` | Removes a previously installed gateway upstream. |
| `agent.install` | `node_ids` plus optional `platform` | Installs an ACP provider entry locally (`local` or host name) or asks a remote node to install a supported package distribution. |
| `agent.uninstall` | `id` | Removes the local ACP provider entry. |

For MCP installs, HTTP transports are added as gateway remote URLs after SSRF
validation. Stdio packages become command configs. Required environment values
must be supplied through `env_values` or an explicit env-var reference such as
`bearer_token_env`; raw bearer token values must not be embedded in logs or docs.

## Bounded Discovery

`mcp.list` is intentionally bounded:

- default `limit` is 10
- maximum `limit` is 100
- pagination uses the returned `metadata.nextCursor`
- `search` wins over `owner` when both are supplied
- `owner` is a GitHub convenience filter that maps to `search=io.github.{owner}/`
- local Lab metadata filters include `featured`, `reviewed`, `recommended`,
  `hidden`, and `tag`

`mcp.list` on `/v1/marketplace` reads the local SQLite mirror populated by
`mcp.sync`. The wire-compatible `GET /v0.1/servers` surface reads the same
store and defaults to 20 rows per page for REST clients.

`gateway.mcp.list` is a separate gateway runtime inventory action. It lists
configured upstream MCP runtime state, discovery counts, and likely stale
process counts; it is not a Marketplace registry search API and should remain
read-only and non-destructive in the generated catalog.

## Confirmation

`ActionSpec.destructive` is the single source of truth for Marketplace
confirmation behavior.

| Surface | Required confirmation |
| --- | --- |
| CLI | `-y` / `--yes` for destructive actions. |
| MCP | Client elicitation accept, or `params.confirm: true` for clients without elicitation. |
| HTTP | `params.confirm: true`; missing or false confirmation returns `kind: "confirmation_required"` with HTTP `422`. |

The confirmation flag is handled by the shared dispatcher before Marketplace
domain parsers run. Do not duplicate confirmation checks inside action-specific
parsers unless a protocol-specific exception is documented.

## Integrity And Safety

Marketplace install paths are intentionally narrow:

- observational plugin actions read from `~/.claude/plugins/` and installed
  plugin paths recorded by Claude/Codex
- workspace mirrors live under the configured Lab stash root, defaulting to
  `~/.labby/stash/plugins/`
- ACP binaries install under `~/.labby/bin/<agent_id>/`
- ACP provider config writes go to `~/.labby/acp-providers.json`

Binary and package integrity policy:

- ACP binary archive URLs must be HTTPS and must not point at loopback, private,
  link-local, unspecified, or common local-only hostnames.
- Archive downloads are streamed to a temp file, hashed with SHA-256, fsynced,
  extracted into a temp dir, then copied into the final install directory.
- Extraction rejects symlinks and entries that escape the extraction root, and
  treats partial extraction as a hard failure.
- Installed ACP binary entries record the computed SHA-256 in the provider
  entry.
- Stdio package distributions (`npx`, `uvx`, MCP Registry package configs) are
  config-only installs. Marketplace records command/package metadata; package
  manager resolution happens when the client runs the command.
- Claude/Codex plugin install/uninstall still delegates to the `claude` binary
  with explicit argv and no shell interpolation.

`plugin.artifacts` is bounded to 256 KiB per file and 200 files per plugin.
Large plugins must return truncated artifact output rather than multi-MB MCP or
HTTP responses.

## Surfaces

CLI:

```bash
labby marketplace sources.list --json
labby marketplace plugins.list --params '{"marketplace":"jmagar-lab"}'
labby marketplace mcp.list --params '{"search":"postgres","limit":10}'
labby marketplace agent.list
labby marketplace mcp.install --params '{"name":"io.github.user/server","gateway_ids":["default"],"confirm":true}' -y
```

MCP:

```jsonc
marketplace({ "action": "mcp.list", "params": { "owner": "modelcontextprotocol", "limit": 10 } })
marketplace({ "action": "agent.get", "params": { "id": "openai/codex-cli" } })
marketplace({ "action": "plugin.install", "params": { "id": "aurora-design@jmagar-lab", "confirm": true } })
```

HTTP:

```bash
curl -s -X POST http://127.0.0.1:8765/v1/marketplace \
  -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"mcp.list","params":{"search":"postgres","limit":10}}'
```

The web UI consumes `/v1/marketplace`; it must not read or write `~/.claude/`,
`~/.labby/bin/`, or `~/.labby/acp-providers.json` directly.
