# HTTP Auth Modes

Lab supports two HTTP auth modes:

- `LAB_AUTH_MODE=bearer`
  Preserve the existing static bearer-token flow with `LAB_MCP_HTTP_TOKEN`.
- `LAB_AUTH_MODE=oauth`
  Run an internal Google-backed OAuth authorization server that issues `lab` JWT access tokens and exposes JWKS plus RFC 9728 metadata.

This document covers mode selection, startup behavior, registration and token flow, JWT validation, and operator-facing constraints.
For the complete generated route/auth matrix, see
[generated/api-routes.md](./generated/api-routes.md).

## Configuration

OAuth mode is configured through env vars and/or `config.toml`. Env vars take precedence over config file values.

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `LAB_AUTH_MODE` | no | `bearer` or `oauth`. Defaults to `bearer`. |
| `LAB_MCP_HTTP_TOKEN` | bearer mode | Static bearer token for protected HTTP routes. |
| `LAB_PUBLIC_URL` | oauth mode | Public base URL for metadata, callback construction, and JWT issuer/audience. Path-prefixed deployments are supported. |
| `LAB_GOOGLE_CLIENT_ID` | oauth mode | Google OAuth client ID. |
| `LAB_GOOGLE_CLIENT_SECRET` | oauth mode | Google OAuth client secret. |
| `LAB_AUTH_SQLITE_PATH` | no | Override path for the SQLite auth database. |
| `LAB_AUTH_KEY_PATH` | no | Override path for the persisted JWT signing key. |
| `LAB_AUTH_ALLOWED_REDIRECT_URIS` | no | Comma-separated redirect URI patterns allowed for dynamic client registration in addition to loopback callbacks. |
| `LAB_AUTH_ADMIN_EMAIL` | oauth mode | Google email address of the bootstrap admin permitted to log in. Normalized to lowercase at startup. **Required** when `LAB_AUTH_MODE=oauth`: startup fails if unset so no Google account can authenticate unless explicitly permitted. The `email_verified` claim in Google's id_token is enforced — accounts with unverified email addresses are rejected even if the address matches. Additional users will be granted access through a SQLite-backed allowlist managed via the web UI (planned). |
| `LAB_GOOGLE_CALLBACK_PATH` | no | Callback path appended to `LAB_PUBLIC_URL`. Defaults to `/auth/google/callback`. |
| `LAB_GOOGLE_SCOPES` | no | Comma-separated Google scopes. Defaults to `openid,email,profile`. |
| `LAB_AUTH_REGISTER_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `POST /register`. Defaults to `20`. |
| `LAB_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `/authorize` and browser login initiation. Defaults to `60`. |
| `LAB_AUTH_MAX_PENDING_OAUTH_STATES` | no | Maximum non-expired pending authorization + browser-login states stored at once. Defaults to `1024`. |

## Startup Behavior

When OAuth mode is configured, `labby serve` performs these steps at startup:

1. Validate that `LAB_PUBLIC_URL`, Google credentials, and `LAB_AUTH_ADMIN_EMAIL` are present.
2. Open the SQLite auth store in WAL mode with a non-zero busy timeout.
3. Load or generate the persisted RSA signing key.
4. Build the concrete Google provider callback URL from `LAB_PUBLIC_URL` and `LAB_GOOGLE_CALLBACK_PATH`.

Startup fails closed if any of those steps fail.

Startup also fails if:

- `LAB_AUTH_MODE=oauth` is set without `LAB_PUBLIC_URL`
- Google client credentials are missing
- `LAB_AUTH_ADMIN_EMAIL` is missing — fail-closed default so no Google account can authenticate without explicit permission
- the auth database or signing key has insecure file permissions

## Registration and Authorize Flow

OAuth mode exposes:

- `POST /register`
- `GET /authorize`
- `GET /auth/google/callback`
- `POST /token`

Registration rules in the initial launch:

- loopback redirect URIs are always accepted
- optional non-loopback redirect URI patterns can be allowed with `LAB_AUTH_ALLOWED_REDIRECT_URIS` or `[auth].allowed_client_redirect_uris`
- unlisted public HTTPS redirect URIs are rejected
- `POST /register`, `/authorize`, and hosted browser-login initiation are process-locally rate limited
- new login/authorization state is rejected once the pending non-expired state cap is reached

Flow summary:

1. A client registers a loopback redirect URI or one that matches the configured allowlist.
2. The client sends the user to `/authorize` with `response_type=code`.
3. `lab` stores the request state, generates PKCE data, and redirects to Google.
4. Google redirects back to `/auth/google/callback`.
5. `lab` enforces the email allowlist (currently `LAB_AUTH_ADMIN_EMAIL`; expanding to a SQLite-backed user list managed via the web UI). The id_token's `email_verified` claim is required — unverified accounts are rejected even when the address matches. Browser-login callers receive a 401; OAuth-client callers receive an RFC 6749 §4.1.2.1 redirect with `error=access_denied`.
6. `lab` exchanges the Google code server-side, stores a local authorization code, and redirects the client back to its registered redirect URI with the local code.
7. The client exchanges that local code at `/token` for a `lab` access token and, when Google granted offline access, a `lab` refresh token.

Google access and refresh tokens remain server-side only.

Google-specific notes:

- `lab` sends `access_type=offline` when redirecting to Google so the provider can issue a refresh token
- `lab` also sends `prompt=consent` so a fresh Google consent flow can return a new refresh token after the app was previously authorized without offline access
- if Google still does not return an upstream refresh token, `lab` omits `refresh_token` from its token response and later refresh grants fail closed
- `lab` validates the Google `id_token` cryptographically against Google JWKS and rejects tokens with the wrong issuer, audience, or expiry before minting any local identity

## Browser-Local Callback Forwarding

`lab` also ships a local OAuth callback forwarder for browser-side machines:

```bash
labby oauth relay-local --machine dookie --port 38935
labby oauth relay-local --forward-base http://100.88.16.79:38935/callback/dookie --port 38935
```

This helper exists for cases where:

- the browser receives a loopback redirect on one machine
- the actual OAuth client callback listener is running on another machine
- you need to forward the final callback request without reimplementing the OAuth flow

Important constraints:

- `relay-local` binds only to `127.0.0.1:<port>` on the browser machine
- it forwards only the final callback request
- it forwards only a callback-safe header allowlist; `Cookie`,
  `Authorization`, and similar ambient credentials are stripped
- it mirrors only a callback-safe response header allowlist; `Set-Cookie` and
  other credential-bearing response headers are not relayed back through the
  localhost helper
- it does not mint tokens, store PKCE state, or complete the OAuth exchange itself
- the real client listener must already be running and reachable before the callback arrives

## Node Runtime Relay Start

The same local relay can be started remotely on a fleet node through:

```http
POST /v1/nodes/oauth/relay/start
```

Example body:

```json
{
  "bind_addr": "127.0.0.1:38935",
  "target_url": "http://100.88.16.79:38935/callback/dookie",
  "default_port": 38935,
  "request_timeout_ms": 30000
}
```

This reuses the existing local relay implementation. It does not change OAuth token issuance or PKCE handling.

In the current v1 trust model, this endpoint is intended for controller-orchestrated node runtime traffic on the tailnet. It is not exposed as a public operator surface on non-controller nodes; the controller invokes it after authenticating to the target node with the same shared bearer/OAuth controls that protect the rest of `/v1/*`.

### Using non-loopback redirect URIs

Loopback redirect URIs are always accepted by `lab-auth`. Public or non-loopback redirect URIs are
rejected unless they match an allowlisted pattern.

Configure extra allowed redirect URI patterns with either:

- `LAB_AUTH_ALLOWED_REDIRECT_URIS`
- `[auth].allowed_client_redirect_uris`

Example:

```env
LAB_AUTH_ALLOWED_REDIRECT_URIS=https://callback.tootie.tv/callback/*
```

```toml
[auth]
allowed_client_redirect_uris = ["https://callback.tootie.tv/callback/*"]
```

Patterns are matched as structured URLs, not raw substrings:

- scheme and port must match exactly
- host wildcards are allowed only as full labels, e.g. `https://*.example.com/callback` or `https://callback.*.tv/callback/*`
- path and query may use simple `*` wildcards
- partial host-label globs such as `https://callback.example.com*` are rejected and do not safely scope a trust boundary

Use this only for redirect URIs you explicitly operate or trust.

## Runtime JWT Validation

Every request to a protected route (`/v1/*`, `/mcp`) must include an `Authorization: Bearer <token>` header.

Validation steps:

1. Decode the JWT header to extract the `kid` (key ID).
2. Look up the signing key in the cached JWKS.
3. If the `kid` is unknown, trigger an eager JWKS refresh (see caching below).
4. Validate the JWT signature using one of the supported algorithms.
5. Validate the `iss` claim matches the configured issuer.
6. Validate the `aud` claim matches the configured audience.
7. Extract scopes from the `scope` claim (space-separated string) or the `scp` claim (JSON array).

### Supported Algorithm

- RS256

### Scopes

Current `lab` tokens use the standard space-delimited `scope` claim.

### AuthContext

On successful validation, an `AuthContext` is injected into the request extensions:

- `sub` — the authenticated user/client identifier from the `sub` claim.
- `scopes` — granted scopes.
- `issuer` — token issuer.

Downstream handlers can read `AuthContext` from request extensions for audit trails and scope-gated access.

## Token Exchange

`POST /token` supports:

- `grant_type=authorization_code`
- `grant_type=refresh_token`

Current constraints:

- authorization-code redemption is atomic and single-use
- `refresh_token` is only issued when Google returned an upstream refresh token
- refresh grants are rejected if the local token is not backed by an upstream refresh token
- refresh tokens do not rotate in this batch
- `/revoke` is not implemented in this batch
- successful and failed `/token` responses must send `Cache-Control: no-store`
  and `Pragma: no-cache`

### Auth Failure Semantics

`lab` distinguishes unauthenticated callers from internal auth outages.

Rules:

- `/auth/session` returns an unauthenticated result only when the request truly
  lacks a valid session
- auth store, signing-key, provider, or persistence failures stay 5xx-class and
  use canonical error envelopes
- `/auth/logout` failures are surfaced as structured errors rather than being
  treated as best-effort success
- provider-facing logs must preserve stable `kind` classification when transport,
  status, decode, or grant failures happen

Browser-session introspection semantics:

- `GET /auth/session` returns `200` with `authenticated: false` only for a true
  logged-out outcome
- the same payload includes `login_available` so browser clients can suppress
  the hosted-login CTA when OAuth browser login is not configured
- a request that carries `Authorization: Bearer <LAB_MCP_HTTP_TOKEN>` is treated
  as an authenticated admin caller and gets `authenticated: true` with
  `sub: "static-bearer"`, `is_admin: true`, and an empty `csrf_token` (CSRF is
  unnecessary for bearer-authenticated requests). This is the bridge that lets
  automation tooling (e.g. `agent-browser --headers`) drive the UI alongside
  OAuth browser users without the flag-and-disable dance
- internal failures from session lookup, persistence, signing, or provider
  coordination remain structured 5xx responses instead of collapsing into
  `authenticated: false`

### Frontend Expectations

The web UI and server-side frontend adapter must treat auth state as a three-way
 distinction:

- `loading`
- `unauthenticated`
- `auth_error`

They must also:

- capture response `x-request-id` values on failures
- avoid showing a hosted-login CTA unless hosted login is actually available
- invalidate or refresh cached session state when later requests fail with
  `auth_failed` or a CSRF-style `validation_failed` response
- not treat unrelated validation failures as implicit logout/session-expiry events

### OAuth Error Kinds

Most auth-route failures use the canonical error envelope described in
`docs/ERRORS.md`.

Documented auth-specific exception:

- `invalid_grant` remains a stable OAuth token/authorization error for
  authorization-code and refresh-token redemption failures such as expired,
  unknown, or mismatched grants

## RFC 9728 Protected Resource Metadata

Lab exposes a metadata endpoint so MCP clients can discover which authorization server to use:

```http
GET /.well-known/oauth-protected-resource
```

This endpoint is **unauthenticated** — clients need it before they have a token.

Response:

```json
{
  "resource": "https://lab.example.com",
  "authorization_servers": ["https://lab.example.com"],
  "scopes_supported": ["lab"],
  "bearer_methods_supported": ["header"]
}
```

### WWW-Authenticate Header

When a request fails authentication (401), the response includes:

```http
WWW-Authenticate: Bearer resource_metadata="https://lab.example.com/.well-known/oauth-protected-resource"
```

This header is only included when `LAB_PUBLIC_URL` is configured. If not, the header is omitted rather than advertising localhost.

## Auth Precedence

When both static bearer and OAuth are configured, auth is checked in this order:

1. **Static bearer token** — constant-time comparison via `LAB_MCP_HTTP_TOKEN`. If it matches, the request is authenticated with implicit `lab:read` and `lab:admin` scopes.
2. **OAuth JWT** — if the static bearer check fails (or no static token is configured), the token is validated as a JWT against the cached JWKS. OAuth-issued tokens currently carry the single supported scope `lab`.
3. **401** — if both checks fail (or neither auth method is configured for the token presented).

Static bearer tokens bypass all JWT validation. This allows operators to use a simple token for automation while also supporting OAuth for interactive or multi-tenant use.

For node runtime background traffic, the supported auth path in this implementation is the shared static bearer token when `LAB_MCP_HTTP_TOKEN` is configured.

## Safety Gate

Lab refuses to bind on a non-localhost address without any auth configured:

```text
refusing to bind HTTP on 0.0.0.0:8765 without authentication.
Set LAB_MCP_HTTP_TOKEN or LAB_AUTH_MODE=oauth, or bind to 127.0.0.1 for local-only access.
```

Loopback hosts exempt from this check: `127.0.0.1`, `::1`, `[::1]`, `localhost`.

## Example: Deploying with OAuth

```bash
# In ~/.lab/.env
LAB_MCP_TRANSPORT=http
LAB_MCP_HTTP_HOST=0.0.0.0
LAB_MCP_HTTP_PORT=8765
LAB_AUTH_MODE=oauth
LAB_PUBLIC_URL=https://lab.example.com
LAB_GOOGLE_CLIENT_ID=google-client-id
LAB_GOOGLE_CLIENT_SECRET=google-client-secret

# Start
labby serve
```

Verify the metadata endpoint:

```bash
curl https://lab.example.com/.well-known/oauth-protected-resource
```

Call a protected endpoint with a `lab` access token:

```bash
curl -H "Authorization: Bearer eyJhbG..." \
     https://lab.example.com/v1/radarr \
     -d '{"action":"help"}'
```

## Verifying Auth Configuration

Two complementary verification surfaces exist:

### External probe — `scripts/check-oauth.sh`

An operator shell script that tests a **running server** from outside, using only `curl`. Useful after deploy, in CI pipelines, or from a remote machine.

```bash
# Auto-loads ~/.lab/.env; defaults to http://localhost:8080
./scripts/check-oauth.sh

# Point at a specific server
./scripts/check-oauth.sh https://lab.example.com

# Or via env var
LAB_BASE_URL=https://lab.example.com ./scripts/check-oauth.sh
```

The script covers:

- Config presence (`LAB_MCP_HTTP_TOKEN`, `LAB_PUBLIC_URL`, Google credentials, `LAB_WEB_UI_AUTH_DISABLED`)
- Health probes reachable without auth (`/health`, `/ready`)
- Protected endpoints return `401 {kind:auth_failed}` when unauthenticated (`/v1/*`, `/mcp`, `/v0.1/servers`)
- Static bearer token accepted and wrong tokens rejected
- MCP endpoint is bearer-only (session cookies rejected)
- OAuth discovery endpoints are public and structurally valid (`/.well-known/oauth-authorization-server`, `/.well-known/oauth-protected-resource`, `/jwks`)
- Issuer in `/.well-known/oauth-authorization-server` matches `LAB_PUBLIC_URL`
- `WWW-Authenticate: Bearer resource_metadata=...` header present on 401 (RFC 9728)
- Dev marketplace endpoint is unauthenticated for reads, blocked for mutations
- Node self-registration endpoints are public
- Upstream OAuth browser callback is not behind bearer auth

Exit codes: `0` = all pass, `1` = one or more failures.

### Internal pre-flight — `labby doctor`

`labby doctor` is the in-process health audit. It checks config validity, file permissions on `auth.db` and `auth-jwt.pem`, service reachability, and auth configuration before you have a running server to probe. Use the shell script for post-deploy black-box verification; use `labby doctor` for pre-flight and service-level health.

Auth-specific items `labby doctor` covers (or should cover):

- `LAB_PUBLIC_URL` is set when OAuth mode is active
- Google credentials present
- `auth.db` and `auth-jwt.pem` exist and have restrictive permissions (`0600`)
- SQLite store is openable (WAL mode, non-zero busy timeout)
- Signing key is loadable

## Related Docs

- [CONFIG.md](./CONFIG.md) — config loading and env var conventions
- [TRANSPORT.md](./TRANSPORT.md) — HTTP transport setup and middleware
- [ERRORS.md](./ERRORS.md) — `auth_failed` error kind
- [RMCP.md](./RMCP.md) — RMCP auth ownership contract
