# Gateway Management

`lab` exposes a first-class `gateway` management surface for the upstream MCP proxy defined in [UPSTREAM.md](./UPSTREAM.md).

This is separate from the device runtime `master` model. `gateway` remains the upstream MCP control plane and must not be overloaded for fleet device identity, device ingest, or fleet log handling.

Use it when you want to inspect, test, add, update, remove, or reload `[[upstream]]` entries without editing `~/.config/lab/config.toml` by hand.

## Scope

- `[[upstream]]` in `~/.config/lab/config.toml` remains the persisted source of truth.
- `gateway.*` actions mutate that config, reconcile runtime state, and trigger MCP list-changed notifications when the merged catalog changes.
- In-flight MCP requests keep using the pool they already captured. New requests observe the swapped pool after reconcile completes.
- gateway management is exposed on the `master` only; non-master devices do not mount `/v1/gateway` or the `/mcp` transport

Secrets remain indirect:

- config and management responses may include `bearer_token_env`
- token values are never returned
- token values are never written into TOML
- changing an env var alone does not hot-apply; call `gateway.reload`
- tool exposure filters are stored as names/patterns only; the gateway never rewrites upstream tool payloads

## Actions

The complete gateway action inventory is generated from `ActionSpec`:

- [generated action catalog](../generated/action-catalog.md)
- [generated action catalog JSON](../generated/action-catalog.json)

`gateway.add`, `gateway.update`, `gateway.remove`, and `gateway.reload` are
destructive actions in shared action metadata. HTTP callers must send
`params.confirm = true`, CLI callers must confirm interactively or use `--yes` / `-y`, and MCP callers
must go through elicitation when supported.

### Stdio Gateway Safety

Stdio upstreams are privileged because testing or reconciling them starts the
configured command on the local host running `lab`. The gateway admin surface
therefore requires an additional explicit acknowledgement before any stdio
definition is probed or reconciled:

- `gateway.test` with a `spec.command`, or with `name` pointing at a configured
  stdio gateway, requires `params.allow_stdio = true`
- `gateway.add` with `spec.command` requires `params.allow_stdio = true`
- `gateway.update` requires `params.allow_stdio = true` whenever the resulting
  enabled gateway config uses `command`, even if the patch only changes
  unrelated fields

This acknowledgement is separate from destructive confirmation. `confirm: true`
authorizes config mutation; `allow_stdio: true` acknowledges local command
execution. HTTP and MCP callers should only send it after operator approval.

CLI commands expose the same guard as `--allow-stdio`.

## Tool Exposure

Gateway config can optionally restrict which discovered upstream tools are republished by `lab`.

- when `expose_tools` is unset, all discovered upstream tools remain exposed
- `expose_tools` accepts exact tool names and simple `*` wildcards
- an empty allowlist is treated as "clear the filter" rather than "block everything"
- filtered tools disappear from merged MCP `list_tools()` results and cannot be called directly through the proxy

Example:

```toml
[[upstream]]
name = "github"
url = "https://github.example.com/mcp"
bearer_token_env = "GITHUB_MCP_TOKEN"
proxy_resources = false
expose_tools = ["search_repos", "github_*"]
```

Typical patch payloads:

```json
{ "action": "gateway.update", "params": { "confirm": true, "name": "github", "patch": { "expose_tools": ["search_repos", "github_*"] } } }
```

```json
{ "action": "gateway.update", "params": { "confirm": true, "name": "github", "patch": { "expose_tools": null } } }
```

## Tool Search Mode

Gateway tool-search mode is a single gateway-wide switch. It is not configured per upstream server.

When enabled, Lab hides raw proxied upstream tools from MCP `list_tools()` and exposes two synthetic tools instead:

| Tool | Purpose |
|------|---------|
| `tool_search` | Search healthy discovered upstream tools across the gateway. |
| `tool_invoke` | Invoke one tool returned by `tool_search`. |

This keeps the MCP catalog small while still allowing clients to reach every exposed upstream tool. Per-upstream `expose_tools` filters still apply before tools enter the searchable catalog.

Configuration lives at root `[tool_search]` in `config.toml`:

```toml
[tool_search]
enabled = true
top_k_default = 10
max_tools = 5000
```

CLI:

```bash
labby gateway tool-search status
labby gateway tool-search enable
labby gateway tool-search enable --top-k-default 20 --max-tools 8000
labby gateway tool-search disable
```

HTTP/MCP gateway management actions:

```json
{ "action": "gateway.tool_search.get", "params": {} }
```

```json
{ "action": "gateway.tool_search.set", "params": { "enabled": true, "top_k_default": 10, "max_tools": 5000 } }
```

Search call shape on the MCP surface:

```json
{ "query": "github issue search", "top_k": 10, "include_schema": false }
```

Invoke call shape on the MCP surface:

```json
{ "name": "search_issues", "arguments": { "query": "repo:jmagar/lab tool_search" } }
```

Rules:

- `top_k_default` is validated in the range `1..=50`
- `max_tools` is validated in the range `1..=10000`
- `query` must be non-empty and no longer than 500 characters
- `include_schema` defaults to `false`; schemas are sanitized before return when requested
- old `[[upstream]].tool_search` blocks are accepted only as migration input and are dropped on the next gateway config write
- `gateway.update` rejects `patch.tool_search`; use `gateway.tool_search.set` instead

## Validation

- exactly one of `url` or `command` must be set
- `url` must use `http://` or `https://`
- bind-all addresses (`0.0.0.0`, `::`) are rejected
- RFC1918 and other private-network URLs are allowed
- stdio gateways are allowed only as an explicit privileged operator action.
  Proposed or persisted enabled stdio specs can execute local commands during
  `gateway.test`, `gateway.add`, and `gateway.update`. Machine callers must
  pass `allow_stdio: true` in addition to normal destructive confirmation where
  applicable; without that acknowledgement the request fails with
  `kind: "invalid_param"` on `allow_stdio`.

## Reconcile Model

Every mutating action follows the same sequence:

1. read and validate config
2. write `~/.config/lab/config.toml` with temp-file-in-same-dir plus rename
3. build a fresh upstream pool outside the config mutation lock
4. atomically swap the runtime handle
5. notify connected MCP peers when tool/resource/prompt catalogs changed

Observability requirements for that reconcile:

- log intent before config mutation begins
- log each phase transition (`config_write`, `pool_build`, `swap`, `notify`)
- log outcome with success/failure and elapsed time
- redact credential-bearing URLs, commands, args, and token-derived values in
  both logs and returned management views

## Examples

### CLI

```bash
labby gateway list
labby gateway get remote-lab
labby gateway test --name remote-lab
labby gateway add --name remote-lab --url https://lab2.example.com/mcp --bearer-token-env REMOTE_LAB_TOKEN
labby gateway add --name local-tools --command local-mcp-server --allow-stdio
labby gateway update remote-lab --proxy-resources true
labby gateway remove remote-lab
labby gateway reload
```

### MCP

```json
{ "tool": "gateway", "input": { "action": "gateway.list", "params": {} } }
{ "tool": "gateway", "input": { "action": "gateway.add", "params": { "confirm": true, "spec": { "name": "remote-lab", "url": "https://lab2.example.com/mcp", "bearer_token_env": "REMOTE_LAB_TOKEN" } } } }
{ "tool": "gateway", "input": { "action": "gateway.add", "params": { "confirm": true, "allow_stdio": true, "spec": { "name": "local-tools", "command": "local-mcp-server" } } } }
{ "tool": "gateway", "input": { "action": "gateway.reload", "params": { "confirm": true } } }
```

### HTTP API

```json
POST /v1/gateway
{ "action": "gateway.list", "params": {} }
```

```json
POST /v1/gateway
{ "action": "gateway.update", "params": { "confirm": true, "name": "remote-lab", "patch": { "proxy_resources": true } } }
```

## Gateway-Managed Protected MCP Routes

Gateway-managed protected MCP routes let Lab publish an arbitrary MCP backend at
a public host/path while Lab owns the OAuth protected-resource metadata,
challenge, token validation, and public error contract. The edge proxy points
the public MCP URL at Lab; Lab then proxies accepted Streamable HTTP MCP traffic
either to a raw backend MCP endpoint URL or to an existing named Gateway
upstream.

Use this for inline MCP services that should look like their own public OAuth
protected resources instead of appearing only as tools merged into Lab's `/mcp`
catalog.

Example public route:

```text
https://mcp.example.com/syslog
```

Example backend target:

```text
http://node.internal.example:3100/mcp
```

Persisted config lives in `[[protected_mcp_routes]]` entries in
`~/.config/lab/config.toml`:

```toml
[[protected_mcp_routes]]
name = "syslog"
enabled = true
public_host = "mcp.example.com"
public_path = "/syslog"
backend_url = "http://node.internal.example:3100/mcp"
scopes = ["mcp:read", "mcp:write"]
health_path = "/health"
```

Protected routes can also publish an existing named Gateway upstream instead of
proxying directly to a raw backend URL:

```toml
[[protected_mcp_routes]]
name = "axon"
enabled = true
public_host = "mcp.example.com"
public_path = "/axon"
upstream = "axon"
scopes = ["mcp:read", "mcp:write"]
```

When `upstream` is set, the protected route does not need `backend_url`. Lab
resolves the target URL and auth mode from the named `[[upstream]]` entry. For
OAuth upstreams, Lab uses the upstream OAuth credential stored for the shared
Gateway subject `gateway`.

Fields:

| Field | Purpose |
|-------|---------|
| `name` | Stable operator-facing route id. |
| `enabled` | Whether the route participates in metadata, challenge, auth, and proxy resolution. Defaults to `true`. |
| `public_host` | Bare public host only. Do not include scheme, port, or path. |
| `public_path` | Public MCP path prefix. Must include a service segment and cannot use Lab-reserved paths like `/.well-known/*` or `/v1/*`. |
| `upstream` | Optional named Gateway upstream to publish at this path. If the upstream uses OAuth, Lab uses the shared Gateway upstream OAuth credential when proxying. Mutually exclusive with `backend_url`; when set, `backend_url` is intentionally empty. |
| `backend_url` | Full backend Streamable HTTP MCP endpoint URL, for example `http://node.internal.example:3100/mcp`. Origin-only URLs are accepted as legacy input and default to `/mcp`. Mutually exclusive with `upstream`. |
| `backend_mcp_path` | Deprecated compatibility field for older configs. New routes should put the path in `backend_url`. |
| `scopes` | OAuth scopes advertised and enforced for this route. Defaults to `mcp:read` and `mcp:write`. |
| `health_path` | Optional backend health path used by route test actions. |

Management actions:

```json
{ "action": "gateway.protected_route.list", "params": {} }
{ "action": "gateway.protected_route.get", "params": { "name": "syslog" } }
{ "action": "gateway.protected_route.test", "params": { "route": { "name": "syslog", "public_host": "mcp.example.com", "public_path": "/syslog", "backend_url": "http://node.internal.example:3100/mcp" } } }
{ "action": "gateway.protected_route.test", "params": { "route": { "name": "axon", "public_host": "mcp.example.com", "public_path": "/axon", "upstream": "axon" } } }
{ "action": "gateway.protected_route.add", "params": { "route": { "name": "axon", "public_host": "mcp.example.com", "public_path": "/axon", "upstream": "axon", "scopes": ["mcp:read", "mcp:write"] } } }
{ "action": "gateway.protected_route.add", "params": { "route": { "name": "syslog", "public_host": "mcp.example.com", "public_path": "/syslog", "backend_url": "http://node.internal.example:3100/mcp", "scopes": ["mcp:read", "mcp:write"] } } }
{ "action": "gateway.protected_route.update", "params": { "name": "syslog", "route": { "name": "syslog", "enabled": false, "public_host": "mcp.example.com", "public_path": "/syslog", "backend_url": "http://node.internal.example:3100/mcp" } } }
{ "action": "gateway.protected_route.remove", "params": { "name": "syslog" } }
```

CLI equivalents:

```bash
labby gateway protected-route list
labby gateway protected-route get syslog
labby gateway protected-route test \
  --name syslog \
  --public-host mcp.example.com \
  --public-path /syslog \
  --backend-url http://node.internal.example:3100/mcp
labby gateway protected-route test \
  --name axon \
  --public-host mcp.example.com \
  --public-path /axon \
  --upstream axon
labby gateway protected-route add \
  --name syslog \
  --public-host mcp.example.com \
  --public-path /syslog \
  --backend-url http://node.internal.example:3100/mcp \
  --scope mcp:read \
  --scope mcp:write
labby gateway protected-route add \
  --name axon \
  --public-host mcp.example.com \
  --public-path /axon \
  --upstream axon \
  --scope mcp:read \
  --scope mcp:write
labby gateway protected-route update syslog \
  --public-host mcp.example.com \
  --public-path /syslog \
  --backend-url http://node.internal.example:3100/mcp \
  --enabled false
labby gateway protected-route remove syslog
```

### Migration From Legacy Env Routes

Older inline MCP proxy experiments used service-specific env vars such as
`MCP_<SERVICE>_URLS` or `MCP_<SERVICE>_BACKEND`. Move those values into a
Gateway-managed route instead:

| Legacy value | New Gateway field |
|--------------|-------------------|
| Service name in `MCP_<SERVICE>_*` | `name` |
| Public host from the URL clients used | `public_host` |
| Public path from the URL clients used | `public_path` |
| Backend origin from `MCP_<SERVICE>_BACKEND` | `backend_url` |
| Backend MCP endpoint from `MCP_<SERVICE>_BACKEND` | `backend_url` |
| Required OAuth scope policy | `scopes` |

For example, replace:

```bash
MCP_SYSLOG_URLS=https://mcp.example.com/syslog
MCP_SYSLOG_BACKEND=http://node.internal.example:3100/mcp
```

with:

```bash
labby gateway protected-route add \
  --name syslog \
  --public-host mcp.example.com \
  --public-path /syslog \
  --backend-url http://node.internal.example:3100/mcp
```

The same fields are exposed in the Lab Gateway UI. Prefer the UI/CLI fields over
ad hoc env parsing so route validation, duplicate detection, OAuth metadata,
and public error redaction all use the same source of truth.

### Edge Proxy Requirements

The edge proxy must preserve the request authority and scheme Lab uses to match
the configured public resource:

- preserve `Host`
- set `X-Forwarded-Proto` to the original client scheme
- forward `Authorization`, `Accept`, `Content-Type`, and MCP session headers
- disable request/response buffering for the MCP proxy path
- avoid response compression on the MCP proxy path
- use long read/write/idle timeouts suitable for Streamable HTTP and SSE
- do not rewrite the public path before Lab sees it

Public route OAuth is not the same as Lab's static bearer compatibility path.
`Authorization: Bearer $LAB_MCP_HTTP_TOKEN` remains an operator/admin shortcut
for Lab's admin/API routes, but public Gateway-managed MCP routes validate Lab
OAuth JWTs for the route resource, for example
`https://mcp.example.com/syslog`. Do not use the static bearer token as the
public MCP client credential for these routes.

Public errors must not leak backend origins, backend paths, private IPs, token
env var names, or upstream transport errors. Unknown, disabled, unhealthy, and
auth-failed routes should return stable public errors that identify only the
public route and public error kind.

### SWAG / nginx

For SWAG or plain nginx, route the MCP host/path to Lab and keep streaming
behavior unbuffered:

```nginx
location /syslog {
    proxy_pass http://labby:8765;
    proxy_http_version 1.1;

    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header Authorization $http_authorization;
    proxy_set_header Accept $http_accept;
    proxy_set_header Content-Type $content_type;

    proxy_buffering off;
    proxy_request_buffering off;
    gzip off;
    proxy_read_timeout 1h;
    proxy_send_timeout 1h;
}

location /.well-known/oauth-protected-resource/syslog {
    proxy_pass http://labby:8765;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

If your SWAG include stack has a shared OAuth discovery location, make sure it
does not swallow path-suffixed metadata such as
`/.well-known/oauth-protected-resource/syslog`. Those requests must reach Lab.

### Traefik

With Traefik, match both the public MCP path and the route-specific protected
resource metadata path, and forward to the Lab service:

```yaml
http:
  routers:
    syslog-mcp:
      rule: Host(`mcp.example.com`) && (PathPrefix(`/syslog`) || PathPrefix(`/.well-known/oauth-protected-resource/syslog`))
      entryPoints: [websecure]
      service: labby
      tls: {}
  services:
    labby:
      loadBalancer:
        servers:
          - url: http://labby:8765
        passHostHeader: true
```

Do not attach compression or buffering middleware to this router. Set transport
timeouts high enough for long-lived SSE reads.

### Generic Tunnels

For Cloudflare Tunnel, Tailscale Funnel, Pangolin, or another generic tunnel,
publish the public host to Lab's HTTP listener and keep the path intact. The
tunnel or local reverse proxy in front of Lab must pass the original `Host` and
scheme-equivalent `X-Forwarded-Proto` headers. Avoid tunnel features that buffer
large request bodies, compress event streams, or enforce short idle timeouts on
SSE connections.

### Verification Checklist

Set:

```bash
BASE=https://mcp.example.com
ROUTE=/syslog
TOKEN=<lab-oauth-access-token-for-this-resource>
SESSION=<mcp-session-id-from-initialize-response>
```

Metadata is public and route-specific:

```bash
curl -i "$BASE/.well-known/oauth-protected-resource$ROUTE"
```

Expected:

- `200`
- JSON `resource` is `https://mcp.example.com/syslog`
- `authorization_servers` points at the Lab issuer/public URL
- `scopes_supported` includes the route scopes
- no backend URL appears in headers or body

Unauthenticated MCP request returns a challenge:

```bash
curl -i -X POST "$BASE$ROUTE" \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}'
```

Expected:

- `401`
- `WWW-Authenticate: Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource/syslog"`
- structured public auth error
- no backend URL appears in headers or body

OAuth resource flow:

```bash
curl -i "$BASE/.well-known/oauth-protected-resource$ROUTE"
```

Use the advertised authorization server to request a token for resource
`https://mcp.example.com/syslog` and the configured scopes. The resulting access
token must be presented to the public route, not to the backend.

If the protected route publishes a named upstream that also uses OAuth, Lab
performs a second, separate auth step behind the route: it uses the upstream
OAuth credential stored for the shared Gateway subject when proxying to the
private upstream MCP server. The public Lab token is never passed through to the
upstream authorization server.

Streamable HTTP initialize:

```bash
curl -i -X POST "$BASE$ROUTE" \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl","version":"0"}}}'
```

Expected:

- success response from the backend MCP server
- MCP session header present when the backend is stateful
- no public response reveals `backend_url`

GET SSE stream:

```bash
curl -i -N "$BASE$ROUTE" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Mcp-Session-Id: $SESSION" \
  -H 'Accept: text/event-stream'
```

Expected:

- `200` with `Content-Type: text/event-stream`, or the backend's valid MCP
  stream response
- no buffering-delayed first bytes once the backend emits events
- connection is not closed by the edge timeout during normal idle periods

DELETE session:

```bash
curl -i -X DELETE "$BASE$ROUTE" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Mcp-Session-Id: $SESSION"
```

Expected:

- backend MCP session is terminated or acknowledged according to backend
  Streamable HTTP behavior
- no backend URL appears in headers or body

Disabled and unknown route behavior:

```bash
curl -i "$BASE/.well-known/oauth-protected-resource/disabled"
curl -i -X POST "$BASE/disabled" -H 'Content-Type: application/json' --data '{}'
curl -i "$BASE/.well-known/oauth-protected-resource/not-a-route"
curl -i -X POST "$BASE/not-a-route" -H 'Content-Type: application/json' --data '{}'
```

Expected:

- disabled routes do not advertise protected-resource metadata and do not proxy
  to the backend
- unknown routes do not advertise metadata and do not proxy to any backend
- public errors are stable and redacted; they must not reveal backend origins,
  backend paths, private IPs, or configured token env var names

## Upstream OAuth Routes

For upstreams configured with `[upstream.oauth]` (see
[CONFIG.md](../runtime/CONFIG.md#upstream-oauth-authorization_code--pkce) and
[UPSTREAM.md](./UPSTREAM.md#upstream-oauth-authorization_code--pkce)), the
gateway mounts four master-only HTTP routes. All four require an authenticated
session and the master-only middleware; non-master sessions get `403`.

| Method | Path | Purpose |
|--------|------|---------|
| `POST` | `/v1/gateway/oauth/start` | Begin authorization for the shared gateway subject `gateway`. Body `{ "upstream": "<name>" }`. Returns `{ "authorization_url": "..." }` (JSON only — no browser-redirect mode). |
| `GET` | `/auth/upstream/callback` | Authorization-code callback. Validates the authenticated session, atomically takes the pending state row (bound to `(upstream, subject)`), exchanges the code, persists encrypted credentials, redirects to `/gateway/oauth/result?upstream=<name>&status=<ok\|fail>`. |
| `GET` | `/v1/gateway/oauth/status?upstream=<name>` | Returns `{ "authenticated": bool, "upstream": "<name>", "expires_within_5m": bool }`. Deliberately omits subject and raw expiry timestamp to avoid enumeration and fingerprinting. |
| `POST` | `/v1/gateway/oauth/clear?upstream=<name>&confirm=true` | Destructive. Requires both `upstream` (the upstream name) and `confirm=true` as query parameters. Without `confirm=true`, returns `422` with JSON `{ "kind": "confirmation_required", ... }`. With confirm, deletes persisted credentials and evicts the cached `AuthClient`. In-flight requests complete naturally under the old credential (graceful drain by Rust ownership — not a designed protocol). |

### OAuth Operator Examples

CLI:

```bash
labby gateway mcp auth start chrome-devtools
labby gateway mcp auth open chrome-devtools --wait
labby gateway mcp auth status chrome-devtools
labby gateway mcp auth clear chrome-devtools
```

MCP tool calls:

```json
{ "tool": "gateway", "input": { "action": "gateway.oauth.start", "params": { "upstream": "chrome-devtools" } } }
{ "tool": "gateway", "input": { "action": "gateway.oauth.status", "params": { "upstream": "chrome-devtools" } } }
{ "tool": "gateway", "input": { "action": "gateway.oauth.clear", "params": { "confirm": true, "upstream": "chrome-devtools" } } }
```

These actions now operate on the shared gateway OAuth subject `gateway`, so the
web UI, CLI, and MCP tool surface all refer to the same stored upstream
credential row.

When an OAuth upstream is also published through a protected MCP route with
`upstream = "<name>"`, successful upstream authorization is required before the
route can proxy MCP traffic. `gateway.test` and the Gateway UI capability
checks use the same shared subject and should report discovered tools/resources
after authorization.

Callback security invariants (enforced in code, spec-required):

- The callback is a browser-facing redirect endpoint. Subject is resolved from
  the authenticated browser session cookie, **not** from the `state` parameter
  or the pending state row. No session → `oauth_state_invalid`.
- The `upstream` query parameter is forwarded to the manager, which enforces it
  against the pending state row's upstream name via the SQL primary key
  (`(upstream_name, subject, csrf_token)`).
- `state` is matched via a single `DELETE ... RETURNING` to prevent replay
  across connection-pool races.
- The result page HTML-escapes the operator-controlled `upstream` name.

### Reload And Credential Lifecycle

- `gateway.reload` eagerly evicts all cached `AuthClient` entries for every
  OAuth upstream in the current config, then rebuilds a fresh upstream pool.
  OAuth upstreams are rediscovered with the shared `gateway` subject when the
  upstream OAuth runtime is configured.
  It does **not** delete persisted credential rows — `AuthClient`s are rebuilt
  on the next request using whatever credentials are in the store.
- `clear_credentials` is the only way to invalidate a persisted credential.
  It evicts the cache entry and deletes the row; in-flight `Arc<AuthClient>`
  holders complete naturally under the old token.
- Expired access-only credential rows (no refresh token) are pruned by the
  60-second `cleanup_expired` background task, alongside expired PKCE state.

## Limitations

- `gateway.reload` is the only action that promises to pick up changed bearer-token env vars.
- The product HTTP API exposes `/v1/gateway` for gateway management, but it still does not proxy arbitrary upstream MCP tools through `/v1/*`.
- Runtime counts depend on current discovery state; an unreachable upstream can remain configured while reporting zero discovered items.
- Gateway mutations rewrite `config.toml` by serializing the full `LabConfig` struct. TOML comments and unknown keys not represented in the struct are dropped on write. A migration to `toml_edit` for comment-preserving round-trips is deferred.
