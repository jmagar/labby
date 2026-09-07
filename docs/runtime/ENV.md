---
title: "Environment Variables"
created: "2026-07-30"
updated: "2026-08-01"
---

# Environment Variables

This document lists the `labby` environment variables that matter for transport
and auth setup. The complete per-service env inventory is generated from
`PluginMeta` and lives in
[generated/env-reference.md](../generated/env-reference.md) and
[generated/env-reference.json](../generated/env-reference.json).

## State Root

`LABBY_HOME` selects Labby's durable state root and must be absolute. With an
explicit value, configuration is read from `$LABBY_HOME/config.toml`, dotenv
values from `$LABBY_HOME/.env`, and the access-control database from the fixed
path `$LABBY_HOME/access.db`. Without it, the durable state root defaults to
`~/.labby`. Do not use a relative working-directory path for daemon or stdio
launches.

An explicit `LABBY_HOME` is exclusive: Labby does not consult a conflicting
current-directory `config.toml` or `.env`, and gateway credential writes use
the same selected root. Without an explicit root, the same files live under
`~/.labby`; current-directory files are not implicit overrides. Selected dotenv
files fail visibly on read or parse errors rather than producing partial
settings state.

The fixed `labby.service` lifecycle preflight resolves its port only from
`/home/labby/.labby/.env`, then `/home/labby/.labby/config.toml`, then the
built-in `8765` default. An invoking administrator's `LABBY_MCP_HTTP_PORT`,
`HOME`, or `LABBY_HOME` is not inherited by systemd and cannot influence the
install/restart collision check.

The access store has no independent environment override.
`LABBY_AUTH_SQLITE_PATH` selects the OAuth authorization store, not
`access.db`. A standalone stdio fallback uses its own resolved state root, so
configure an explicit remote daemon target when stdio must share the daemon's
project and membership state.

## Depot Discovery Credentials

Named Depot discovery providers reference host-managed `LABBY_DEPOT_*_TOKEN`
keys from TOML. These keys follow the normal process-over-dotenv precedence;
their values stay server-side. Public Depot has a fixed endpoint and no token
override. Discovery configuration is independent of exact-acquisition source
credentials.

The legacy keys are `LABBY_DEPOT_URL`, `LABBY_DEPOT_ENABLED`, and
`LABBY_DEPOT_TOKEN`. The discovery configuration normalizer distinguishes an
absent enable flag from explicit disable and requires a token for an enabled
legacy URL. A persisted migration marker or removal tombstone takes precedence
over legacy environment normalization. See [CONFIG.md](CONFIG.md#depot-discovery-configuration).

## Direct Stdio Proxy

The default bearer secret is separate from the hosted daemon administrator
token:

```env
LABBY_PROXY_BEARER_TOKEN=replace-with-a-generated-secret
```

`proxy.bearer_token_env` may name another key. `labby setup proxy --auth
bearer` generates and writes the value when it is absent; piping a value to
`--bearer-token-stdin` replaces it without writing the literal to TOML.

Other proxy-adjacent process controls are:

```env
LABBY_TAILSCALE_BIN=tailscale
LABBY_GW_UPSTREAM_STDERR=debug
```

`LABBY_PROXY_TEST_RENEW_MS` exists only under the `proxy-testkit` feature and
is not production configuration. OAuth proxy runs also use the live-daemon
discovery/auth variables described below. There are no environment aliases for
the non-secret `[proxy]` exposure, auth, path, port/range, scope, inheritance,
or shutdown preferences. See the
[stdio MCP proxy guide](../guides/STDIO_MCP_PROXY.md#configuration-and-precedence)
and generated [proxy environment inventory](../generated/env-reference.md).

## HTTP Auth

Bearer mode:

```env
LABBY_AUTH_MODE=bearer
LABBY_MCP_HTTP_TOKEN=replace-me
```

OAuth mode:

```env
LABBY_AUTH_MODE=oauth
LABBY_PUBLIC_URL=https://lab.example.com
LABBY_GOOGLE_CLIENT_ID=google-client-id
LABBY_GOOGLE_CLIENT_SECRET=google-client-secret
LABBY_AUTH_ADMIN_EMAIL=admin@example.com
```

Authelia alternative (open beta; do not configure Google credentials at the same time):

```env
LABBY_AUTH_MODE=oauth
LABBY_AUTH_PROVIDER=authelia
LABBY_PUBLIC_URL=https://lab.example.com
LABBY_AUTHELIA_ISSUER_URL=https://auth.example.com
LABBY_AUTHELIA_CLIENT_ID=labby
LABBY_AUTHELIA_CLIENT_SECRET=replace-me
# Optional only when the exact issuer uses a private CA:
# LABBY_AUTHELIA_TRUSTED_PRIVATE_ORIGIN=https://auth.example.com
# LABBY_AUTHELIA_CA_CERT_PATH=/etc/labby/authelia-ca.pem
LABBY_AUTH_ADMIN_EMAIL=admin@example.com
```

Optional auth overrides:

```env
LABBY_AUTH_SQLITE_PATH=/var/lib/labby/auth.db
LABBY_AUTH_KEY_PATH=/var/lib/labby/auth-jwt.pem
LABBY_AUTH_ALLOWED_REDIRECT_URIS=https://callback.example.com/callback/*
LABBY_AUTH_ALLOWED_EMAIL_DOMAINS=example.com,corp.example.com
LABBY_GOOGLE_CALLBACK_PATH=/auth/google/callback
LABBY_GOOGLE_SCOPES=openid,email,profile
LABBY_AUTH_ACCESS_TOKEN_TTL_SECS=3600
LABBY_AUTH_REFRESH_TOKEN_TTL_SECS=2592000
LABBY_AUTH_CODE_TTL_SECS=300
LABBY_AUTH_TOKEN_REQUESTS_PER_MINUTE=120
LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY=false
LABBY_AUTH_MACHINE_CLIENTS_JSON=[{"client_id":"ci-agent","client_secret":"replace-me","resources":["https://lab.example.com/mcp"],"scopes":["lab"]}]
LABBY_AUTH_ENTERPRISE_ISSUERS_JSON=[{"issuer":"https://idp.example.com","jwks_uri":"https://idp.example.com/jwks","allowed_client_ids":["ci-agent"]}]
```

These non-secret overrides can also live in `config.toml` under `[auth]`.

Rules:

- `LABBY_AUTH_MODE` defaults to `bearer`
- bearer mode keeps using `LABBY_MCP_HTTP_TOKEN`
- oauth mode requires `LABBY_PUBLIC_URL`, `LABBY_AUTH_ADMIN_EMAIL`, and exactly one complete Google or Authelia provider configuration
- Authelia support is open beta and pinned in CI to 4.39.10. Register only the exact `/auth/oidc/callback`, `client_secret_basic`, authorization code flow, and PKCE S256 with `openid email profile`; do not grant `offline_access`.
- `LABBY_AUTH_ADMIN_EMAIL` is the provider-neutral bootstrap admin email; startup fails closed if unset under oauth mode so no identity can authenticate without explicit permission. The SQLite-backed allowlist grants access to additional users.
- `LABBY_AUTH_ALLOWED_EMAIL_DOMAINS` grants access to verified identities in the configured domains. For Google it is matched against the provider-asserted `hd` (hosted domain) claim, never the address suffix. For Authelia it is matched against the domain of the verified email claim and is not equivalent to a Google `hd` assertion. Empty (the default) disables domain-based access.
- `LABBY_GOOGLE_CALLBACK_URL` optionally sends the browser callback to a webapp host that differs from the stable OAuth issuer in `LABBY_PUBLIC_URL`
- `LABBY_AUTH_CODEX_ISSUER_COMPATIBILITY=true` is an explicit temporary workaround for [openai/codex#34684](https://github.com/openai/codex/issues/34684); it disables RFC 9207 response-issuer advertisement and emission and should be removed after affected Codex clients are fixed
- the old external issuer variables (`LABBY_OAUTH_ISSUER`, `LABBY_OAUTH_AUDIENCE`, `LABBY_OAUTH_CLIENT_ID`) are no longer used
- `LABBY_PUBLIC_URL` also feeds RFC 9728 metadata, JWT issuer/audience, and HTTP allowed-host derivation

## Remote Gateway CLI Usage

`LABBY_SERVER_URL` is the persisted fallback for plugin setup connectivity
checks and plugin-setting export. It configures neither the daemon listener nor
general gateway CLI/stdio discovery; those use the variables documented below.
As client-only setup state, it is intentionally outside the generated
per-service environment inventory.

`labby gateway <subcommand>` (add/update/remove/reload/enable/disable/list/
mcp auth */protected-route */discover/import/code *) prefers the live
`labby serve` daemon's HTTP API over its own local `config.toml` mutation --
see `docs/services/GATEWAY.md` for why that split exists. To reach a daemon
running on a different host (not just the one the CLI happens to run on),
the invoking machine should configure an explicit client target and the
daemon's bearer token:

```env
LABBY_MCP_HTTP_TOKEN=same-token-as-the-daemon
LABBY_SERVER_URL=https://labby.example.com
```

- `LABBY_MCP_HTTP_TOKEN` must be the *same* token the daemon itself uses for
  bearer auth (copy it from the daemon host's `~/.labby/.env`). Without it,
  protected operations fail with `auth_required` or `auth_failed`, as
  applicable.
- `CLAUDE_PLUGIN_OPTION_SERVER_URL` is the invocation-scoped target supplied by
  the Labby Claude plugin. It takes precedence over `LABBY_SERVER_URL` and is
  authenticated only with its paired `CLAUDE_PLUGIN_OPTION_API_TOKEN`; it
  never inherits `LABBY_MCP_HTTP_TOKEN`.
- `LABBY_SERVER_URL` is the ordinary CLI/stdio client target. Both explicit
  targets fail closed: invalid, unreachable, unauthorized, incompatible, or
  post-detection failures are returned and never read or execute against a
  local `config.toml`.
- Without either explicit target, detection remains opportunistic: local bind,
  then `LABBY_MCP_GATEWAY_URL`, then `LABBY_PUBLIC_URL`. Bounded exhaustion may
  fall back to standalone local state for bootstrap compatibility.
- A terminal `/mcp` is normalized to the daemon base while a reverse-proxy path
  prefix is preserved. Remote targets require HTTPS; loopback HTTP is allowed.
  Redirects, URL credentials, query strings, and fragments are rejected.
- `LABBY_SERVER_URL` and `LABBY_MCP_HTTP_TOKEN` form one trusted operator
  authority domain. The invocation-scoped plugin target and
  `CLAUDE_PLUGIN_OPTION_API_TOKEN` form a separate paired authority domain.
  Do not point either target at a server you do not administer.

Verified from a temporary bare client home with no `config.toml` or local
database: `gateway get` reached the configured live daemon through both
explicit target variables with their paired credentials, without creating
local state. The local
`GatewayManager` is built lazily only when opportunistic detection returns no
daemon; explicit failures and successful remote dispatches never create it.

## Remote MCP Stdio Usage

`labby serve --transport stdio` (a.k.a. `labby mcp`) applies the same
principle at the protocol level, not just for gateway-management actions:
before building anything local, it probes for a live daemon exactly like the
CLI does above. If one is reachable, the stdio process runs as a pure
bridge -- every `tools/`, `resources/`, and `prompts/` request coming in over
stdio is forwarded to the live daemon's own MCP endpoint and the response
piped straight back, with no local `GatewayManager`, upstream pool, or OAuth
state of its own. This is what keeps a locally spawned stdio MCP client
(e.g. an editor or agent configured to run `labby mcp` instead of connecting
to the daemon directly over HTTP) from becoming a second, silently-diverging
gateway instance. Explicit targets use bounded MCP initialization and fail
closed; standalone fallback applies only after opportunistic discovery returns
no daemon.

## Service Environment Variables

### Code Mode runner isolation

Code Mode uses the direct process backend by default. Linux/KVM deployments may
opt into Microsandbox runner isolation with all three variables:

```env
LABBY_CODE_MODE_RUNNER_BACKEND=microsandbox
LABBY_CODE_MODE_MICROSANDBOX_EXE=/absolute/root-or-service-owned/path/to/msb
LABBY_CODE_MODE_MICROSANDBOX_IMAGE=debian@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
LABBY_CODE_MODE_MICROSANDBOX_MAX_RUNNERS=4
```

- `LABBY_CODE_MODE_RUNNER_BACKEND` accepts `process` (default) or
  `microsandbox` (Linux only).
- `LABBY_CODE_MODE_MICROSANDBOX_EXE` is required for `microsandbox` and must be
  an absolute executable path owned by root or the service user and not writable
  by group/other.
- `LABBY_CODE_MODE_MICROSANDBOX_IMAGE` is required for `microsandbox`, must be
  an immutable OCI digest reference (`name@sha256:<64 hex>`), and must already
  be cached. URLs, userinfo, queries, and tag-only references are rejected.
  Runtime pulls are disabled with `--pull never`. Before `labby setup
  host-service install` or `restart` stops the healthy service, Labby preflights
  this setting. A legacy mutable alias or short pinned reference is migrated only
  when its exact digest can be proven from the `labby` service user's existing
  Microsandbox cache: Labby registers the canonical registry+digest reference,
  atomically rewrites the persistent `.env` or systemd drop-in, reloads systemd
  when needed, and verifies the effective value. Missing cache state, an unsafe
  alias, or an untraceable persistent source fails the preflight before restart.
- `LABBY_CODE_MODE_MICROSANDBOX_MAX_RUNNERS` optionally bounds concurrent
  microVMs process-wide (default `4`, hard maximum `16`) independently of the
  generic runner-pool size and overflow settings.

The host must separately provide working KVM access plus compatible `msb` and
`libkrunfw` installations. See [CODE_MODE.md](../dev/CODE_MODE.md#microsandbox-runner-isolation-opt-in).

Supported environment variables are generated from current product metadata.
Gateway upstream secrets are referenced indirectly by environment-variable name;
for example, a persisted upstream may point at
`LABBY_GW_GITHUB_AUTH_HEADER` without storing its value in TOML.

Use [../generated/env-reference.md](../generated/env-reference.md) for the current
required/optional environment-variable matrix, secret flags, and examples.

### Access-store migration approval

Opening an existing access schema v5 or v6 with a schema-v7 binary is denied
unless the operator supplies an approval document bound to an independent
rollback checkpoint, the exact source and target, and an explicit activation:

```env
LABBY_ACCESS_MIGRATION_EVIDENCE=/run/labby/access-migration-v7.json
```

The JSON document uses schema `labby.access-migration-approval/v1` and contains
`operation_id`, `source_version`, `target_version`, `target_fingerprint`,
`source_sha256`, `checkpoint_path`, `checkpoint_sha256`, and `activate: true`.
The checkpoint must be a separate file whose bytes exactly match both the
quiesced source and the recorded digests. Set this only
after completing and retaining the rehearsal evidence described in the
multi-user migration runbook. Missing, stale, mismatched, or replayed evidence
leaves the access runtime unavailable without changing the database.

## Provisioning Environment

`labby setup --provision` and `scripts/incus-bootstrap.sh` also honor:

```env
TS_AUTHKEY=tskey-auth-...
```

When set, provisioning installs Tailscale and joins the host/container to the
tailnet using `tailscale up --auth-key=file:/run/labby-ts-authkey`. The key is
written only to a root-owned runtime file for the join, then removed. Leave it
unset to skip Tailscale join.
