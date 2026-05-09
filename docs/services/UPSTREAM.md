# Upstream MCP Proxy

Lab can act as an MCP gateway, proxying tool calls and resource reads to upstream MCP servers. This lets a single `lab` instance aggregate tools from multiple MCP servers behind one authenticated endpoint.

Upstream servers are first-class providers in the merged MCP tool catalog. After discovery, their tools appear in `list_tools()` beside built-in `lab` tools. Callers do not need a separate tool or namespace to invoke proxied upstream tools themselves.

If gateway-wide `[tool_search].enabled = true`, raw upstream tools are hidden from `list_tools()` and exposed through synthetic `tool_search` / `tool_invoke` helpers instead. That mode is documented in [GATEWAY.md](./GATEWAY.md#tool-search-mode).

`lab` also exposes a separate `gateway` management surface for editing and reloading upstream definitions. That management surface is documented in [GATEWAY.md](./GATEWAY.md).

Gateway-managed protected MCP routes are a different mode: they publish an
inline public MCP route with Lab-owned OAuth protected-resource metadata and
proxy the whole Streamable HTTP MCP route to a backend. Use
[GATEWAY.md — Gateway-Managed Protected MCP Routes](./GATEWAY.md#gateway-managed-protected-mcp-routes)
for that setup instead of `[[upstream]]` tool merging.

The upstream pool lives in `crates/lab/src/dispatch/upstream/` because it is shared infrastructure. The runtime proxy path described in this document is wired into the MCP surface. The HTTP API now exposes `/v1/gateway` for gateway management, but it still does not proxy arbitrary upstream MCP tools.

## What Operators Configure

To proxy an upstream server through `lab`, you configure one or more `[[upstream]]` entries in `~/.config/lab/config.toml`, optionally provide bearer-token env vars in `~/.lab/.env`, then start `labby serve` normally.

`lab` will:

1. connect to every configured upstream at startup
2. run tool discovery against each upstream
3. merge discovered tools into its own MCP catalog
4. serve the combined catalog through whichever MCP transport you expose from `lab`

OAuth upstreams are discovered at startup only when Lab has upstream OAuth
runtime state and a stored credential for the shared Gateway subject. Without
that credential, subject-less discovery deliberately skips OAuth upstreams so a
user-specific token view is not cached globally.

That means the client connects only to `lab`:

- `labby mcp` for stdio clients such as Claude Desktop
- `labby serve` for streamable HTTP MCP clients

The client never connects directly to the upstreams once `lab` is acting as the gateway.

## Configuration

Upstream servers are configured in `config.toml` using `[[upstream]]` array entries.

### HTTP Upstream

```toml
[[upstream]]
name = "remote-lab"
url = "https://lab2.example.com/mcp"
bearer_token_env = "LAB_UPSTREAM_TOKEN"
proxy_resources = true
expose_tools = ["search_repos", "github_*"]
```

### Stdio Upstream

```toml
[[upstream]]
name = "local-server"
command = "my-mcp-server"
args = ["--port", "5000"]
proxy_resources = false
```

Stdio upstreams execute a local child process on the host running `lab`.
Gateway admin actions that test or reconcile enabled stdio definitions require
an explicit `allow_stdio: true` acknowledgement in addition to any destructive
confirmation. See [GATEWAY.md](./GATEWAY.md#stdio-gateway-safety).

### Config Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Human-readable name. Must be non-empty, unique, and URI-safe (no `/`, `?`, `#`). |
| `url` | string | one of url/command | HTTP(S) URL of the upstream MCP server. |
| `command` | string | one of url/command | Command to run for stdio transport. |
| `args` | string[] | no | Arguments for the stdio command. |
| `bearer_token_env` | string | no | Name of an env var holding a bearer token. Not the token itself. |
| `proxy_resources` | bool | no | Whether to proxy resources from this upstream. Default: `false`. |
| `expose_tools` | string[] | no | Optional allowlist of tool names/patterns to expose from this upstream. Supports exact names and `*` wildcards. |

Exactly one of `url` or `command` must be set.

### Config File Locations

`lab` loads configuration from:

1. process environment
2. `~/.lab/.env`
3. `~/.config/lab/config.toml`

So a typical gateway setup looks like:

`~/.config/lab/config.toml`

```toml
[mcp]
transport = "http"
host = "127.0.0.1"
port = 8765

[[upstream]]
name = "remote-lab"
url = "https://lab2.example.com/mcp"
bearer_token_env = "REMOTE_LAB_TOKEN"
proxy_resources = true
expose_tools = ["radarr", "search_*"]

[[upstream]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/srv/data"]
proxy_resources = false
```

`~/.lab/.env`

```bash
REMOTE_LAB_TOKEN=replace-me
LAB_MCP_HTTP_TOKEN=replace-this-too
```

### Config Validation

Validation runs before discovery. Invalid entries are skipped with a warning during startup discovery. The runtime `gateway` management surface rejects invalid mutations before writing them to disk.

| Condition | Result |
|-----------|--------|
| Empty name | Skipped |
| Duplicate name | Startup keeps the first and warns; runtime gateway mutations reject the write |
| Name contains `/`, `?`, or `#` | Skipped |
| URL not `http://` or `https://` | Skipped |
| URL uses bind-all address (`0.0.0.0`, `::`) | Skipped |
| Both `url` and `command` set | Skipped |
| Neither `url` nor `command` set | Skipped |

### Bearer Token

The `bearer_token_env` field names an environment variable — it does not contain the token directly. At connection time, the pool reads the env var and passes the token as an auth header for HTTP upstreams, or injects it into the child process environment for stdio upstreams.

If the named env var is not set, the connection proceeds without auth (HTTP upstreams log a warning; stdio upstreams currently skip injection silently).

Changing a bearer-token env var does not hot-apply by itself. Use `gateway.reload` when you want the live pool to re-read `bearer_token_env`.

## Upstream OAuth (authorization_code + PKCE)

OAuth-protected upstream MCP servers are authenticated for a shared gateway
credential rather than by a static bearer token. Configuration shape and examples live in
[CONFIG.md — Upstream OAuth](../runtime/CONFIG.md#upstream-oauth-authorization_code--pkce).
Operator browser flow lives in [GATEWAY.md](./GATEWAY.md).

### Scope

- HTTP upstream transport only. Stdio upstreams cannot use OAuth in this phase
  because stdio sessions do not carry a stable authenticated subject.
- Subject-less discovery skips OAuth upstreams. The hosted gateway startup,
  `gateway.reload`, and `gateway.test` use the explicit shared subject
  `gateway` so configured OAuth upstreams can be discovered after an operator
  completes the upstream OAuth flow. If no credential exists yet, they remain
  configured but report no discovered capabilities until authorization succeeds.
  The authorization initiation flow (`POST /v1/gateway/oauth/start`) requires
  an authenticated HTTP session.
- `/mcp` over HTTP and the hosted web UI are the supported call surfaces.

### Flow

1. Operator runs `POST /v1/gateway/oauth/start { "upstream": "<name>" }`; the
   server returns a JSON `{ "authorization_url": "..." }` body.
2. Browser navigates to that URL; the upstream AS authenticates the user.
3. AS redirects to `/auth/upstream/callback?code=...&state=...&upstream=<name>`
   on the same origin as `LAB_PUBLIC_URL`.
4. `lab` validates the authenticated session, atomically takes the pending
   state row (`DELETE ... RETURNING`), exchanges the code for tokens, encrypts
   the token response with chacha20poly1305, and persists it keyed by
   `(upstream_name, "gateway")`.
5. Subsequent `/mcp` and UI requests find the persisted credential and proxy
   through a per-`(upstream, subject)` `AuthClient` cached in the gateway. The
   default shared subject is `gateway`.

CLI examples:

```bash
labby gateway mcp auth start chrome-devtools
labby gateway mcp auth open chrome-devtools --wait
labby gateway mcp auth status chrome-devtools
labby gateway mcp auth clear chrome-devtools
```

### Spec-Aligned Invariants

- **PKCE S256-only.** The AS metadata must advertise `S256` in
  `code_challenge_methods_supported`. Missing or `plain`-only metadata is
  refused with `oauth_unsupported_method`; `lab` never falls back to `plain`.
- **RFC 8707 `resource`.** The canonical upstream MCP URL (RFC 3986 §6.2.2
  normalized: lowercase scheme + host, normalized percent-encoding, default
  port elided, trailing slash preserved as configured) is sent on **both** the
  authorization request and the token request, byte-identical between the
  two. Canonicalization runs at config-validation time so the stored URL and
  the `resource` wire value are the same string. Mismatched `aud` claims on
  the returned token surface as `oauth_resource_mismatch`.

  **Known gap (upstream).** rmcp 1.4's refresh path does not re-emit the
  `resource` parameter on the `refresh_token` grant. Most authorization
  servers continue to honor the audience bound at initial exchange, so this
  is acceptable in practice today, but an AS that requires `resource` on
  every token-endpoint call will reject refreshes. Tracked for follow-up
  once rmcp exposes a refresh hook we can extend.
- **Issuer binding.** After AS metadata discovery, `metadata.issuer` is
  required — missing `issuer` surfaces as `oauth_issuer_mismatch`. The
  `authorization_endpoint`, `token_endpoint`, `revocation_endpoint`, and
  (when present) `registration_endpoint` and `userinfo_endpoint` origins
  (scheme + host + port) must match the issuer origin; any drift surfaces as
  `oauth_issuer_mismatch` (RFC 8414 §3.3).
- **No Google reuse.** Outbound upstream OAuth is distinct from the inbound
  `lab-auth` Google provider used for user login to `lab`. They do not share
  code, clients, or tokens.

### Per-`(upstream, subject)` Client Cache

The gateway maintains a `DashMap<(upstream_name, subject), AuthClient>`
built atomically per key. Two subjects calling the same OAuth upstream get
two isolated `AuthClient` instances; one subject's tokens are never visible
to another.

Current operator surfaces default to the shared subject `gateway`, so the
common path is one cached `AuthClient` per upstream for the whole gateway.

The cache stores the `client_id` each entry was built with. A `gateway.reload`
that changes an upstream's `client_id` evicts cached entries with a stale
`client_id`; subsequent calls rebuild them. This closes a silent re-bind gap
where a config edit would otherwise keep old credentials attached to a new
upstream definition.

OAuth-tagged upstreams are never discovered by the subject-less
`discover_all` path. Gateway-owned startup/reload/test discovery uses an
explicit subject-scoped path with the shared `gateway` subject; MCP request
paths that need a real user subject use the per-request subject-scoped helpers.
The circuit breaker and catalog merging infrastructure applies to
static-bearer upstreams; OAuth upstreams are connected through the
subject-scoped auth client cache.

### Refresh Semantics

Refresh is single-flight per `(upstream_name, subject)` using a `tokio::sync::Mutex`
keyed on the pair. Lock entries are retained for the lifetime of the process.

Today the manager runs **proactive refresh only**:

- **Proactive:** before dispatching a request, if the cached access token is
  less than 30 seconds from expiry, refresh under the per-key lock first.
- **Reactive (401):** **deferred.** MCP traffic flows through rmcp's
  `StreamableHttpClientWorker`, which hides the raw HTTP response from the
  gateway, so a 401 on an MCP call currently surfaces as a generic transport
  error rather than `oauth_needs_reauth`. Operators recover by calling
  `POST /v1/gateway/oauth/start` to re-authorize. When this is wired, only
  idempotent methods (`GET`/`HEAD`/`OPTIONS`) will retry after refresh;
  non-idempotent methods (`POST`, including MCP `tool_call`) will surface
  the original 401 as `oauth_needs_reauth` without retry, because a retry
  could double-execute a destructive tool call.

On `invalid_grant` (refresh token revoked or rotated twice), `lab` returns
`oauth_needs_reauth` to the caller. The user re-initiates authorization.

### `oauth_needs_reauth` Triggers

A caller sees `oauth_needs_reauth` in any of these situations:

- no credential exists yet for `(upstream, subject)`
- the refresh token was rejected with `invalid_grant`
- decryption of the stored `token_blob` failed (operator rotated
  `LAB_OAUTH_ENCRYPTION_KEY`)
- (future, once reactive 401 is wired) a 401 arrived on a non-idempotent
  request and retry is not safe

Recovery is identical in all cases: start a new authorization via
`POST /v1/gateway/oauth/start`.

### Token-At-Rest Encryption

Persisted token responses are sealed with chacha20poly1305 AEAD. A fresh 12-byte
nonce is generated on every `seal()` call; the refresh upsert stores the new
nonce and must never preserve the previous one. The key is loaded once at
startup from `LAB_OAUTH_ENCRYPTION_KEY`; see [CONFIG.md](../runtime/CONFIG.md#environment-variables-2)
for rotation.

### Prior Art

The cache implementation still supports per-`(upstream, subject)` isolation
internally, but the current operator-facing flow defaults to the shared subject
`gateway` for all three surfaces.

## Discovery

At startup, lab connects to all configured upstreams in parallel. Each upstream gets a 15-second timeout for connection and tool discovery (`list_tools()`).

Failed upstreams are marked unhealthy. Healthy upstreams continue operating. A single failed upstream does not prevent others from connecting.

After startup, proxied RMCP operations continue to use explicit per-RPC
timeouts. Tool calls, prompt reads, resource reads, and discovery/listing
operations must fail closed with logged timeout/error events rather than
blocking indefinitely behind one hung upstream.

```text
upstream discovery succeeded  upstream=remote-lab tool_count=12
upstream discovery failed     upstream=broken-server error="connection refused"
upstream discovery timed out  upstream=slow-server timeout_secs=15
```

## How Routing Works

The combined catalog is exposed as one MCP server, but ownership is still resolved internally.

For each incoming MCP tool call:

1. `lab` checks whether the tool name belongs to a built-in local service
2. if not, it checks the discovered upstream tool map
3. if an upstream owns that tool name, the request is proxied there using the original MCP arguments
4. the upstream result is normalized into `lab`'s usual success/error envelope shape

This internal precedence rule does not make upstream tools second-class. It is just how collisions are resolved.

## Tool Collision Handling

When upstream tools are merged into the lab tool catalog:

1. **Built-in lab services always take precedence.** If an upstream exposes a tool named `radarr`, the upstream tool is silently dropped (with a warning logged).
2. **Cross-upstream duplicates: first discovered wins.** If two upstreams expose a tool named `my-tool`, the second is skipped with a warning.

Upstream tools appear alongside built-in tools in `list_tools()`. Callers do not need to know whether a tool is built-in or proxied.

## Tool Exposure Filtering

Each upstream may optionally set `expose_tools` to restrict which discovered tools become visible downstream.

- unset `expose_tools` means "expose all discovered tools"
- exact entries match one tool name
- entries containing `*` use simple wildcard matching
- malformed exposure policies fail closed: the upstream stays connected, but no discovered tools from that upstream are exposed until the config is fixed

The exposure policy applies in two places:

1. merged tool discovery, so filtered tools are absent from `list_tools()`
2. direct proxied tool calls, so filtered tools behave as if they were never exposed

## Circuit Breaker

Each upstream has independent health tracking.

| Constant | Value |
|----------|-------|
| `CIRCUIT_BREAKER_THRESHOLD` | 3 consecutive failures |
| `REPROBE_INTERVAL` | 30 seconds |

### State Transitions

- **Healthy** — upstream is routable. 0 consecutive failures.
- **Unhealthy (below threshold)** — upstream has 1-2 consecutive failures. Still routable and included in tool listings.
- **Unhealthy (at/above threshold)** — upstream has 3+ consecutive failures. Excluded from tool listings.

### What Counts as a Failure

- Connection errors
- Tool call errors (`is_error` responses)
- Prompt and resource proxy errors
- Dropped connections
- Timeouts
- Response size cap exceeded

### Recovery

- A successful proxied call resets the upstream to healthy (0 failures).
- The code defines a `REPROBE_INTERVAL` of 30 seconds and tracks when an upstream became unhealthy.
- Automatic scheduled re-probing is not currently wired into the runtime. In practice, recovery happens when a later proxied call or resource request succeeds.

## Response Size Cap

Upstream responses are subject to a size cap to prevent oversized payloads from consuming memory or being forwarded to callers.

| Setting | Default |
|---------|---------|
| `LAB_UPSTREAM_MAX_RESPONSE_BYTES` | 10 MB (10,485,760 bytes) |

The check is **post-hoc** — rmcp materializes the full response in memory before lab can inspect it. The cap prevents forwarding oversized payloads to callers but cannot prevent the memory allocation itself. A streaming limit would require rmcp transport-level support.

The cap applies to both `call_tool` and `read_resource` responses.

## Resource Proxying

Resource proxying is opt-in per upstream via `proxy_resources = true`.

### URI Namespacing

Upstream resources are prefixed to avoid URI collisions with lab's own resources:

```text
lab://upstream/{name}/{original_uri}
```

For example, if upstream `remote-lab` exposes a resource `lab://radarr/actions`, it appears as:

```text
lab://upstream/remote-lab/lab://radarr/actions
```

### Operations

- `list_resources()` queries all resource-enabled upstreams and returns namespaced URIs.
- `read_resource()` strips the prefix, identifies the upstream by name, and forwards the read.

Failed resource listings from individual upstreams are logged as warnings. Other upstreams continue to serve.

The same graceful-degradation rule applies to prompt/resource discovery and
reads: one upstream failure must not prevent healthy upstreams from serving
partial results.

## What Is Exposed Where

### MCP

The upstream gateway is active on both MCP transports exposed by `lab`:

- stdio
- streamable HTTP at `/mcp`

If an upstream tool is discovered successfully, MCP clients connected to `lab` can call it as a normal tool.

### HTTP API

The product HTTP API under `/v1/*` does not proxy arbitrary upstream MCP tools. It serves built-in `lab` routes plus `/v1/gateway` for gateway management.

Keep this distinction explicit in operator docs:

- use MCP when you want the upstream gateway behavior
- use `/v1/gateway` when you want to manage `[[upstream]]` entries over HTTP
- use the rest of `/v1/*` for `lab`'s built-in HTTP API surface

## End-to-End Setup

### 1. Configure upstreams

Add one or more `[[upstream]]` entries to `~/.config/lab/config.toml`.

### 2. Provide any required secrets

Set bearer-token env vars named by `bearer_token_env` in `~/.lab/.env` or the process environment.

### 3. Start `lab`

For local stdio clients:

```bash
labby mcp
```

For network MCP clients:

```bash
labby serve
```

### 4. Point the client at `lab`, not the upstreams

Example `.mcp.json` for stdio:

```json
{
  "mcpServers": {
    "lab": {
      "command": "labby",
      "args": ["serve"]
    }
  }
}
```

Example HTTP MCP endpoint:

```text
https://lab.example.com/mcp
```

### 5. Verify discovery

Startup logs should include lines like:

```text
upstream discovery succeeded  upstream=remote-lab tool_count=12
```

Then an MCP client connected to `lab` should see the upstream tools in `list_tools()`.

## Operational Notes

- Upstream tool schemas are cached from discovery and reused for MCP tool metadata.
- Upstream calls preserve the original MCP argument payload rather than forcing it through `lab`'s `action` + `params` wrapper.
- Upstream errors are normalized into `lab` envelopes and usually surface as `upstream_error`, `network_error`, `server_error`, `decode_error`, or `internal_error`.
- Response-size limits are enforced after the upstream response is materialized in memory.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LAB_UPSTREAM_MAX_RESPONSE_BYTES` | 10485760 | Maximum response size from upstream servers. |
| (per `bearer_token_env`) | — | Bearer token for each upstream, named in config. |

## Observability

Discovery events are logged at `INFO` (success) and `WARN` (failure/timeout).

Circuit breaker state changes are logged:

- `WARN` when the breaker opens (3+ failures).
- `INFO` when the breaker resets (successful call after failure).

Tool collision warnings are logged at `WARN`.

## Related Docs

- [CONFIG.md](../runtime/CONFIG.md) — `[[upstream]]` config section
- [MCP.md](../surfaces/MCP.md) — upstream tool merging in MCP surface
- [ERRORS.md](../dev/ERRORS.md) — `upstream_error` kind
- [TRANSPORT.md](../surfaces/TRANSPORT.md) — HTTP transport setup
