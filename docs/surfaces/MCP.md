# MCP

`lab` exposes homelab operations through a compact MCP surface designed for agents, not a giant tool registry.

The RMCP SDK integration contract that underpins this surface lives in [RMCP.md](./RMCP.md). This document owns product-facing MCP behavior; `RMCP.md` owns how `lab` uses the RMCP library to implement it.

## Transport Modes

`lab` exposes two MCP entrypoints:

- `labby mcp`: local stdio child-process MCP clients such as Claude Desktop and `.mcp.json`
- `labby serve`: hosted HTTP runtime, including streamable HTTP MCP at `/mcp`

Rules:

- `labby serve` starts the hosted HTTP runtime by default
- `labby mcp` is the explicit child-process stdio entrypoint
- HTTP supports `LAB_AUTH_MODE=bearer|oauth`
- bearer mode preserves `LAB_MCP_HTTP_TOKEN`
- oauth mode requires `LAB_PUBLIC_URL` and Google client credentials
- transport changes must not change dispatch or catalog behavior
- HTTP transport may expose opt-in CORS origins

When the process resolves as a non-controller node, MCP is not exposed at all. Non-controller nodes keep only the local node runtime and the `/v1/nodes/*` HTTP namespace.

## Server Capabilities

`labby serve` advertises these MCP capabilities:

- tools
- resources
- prompts
- completions
- logging

Those capabilities are enabled together on the same server surface. Capability
support must reflect the running server, not a partial or hypothetical build.

## HTTP Auth Surface

When `labby serve` is active, `lab` exposes two auth modes:

- `LAB_AUTH_MODE=bearer`
  `LAB_MCP_HTTP_TOKEN` remains the only credential. This preserves existing HTTP deployments.
- `LAB_AUTH_MODE=oauth`
  `lab` runs its own authorization server, brokers Google sign-in server-side, and issues `lab` access tokens plus refresh tokens only when upstream Google auth granted offline refresh capability.

OAuth mode keeps Google access and refresh tokens inside the server. MCP clients only receive `lab` tokens.

OAuth mode adds these unauthenticated discovery and auth endpoints alongside `/mcp`:

- `/.well-known/oauth-authorization-server`
- `/.well-known/oauth-protected-resource`
- `/jwks`
- `/register`
- `/authorize`
- `/auth/google/callback`
- `/token`

Dynamic client registration is intentionally restricted in this first launch:

- redirect URIs must use loopback hosts only (`127.0.0.1`, `localhost`, `::1`)
- `/revoke` is not implemented in this batch
- refresh-token rotation is not implemented in this batch

## HTTP Route Posture

When HTTP serving is enabled, the route classes have separate auth contracts:

- `/mcp` is the MCP streamable HTTP endpoint. If bearer or OAuth auth is configured, it requires token auth and does not accept browser sessions.
- `/v1/*` is the product API. If bearer or OAuth auth is configured, it is protected even when browser UI auth is disabled for static assets.
- Static web assets serve the Labby browser app shell. Disabling browser UI auth only changes browser-session behavior for the web UI; it is not a switch that disables `/v1` or `/mcp` auth.
- `/dev/*` routes are development preview routes. They are authenticated whenever bearer or OAuth auth is configured. They are only open when the server is intentionally running with no auth configured, and that posture is local/dev only, not production.

## One Tool Per Service

Each service exposes exactly one MCP tool named after the service.

Examples:

```json
{ "tool": "radarr", "input": { "action": "movie.search", "params": { "query": "The Matrix" } } }
{ "tool": "plex", "input": { "action": "library.list" } }
```

This avoids exploding the tool list into hundreds of tiny tools.

Canonical service tool schema:

- `name`: service name
- `input.action`: required string
- `input.params`: optional object

## Action Model

All service tools use the same input shape:

- `action`: dotted action name such as `movie.search`
- `params`: action-specific object

Naming rule:

- lowercase
- dot-separated
- `<resource>.<verb>`

Examples:

- `movie.search`
- `queue.list`
- `system.status`

## Action Catalog

Every service declares its action catalog via `ActionSpec` and `ParamSpec`.

That catalog is the source of truth for:

- dispatch validation
- `help` action output
- MCP resources
- top-level aggregated discovery
- destructive-op policy

The complete generated action inventory is
[generated/action-catalog.md](./generated/action-catalog.md). Its JSON contract
is [generated/action-catalog.json](./generated/action-catalog.json) and is a
global inventory, not the active runtime exposure policy for gateway-filtered
MCP sessions.

## Discovery

There are three discovery surfaces:

- per-service `help` action
- per-service resources such as `lab://radarr/actions`
- top-level `lab://catalog`

This means agents can discover the available tool shape without guessing.

Per-service resource forms:

- `lab://<service>/actions`

The top-level discovery resource is:

- `lab://catalog`

## Top-Level Catalog

`lab://catalog` is generated from the same action metadata that powers
per-service help. It must never be maintained as a second hand-written
registry.

Operator-facing surfaces such as `device` and `logs` are registered MCP
services when they are present in the runtime registry.

## Result Envelope

All MCP tool responses follow a consistent envelope so callers do not need to parse arbitrary strings.

The canonical envelope and error contract lives in [design/SERIALIZATION.md](./design/SERIALIZATION.md) and [ERRORS.md](./ERRORS.md).

Success shape:

- `ok: true`
- `service`
- `action`
- `data`
- optional `meta`

Error shape:

- `ok: false`
- `service`
- `action`
- structured `error`

The envelope is intended to be the only thing an MCP client needs to parse. Multi-block or prose-heavy responses are explicitly not the default contract.

## Prompt Templates

`lab` currently exposes two prompt templates:

- `run-action`
- `service-discover`

`run-action` is the structured execution prompt. It includes:

- the selected service description when the service exists in the live registry
- the selected action description, destructive flag, return hint, and declared parameter list when the action exists
- explicit mention of the built-in `help` and `schema` actions for follow-up discovery

`service-discover` is the discovery prompt. It includes:

- the selected service description, category, and status from the live registry
- an inline action list with destructive/read-only labeling
- explicit guidance on when to call `help` or `schema`

Prompt text must be derived from the runtime registry and action metadata, not hand-maintained parallel docs.

## Completions

MCP completions are exposed for prompt and resource references, not arbitrary tool arguments.

Current completion behavior:

- prompt `service` arguments complete from the live service registry
- `run-action.action` completes from the registry-wide sorted, deduplicated action-name cache
- completion matching is simple prefix matching
- unknown prompt or resource references return empty completion sets, not errors

The cached global action-name list exists to avoid re-sorting the full action set on every completion request.

## Logging Notifications

`lab` supports MCP logging via `logging/setLevel` and server-to-client logging notifications.

Rules:

- notifications are dispatch-boundary only
- the client must opt in by calling `set_level`
- the disabled sentinel is internal-only implementation detail; clients observe standard RFC 5424 severity semantics
- success notifications emit `service`, `action`, and `elapsed_ms`
- error notifications also include a sanitized `error` string

Sanitization requirements:

- `internal_error` and `server_error` notifications must not expose raw backend details
- error strings are reduced to a single line
- home-directory paths are redacted before the notification is sent

MCP logging notifications are supplemental observability for the client session. They must never change dispatch results or leak secrets.

## Structured Error Kinds

Cross-service error vocabulary includes:

- `unknown_action`
- `unknown_subaction`
- `missing_param`
- `invalid_param`
- `unknown_instance`
- `auth_failed`
- `not_found`
- `rate_limited`
- `validation_failed`
- `network_error`
- `server_error`
- `decode_error`
- `internal_error`

Additional dispatch-level cases include:

- `confirmation_required`

The goal is self-correcting clients, not human-only diagnostics.

The stable semantics for these kinds are defined in [ERRORS.md](./ERRORS.md). Do not invent transport-local variants.

## Multi-Instance Services

When a service has multiple configured instances, MCP actions accept `params.instance`.

Rules:

- the dispatcher handles instance lookup
- service clients remain instance-agnostic
- unknown labels return `unknown_instance` with valid labels

## Destructive Operations

Destructive operations are marked in `ActionSpec.destructive`.

That one flag drives:

- MCP elicitation prompts
- CLI confirmation behavior

The same action metadata is used for both surfaces so the risk policy cannot drift.

Representative destructive actions include:

- container removal or stack teardown
- media deletion with file removal
- queue purge and history deletion
- network device restart or forget flows

## Elicitation Policy

MCP destructive calls require explicit confirmation. The server first uses MCP
elicitation when the client supports it. If elicitation is unavailable, callers
must pass `params.confirm: true`; otherwise the dispatcher returns
`confirmation_required`.

Prompts must include:

- service
- action
- key params
- plain-language risk description

## Registry

The runtime registry only exposes enabled services. Discovery reflects the running server, not the theoretical max build.

That means:

- compiled features matter
- `[services].built_in_upstream_apis_enabled = false` removes built-in upstream API integrations on the next server start while preserving bootstrap/operator tools
- `--services` filtering matters
- `lab://catalog` only shows what is actually available

The same catalog builder must feed:

- `lab://catalog`
- `lab://catalog`
- CLI help/catalog rendering

## Upstream Tool Merging

When upstream MCP servers are configured (see [UPSTREAM.md](./UPSTREAM.md)), their tools are merged into the `list_tools` response alongside built-in service tools.

## Marketplace Artifact Actions

The `marketplace` service exposes artifact fork, diff, patch, and update workflows via its normal single-tool `action` + `params` shape.

Artifact actions:

- `artifact.fork` returns `ForkResult`
- `artifact.list` returns `ForkedPluginStatus[]`
- `artifact.unfork` returns `UnforkResult` and is destructive
- `artifact.reset` returns `ResetResult` and is destructive
- `artifact.diff` returns `ArtifactDiffResult`
- `artifact.patch` returns `PatchResult`
- `artifact.update.check` returns `UpdateCheckResult[]`
- `artifact.update.preview` returns `UpdatePreviewResult`
- `artifact.update.apply` returns `ApplyResult` and is destructive
- `artifact.merge.suggest` returns `MergeSuggestResult`
- `artifact.config.set` returns `ConfigSetResult`

Destructive artifact actions use the shared `ActionSpec.destructive` gate. MCP clients confirm through elicitation, CLI callers use `-y` / `--yes`, and HTTP callers include `params.confirm: true`; the confirmation key is stripped before marketplace parsers and domain handlers run.

`artifact.fork` accepts required `params.plugin_id` and optional `params.artifacts`. When `artifacts` is omitted, the action targets a plugin-level fork; otherwise each value must be a relative artifact path.

```json
{
  "action": "artifact.fork",
  "params": { "plugin_id": "demo-plugin@demo-market", "artifacts": ["agents/demo.md"] }
}
```

`artifact.list` lists forked plugin artifact stashes. It accepts optional `params.plugin_id` to scope the result to a single plugin.

`artifact.unfork` accepts required `params.plugin_id` and optional `params.artifacts`, and removes fork tracking metadata after confirmation.

`artifact.reset` accepts required `params.plugin_id` and optional `params.artifacts`, and resets forked content after confirmation.

`artifact.diff` accepts required `params.plugin_id` and optional `params.artifact_path`.

`artifact.patch` accepts required `params.plugin_id`, `params.artifact_path`, and `params.patch`, plus optional `params.description`.

`artifact.update.check` accepts an optional `params.plugin_id`. When omitted, it scans all forked `.stash.json` files under the artifact stash root. The response is an array of `{ plugin_id, current_version, available_version, update_available }` records. The checker runs a hardened `git fetch` for the owning marketplace source before reading fetched remote refs.

```json
{
  "action": "artifact.update.check",
  "params": { "plugin_id": "demo-plugin@demo-market" }
}
```

`artifact.update.preview` accepts required `params.plugin_id`, computes a three-way update preview, and writes `.pending-update.json` for the later apply step.

```json
{
  "action": "artifact.update.preview",
  "params": { "plugin_id": "demo-plugin@demo-market" }
}
```

The preview response includes `upstream_version`, `upstream_commit`, `clean_merges`, `conflicts`, `unchanged`, `upstream_only`, and `user_only`. Clean merges include display diffs where available, while conflicts include base/yours/theirs content and conflict line ranges.

`artifact.update.apply` accepts required `params.plugin_id` and optional `params.strategy`, where strategy is one of `keep_mine`, `take_upstream`, `always_ask`, or `ai_suggest`. Because it is destructive, HTTP callers must also include `params.confirm: true`.

`artifact.merge.suggest` accepts required `params.plugin_id` and `params.artifact_path`.

`artifact.config.set` accepts required `params.plugin_id`, optional `params.strategy`, and optional `params.notify`.

Rules:

- built-in lab service tools always take precedence over upstream tools with the same name
- cross-upstream duplicate tool names: first discovered wins, later tools are skipped with a warning
- upstream tools with open circuit breakers (3+ consecutive failures) are excluded from `list_tools`
- callers do not need to distinguish between built-in and upstream tools

## Upstream Proxy Dispatch

When `call_tool` receives a tool name that is not a built-in service, the dispatcher checks the upstream pool:

- if the tool belongs to a healthy upstream, the call is forwarded
- the upstream pool records success or failure for circuit breaker tracking
- on failure, the response uses the `upstream_error` error kind
- response size is capped at `LAB_UPSTREAM_MAX_RESPONSE_BYTES` (default 10 MB)

## Resource Proxying

Upstream resource proxying is opt-in per upstream (`proxy_resources = true`).

Upstream resources are namespaced under `lab://upstream/{name}/{original_uri}` to avoid collisions with lab's own resources.

`list_resources` and `read_resource` are proxied to enabled upstreams. Failed resource listings from individual upstreams are logged as warnings; other upstreams continue to serve.

## Resources

Primary resource surfaces:

- `lab://catalog`
- `lab://<service>/actions`
- `lab://upstream/{name}/{original_uri}` (when upstream resource proxying is enabled)

These are generated from the same catalog data as tool-based help, with upstream resources appended at runtime.
