# Configuration

Configuration is intentionally split between secrets and preferences.

## What goes where

| Category | Where | Examples |
|----------|-------|----------|
| Secrets | `~/.lab/.env` | `*_API_KEY`, `*_TOKEN`, `*_PASSWORD`, `LAB_MCP_HTTP_TOKEN` |
| Service endpoints | `~/.lab/.env` | `RADARR_URL`, `PLEX_URL`, other per-instance upstream URLs |
| Non-secret preferences and defaults | `config.toml` | logging, MCP transport, CORS, admin flags, registry URLs, workspace roots, per-service prefs |

All `config.toml` values can still be overridden by env vars. Env always wins.

## `/settings` source-aware editor

The Labby `/settings` UI is schema-backed by Rust. It exposes configuration
from three layers, highest precedence first:

1. CLI flags and process environment variables.
2. `~/.lab/.env`.
3. `config.toml`, searched from current directory, `~/.lab/config.toml`, then
   `~/.config/lab/config.toml`.

When an environment variable overrides a `config.toml` field, the UI shows the
override source. Changing the TOML value is still allowed for safe scalar fields,
but it will not affect the current runtime until the env override is removed and
the relevant process is restarted or reloaded.

### Settings write policy

- Low-risk env keys are updated through `setup.settings.env.update`, which
  performs a targeted atomic merge without committing unrelated draft entries.
- Low-risk scalar TOML keys are updated through `setup.settings.config.update`,
  which is admin-only, schema-approved, backed up before write, validated with
  `LabConfig::validate`, and written atomically.
- Complex sections such as `upstream`, `protected_mcp_routes`,
  `virtual_servers`, and `deploy` are read-only in `/settings` until typed
  editors exist.
- Secrets are never returned raw. Config-backed secret writes require a future
  write-only flow and are not part of the scalar settings editor.

### Settings apply modes

- `immediate`: runtime behavior is updated in-process by the settings action.
- `partial`: config is updated, but only some runtime readers observe it without restart.
- `restart`: restart `labby serve` for the setting to fully apply.
- `read_only`: visible but not editable from this settings slice.

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

### First-run bootstrap

On first run, `labby serve` detects a missing MCP token (no `LAB_MCP_HTTP_TOKEN`
and `LAB_AUTH_MODE` != `oauth`) and self-bootstraps: it generates a 64-char hex
bearer token, writes a minimal `~/.lab/.env` (token + loopback MCP defaults via
the atomic `env_merge` path), reloads that file into the process environment via
`dotenvy` so the token is visible process-wide, prints the
`http://<host>:<port>/setup` URL once, points the operator to the generated env
file for the token, and continues startup. The web `/setup` wizard then owns all
further configuration. Set `LAB_MCP_HTTP_TOKEN` or `LAB_AUTH_MODE=oauth`
beforehand to opt out. The generated `~/.lab/.env` is written `0600` on Unix;
**Windows ACL hardening is still pending**
(`env_merge::set_secure_perms` is a no-op on non-unix), so on Windows the token
file sits at default ACLs. The `setup.bootstrap` action exposes this primitive
to the wizard and CLI.

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

### `[gateway]`

Spawn-guard preferences for stdio upstream commands. The gateway only allows
known-safe runtimes (`npx`, `uvx`, `docker`, `node`, `python`, `python3`,
`deno`, `pipx`, `dnx`) as the `command` of a stdio upstream by default.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `extra_stdio_commands` | — | `[]` | Additional basenames allowed as stdio upstream commands, beyond the built-in runtime list. |
| `disable_spawn_guard` | — | `false` | Skip the command allowlist entirely. Operator takes full responsibility for any command in the gateway config. |

Example — add a custom binary and a local MCP server:

```toml
[gateway]
extra_stdio_commands = ["myserver", "labby"]
```

Example — disable the guard entirely:

```toml
[gateway]
disable_spawn_guard = true
```

Rules:

- entries in `extra_stdio_commands` are matched against the **basename** of the upstream `command` path, so `/usr/local/bin/myserver` matches `"myserver"`.
- HTTP upstreams are never subject to the spawn guard — only `command`-based (stdio) upstreams are checked.
- `disable_spawn_guard = true` takes precedence over `extra_stdio_commands`; both can be set together for documentation purposes.

### `[code_mode]`

Gateway-wide Code Mode exposure and execution limits. When enabled, raw gateway
tools are hidden from MCP `list_tools()` and the gateway advertises the single
synthetic `codemode` tool instead.

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `enabled` | — | `false` | Replace raw proxied upstream tools with the synthetic Code Mode `codemode` tool for the gateway. Discovery happens inside the sandbox with `codemode.search()` and `codemode.describe()`. |
| `trace_params` | — | `true` | Include only redacted and capped upstream tool params in Code Mode call traces and history. Set false to omit params from traces entirely. |
| `timeout_ms` | — | `30000` | Maximum wall-clock time for one Code Mode execution. Valid range: 1-60000. |
| `max_tool_calls` | — | `1000` | Maximum host-brokered upstream tool calls allowed in one execution. Valid range: 1-10000. |
| `max_response_bytes` | — | `24576` | Maximum serialized response envelope size returned by `codemode`. Valid range: 1024-1048576. |
| `max_response_tokens` | — | `6000` | Approximate maximum response tokens returned by `codemode`. Valid range: 256-256000. |
| `token_estimate_divisor` | — | `4` | Byte-to-token estimate divisor for response limiting. Valid range: 1-64. |
| `max_log_entries` | — | `1000` | Maximum captured console log lines per execution. Valid range: 1-100000. |
| `max_log_bytes` | — | `65536` | Maximum captured console log bytes per execution. Valid range: 1-104857600. |

Example:

```toml
[code_mode]
enabled = true
trace_params = true
max_tool_calls = 1000
timeout_ms = 30000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

Operators can change the main execution limits without hand-editing TOML using
`gateway.code_mode.get` and `gateway.code_mode.set`. The action accepts all
fields listed above and validates them with the same ranges used for file
configuration.

#### Code Mode Artifacts

`codemode` exposes a sandbox helper for large outputs:

```js
const artifact = await writeArtifact("reports/brief.md", markdown, {
  contentType: "text/markdown"
});
return { artifact, summary: "Brief generated" };
```

Artifacts are host-brokered writes, not direct sandbox filesystem access. The
runner emits an artifact request and Labby validates the path before writing.
The path must be a non-empty **relative** path with no `..` segments (checked
after `\`→`/` normalization), and the joined destination is confirmed to stay
within the per-run root — rejecting symlinked ancestors — before any write. The
content is then written into a fresh per-run directory under
`$LAB_HOME/code-mode-artifacts/<run_id>/` and Labby returns a receipt:

```json
{
  "path": "reports/brief.md",
  "absolute_path": "~/.lab/code-mode-artifacts/01J.../reports/brief.md",
  "content_type": "text/markdown",
  "bytes": 18342,
  "sha256": "..."
}
```

Artifact **content** is written to disk and never returned to the model (only
the receipt is), so its size cap is a resource bound, **not** a context guard. A
single artifact defaults to a **8 MiB** cap, overridable with
`LAB_CODE_MODE_ARTIFACT_MAX_MIB` (in MiB); oversized content is rejected with
`invalid_param`. Keep it below ~64 so a write stays under the runner's JS heap
and fails cleanly instead of as an opaque out-of-memory trap. `options.contentType`
defaults to `text/plain` when omitted or blank and is itself capped at 256 bytes
(it *does* ride the receipt back into the response). Artifact writes do not
bypass `timeout_ms` or final response caps. Each write counts against a
**separate** budget from tool calls — both are bounded by `max_tool_calls`
(default `1000`), but artifact writes and upstream tool calls have independent
counters, so a write-heavy run never starves its tool-call allowance and vice
versa. They are the preferred way to keep large markdown reports, source tables,
crawl manifests, and follow-up snippets out of the final JSON response while
still making them available on disk.

The store is bounded on two axes, pruned on the first artifact write of a run
(never on search or no-write runs):

- **Run count** — keeps the newest `LAB_CODE_MODE_ARTIFACT_RETENTION_RUNS`
  (default `200`) run directories; `0` disables the count rule.
- **Total bytes** — drops the oldest runs until the whole store fits
  `LAB_CODE_MODE_ARTIFACT_MAX_STORE_MIB` (default `4096`, i.e. 4 GiB); `0`
  disables the byte rule. This matters once artifacts can be several MiB each,
  where a run-count cap alone no longer bounds disk usage.

Only ULID-named run directories this feature created are ever pruned, and a
still-executing run is never collected. Set both knobs to `0` for unbounded
growth.

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
target_url = "http://node.internal.example:38935/callback/dookie"
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

### `[services]`

| Key | Env override | Default | Description |
|-----|-------------|---------|-------------|
| `built_in_upstream_apis_enabled` | none | `true` | Compatibility switch for compiled built-in service integrations that call external service APIs. This checkout does not ship the older first-party upstream integration features, so the setting has no effect unless such features are reintroduced. |

When `built_in_upstream_apis_enabled = false`, Lab preserves stored credentials
and config on disk. Disablement controls runtime exposure only; it does not
delete `.env` values. If first-party upstream integrations are reintroduced,
runtime discovery (`lab://catalog`, MCP list tools, HTTP route mounting, and
action dispatch) should reflect the value captured when the server process
started.

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
- do not print secret env values in doctor output, logs, docs, or UI prompts
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
| `LAB_AUTH_ADMIN_EMAIL` | oauth mode | Google email of the bootstrap admin permitted to log in. Normalized to lowercase. **Required** in oauth mode — startup fails if unset so no Google account can authenticate unless explicitly permitted. The id_token's `email_verified` claim is enforced (unverified accounts are rejected even when the address matches). Additional users are granted through the SQLite-backed allowlist managed from Labby settings. |
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
target_url = "http://node.internal.example:38935/callback/dookie"
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

Full details in [UPSTREAM.md](../services/UPSTREAM.md).

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

### Gateway-Managed Protected MCP Routes

Gateway-managed protected MCP routes are configured with
`[[protected_mcp_routes]]`. They publish a public OAuth-protected MCP resource
at `https://<public_host><public_path>` and proxy accepted Streamable HTTP MCP
traffic either to a raw backend MCP endpoint (`backend_url`) or to an existing
named Gateway upstream (`upstream`).

Use these for inline public MCP routes that need Lab-owned OAuth protected
resource metadata, 401 challenges, token validation, and redacted public errors.
Do not model them with legacy `MCP_<SERVICE>_URLS` or
`MCP_<SERVICE>_BACKEND` env vars; use the Gateway UI or
`labby gateway protected-route ...` so validation and duplicate detection share
the same source of truth.

```toml
[[protected_mcp_routes]]
name = "syslog"
enabled = true
public_host = "mcp.example.com"
public_path = "/syslog"
backend_url = "http://node.internal.example:3100/mcp"
scopes = ["mcp:read", "mcp:write"]
health_path = "/health"
```

To publish a Gateway-managed upstream, use `upstream` instead of `backend_url`.
Lab resolves the upstream by name and applies that upstream's configured auth,
including upstream OAuth credentials stored for the shared Gateway subject.

```toml
[[protected_mcp_routes]]
name = "axon"
enabled = true
public_host = "mcp.example.com"
public_path = "/axon"
upstream = "axon"
scopes = ["mcp:read", "mcp:write"]
```

Rules:

- `public_host` is a bare host with no scheme, port, or path.
- `public_path` must include a service segment and cannot overlap Lab reserved
  routes such as `/.well-known/*` or `/v1/*`.
- Set exactly one of `upstream` or `backend_url`.
- `upstream` publishes a named Gateway upstream and reuses its configured
  upstream auth, including upstream OAuth. In this mode `backend_url` is
  intentionally empty because the endpoint URL comes from the upstream config.
- `backend_url` is the full backend MCP endpoint URL. Origin-only values are
  accepted as legacy input and default to `/mcp`.
- `backend_mcp_path` is deprecated compatibility input for older configs.
- `scopes` defaults to `mcp:read` and `mcp:write` when omitted.
- Disabled routes do not publish metadata, issue challenges, or proxy traffic.
- Public errors must not reveal `backend_url`, `backend_mcp_path`, private IPs,
  or token env var names.

Full operator setup, SWAG/nginx, Traefik, tunnel guidance, migration examples,
and curl verification live in [GATEWAY.md](../services/GATEWAY.md#gateway-managed-protected-mcp-routes).

### Upstream OAuth (authorization_code + PKCE)

An upstream MCP server that advertises OAuth Protected Resource Metadata can be
authenticated via an `[upstream.oauth]` block instead of a shared bearer token.
Current Gateway UI/API/CLI flows store credentials for the shared Gateway
subject `gateway`; subject-scoped MCP internals still use the same per-subject
cache shape so future per-user routing can remain isolated. `oauth` and
`bearer_token_env` are **mutually exclusive** — setting both is a
config-validation error.

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
(documented in [UPSTREAM.md](../services/UPSTREAM.md)). DCR-issued credentials are
persisted alongside tokens and reused on restart.

Some OAuth providers publish split endpoint origins while keeping a stable
issuer. Lab still enforces issuer binding by default, but allows known provider
deployments such as Google's `https://accounts.google.com` issuer with the
`https://oauth2.googleapis.com` token endpoint. This supports Google-hosted MCP
servers such as Google Drive MCP without weakening issuer checks for arbitrary
upstreams.

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

Full details in [TRANSPORT.md](../surfaces/TRANSPORT.md).

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
