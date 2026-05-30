# Nginx includes used by SWAG configs

All of these are deployed in `/mnt/appdata/swag/nginx/` (mapped from the host SWAG appdata mount). Your config just references them via `include /config/nginx/<file>.conf` — that path is the in-container view. Do not re-author them.

## Always

| Include | Purpose |
|---|---|
| `ssl.conf` | TLS termination — listens 443, ALPN, OCSP, wildcard cert for `*.tootie.tv`. Always include at the top of the server block. |
| `proxy.conf` | Common upstream proxy headers (X-Forwarded-*, Host, Connection, WebSocket upgrade). Include inside every location block that proxies. |
| `resolver.conf` | Docker DNS resolver — required for in-network upstream resolution. Include inside every proxying location block, before `proxy_pass`. |

## MCP services (`mcp-aware`)

| Include | Purpose |
|---|---|
| `mcp-server.conf` | The "Axon Standard" server-side MCP sidecar. Provides `/.well-known/oauth-authorization-server`, `/jwks`, `/register`, `/authorize`, `/token`, `/revoke`, `/_oauth_verify`, `/health`, origin validation (`$origin_valid`), and security headers. Include at the **server level**. Requires `$upstream_*`, `$mcp_upstream_*`, and `$oauth_upstream` to be `set` before the include. |
| `mcp-location.conf` | Per-location MCP transport: zero-buffering, 24h streaming timeouts, MCP/SSE/CORS headers. Include inside `/mcp` and `/(session|sessions)` location blocks after `proxy.conf`. |

## Authentication overlays (zero or one set per server)

Pair the `-server` include at the server level with the `-location` include inside `location /`.

| Include set | When |
|---|---|
| `authelia-server.conf` + `authelia-location.conf` | App at `/` should require Authelia 2FA. Default for most services. |
| `authentik-server.conf` + `authentik-location.conf` | Alternate IdP — only when explicitly asked. |
| `tinyauth-server.conf` + `tinyauth-location.conf` | Lightweight auth — only when explicitly asked. |
| (none) | The upstream handles auth itself end-to-end (e.g. `lab`, anything with built-in OAuth). |

## Combinations seen in production

| Service | Includes at server level | Includes in `location /` |
|---|---|---|
| `syslog` | `mcp-server.conf`, `authelia-server.conf` | `authelia-location.conf`, `resolver.conf`, `proxy.conf` |
| `axon` | `mcp-server.conf`, `authelia-server.conf` | `authelia-location.conf`, `resolver.conf`, `proxy.conf` |
| `lab` | `mcp-server.conf` | `resolver.conf`, `proxy.conf` |

## Avoid

- **`oauth.conf`** — exists as a `.bak.<timestamp>` in `/mnt/appdata/swag/nginx/` from a prior architecture. Don't reintroduce. The current world is `mcp-server.conf` + `mcp-location.conf`.
- **Re-authoring any of these in-line** in your subdomain conf. Always include, never inline.
