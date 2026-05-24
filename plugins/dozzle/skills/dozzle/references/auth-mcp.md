# Dozzle Auth and MCP

Use this reference when the user asks whether Dozzle is safe, how to configure
Dozzle authentication, or how Dozzle MCP should be exposed.

## Auth Providers

Dozzle auth is configured with `--auth-provider` or `DOZZLE_AUTH_PROVIDER`.

### Simple Auth

Use `simple` when Dozzle should manage users itself.

```bash
DOZZLE_AUTH_PROVIDER=simple
```

Generate a users file with bcrypt-hashed passwords:

```bash
docker run -it --rm amir20/dozzle generate admin \
  --password '<password>' \
  --email '<email>' \
  --name '<name>' > users.yml
```

Mount the users file into Dozzle's data path, commonly `/data/users.yml`, and
mount persistent `/data` storage. Simple auth supports user roles and filters;
use roles such as `shell`, `actions`, and `download` only for users who need
those capabilities.

### Forward-Proxy Auth

Use `forward-proxy` when Authelia, Cloudflare Access, oauth2-proxy, or another
reverse proxy handles login and 2FA.

```bash
DOZZLE_AUTH_PROVIDER=forward-proxy
```

Dozzle maps identity from proxy headers. Configure these when the proxy uses
non-default names:

```bash
DOZZLE_AUTH_HEADER_USER
DOZZLE_AUTH_HEADER_EMAIL
DOZZLE_AUTH_HEADER_NAME
DOZZLE_AUTH_HEADER_FILTER
DOZZLE_AUTH_HEADER_ROLES
DOZZLE_AUTH_LOGOUT_URL
```

When forward-proxy auth is used, verify there is no direct URL that bypasses
the proxy unless the network layer is trusted and restricted, for example by
Tailscale ACLs or firewall rules.

## MCP

Dozzle MCP is disabled by default. Enable it with:

```bash
DOZZLE_ENABLE_MCP=true
```

The endpoint is:

```text
/api/mcp
```

If Dozzle is mounted under a base path, include that base path before
`/api/mcp`.

This Lab plugin installs a client entry for Dozzle's native Streamable HTTP MCP
server:

```json
{
  "mcpServers": {
    "dozzle": {
      "type": "http",
      "url": "${userConfig.dozzle_mcp_url}"
    }
  }
}
```

Set `dozzle_mcp_url` to the reachable endpoint, for example
`http://localhost:8080/api/mcp` or `https://dozzle.example.com/api/mcp`. Do not
point this at a Lab command-wrapper MCP entry; that is not Dozzle's native MCP
server.

Dozzle documents its MCP tools as read-only. Do not conflate Dozzle MCP access
with the separate shell/actions/download web capabilities.

Authentication behavior:

- With `simple` auth, MCP clients need a valid JWT token in
  `Authorization: Bearer <token>`. The token is obtained from Dozzle's token
  endpoint after authenticating.
- With `forward-proxy` auth, MCP clients should connect through the same proxy
  so the proxy can authenticate the request and inject identity headers.
- With no auth provider, `/api/mcp` is publicly accessible to any caller that
  can reach the Dozzle URL.

Do not configure a client to use `/api/mcp` unless the user asks for Dozzle MCP
or an MCP-capable client integration.

## Security Checks

Before saying a Dozzle deployment is protected, verify:

- The public URL is behind the intended auth provider or proxy.
- Direct Tailnet, LAN, or localhost URLs are either trusted or blocked by ACLs.
- `authProvider` from `config__json` matches expectations.
- Shell/actions/download are enabled only when intended.
- Docker socket mounts are read-only when write access is not required.
