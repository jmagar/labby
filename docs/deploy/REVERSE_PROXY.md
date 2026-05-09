# Reverse Proxy Deployment

Lab can serve the web UI, OAuth server, native `/mcp`, and Gateway-managed
protected MCP routes from the same HTTP listener. Put your reverse proxy in
front of that listener and configure public MCP routes in Lab.

## Model

- `LAB_PUBLIC_URL` is the Lab app and OAuth issuer, for example `https://lab.example.com`.
- Each protected MCP route has its own public resource identity, for example `https://mcp.example.com/tools`.
- The reverse proxy forwards public hosts to Lab without rewriting the path.
- Lab matches `Host + path`, serves route-specific OAuth metadata, validates route-audience JWTs, and proxies accepted MCP traffic to the private backend.

## Required Proxy Behavior

- Preserve the original `Host` header.
- Set `X-Forwarded-Proto` to the original scheme.
- Forward `Authorization`, `Accept`, `Content-Type`, `Mcp-Session-Id`, `Mcp-Protocol-Version`, and `Last-Event-Id`.
- Disable request and response buffering on MCP paths.
- Disable compression on MCP paths.
- Use read/write/idle timeouts suitable for long-lived Streamable HTTP and SSE.
- Forward `/.well-known/oauth-protected-resource/<route>` to Lab.

Lab intentionally uses `Host` for protected-route lookup by default. Do not rely
on spoofable `X-Forwarded-Host` unless a future trusted-proxy mode explicitly
enables it.

## nginx or SWAG

Host-level forwarding is the portable baseline. Both `lab.example.com` and
`mcp.example.com` can point at the same Lab container/listener.

```nginx
server {
    server_name mcp.example.com;

    location / {
        proxy_pass http://labby:8765;
        proxy_http_version 1.1;

        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header Authorization $http_authorization;
        proxy_set_header Accept $http_accept;
        proxy_set_header Content-Type $content_type;
        proxy_set_header Mcp-Session-Id $http_mcp_session_id;
        proxy_set_header Mcp-Protocol-Version $http_mcp_protocol_version;
        proxy_set_header Last-Event-Id $http_last_event_id;

        proxy_buffering off;
        proxy_request_buffering off;
        gzip off;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
    }
}
```

Use the same upstream for `lab.example.com`; the app host can keep normal
buffering for static assets if your proxy lets you scope MCP streaming behavior
to `mcp.example.com`.

## Caddy

```caddyfile
mcp.example.com {
    reverse_proxy labby:8765 {
        header_up Host {host}
        header_up X-Forwarded-Proto {scheme}
        flush_interval -1
    }
}

lab.example.com {
    reverse_proxy labby:8765 {
        header_up Host {host}
        header_up X-Forwarded-Proto {scheme}
    }
}
```

## Traefik

```yaml
http:
  routers:
    lab-app:
      rule: Host(`lab.example.com`)
      entryPoints: [websecure]
      service: labby
      tls: {}
    lab-mcp:
      rule: Host(`mcp.example.com`)
      entryPoints: [websecure]
      service: labby
      tls: {}
  services:
    labby:
      loadBalancer:
        passHostHeader: true
        servers:
          - url: http://labby:8765
```

Avoid compression and buffering middleware on the MCP router.

## Cloudflare Tunnel

Create public hostnames for the app and MCP gateway that both target Lab:

```yaml
ingress:
  - hostname: lab.example.com
    service: http://labby:8765
  - hostname: mcp.example.com
    service: http://labby:8765
  - service: http_status:404
```

Do not place an Access policy in front of the MCP route unless it is compatible
with MCP OAuth clients. Lab needs to return its own OAuth metadata and bearer
challenge.

## Tailscale Funnel

Expose Lab's HTTP listener through Funnel for each public hostname you use, or
put Funnel in front of a local reverse proxy that preserves `Host` and forwards
to Lab. Keep the public route path intact.

## Verification

Run the built-in proxy doctor from any environment that resolves the public
hosts:

```bash
just protected-mcp-smoke -- \
  --app-url https://lab.example.com \
  --mcp-url https://mcp.example.com \
  --route /tools
```

The `just` target wraps `scripts/protected-mcp-smoke`, which delegates to
`labby doctor proxy`. Use `LABBY_BIN=/path/to/labby` or `--labby-bin
/path/to/labby` when testing a specific binary.

The check verifies app health, route-specific protected-resource metadata, and
the expected unauthenticated OAuth bearer challenge on the protected route.
