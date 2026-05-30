# Three deployed SWAG MCP configs, annotated

Pulled from `/mnt/appdata/swag/nginx/proxy-confs/` on `squirts`. Use these as cookie-cutters — pick the shape that matches the service you're adding, then change names/ports.

## Pattern A — Authelia on `/`, MCP sidecar exposes `/mcp` (most common)

**Used by:** `syslog`, `axon`

Authelia gates the human-facing app at `/`. The MCP endpoints exposed by `mcp-server.conf` (well-known, `/jwks`, `/authorize`, `/token`, etc.) are accessible without Authelia because MCP clients authenticate themselves with bearer tokens, not browser cookies.

```nginx
## Version 2026/05/14 - Standardized
# Service: syslog-mcp
# Domain: syslog.tootie.tv
# Auth: Authelia on /

server {
    listen 443 ssl;
    listen [::]:443 ssl;
    server_name syslog.tootie.tv;
    include /config/nginx/ssl.conf;
    client_max_body_size 0;

    set $upstream_app "100.88.16.79";
    set $upstream_port "3100";
    set $upstream_proto "http";

    set $mcp_upstream_app "100.88.16.79";
    set $mcp_upstream_port "3100";
    set $mcp_upstream_proto "http";

    include /config/nginx/mcp-server.conf;
    include /config/nginx/authelia-server.conf;        # ← present

    location /mcp {
        if ($origin_valid = 0) {
            add_header Content-Type "application/json" always;
            return 403 '{"error":"origin_not_allowed","message":"Origin header validation failed"}';
        }
        include /config/nginx/resolver.conf;
        include /config/nginx/proxy.conf;
        include /config/nginx/mcp-location.conf;
        proxy_pass $mcp_upstream_proto://$mcp_upstream_app:$mcp_upstream_port;
    }

    location ~* ^/(session|sessions) {
        include /config/nginx/resolver.conf;
        include /config/nginx/proxy.conf;
        include /config/nginx/mcp-location.conf;
        proxy_pass $mcp_upstream_proto://$mcp_upstream_app:$mcp_upstream_port;
    }

    location / {
        include /config/nginx/authelia-location.conf;  # ← present
        include /config/nginx/resolver.conf;
        include /config/nginx/proxy.conf;
        proxy_pass $upstream_proto://$upstream_app:$upstream_port;
    }
}
```

Tell swag-mcp:

```
create_config({
  service_name:    "syslog",
  server_name:     "syslog.tootie.tv",
  upstream_app:    "100.88.16.79",
  upstream_port:   3100,
  upstream_proto:  "http",
  mcp_upstream_app:   "100.88.16.79",
  mcp_upstream_port:  3100,
  mcp_upstream_proto: "http",
  auth_method:     "authelia",
  enable_quic:     false,
})
```

## Pattern B — Upstream owns OAuth, no Authelia anywhere

**Used by:** `lab`

The upstream MCP server (in `lab`'s case, the lab gateway itself) implements OAuth 2.1 end-to-end and exposes its own auth UI at `/`. SWAG just terminates TLS, validates origin, and forwards everything.

Diff vs. Pattern A:
- **No** `include /config/nginx/authelia-server.conf;` at the server level
- **No** `include /config/nginx/authelia-location.conf;` inside `location /`

```nginx
## Version 2026/05/14 - Standardized
# Service: lab
# Domain: lab.tootie.tv
# Auth: OAuth (built-in to lab MCP server) — no Authelia on /

server {
    listen 443 ssl;
    listen [::]:443 ssl;
    server_name lab.tootie.tv;
    include /config/nginx/ssl.conf;
    client_max_body_size 0;

    set $upstream_app "100.88.16.79";
    set $upstream_port "8765";
    set $upstream_proto "http";

    set $mcp_upstream_app "100.88.16.79";
    set $mcp_upstream_port "8765";
    set $mcp_upstream_proto "http";

    include /config/nginx/mcp-server.conf;
    # ← no authelia-server.conf

    location /mcp {
        if ($origin_valid = 0) {
            add_header Content-Type "application/json" always;
            return 403 '{"error":"origin_not_allowed","message":"Origin header validation failed"}';
        }
        include /config/nginx/resolver.conf;
        include /config/nginx/proxy.conf;
        include /config/nginx/mcp-location.conf;
        proxy_pass $mcp_upstream_proto://$mcp_upstream_app:$mcp_upstream_port;
    }

    location ~* ^/(session|sessions) {
        include /config/nginx/resolver.conf;
        include /config/nginx/proxy.conf;
        include /config/nginx/mcp-location.conf;
        proxy_pass $mcp_upstream_proto://$mcp_upstream_app:$mcp_upstream_port;
    }

    location / {
        # ← no authelia-location.conf
        include /config/nginx/resolver.conf;
        include /config/nginx/proxy.conf;
        proxy_pass $upstream_proto://$upstream_app:$upstream_port;
    }
}
```

Tell swag-mcp:

```
create_config({
  service_name:    "lab",
  server_name:     "lab.tootie.tv",
  upstream_app:    "100.88.16.79",
  upstream_port:   8765,
  upstream_proto:  "http",
  mcp_upstream_app:   "100.88.16.79",
  mcp_upstream_port:  8765,
  mcp_upstream_proto: "http",
  auth_method:     "none",
  enable_quic:     false,
})
```

## Pattern C — Plain web app, no MCP semantics

When the service doesn't speak MCP at all (no `/.well-known/oauth-*`, no streaming `/mcp`), the cleanest thing is **don't render through `mcp.subdomain.conf.j2`** — just write a vanilla LinuxServer-style subdomain conf. swag-mcp's template adds dead routes for non-MCP apps; harmless, but noisy.

If you must use the same template (so swag-mcp can manage the file), the unused `/mcp` and `/session*` blocks just sit there returning 502s nobody calls. Set `mcp_upstream_*` equal to `upstream_*` and `auth_method` to whatever's appropriate.

For new non-MCP services, prefer hand-writing a config based on the upstream LinuxServer.io sample (`/mnt/appdata/swag/nginx/proxy-confs/<name>.subdomain.conf.sample` if one exists), or copy `lab.subdomain.conf` and strip the `/mcp` and `/session*` blocks.

## How the three differ at a glance

|  | syslog | lab | axon |
|---|---|---|---|
| `auth_method` | `authelia` | `none` | `authelia` |
| `authelia-server.conf` include | yes | no | yes |
| `authelia-location.conf` in `/` | yes | no | yes |
| `mcp-server.conf` include | yes | yes | yes |
| `mcp-location.conf` in `/mcp` and `/session*` | yes | yes | yes |
| Origin guard on `/mcp` | yes | yes | yes |
| Upstream | `100.88.16.79:3100` | `100.88.16.79:8765` | `100.88.16.79:8001` |
| OAuth-protected `/mcp`? | yes (via sidecar) | yes (upstream-owned) | yes (via sidecar) |
