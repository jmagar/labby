# Environment Variables

This document lists the `lab` environment variables that matter for transport
and auth setup. The complete per-service env inventory is generated from
`PluginMeta` and lives in
[generated/env-reference.md](./generated/env-reference.md) and
[generated/env-reference.json](./generated/env-reference.json).

## HTTP Auth

Bearer mode:

```env
LAB_AUTH_MODE=bearer
LAB_MCP_HTTP_TOKEN=replace-me
```

OAuth mode:

```env
LAB_AUTH_MODE=oauth
LAB_PUBLIC_URL=https://lab.example.com
LAB_GOOGLE_CLIENT_ID=google-client-id
LAB_GOOGLE_CLIENT_SECRET=google-client-secret
LAB_AUTH_ADMIN_EMAIL=admin@example.com
```

Optional auth overrides:

```env
LAB_AUTH_SQLITE_PATH=/var/lib/labby/auth.db
LAB_AUTH_KEY_PATH=/var/lib/labby/auth-jwt.pem
LAB_AUTH_ALLOWED_REDIRECT_URIS=https://callback.example.com/callback/*
LAB_GOOGLE_CALLBACK_PATH=/auth/google/callback
LAB_GOOGLE_SCOPES=openid,email,profile
LAB_AUTH_ACCESS_TOKEN_TTL_SECS=3600
LAB_AUTH_REFRESH_TOKEN_TTL_SECS=2592000
LAB_AUTH_CODE_TTL_SECS=300
```

These non-secret overrides can also live in `config.toml` under `[auth]`.

Rules:

- `LAB_AUTH_MODE` defaults to `bearer`
- bearer mode keeps using `LAB_MCP_HTTP_TOKEN`
- oauth mode requires `LAB_PUBLIC_URL`, `LAB_GOOGLE_CLIENT_ID`, `LAB_GOOGLE_CLIENT_SECRET`, and `LAB_AUTH_ADMIN_EMAIL`
- `LAB_AUTH_ADMIN_EMAIL` is the bootstrap admin Google email; startup fails closed if unset under oauth mode so no Google account can authenticate without explicit permission. Future SQLite-backed allowlist (web-UI managed) will grant access to additional users.
- the old external issuer variables (`LAB_OAUTH_ISSUER`, `LAB_OAUTH_AUDIENCE`, `LAB_OAUTH_CLIENT_ID`) are no longer used
- `LAB_PUBLIC_URL` also feeds RFC 9728 metadata, JWT issuer/audience, and HTTP allowed-host derivation

## Service Environment Variables

Service credentials follow the standard pattern `{SERVICE}_URL`,
`{SERVICE}_API_KEY`, `{SERVICE}_TOKEN`, `{SERVICE}_USERNAME`, and
`{SERVICE}_PASSWORD`, with service-specific exceptions declared in
`PluginMeta`.

Named instances insert the label before the suffix, for example:

```env
JELLYFIN_NODE2_URL=http://node2.local:8096
JELLYFIN_NODE2_API_KEY=replace-me
OPENACP_NODE2_URL=http://node2.local:21420
OPENACP_NODE2_TOKEN=replace-me
```

Use [generated/env-reference.md](./generated/env-reference.md) for the current
required/optional env var matrix, default ports, secret flags, and examples.

## Provisioning Environment

`labby setup --provision` and `scripts/incus-bootstrap.sh` also honor:

```env
TS_AUTHKEY=tskey-auth-...
```

When set, provisioning installs Tailscale and joins the host/container to the
tailnet using `tailscale up --auth-key=file:/run/labby-ts-authkey`. The key is
written only to a root-owned runtime file for the join, then removed. Leave it
unset to skip Tailscale join.
