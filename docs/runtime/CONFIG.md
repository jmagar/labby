# Configuration

Configuration is intentionally split between secrets and preferences.

## What goes where

| Category | Where | Examples |
|----------|-------|----------|
| Secrets | `~/.lab/.env` | `*_API_KEY`, `*_TOKEN`, `*_PASSWORD`, `LAB_MCP_HTTP_TOKEN` |
| Service endpoints | `~/.lab/.env` | `RADARR_URL`, `PLEX_URL`, other per-instance upstream URLs |
| Non-secret preferences and defaults | `config.toml` | logging, MCP transport, CORS, admin flags, registry URLs, workspace roots, per-service prefs |

All `config.toml` values can still be overridden by env vars. Env always wins.

## Sources

Secrets and service instance endpoints live in:

- `~/.lab/.env`

Preferences live in (first found wins):

- `./config.toml` (repo/CWD override)
- `~/.lab/config.toml` (user-level, next to `.env`)
- `~/.config/lab/config.toml` (XDG-style fallback)

Copy `config/config.example.toml` to `~/.lab/config.toml` and uncomment sections as needed.

## Load Order

Startup sequence:

1. `config.toml` (first found from the search order above)
2. Tracing init (using `[log]` section from config.toml)
3. `~/.lab/.env` (secrets + URLs via `dotenvy`)
4. `./.env` from CWD (dev convenience, non-fatal if missing)

Value precedence at point of use (highest wins):

1. CLI flags (e.g. `--transport`, `--json`, `--port`)
2. Environment variables (whether from `.env` or the shell)
3. `config.toml`
4. Built-in defaults

## Config sections

### `[output]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `format` | `--json` flag | `"human"` | Default output format |

### `[log]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `filter` | `LAB_LOG` | `"labby=info,lab_apis=warn"` | Tracing filter directive |
| `format` | `LAB_LOG_FORMAT` | `"text"` | Log format: `"text"` or `"json"` |

### `[local_logs]`

Controller runtime log store preferences.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `store_path` | `LAB_LOCAL_LOGS_STORE_PATH` | `~/.lab/logs.db` | Embedded SQLite store path for persisted controller runtime logs |
| `retention_days` | `LAB_LOCAL_LOGS_RETENTION_DAYS` | `7` | Time-based retention window in days |
| `max_bytes` | `LAB_LOCAL_LOGS_MAX_BYTES` | `268435456` | Size-based retention limit in logical stored bytes |
| `queue_capacity` | `LAB_LOCAL_LOGS_QUEUE_CAPACITY` | `1024` | Bounded ingest queue size for the long-lived runtime |
| `subscriber_capacity` | `LAB_LOCAL_LOGS_SUBSCRIBER_CAPACITY` | `256` | Bounded live fanout ring size for SSE subscribers |

Example:

```toml
[local_logs]
store_path = "/var/lib/lab/logs.db"
retention_days = 14
max_bytes = 536870912
queue_capacity = 2048
subscriber_capacity = 512
```

Rules:

- retention is whichever limit hits first: age or size
- oldest events are evicted first by the shared local log subsystem
- these knobs affect the controller runtime log store only; they do not change node log retention
- the browser log console and `labby logs local *` commands both read this same local store contract

### `[mcp]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `transport` | `LAB_MCP_TRANSPORT` | `"http"` | MCP transport: `"stdio"` or `"http"` |
| `host` | `LAB_MCP_HTTP_HOST` | `"127.0.0.1"` | HTTP bind address |
| `port` | `LAB_MCP_HTTP_PORT` | `8765` | HTTP bind port |
| `session_ttl_secs` | `LAB_MCP_SESSION_TTL_SECS` | `300` | HTTP MCP session keep-alive TTL (seconds) |
| `stateful` | `LAB_MCP_STATEFUL` | `true` | Whether HTTP MCP uses stateful sessions |
| `allowed_hosts` | `LAB_MCP_ALLOWED_HOSTS` | `[]` | Additional allowed hosts for DNS rebinding protection |

### `[api]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `cors_origins` | `LAB_CORS_ORIGINS` | `[]` | Additional CORS origins (loopback always included) |

### `[node]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `controller` | — | local hostname | Hostname of the fleet controller node. |
| `log_retention_days` | — | `7` | Retention window for durable node log events. |

Example:

```toml
[node]
controller = "tootie"
```

Rules:

- when omitted, the local machine resolves itself as the controller
- non-controller nodes use this hostname plus `mcp.port` to reach `http://<controller>:<port>`
- the node runtime uses this for websocket node sessions, metadata/status/log delivery, and controller-routed CLI commands such as `labby nodes list`
- legacy `[device].master` is still read for compatibility, but new config should use `[node].controller`
- Docker/Compose deployments should expose the host hostname to the container.
  The bundled Compose file mounts `/etc/hostname` at `/run/host/hostname`, which
  is checked before the container's own `HOSTNAME`. Set `LAB_HOST_HOSTNAME`
  only when that host-file mount is unavailable.

### `[web]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `assets_dir` | `LAB_WEB_ASSETS_DIR` | auto-detect | Path to exported Labby assets served by `labby serve` |

### `[workspace]`

Shared local workspace/stash preferences.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `root` | — | `~/.lab/stash` | Shared Lab workspace root. Backs the read-only attachment picker and local writable stash workspaces. Marketplace editable plugin mirrors live under `<root>/plugins/`. |

Example:

```toml
[workspace]
root = "~/.lab/stash"
```

### `[mcpregistry]`

MCP Registry upstream preferences.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `url` | — | `https://registry.modelcontextprotocol.io` | Base URL for the upstream MCP Registry used by marketplace `mcp.*` actions and registry background sync. |

Example:

```toml
[mcpregistry]
url = "https://registry.modelcontextprotocol.io"
```

### `[tool_search]`

Gateway-wide MCP tool-search mode.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `enabled` | — | `false` | Replace raw proxied upstream tools with synthetic `tool_search` and `tool_invoke` tools for every gateway upstream. |
| `top_k_default` | — | `10` | Default number of search results when a `tool_search` call omits `top_k`. Valid range: 1-50. |
| `max_tools` | — | `5000` | Maximum number of healthy discovered tools to index per rebuild. Valid range: 1-10000. |

Example:

```toml
[tool_search]
enabled = true
top_k_default = 10
max_tools = 5000
```

Rules:

- this is a single gateway-wide switch, not a per-`[[upstream]]` setting
- when enabled, raw upstream tools are hidden from MCP `list_tools`; clients discover them through `tool_search` and invoke them through `tool_invoke`
- when disabled, upstream tools are exposed normally according to each upstream's `expose_tools` policy
- old `[[upstream]].tool_search` config is read only for migration compatibility and is dropped the next time gateway config is written
- operators can change it without hand-editing TOML using `labby gateway tool-search status`, `labby gateway tool-search enable`, and `labby gateway tool-search disable`

### `[oauth.machines.<id>]`

Named OAuth callback forwarding targets for `labby oauth relay-local`.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `target_url` | — | required | Full callback base URL to forward to |
| `description` | — | `null` | Optional operator-facing note |
| `default_port` | — | `null` | Optional preferred local callback port |

Example:

```toml
[oauth.machines.dookie]
target_url = "http://100.88.16.79:38935/callback/dookie"
description = "dookie Codex callback listener"
default_port = 38935
```

This is used by:

```bash
labby oauth relay-local --machine dookie --port 38935
```

`oauth.machines` config is TOML-only. There is no env-var override for the named machine map.

The node runtime reuses the same target model when `POST /v1/nodes/oauth/relay/start` is used to start a local relay remotely on a node.

### `[admin]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `enabled` | `LAB_ADMIN_ENABLED=1` | `false` | Enable the `lab_admin` MCP tool |

### `[services.tailscale]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `tailnet` | `TAILSCALE_TAILNET` | `"-"` | Tailnet name (auto-detect) |

## Ownership

Config loading lives in `lab`, not `lab-apis`.

The SDK should not read files or ambient env automatically. It should receive explicit values when clients are constructed.

## Typed Config

Preferences are loaded into a typed config structure (`LabConfig`), not a stringly-typed map.

Rules:

- partial config files are valid
- validation happens once at load time
- service-specific config blocks use their own types under `[services.*]`

## Secret Handling

Credentials belong in env, not TOML.

Examples:

```env
RADARR_URL=http://localhost:7878
RADARR_API_KEY=abc123
PLEX_URL=http://localhost:32400
PLEX_TOKEN=xyz789
JELLYFIN_URL=http://localhost:8096
JELLYFIN_API_KEY=replace-me
OPENACP_URL=http://127.0.0.1:21420
OPENACP_TOKEN=replace-me
```

Rules:

- do not echo secrets in logs
- do not print secret env values in doctor or TUI prompts
- do not write credentials outside the designated env-management flows
- do not store secrets in config TOML

## Multi-Instance Naming

Multi-instance services use a predictable naming scheme.

Default instance:

```env
UNRAID_URL=https://tower.local/graphql
UNRAID_API_KEY=...
```

Named instance:

```env
UNRAID_SHART_URL=https://other.local/graphql
UNRAID_SHART_API_KEY=...
```

Jellyfin follows the same pattern:

```env
JELLYFIN_URL=http://localhost:8096
JELLYFIN_API_KEY=...
JELLYFIN_NODE2_URL=http://node2.local:8096
JELLYFIN_NODE2_API_KEY=...
```

OpenACP uses bearer tokens instead of API keys:

```env
OPENACP_URL=http://127.0.0.1:21420
OPENACP_TOKEN=...
OPENACP_NODE2_URL=http://node2.local:21420
OPENACP_NODE2_TOKEN=...
```

Lab does not discover or read upstream OpenACP `api-secret` files
automatically. Provide the token explicitly in `~/.lab/.env`.

Rules:

- unlabeled keys define the `default` instance
- labeled keys define additional instances
- labels are derived from env, not hardcoded in source

## Default Instance Resolution

Resolution order:

1. explicit `default` instance
2. the sole configured instance
3. otherwise, require an explicit instance

## Future Migration Path

If env-based instance definitions become unwieldy, the project can later move instance metadata into TOML while still keeping secrets in env. The public CLI and MCP instance surface should remain stable across that migration.

## HTTP Auth Configuration

HTTP auth is mode-based.

- `LAB_AUTH_MODE=bearer` preserves the existing static bearer flow and still uses `LAB_MCP_HTTP_TOKEN`.
- `LAB_AUTH_MODE=oauth` enables the internal Google-backed authorization server and requires `LAB_PUBLIC_URL`.

Full details in [OAUTH.md](./OAUTH.md).

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `LAB_AUTH_MODE` | no | `bearer` or `oauth`. Defaults to `bearer`. |
| `LAB_MCP_HTTP_TOKEN` | bearer mode only | Static bearer token for protected HTTP routes. |
| `LAB_PUBLIC_URL` | oauth mode | Public base URL for metadata, JWT issuer/audience, callback construction, and allowed-host derivation. Path-prefixed deployments are supported. |
| `LAB_AUTH_SQLITE_PATH` | no | Override path for the auth SQLite database. Defaults to `~/.lab/auth.db`. |
| `LAB_AUTH_KEY_PATH` | no | Override path for the persisted JWT signing key. Defaults to `~/.lab/auth-jwt.pem`. |
| `LAB_GOOGLE_CLIENT_ID` | oauth mode | Google OAuth client ID. |
| `LAB_GOOGLE_CLIENT_SECRET` | oauth mode | Google OAuth client secret. |
| `LAB_GOOGLE_CALLBACK_PATH` | no | Callback path appended to `LAB_PUBLIC_URL`. Defaults to `/auth/google/callback`. |
| `LAB_GOOGLE_SCOPES` | no | Comma-separated Google scopes. Defaults to `openid,email,profile`. |
| `LAB_AUTH_ALLOWED_REDIRECT_URIS` | no | Comma-separated non-loopback redirect URI patterns. Host wildcards must be full labels, not raw suffix globs. |
| `LAB_AUTH_ADMIN_EMAIL` | oauth mode | Google email of the bootstrap admin permitted to log in. Normalized to lowercase. **Required** in oauth mode — startup fails if unset so no Google account can authenticate unless explicitly permitted. The id_token's `email_verified` claim is enforced (unverified accounts are rejected even when the address matches). Additional users will be granted via a SQLite-backed allowlist managed in the web UI (planned). |
| `LAB_AUTH_ACCESS_TOKEN_TTL_SECS` | no | Override lab-issued JWT access token lifetime. Defaults to `3600`. |
| `LAB_AUTH_REFRESH_TOKEN_TTL_SECS` | no | Override refresh token lifetime. Defaults to `2592000` (30 days). |
| `LAB_AUTH_CODE_TTL_SECS` | no | Override authorization code lifetime. Defaults to `300`. |
| `LAB_AUTH_REGISTER_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `POST /register`. Defaults to `20`. |
| `LAB_AUTH_AUTHORIZE_REQUESTS_PER_MINUTE` | no | Process-local rate limit for `/authorize` and hosted browser-login initiation. Defaults to `60`. |
| `LAB_AUTH_MAX_PENDING_OAUTH_STATES` | no | Maximum non-expired authorization and browser-login states kept in the auth store. Defaults to `1024`. |

### config.toml

```toml
[auth]
mode = "oauth"
public_url = "https://lab.example.com"
google_client_id = "google-client-id"
google_client_secret = "google-client-secret"
google_callback_path = "/auth/google/callback"
google_scopes = ["openid", "email", "profile"]
access_token_ttl_secs = 3600
refresh_token_ttl_secs = 2592000
auth_code_ttl_secs = 300
register_requests_per_minute = 20
authorize_requests_per_minute = 60
max_pending_oauth_states = 1024
```

Environment variables override `[auth]` values field-by-field.

### OAuth Relay Machine Targets

`target_url` is the full callback base URL, not just a host.

## Node Runtime Auth

If the controller protects `/v1/*` with `LAB_MCP_HTTP_TOKEN`, controller-routed `labby nodes` / `labby logs` commands reuse that bearer token automatically.

Fleet websocket sessions are separate from that bearer auth path. Node-to-controller delivery is admitted through the enrollment store using the node token (`device_token` wire field) presented during websocket `initialize`.

There is not a separate `[node]` auth block in this implementation.

```toml
[oauth.machines.dookie]
target_url = "http://100.88.16.79:38935/callback/dookie"
description = "Dookie Claude callback target"
default_port = 38935
```

Machine IDs are stable config keys. When `labby oauth relay-local --machine dookie --port 38935`
runs, it resolves the forwarding target from this map and preserves the incoming suffix path and
query string when proxying the callback.

## Web UI Hosting

When `labby serve` can find exported Labby assets, it serves the web UI from the
same origin as the API and MCP server. Asset directory resolution is:

1. `LAB_WEB_ASSETS_DIR`
2. `[web].assets_dir` in `config.toml`
3. repo-local fallback: `apps/gateway-admin/out`

The web shell is public; the UI then talks to same-origin `/v1/*` and `/mcp`.
Set `LAB_WEB_UI_AUTH_DISABLED=true` only for local development or trusted
reverse-proxy setups where browser auth is intentionally bypassed. The legacy
alias `LAB_WEB_UI_DISABLE_AUTH` is still accepted, but new configs should use
`LAB_WEB_UI_AUTH_DISABLED`.

## Upstream MCP Servers

Lab can proxy tool calls and resource reads to upstream MCP servers.

Full details in [UPSTREAM.md](./UPSTREAM.md).

### config.toml

```toml
[[upstream]]
name = "remote-lab"
url = "https://lab2.example.com/mcp"
bearer_token_env = "LAB_UPSTREAM_TOKEN"
proxy_resources = true
expose_tools = ["search_repos", "github_*"]

[[upstream]]
name = "local-server"
command = "my-mcp-server"
args = ["--port", "5000"]
proxy_resources = false
```

`expose_tools` is optional. When present, it limits which discovered upstream tools are republished by the gateway. Entries support exact names and simple `*` wildcards.

### Upstream OAuth (authorization_code + PKCE)

An upstream MCP server that advertises OAuth Protected Resource Metadata can be
authenticated per-user via an `[upstream.oauth]` block instead of a shared
bearer token. `oauth` and `bearer_token_env` are **mutually exclusive** — setting
both is a config-validation error.

Shared shape:

```toml
[upstream.oauth]
mode = "authorization_code_pkce"   # only supported mode today
scopes = ["mcp"]                    # optional; omit to let RMCP auto-select

[upstream.oauth.registration]
strategy = "..."                    # client_metadata_document | preregistered | dynamic
```

Client ID Metadata Document (preferred per the MCP authorization spec):

```toml
[[upstream]]
name = "acme"
url = "https://acme.example.com/mcp"

[upstream.oauth]
mode = "authorization_code_pkce"
scopes = ["mcp"]

[upstream.oauth.registration]
strategy = "client_metadata_document"
url = "https://acme.example.com/.well-known/oauth-client"
```

Preregistered public client (no `client_secret`):

```toml
[upstream.oauth.registration]
strategy = "preregistered"
client_id = "lab-public-client"
```

Preregistered confidential client (`client_secret` named by env var):

```toml
[upstream.oauth.registration]
strategy = "preregistered"
client_id = "lab-confidential-client"
client_secret_env = "ACME_UPSTREAM_CLIENT_SECRET"
```

Dynamic Client Registration (RFC 7591):

```toml
[upstream.oauth.registration]
strategy = "dynamic"
```

Dynamic registration may require an initial access token; supply it via env
(documented in [UPSTREAM.md](./UPSTREAM.md)). DCR-issued credentials are
persisted alongside tokens and reused on restart.

Setting `oauth` and `bearer_token_env` on the same upstream produces a
validation error at startup and rejects gateway mutations that would create the
same conflict:

```text
upstream 'acme' has both bearer_token_env and oauth configured — pick one
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LAB_UPSTREAM_MAX_RESPONSE_BYTES` | 10485760 | Maximum response size from upstream servers. |
| `LAB_OAUTH_ENCRYPTION_KEY` | — | **Required when any upstream has `[upstream.oauth]` set.** Base64-encoded 32-byte key used with chacha20poly1305 to encrypt persisted upstream OAuth token responses at rest. Loaded once at startup; startup fails fast if missing, not decodable, or not exactly 32 bytes. Generate with `openssl rand -base64 32`. |
| (per `bearer_token_env`) | — | Bearer token for each upstream, named in config. |
| (per `client_secret_env`) | — | OAuth client secret for a preregistered confidential upstream, named in config. |

**Key rotation procedure:** rotate by (1) generating a new key, (2) clearing
all persisted upstream OAuth credentials (`POST /v1/gateway/oauth/clear?upstream=<name>&confirm=true`
per upstream, or remove rows from `upstream_oauth_credentials`), (3) updating
`LAB_OAUTH_ENCRYPTION_KEY` in `~/.lab/.env`, (4) restarting `lab`, (5) asking
each user to re-authorize each upstream. Decryption under the wrong key
surfaces as `oauth_needs_reauth`, never as an internal error.

## Transport and Session Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `LAB_MCP_TRANSPORT` | `http` | Transport: `stdio` or `http`. |
| `LAB_MCP_HTTP_HOST` | `127.0.0.1` | HTTP bind address. |
| `LAB_MCP_HTTP_PORT` | `8765` | HTTP bind port. |
| `LAB_MCP_HTTP_TOKEN` | — | Static bearer token for HTTP auth. |
| `LAB_MCP_SESSION_TTL_SECS` | `300` | MCP session keep-alive TTL (seconds). |
| `LAB_MCP_STATEFUL` | `true` | Whether to use stateful MCP sessions. |
| `LAB_MCP_ALLOWED_HOSTS` | — | Comma-separated hostnames for DNS rebinding protection. |
| `LAB_CORS_ORIGINS` | — | Comma-separated CORS origin allowlist. |
| `LAB_WEB_ASSETS_DIR` | — | Override path to exported Labby assets for `labby serve`. |

Full details in [TRANSPORT.md](./TRANSPORT.md).

## Docker Deployment

`docker-compose.yml` mounts config and secrets from the host user's `~/.lab/`
directory so the container and a local `labby serve` process share the same
configuration without duplicating files:

```yaml
volumes:
  - ${HOME}/.lab/config.toml:/home/lab/.config/lab/config.toml:ro
env_file:
  - path: ${HOME}/.lab/.env
    required: false
```

**Implications:**

- Changes to `~/.lab/config.toml` take effect on the next container restart
  (`docker compose restart labby-master`).
- Copy `config/config.example.toml` to `~/.lab/config.toml` and uncomment
  sections as needed.
- The container overrides two env vars that would be wrong inside the container
  even if set in `~/.lab/.env`:
  - `LAB_WEB_ASSETS_DIR=""` — clears any host filesystem path so the binary
    falls back to its embedded assets.
  - `LAB_LOCAL_LOGS_STORE_PATH="/home/lab/.local/share/lab/logs.db"` — routes
    the log store into the named `labby-data` volume.
- Docker-specific ACP provider config is mounted from
  `config/acp-providers.docker.json` to `/home/lab/.lab/acp-providers.json`.
  That file uses container paths and passes
  `sandbox_mode="danger-full-access"` to Codex ACP because Docker's default
  seccomp profile blocks the nested namespace sandbox used by Codex
  `workspace-write` and `read-only` modes. Provider config changes affect new
  sessions; already-running provider subprocesses keep their launch state until
  a new session or container restart.
- The dev image (`config/Dockerfile.fast`) pre-installs the three ACP adapters
  (`claude-agent-acp`, `codex-acp`, `gemini`) into `/opt/acp-adapters/` and
  symlinks the binaries into `/usr/local/bin/`. The provider config calls
  those binaries directly — no `npx -y` round-trip on each spawn. The
  `@anthropic-ai/claude-agent-sdk` version is pinned via an `overrides` entry
  so the bundled Claude Code binary stays compatible with the host's
  credential format (mismatched SDK/credentials cause the Claude Code binary
  to `SIGILL` on `session/new`). Bumping any adapter version requires a
  `docker compose build labby-master` rebuild; the labby binary itself can be
  hot-swapped with `just dev` or `just dev-debug` without an image rebuild.
- `docker-compose.yml` sets `init: true` for `labby-master` so Docker's tiny
  init reaps provider grandchildren orphaned by the ACP process group when
  Lab terminates a session. (Historically this was needed to clean up `npx`
  launch wrappers; with the pre-installed adapters there's nothing extra to
  reap, but `init: true` remains a defense-in-depth default.)
- `just dev-debug` rebuilds the local debug binary, hot-swaps it into the
  dev container through the bind-mounted `bin/labby`, and restarts the
  container. Config-only edits usually need only a restart; Rust ACP health or
  preflight edits need `just dev-debug` or another binary rebuild path.
- After restart, `just acp-smoke --provider-only` checks the mounted Docker ACP
  config and `/v1/acp/provider` health. `just acp-smoke` also creates a Codex
  ACP session, sends a minimal `pwd` prompt, and streams a short event sample.

## `.mcp.json` Environment

When `lab` is integrated into an MCP client config, the env file path should be explicit and stable so plugin installation and operator tooling update the same source of truth.
