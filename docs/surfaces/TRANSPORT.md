# Transport Configuration

Lab supports two MCP transports: stdio and streamable HTTP. Both expose the same server behavior — transport choice does not change the catalog, schemas, envelopes, or destructive-op policy.

## Stdio

Stdio is the explicit child-process transport. Use it for Claude Desktop, IDE extensions, and any MCP client that launches lab as a child process.

No authentication is required — security is provided by process-level isolation. The parent process owns the stdio pipes and controls access.

```bash
labby mcp
labby mcp --services marketplace
```

No network listener is opened. No host, port, or auth configuration is needed.

This shortcut is contractual: code, CLI help, tests, and operator docs must all
agree that `labby mcp` is the stdio child-process entrypoint unless changed intentionally in one
coordinated update.

## Streamable HTTP (Default)

The HTTP transport mounts the MCP protocol at `/mcp` inside the axum HTTP server, alongside the
REST API at `/v1/*`. When exported Labby assets are available, the same server also hosts the web UI
from `/`.

```bash
labby serve
labby serve --services gateway,marketplace
```

### Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `LAB_MCP_TRANSPORT` | `http` | Transport selection. Set to `stdio` for child-process use. |
| `LAB_MCP_HTTP_HOST` | `127.0.0.1` | Bind address. |
| `LAB_MCP_HTTP_PORT` | `8765` | Bind port. |
| `LAB_MCP_HTTP_TOKEN` | — | Static bearer token for authentication. |
| `LAB_MCP_SESSION_TTL_SECS` | `300` | Session keep-alive TTL in seconds. |
| `LAB_MCP_STATEFUL` | `true` | Whether to use stateful MCP sessions. |
| `LAB_MCP_ALLOWED_HOSTS` | — | Comma-separated hostnames for DNS rebinding protection. |
| `LAB_PUBLIC_URL` | — | Public URL of this lab instance. Its host is added to the allowed-host list in OAuth mode. |
| `LAB_CORS_ORIGINS` | — | Comma-separated CORS origin allowlist. |
| `LAB_WEB_ASSETS_DIR` | auto-detect | Optional path to exported Labby assets served by `labby serve`. |

Config TOML equivalents (env vars take precedence):

```toml
[mcp]
transport = "http"
host = "127.0.0.1"
port = 8765

[web]
assets_dir = "/path/to/labby/out"
```

CLI flags take precedence over env vars, which take precedence over config.toml:

1. `--host`, `--port` (CLI)
2. `LAB_MCP_HTTP_HOST`, `LAB_MCP_HTTP_PORT`, `LAB_MCP_TRANSPORT` (env)
3. `mcp.host`, `mcp.port`, `mcp.transport` (config.toml)
4. Defaults: `127.0.0.1`, `8765`, `http`

### Session Management

The HTTP transport uses RMCP's `StreamableHttpService` with a `LocalSessionManager`.

- `LAB_MCP_SESSION_TTL_SECS` controls the session keep-alive duration (default: 300 seconds / 5 minutes).
- `LAB_MCP_STATEFUL` controls whether stateful sessions are used (default: `true`). Set to `false` for stateless operation.

### DNS Rebinding Protection

The HTTP transport validates the `Host` header against an allowed hosts list. This prevents DNS rebinding attacks where a malicious page redirects its hostname to `127.0.0.1`.

Allowed hosts are assembled from:

1. **Always included:** `localhost`, `127.0.0.1`, `::1`.
2. **`LAB_MCP_ALLOWED_HOSTS`** — comma-separated additional hostnames.
3. **`LAB_PUBLIC_URL`** — when OAuth mode is enabled, the hostname is automatically extracted and added.

Wildcard (`*`) is rejected with a warning — it would disable Host header validation entirely.

### Authentication

Protected routes (`/v1/*` and `/mcp`) require authentication when a static bearer token or OAuth mode is configured. Unauthenticated routes (`/health`, `/ready`, and OAuth metadata endpoints) are always accessible. The complete generated route inventory, including auth/runtime posture, lives in [generated/api-routes.md](../generated/api-routes.md).

The Labby web UI shell is served publicly when web assets are enabled. The UI then calls the same-origin
API and MCP routes on the same port.

Auth methods (see [OAUTH.md](../runtime/OAUTH.md) for details):

- **Static bearer token** via `LAB_MCP_HTTP_TOKEN` — constant-time comparison.
- **OAuth mode** via `LAB_AUTH_MODE=oauth`, `LAB_PUBLIC_URL`, and Google client credentials.
- Both can be active simultaneously. Static bearer is checked first.
- If neither auth method is configured, the router permits local loopback requests only; non-loopback binds are rejected by the safety gate below.

Auth-adjacent routes mounted on this server, including `/auth/session`,
`/auth/logout`, `/authorize`, `/auth/google/callback`, and `/token`, remain
part of the same request-id and structured-error contract even when their
payloads are not normal `/v1/{service}` dispatches.

### Reverse Proxy Requirements For MCP Routes

Streamable HTTP MCP routes are long-lived protocol routes, not ordinary JSON
REST calls. This applies to Lab's own `/mcp` route and to Gateway-managed
protected MCP routes such as `https://mcp.example.com/syslog`.

Any SWAG/nginx, Traefik, or tunnel layer in front of Lab must:

- preserve `Host`
- set `X-Forwarded-Proto` to the original client scheme
- forward `Authorization`, `Accept`, `Content-Type`, and MCP session headers
- avoid request buffering and response buffering on the MCP route
- avoid compression on the MCP route
- use read/write/idle timeouts that allow long-lived SSE streams
- keep the public path intact until Lab receives the request

Gateway-managed protected MCP routes also need the matching route-specific
metadata path to reach Lab, for example:

```text
/.well-known/oauth-protected-resource/syslog
```

Do not let a shared OAuth discovery include or edge auth layer swallow those
path-suffixed metadata requests. Lab must generate the route-specific metadata
and 401 `WWW-Authenticate` challenge so clients bind tokens to the public route
resource, not to the private backend URL.

See [GATEWAY.md](../services/GATEWAY.md#gateway-managed-protected-mcp-routes)
for SWAG/nginx, Traefik, generic tunnel examples, and curl verification.

### Safety Gate

Lab refuses to bind on a non-localhost address without auth:

```text
refusing to bind HTTP on 0.0.0.0:8765 without authentication.
Set LAB_MCP_HTTP_TOKEN or LAB_AUTH_MODE=oauth, or bind to 127.0.0.1 for local-only access.
```

Loopback addresses (`127.0.0.1`, `::1`, `[::1]`, `localhost`) are exempt.

## Middleware Stack (HTTP)

The HTTP server applies middleware in this order (outermost to innermost):

| Layer | Description |
|-------|-------------|
| `SetRequestId` | Generates a UUID v4 `x-request-id` for every request that lacks one. |
| `TraceLayer` | Tracing spans per request with method, path, status, and latency. |
| `PropagateRequestId` | Echoes `x-request-id` back in the response header. |
| `TimeoutLayer` | 30-second request timeout. Returns 504 on expiry. |
| `CompressionLayer` | gzip response compression. |
| `CorsLayer` | Explicit origin allowlist (see below). |
| Auth middleware | Bearer token and/or OAuth JWT validation. Applied to protected routes only. |

### CORS

CORS is configured with an explicit origin allowlist. It is not permissive by default.

Always allowed (loopback with common dev ports):

- `http://localhost`, `http://localhost:3000`, `http://localhost:5173`, `http://localhost:8080`
- `http://127.0.0.1`, `http://127.0.0.1:3000`, `http://127.0.0.1:5173`, `http://127.0.0.1:8080`
- `http://[::1]`

Additional origins via `LAB_CORS_ORIGINS` (comma-separated):

```bash
LAB_CORS_ORIGINS=https://lab.example.com,https://admin.example.com
```

Unparseable entries are logged as warnings and skipped.

Allowed methods: GET, POST, OPTIONS.
Allowed headers: `Authorization`, `Content-Type`, `x-request-id`.

## Route Layout

The HTTP router is role-aware:

- the controller exposes the full operator control plane
- a non-controller node keeps only `/health`, `/ready`, and `/v1/nodes/*`
- non-controller nodes do not expose `/mcp`, `/v1/{service}`, `/v1/gateway`, `/v1/openapi.json`, `/v1/docs`, or the Web UI

When HTTP transport is active, the generated route inventory is the canonical
path/auth matrix:

- [generated/api-routes.md](../generated/api-routes.md)
- [generated/api-routes.json](../generated/api-routes.json)
- [generated/openapi.json](../generated/openapi.json)

## Example: Local Development

```bash
# Bind to localhost, no auth needed when neither static bearer nor OAuth is configured
labby serve
# → listening on 127.0.0.1:8765

curl http://localhost:8765/health
curl http://localhost:8765/v1/marketplace -d '{"action":"help"}'
# if exported Labby assets exist:
open http://localhost:8765/
```

## Example: Network Deployment

```bash
# In ~/.lab/.env
LAB_MCP_TRANSPORT=http
LAB_MCP_HTTP_HOST=0.0.0.0
LAB_MCP_HTTP_PORT=8765
LAB_MCP_HTTP_TOKEN=$(openssl rand -hex 32)
LAB_PUBLIC_URL=https://lab.example.com

labby serve
```

```bash
curl -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" \
     https://lab.example.com/v1/marketplace \
     -d '{"action":"help"}'
```

## Gateway Trust Model

Gateway admin actions (`/v1/gateway/*`, `labby mcp` → `gateway` tool) manage the upstream MCP
server registry.  **They can spawn arbitrary local stdio commands with labby's full process
environment.**  The implications are non-negotiable:

### HTTP surface (`labby serve`)

1. **`/v1/gateway` is never mounted when auth is not configured.**
   `api/router.rs` refuses to mount the gateway route group when neither a static bearer token
   (`LAB_MCP_HTTP_TOKEN`) nor OAuth state (`LAB_AUTH_MODE=oauth`) is configured.  The startup
   warning emitted is:
   ```
   gateway service routes not mounted: HTTP API has no auth configured
   ```

2. **All gateway actions except `help` and `schema` require the `lab:admin` scope.**
   This is enforced by `ActionSpec.requires_admin` in `dispatch/gateway/catalog.rs` — the
   single source of truth.  Both the API handler (`api/services/gateway.rs`) and the MCP
   gate (`mcp/context.rs`) read directly from this catalog field; there are no separate
   bespoke match arms that can drift out of sync.

3. **Requests with no `AuthContext` are denied admin actions.**
   On the HTTP surface, a missing `Authorization` header yields no `AuthContext` in request
   extensions.  The gateway handler treats this as "no admin scope" and returns `403 Forbidden`.
   (`is_none_or(...)` would have allowed unauthenticated requests — it is NOT used here.)

4. **`~/.lab/.env` permissions are tightened at startup.**
   `cli/serve.rs` calls `heal_env_file_permissions(&env_path)` during gateway manager
   initialization to chmod `.env` and any `.env.bak.*` sibling files to `0600`, correcting
   any file that was created without strict permissions.

### MCP surface (`labby mcp`)

Stdio transport has no per-request auth by design — the parent process owns the pipes and
controls access.  `None` auth means the caller is the orchestrating process (e.g. Claude Code)
and is trusted as an operator.  Gateway admin actions are therefore allowed on stdio transport
without a scope check.

**Do not expose `labby mcp` through a network proxy without front-side authentication.**
An unauthenticated network-accessible stdio process can install arbitrary-command upstreams
via `gateway.add`.

### What operators must ensure

| Scenario | Required |
|----------|----------|
| `labby serve` on LAN / internet | `LAB_MCP_HTTP_TOKEN` or `LAB_AUTH_MODE=oauth` |
| `labby serve` on loopback only | Auth still recommended; loopback-bind is the fallback guard |
| `labby mcp` over network proxy | Front-side auth on the proxy, not in lab |
| `labby mcp` as child process | No extra config — process isolation is the boundary |

## Related Docs

- [OAUTH.md](../runtime/OAUTH.md) — bearer vs OAuth mode, registration flow, and JWT validation
- [UPSTREAM.md](../services/UPSTREAM.md) — upstream MCP proxy
- [CONFIG.md](../runtime/CONFIG.md) — env var and config.toml loading
- [MCP.md](./MCP.md) — MCP protocol surface
- [RMCP.md](./RMCP.md) — RMCP SDK integration contract
- [DEVICE_RUNTIME.md](../runtime/DEVICE_RUNTIME.md) — controller/node runtime model
