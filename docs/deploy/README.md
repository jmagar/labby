# Reverse Proxy Deployment — Index

Lab exposes a single HTTP listener that serves the web UI, OAuth server,
native `/mcp`, and Gateway-managed protected MCP routes simultaneously.
A public TLS reverse proxy forwards traffic to that listener without
per-route auth handling.

## Required Proxy Behavior

All proxies must satisfy these constraints regardless of the specific tool:

- Preserve the original `Host` header (Lab uses `Host` for protected-route
  lookup; do NOT rely on `X-Forwarded-Host`).
- Set `X-Forwarded-Proto` to the original scheme.
- Forward `Authorization`, `Accept`, `Content-Type`, `Mcp-Session-Id`,
  `Mcp-Protocol-Version`, and `Last-Event-Id` headers.
- Disable request and response buffering on MCP paths (chunked/SSE traffic).
- Disable compression on MCP paths.
- Use read/write/idle timeouts suitable for long-lived Streamable HTTP and
  SSE sessions (1 hour or configurable).
- Forward `/.well-known/oauth-protected-resource/<route>` to Lab unchanged.
- Do NOT add per-route proxy locations or `oauth.conf`/`auth_request` blocks
  for MCP paths — Lab handles OAuth itself.

## Deployment Shape

```
Internet
  │
  ├── lab.example.com (HTTPS)   ─┐
  │                               ├── reverse proxy ──► Lab HTTP listener :8765
  └── mcp.example.com (HTTPS)  ─┘
```

Both hostnames point at the same Lab listener. Lab matches `Host + path` to
serve the right content and handle protected MCP route auth.

### Single-host mode

If you prefer one hostname, point `lab.example.com` at Lab for everything and
configure protected MCP routes under a distinct path prefix, e.g.
`/gateway/tools`. Both the app and the MCP route share the same proxy block.

## Examples by Proxy

| Proxy | Guide |
|-------|-------|
| nginx / SWAG | [REVERSE_PROXY.md — nginx/SWAG section](REVERSE_PROXY.md#nginx-or-swag) |
| Caddy | [REVERSE_PROXY.md — Caddy section](REVERSE_PROXY.md#caddy) |
| Traefik | [REVERSE_PROXY.md — Traefik section](REVERSE_PROXY.md#traefik) |
| Cloudflare Tunnel | [REVERSE_PROXY.md — Cloudflare Tunnel section](REVERSE_PROXY.md#cloudflare-tunnel) |
| Tailscale Funnel | [REVERSE_PROXY.md — Tailscale Funnel section](REVERSE_PROXY.md#tailscale-funnel) |

## Migrating From SWAG auth_request

If you previously protected MCP services with SWAG `auth_request` or
hand-written per-service `location` blocks, see
[SWAG_MIGRATION.md](SWAG_MIGRATION.md) for the migration shape and
what to remove.

## Verification

After deploying, run the built-in proxy doctor from any environment that
resolves the public hostnames:

```bash
labby doctor proxy \
  --app-url https://lab.example.com \
  --mcp-url https://mcp.example.com \
  --route /tools
```

This checks:

- Lab app health endpoint is reachable through the proxy.
- Protected-resource OAuth metadata is reachable and matches the configured route.
- Unauthenticated access to the protected route returns an OAuth bearer challenge.
- A wrong path returns 404 rather than a backend error or leak.

## Environment Variables

| Variable | Purpose |
|---|---|
| `LAB_PUBLIC_URL` | Public app (UI + OAuth issuer) base URL, e.g. `https://lab.example.com` |
| `LAB_MCP_GATEWAY_URL` | Separate MCP gateway base URL when hosted on its own hostname, e.g. `https://mcp.example.com`. Falls back to `LAB_PUBLIC_URL` when not set. |

Both can also be set in `config.toml` under `[public_urls]`:

```toml
[public_urls]
app = "https://lab.example.com"
mcp_gateway = "https://mcp.example.com"
```

Read the resolved values at any time:

```bash
labby gateway public-urls
```
