# Hand-write fallback

Use only when swag-mcp is unreachable (container down, gateway errors, network partition). Otherwise call `swag` (`action: "create"`) — it does this for you.

## The real baseline: LinuxServer's `_template.subdomain.conf.sample`

The authoritative baseline lives on squirts at `/mnt/appdata/swag/nginx/proxy-confs/_template.subdomain.conf.sample`. It ships with the SWAG container, gets updated by LinuxServer, and is the right starting point for any **non-MCP** service. Read it first before writing anything:

```bash
ssh squirts cat /mnt/appdata/swag/nginx/proxy-confs/_template.subdomain.conf.sample
```

(Or via swag-mcp: `swag` `action: "list", list_filter: "samples"` then `action: "view"` on the entry.)

The baseline shape — strip the `# REMOVE THIS LINE BEFORE SUBMITTING` comments and you get:

```nginx
## Version 2025/07/18

server {
    listen 443 ssl;
#    listen 443 quic;
    listen [::]:443 ssl;
#    listen [::]:443 quic;

    server_name <container_name>.*;

    include /config/nginx/ssl.conf;

    client_max_body_size 0;

    # Uncomment ONE of these to gate / with auth:
    #include /config/nginx/ldap-server.conf;
    #include /config/nginx/authelia-server.conf;
    #include /config/nginx/authentik-server.conf;
    #include /config/nginx/tinyauth-server.conf;

    location / {
        # Uncomment the matching auth-location include for whichever auth-server you enabled above:
        #include /config/nginx/ldap-location.conf;
        #include /config/nginx/authelia-location.conf;
        #include /config/nginx/authentik-location.conf;
        #include /config/nginx/tinyauth-location.conf;

        include /config/nginx/proxy.conf;
        include /config/nginx/resolver.conf;
        set $upstream_app <container_name>;
        set $upstream_port <port_number>;
        set $upstream_proto <http or https>;
        proxy_pass $upstream_proto://$upstream_app:$upstream_port;
    }
}
```

Things worth understanding before you change anything:

- **`server_name <container_name>.*`** is the LSIO wildcard convention — it matches `<container_name>` under any base domain (so the same config works across multiple registered domains). The deployed configs on this host use the literal FQDN (`<service>.tootie.tv`) instead. Either form works; if you stick to LSIO style, use the wildcard; if you want to mirror what `syslog`/`lab`/`axon` look like, use the literal FQDN.
- **`set $upstream_*` lives INSIDE `location /`** in the LSIO baseline. That's correct for single-location services. The deployed MCP configs hoist these into the server block because multiple location blocks share them — that's an MCP-specific deviation, not the default.
- **Auth includes are commented out.** Uncomment the pair (server-level + matching location-level) for the auth provider you want. Pick zero or one.
- **`client_max_body_size 0`** disables the upload limit — keep it unless you have a reason to cap.
- **QUIC lines are commented.** Leave them off unless the user explicitly wants HTTP/3.

## MCP services — diff from the baseline

The deployed `syslog` / `lab` / `axon` configs add a few things on top of the LSIO baseline. If you're adding an MCP-aware service, layer these in:

1. **Hoist `set $upstream_*` to the server block.** And add a parallel `set $mcp_upstream_*` block. Multiple locations need these vars.
2. **Use the literal FQDN** for `server_name` (`<service>.tootie.tv`), not the LSIO wildcard. Convention here.
3. **Add `include /config/nginx/mcp-server.conf;`** at the server level for the Axon Standard sidecar (well-known endpoints, `/_oauth_verify`, `/health`, origin validation, security headers).
4. **Add a `location /mcp` block** with the origin guard, `mcp-location.conf` include, and `proxy_pass` to `$mcp_upstream_*`.
5. **Add a `location ~* ^/(session|sessions)` block** that mirrors `/mcp` (also streaming, same includes).
6. **Keep `location /`** for the human-facing app (with the auth-location include if applicable).

See `references/examples.md` for the result.

## When you're hand-writing

1. Decide MCP-aware vs plain web.
2. Plain web → copy `_template.subdomain.conf.sample`, replace tags. Done.
3. MCP-aware → copy a deployed config (`lab.subdomain.conf` for OAuth-owned services, `syslog.subdomain.conf` or `axon.subdomain.conf` for Authelia-gated), change names/ports. Done.
4. Save to `/mnt/appdata/swag/nginx/proxy-confs/<service>.subdomain.conf`.
5. Wait ~30s for SWAG's filewatch. If you can't wait or want fast feedback on parse errors:
   ```bash
   ssh squirts 'docker exec swag nginx -t && docker exec swag nginx -s reload'
   ```
6. Verify: `curl -sSI https://<service>.tootie.tv`. Tail `docker logs swag --tail 100` on parse errors.
