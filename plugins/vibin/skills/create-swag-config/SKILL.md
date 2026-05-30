---
name: create-swag-config
description: 'Use whenever the user wants to add or scaffold a new reverse proxy config for SWAG — the LinuxServer.io SWAG proxy on `squirts` (`/mnt/appdata/swag/nginx/proxy-confs/`) that fronts every `*.tootie.tv` subdomain. Triggers: "create a swag config", "add a swag proxy for X", "add X to swag", "make a subdomain config", "expose X on tootie.tv", "add reverse proxy for X", "new tootie.tv subdomain", "proxy X through swag", "scaffold a swag entry", "wire up a SWAG mcp config". Always prefer the `swag-mcp` MCP server (registered as `swag` at `https://swag.tootie.tv/mcp`, exposes one action-routed tool: `swag` with actions list/create/view/edit/update/remove/logs/backups/health_check) over hand-writing files — it validates inputs, renders the canonical Jinja2 template, writes the file with the correct name, manages backups, and exposes its own health_check. Hand-write only when swag-mcp is unreachable. Does NOT trigger for nginx work outside this homelab.'
---

# create-swag-config

Add a new reverse proxy entry to **SWAG** (LinuxServer.io) on host `squirts`. SWAG fronts every `*.tootie.tv` subdomain — currently ~128 active configs. Every entry is one file at `/mnt/appdata/swag/nginx/proxy-confs/<service>.subdomain.conf` rendered from a single canonical Jinja2 template (`mcp.subdomain.conf.j2`) that supports OAuth-protected `/mcp`, optional Authelia gating on `/`, and MCP streaming semantics.

Layout on squirts:

- Repo: `~/code/swag-mcp/`
- Template: `~/code/swag-mcp/templates/mcp.subdomain.conf.j2`
- Deployed includes: `/mnt/appdata/swag/nginx/{mcp-server,mcp-location,authelia-server,authelia-location,proxy,resolver,ssl}.conf`
- Deployed configs: `/mnt/appdata/swag/nginx/proxy-confs/*.subdomain.conf`
- SWAG container: `swag` (image `lscr.io/linuxserver/swag`)
- swag-mcp container: `swag-mcp` (image `ghcr.io/jmagar/swag-mcp:<ver>`)

## Preferred path — call swag-mcp

`swag-mcp` is an MCP server that owns SWAG config CRUD. **Use it instead of hand-editing whenever possible.** It validates inputs, renders the canonical template, writes the file with the correct name, keeps backups of replaced configs, and exposes health checks. Hand-editing exists as a fallback, not a default.

The server is registered in `~/.claude.json` as `swag` (URL: `https://swag.tootie.tv/mcp`). It exposes **one** action-routed tool — `swag` — plus a `swag_help` reference tool. All operations go through `swag` with an `action` parameter.

### Actions

| Action | Purpose | Key params |
|---|---|---|
| `list` | List proxy configs | `list_filter`: `all` \| `active` \| `samples` |
| `create` | Render template + write `<config_name>` | `config_name`, `server_name`, `upstream_app`, `upstream_port`, `upstream_proto`, `auth_method`, optional `mcp_upstream_*`, `enable_quic` |
| `view` | Read an existing config's contents | `config_name` |
| `edit` | Replace full file contents | `config_name`, `new_content` |
| `update` | Patch a single field | `config_name`, `update_field` (e.g. `port`, `upstream`, `app`, `add_mcp`), `update_value` |
| `remove` | Delete a config (auto-backup) | `config_name` |
| `logs` | Tail SWAG container logs | `log_type`: `nginx-access` \| `nginx-error` \| `fail2ban`, `lines` |
| `backups` | Manage backups | `backup_action`: `list` \| `cleanup`, optional `retention_days` |
| `health_check` | HTTP check a domain | `domain` |

### Concrete call — adding `foo` on port 9000 with Authelia

```json
{
  "action": "create",
  "config_name": "foo.subdomain.conf",
  "server_name": "foo.tootie.tv",
  "upstream_app": "100.88.16.79",
  "upstream_port": 9000,
  "upstream_proto": "http",
  "auth_method": "authelia",
  "enable_quic": false
}
```

Typical first call when the user says "add X":

1. `swag` with `action: "list"`, `list_filter: "active"` — see what's already there, avoid name collisions, sanity-check similar services for the right shape.
2. `swag` with `action: "create"` and the fields below.
3. After create returns, `swag` with `action: "health_check", domain: "<service>.tootie.tv"` to confirm the new vhost responds. SWAG's filewatch picks up the file in roughly 30 seconds — if `health_check` returns a connection error immediately, wait and retry rather than restarting the container.
4. If something's wrong, `swag` with `action: "logs", log_type: "nginx-error", lines: 100` to see the parse error.

### `samples` filter

`list_filter: "samples"` returns LinuxServer-shipped reference configs (`*.subdomain.conf.sample` files for common apps — collabora, wallabag, etc.). Use them as a starting point for non-MCP services where the SWAG community already has a known-good config; `view` the sample, copy what's relevant, then `create` your version.

## Decision tree before `create`

Three real-world shapes to match (deployed configs to crib from):

| Shape | Example services | `auth_method` | Notes |
|---|---|---|---|
| **MCP service, Authelia on `/`, OAuth on `/mcp`** | `syslog`, `axon` | `authelia` | App at `/` is Authelia-gated; `/mcp` is OAuth-verified by the upstream's sidecar; well-known endpoints exposed by `mcp-server.conf` include. |
| **MCP service, upstream owns OAuth, no Authelia** | `lab` | `none` | Lab MCP server handles OAuth itself end-to-end; SWAG just forwards. |
| **Plain web app, no MCP** | most legacy services | `authelia` (or another) | The unified template still works — set `mcp_upstream_*` to the same as `upstream_*`; nothing routes through `/mcp` if the app doesn't speak it. For purely non-MCP apps, consider hand-writing from a LinuxServer sample (`list_filter: "samples"`) instead. |

Pick the shape, then collect:

- `config_name` — `<service>.subdomain.conf` (SWAG's filewatch keys on this exact pattern).
- `server_name` — almost always `<service>.tootie.tv`.
- `upstream_app` — host or IP. Tailscale IPs like `100.88.16.79` are common here; container names work when the upstream shares SWAG's Docker network.
- `upstream_port` — app port.
- `upstream_proto` — `http` or `https`. Almost always `http` (TLS terminates at SWAG).
- `mcp_upstream_app` / `_port` / `_proto` — for MCP-aware apps, point at the MCP HTTP endpoint. Often identical to the main upstream when one process serves both.
- `auth_method` — `authelia`, `authentik`, `tinyauth`, `ldap`, or `none`.
- `enable_quic` — almost always `false`. Only flip when the user explicitly asks for HTTP/3.

When the user gives a service and a port and nothing else, default to: `server_name=<service>.tootie.tv`, `upstream_proto=http`, `auth_method=authelia`, `enable_quic=false`, and set `mcp_upstream_*` equal to `upstream_*`. Confirm before writing.

## DNS, certs, and reload behavior

- **DNS is already handled.** `*.tootie.tv` is a wildcard A/CNAME pointing at the SWAG host. Adding a new subdomain needs no DNS work.
- **TLS is already handled.** SWAG holds a wildcard cert for `*.tootie.tv`; the new vhost picks it up via `include /config/nginx/ssl.conf`.
- **Reload is automatic.** LinuxServer SWAG watches `proxy-confs/` and reloads nginx within ~30 seconds of a file change. Don't manually `nginx -s reload` or restart the container unless the watcher seems broken — give it the half-minute first, then check `swag` `action: "logs"` to see whether nginx parsed the new file.

## Fallback — hand-writing

When swag-mcp is unreachable (server down, gateway errors, network partition), write the file directly. The canonical shape and the full template are in `references/fallback-template.md`. The 30-second summary: copy `lab.subdomain.conf` (or `syslog.subdomain.conf` for Authelia-gated services), change names/ports, save to `/mnt/appdata/swag/nginx/proxy-confs/<service>.subdomain.conf`, wait for filewatch, hit `/health` or the root to verify.

## Examples — annotated, taken from deployed configs

See `references/examples.md` for the three shipped patterns side-by-side (syslog, lab, axon) with the differences highlighted. Use it when you need to confirm "should this service be like syslog or like lab" without re-reading the deployed file.

## What's in each include

Header-level summary: every include file is already deployed in `/mnt/appdata/swag/nginx/`; your config just references them. Don't author them again. For a full table of which include does what and when to include which, see `references/includes.md`.

## Verification checklist

Before declaring done:

1. `swag` `action: "view", config_name: "<service>.subdomain.conf"` to confirm the file landed.
2. Wait ~30s for SWAG's filewatch.
3. `swag` `action: "health_check", domain: "<service>.tootie.tv"` — expect 200/302/401 (the 401 is Authelia redirecting unauth'd browsers, that's healthy). Hard fails: 502, 404 "Default backend", TLS hang.
4. For MCP services: hit `https://<service>.tootie.tv/.well-known/oauth-authorization-server` and confirm JSON with `issuer`, `authorization_endpoint`, `token_endpoint`.
5. If anything fails: `swag` `action: "logs", log_type: "nginx-error", lines: 100` — fix, then `update` or `edit` and re-verify.

## Conventions worth honoring

- **One file per subdomain.** Don't bundle multiple services into one `.conf`.
- **Filename = `<service_name>.subdomain.conf`** exactly. SWAG's filewatch keys on this pattern.
- **Backups are auto-managed by swag-mcp.** Don't delete `.bak`/`.backup` files manually — use `swag` `action: "backups", backup_action: "cleanup"` with `retention_days` instead.
- **Tailscale IPs are fine** as upstreams (`100.x.y.z`). They're the norm for upstreams that don't share SWAG's Docker network.
- **`client_max_body_size 0`** on the server block disables the limit — appropriate for MCP/file-upload services.
- **Don't reintroduce `oauth.conf`.** A `.bak` of it exists in `/mnt/appdata/swag/nginx/` from a prior architecture; the current world is `mcp-server.conf` + `mcp-location.conf`.

## When to NOT use this skill

- Changing auth on an *existing* working config → `update` with a field-level patch is enough; full template re-render is overkill.
- Non-tootie.tv config or a totally non-SWAG nginx — out of scope.
- Adding an upstream service itself (Docker container, Tailscale node, port-forward) — that's setup work on the upstream host, not in SWAG.
