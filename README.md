# lab

`lab` is a Rust homelab control plane workspace. It contains a reusable SDK crate
(`lab-apis`) and one product binary (`lab`) that exposes service integrations and
operator capabilities through the CLI, an MCP server, an HTTP API, the Labby web UI,
and a Ratatui plugin manager.

The root README is the public entrypoint. The topic docs in [docs/](./docs/README.md)
are the source of truth for implementation contracts and operator workflows. If this
file and a topic doc disagree, update the owning topic doc and then refresh this file.

## What You Can Do

Lab is not only a service SDK. It is an operator console for a homelab and an AI-tooling
control plane.

| Feature Area | What Lab Provides |
| --- | --- |
| Marketplace browsing | Browse configured Claude Code and Codex plugin marketplaces, installed plugins, official MCP Registry servers, and ACP Registry agents from one catalog. Filter by type, inspect curated Lab metadata, sync the local MCP Registry mirror, and open installed artifact files from the web UI or API. |
| Stash workspaces | Open a marketplace plugin into a Lab-managed stash workspace under `~/.lab/stash`, edit files through the Labby UI, save changes, preview deploy diffs, and deploy the saved workspace back to the Claude Code or Codex target with explicit confirmation. |
| Artifact forks and updates | Track the marketplace artifact fork/update model in the action catalog: fork metadata, upstream/base snapshots, drift checks, update previews, update apply strategies, and AI merge suggestions. The lower-level direct `artifact.fork`, `artifact.diff`, and `artifact.patch` actions are present but still return `not_implemented` until that lifecycle is completed. |
| Device deployment and cherry-pick | Install whole plugins or cherry-pick individual skills, agents, slash commands, MCP server configs, scripts, and other plugin artifacts to any selected enrolled device and scope instead of copying files by hand. |
| MCP Registry search and install | Search, filter, validate, and install servers from the official MCP Registry. Installs can target Lab gateway upstreams or Claude/Codex MCP client configs on fleet devices, with required env values routed to the right config surface. |
| MCP Registry aggregator | Serve Lab's local registry mirror on `/v0.1/*` as a drop-in replacement for the official MCP Registry API, while layering Lab-owned metadata such as featured/reviewed/recommended flags, tags, audit fields, and homelab-specific curation without mutating upstream registry data. |
| ACP Registry agents | Search the ACP Registry for compatible agents, inspect agent details, and install or uninstall agent provider entries through the same marketplace service. ACP agent installs currently write controller-local provider config; remote ACP agent installation returns per-node errors until implemented. |
| Upstream MCP proxy | Point MCP clients at Lab instead of every individual server. Lab connects to configured HTTP or stdio upstream MCP servers, discovers their tools, optionally proxies resources, normalizes errors, applies circuit-breaker health, and republishes the merged catalog behind Lab's authenticated `/mcp` endpoint. |
| Gateway management | Add, test, reload, and remove upstream MCP servers without hand-editing config. Lab supports exposure filters, `tool_search`/`tool_invoke` helper paths, OAuth-backed upstream credentials, Gateway-managed protected MCP routes, and Labby/CLI/API controls for the proxy pool. |
| Virtual Lab servers | Expose configured Lab-backed services as virtual gateway servers, toggle CLI/API/MCP/Web UI surfaces, inspect service action metadata, and set MCP action allowlists per virtual server. |
| Authentication and OAuth | Protect hosted HTTP, MCP, Labby, registry, and gateway-management routes with static bearer auth or Lab's Google-backed OAuth mode. Lab also supports browser sessions, CSRF-protected web UI calls, OAuth metadata/JWKS endpoints, upstream MCP OAuth credential storage, and local or node-started OAuth callback relays. |
| Fleet nodes | Run `labby serve` on multiple machines, enroll non-controller nodes, approve or deny devices, inspect node inventory and MCP client config metadata, run the local OAuth relay on a node, and route status/log events back to the controller over the fleet WebSocket. |
| Logs and activity | Search persisted local runtime logs, tail bounded history, stream live logs to the Labby `/logs` page over SSE, and forward peer syslog batches into the controller when enabled. |
| Labby web UI | Serve the admin UI from the same `labby serve` process as the API and MCP endpoint. The UI covers marketplace, gateway management, registry browsing, logs, setup, activity, settings, docs, and design-system/dev previews. |
| Labby chat | Use the `/chat` web UI as a live ACP client: create/list/resume sessions, send prompts to configured providers, stream session events over SSE, inspect transcript and reasoning/activity lanes, and render tool calls, terminal output, file trees, diffs, code blocks, links, and web previews. |
| TUI plugin manager | Run `lab plugins` to manage local service/plugin installation from a Ratatui interface that reads service metadata and patches `.mcp.json` entries without requiring hand-written MCP config. |
| Generated API docs and catalogs | Use `lab help --json`, MCP `lab://catalog`, per-service action resources, `/v1/{service}/actions`, `/v1/openapi.json`, and `/v1/docs` to discover the exact enabled action surface programmatically. |
| Composable feature set | Pick what Lab exposes at each layer: build only selected integrations with Cargo features, start only selected runtime services with `labby serve --services`, expose only chosen virtual-server surfaces/actions, and deploy only the plugin components you choose to selected devices. |
| Workspace filesystem browser | Browse and preview files under the configured workspace root through the guarded `fs` service for Labby attachment and editor workflows. |
| Setup and health audits | Use `labby init`, `labby doctor`, `labby health`, `labby scaffold service`, and `labby audit onboarding` to bootstrap config, validate service reachability/auth, and keep new integrations aligned with the repo contract. |
| Service operations | Use one action catalog across CLI, MCP, and HTTP to operate Radarr, Sonarr, Plex, UniFi, Unraid, qBittorrent, Gotify, Qdrant, OpenAI-compatible APIs, and the rest of the service integrations. |
| Credential bootstrap | Scan local or SSH appdata paths with `labby extract`, preview diffs, and apply discovered service URLs/API keys into `~/.lab/.env` with backups and atomic writes. |
| Deployment and monitors | Build and push the Lab release binary to SSH targets, manage rollout policy, and use monitor definitions from `plugins/monitors/monitors.json` through `labby deploy monitor`. |

These features are exposed consistently:

- **CLI:** operator commands such as `labby marketplace`, `labby gateway`, `labby nodes`, `labby logs`, `labby doctor`, `labby deploy`, and per-service subcommands.
- **MCP:** compact one-tool-per-service access for agents, with generated action discovery and destructive-action confirmation.
- **HTTP/API:** `/v1/<service>` action dispatch, OpenAPI docs, OAuth/browser sessions, and same-origin Labby integration.
- **Web UI:** Labby pages for marketplace, gateways, logs, registry, setup, activity, and live ACP chat workflows.

## Common Workflows

Browse and install from the official MCP Registry:

```bash
labby marketplace mcp.list --params '{"search":"github","limit":10}'
labby marketplace mcp.install \
  --params '{"name":"io.github.example/server","client_targets":[{"node_id":"local","client":"codex"}],"env_values":{"API_TOKEN":"..."},"confirm":true}' \
  -y
```

Install or cherry-pick marketplace plugin components to devices:

```bash
labby marketplace plugins.list --params '{"runtime":"claude"}'
labby marketplace plugin.cherry_pick \
  --params '{"plugin_id":"ops-pack@homelab","components":["skills/triage/SKILL.md","agents/reviewer.md","commands/deploy.md"],"node_ids":["local","dookie"],"scope":"global","confirm":true}' \
  -y
```

Edit a plugin through the stash workspace and deploy the saved result:

```bash
labby marketplace plugin.workspace --params '{"id":"ops-pack@homelab"}'
labby marketplace plugin.save --params '{"id":"ops-pack@homelab","path":"skills/triage/SKILL.md","content":"..."}'
labby marketplace plugin.deploy.preview --params '{"id":"ops-pack@homelab"}'
labby marketplace plugin.deploy --params '{"id":"ops-pack@homelab","confirm":true}' -y
```

Proxy another MCP server through Lab:

```bash
labby gateway add --name remote-lab --url https://lab2.example.com/mcp --bearer-token-env REMOTE_LAB_TOKEN
labby gateway reload
labby serve --host 127.0.0.1 --port 8765
```

Start Labby and use the web UI:

```bash
just web-build
labby serve --host 127.0.0.1 --port 8765
```

Then open `/marketplace`, `/registry`, `/gateway`, `/logs`, `/setup`, `/activity`,
`/chat`, `/settings`, `/docs`, or `/design-system` on the hosted Lab origin.

Bootstrap and operate a fleet:

```bash
labby extract /mnt/appdata --diff
labby doctor
labby nodes enrollments list
labby logs search dookie oauth
labby deploy plan dookie
```

## Current Limits

- Direct `artifact.fork`, `artifact.diff`, and `artifact.patch` actions are cataloged but currently return `not_implemented`; the update/check/preview/apply model and metadata direction are present, but the lower-level fork/patch lifecycle is not complete.
- ACP agent installs can write controller-local provider config; remote ACP agent installation is limited by the agent distribution and node runtime support, and unsupported remote cases return per-node errors.
- The product HTTP API manages gateway config through `/v1/gateway`, but arbitrary proxied upstream MCP tool calls are exposed through MCP, not through `/v1/*`.
- Labby is served by `labby serve` only when exported static assets exist or `LAB_WEB_ASSETS_DIR` points at them; use `just web-build` to create the export.
- `lab_admin` is compiled by the default `all` feature but remains runtime-gated behind `LAB_ADMIN_ENABLED=1` or `[admin].enabled = true`.

## Current State

Current inventories are generated from code-owned metadata:

- [service catalog](./docs/generated/service-catalog.md)
- [action catalog](./docs/generated/action-catalog.md)
- [environment reference](./docs/generated/env-reference.md)
- [API routes](./docs/generated/api-routes.md)
- [feature matrix](./docs/generated/feature-matrix.md)
- [onboarding audit](./docs/generated/onboarding-audit.md)

Refresh them with `just docs-generate` and verify them with `just docs-check`.
`lab_admin` is compiled by the default `all` feature but only registers in the
runtime registry when `LAB_ADMIN_ENABLED=1` or `[admin].enabled = true`; the
generated docs registry lists it as runtime-conditional.

## Workspace Layout

| Path | Role |
| --- | --- |
| [crates/lab-apis](./crates/lab-apis) | Pure Rust SDK: typed clients, request/response models, auth, shared HTTP behavior, health contracts, plugin metadata |
| [crates/lab](./crates/lab) | Product binary: CLI, MCP, HTTP API, TUI, config loading, dispatch, output rendering, catalog generation |
| [crates/lab-auth](./crates/lab-auth) | HTTP/OAuth auth support used by the hosted runtime |
| [apps/gateway-admin](./apps/gateway-admin/README.md) | Labby admin UI, exported and served by `labby serve` when static assets exist |
| [docs](./docs/README.md) | Topic-based source-of-truth documentation |
| [plugins](./plugins) | Lab plugin and monitor assets used by local workflows |

Core boundary: shared service logic belongs in `lab-apis`; product surfaces and adapters
belong in `lab`.

## Quick Start

The workspace uses Rust 2024 and the root toolchain requirement currently resolves to
Rust 1.90+.

```bash
git clone git@github.com:jmagar/lab.git
cd lab
cargo build --workspace --all-features
cargo install --path crates/lab --bin labby --all-features
```

Secrets and endpoint URLs belong in `~/.lab/.env`. Preferences belong in
`config.toml`, searched in this order: `./config.toml`, `~/.lab/config.toml`,
`~/.config/lab/config.toml`.

Startup reads the first `config.toml`, initializes logging, then loads
`~/.lab/.env` and a CWD `.env` if present. Runtime value precedence is still:

Value precedence is:

1. CLI flags
2. Process environment, including values loaded from `~/.lab/.env`
3. The first `config.toml` found
4. Built-in defaults

Minimal `~/.lab/.env`:

```env
LAB_MCP_HTTP_TOKEN=replace-with-openssl-rand-hex-32

RADARR_URL=http://localhost:7878
RADARR_API_KEY=abc123

SONARR_URL=http://localhost:8989
SONARR_API_KEY=abc123
```

Minimal `~/.lab/config.toml`:

```toml
[mcp]
transport = "http"
host = "127.0.0.1"
port = 8765

[api]
cors_origins = []

[auth]
mode = "bearer"
```

Use [config.example.toml](./config/config.example.toml), [.env.example](./.env.example),
[docs/CONFIG.md](./docs/runtime/CONFIG.md), and [docs/ENV.md](./docs/runtime/ENV.md) for the full
configuration contract.

## Runtime Commands

```bash
lab help
lab help --json
labby serve
labby serve --host 127.0.0.1 --port 8765
labby serve --services radarr,sonarr,plex
labby mcp
labby doctor
labby health
lab plugins
labby extract /mnt/appdata --diff
```

`labby serve` starts the hosted runtime path: the Axum HTTP server for the product API,
OAuth/auth endpoints when configured, the HTTP MCP surface at `/mcp`, and the Labby web
UI when exported assets exist. Use `labby mcp` for stdio MCP clients.

To serve Labby from the same process, build the static export first:

```bash
just web-build
labby serve --host 127.0.0.1 --port 8765
```

Main Labby routes are `/marketplace`, `/registry`, `/gateway`, `/logs`, `/setup`,
`/activity`, `/chat`, `/settings`, `/docs`, and `/design-system`.

`labby mcp` is the stdio-only MCP path for local editor and desktop clients.
It does not start the hosted API or web UI.

## Auth And OAuth

Hosted Lab surfaces are protected by the same auth layer, with deliberately separate
rules for local development, HTTP MCP clients, browser sessions, upstream gateways, and
fleet node enrollment.

HTTP auth modes:

| Mode | Required config | Notes |
| --- | --- | --- |
| Bearer | `LAB_AUTH_MODE=bearer` or default, plus `LAB_MCP_HTTP_TOKEN` for protected deployments | Uses constant-time static bearer-token comparison |
| OAuth | `LAB_AUTH_MODE=oauth`, `LAB_PUBLIC_URL`, `LAB_GOOGLE_CLIENT_ID`, `LAB_GOOGLE_CLIENT_SECRET`, `LAB_AUTH_ADMIN_EMAIL` | Enables Lab's Google-backed auth server, JWT validation, metadata, browser sessions, and callback handling. `LAB_AUTH_ADMIN_EMAIL` is the bootstrap admin Google email; startup fails closed if unset so no Google account can authenticate without explicit permission. Additional users are granted through the SQLite-backed allowlist managed from Labby settings. |

Protected route behavior:

| Surface | Accepted auth | Notes |
| --- | --- | --- |
| `/v1/*` product API | Static bearer token, Lab OAuth JWT bearer token, or Labby browser session cookie | Browser session POSTs use CSRF protection. `LAB_WEB_UI_AUTH_DISABLED` bypasses `/v1` auth only for development. |
| `/mcp` HTTP MCP | Static bearer token or Lab OAuth JWT bearer token | Browser session cookies are not accepted for MCP transport. |
| `/v0.1/*` MCP Registry compatibility routes | Same as protected `/v1` routes | Mounted when the registry feature is enabled. |
| Labby web UI | Browser session in OAuth mode, or the configured development bypass | Static assets and SPA paths are served by `labby serve`; data calls still use `/v1`. |
| `/auth/session` (web UI bootstrap) | Browser session cookie OR static bearer token | When the request carries `Authorization: Bearer <LAB_MCP_HTTP_TOKEN>`, returns a synthetic admin session keyed by `static-bearer`. Lets automation tooling (e.g. `agent-browser --headers`) drive the UI alongside OAuth without disabling it. |
| `/health`, `/ready` | No auth | Intended for probes. |
| `/v1/nodes/hello`, `/v1/nodes/ws` | No bearer middleware | Node WebSocket `initialize` validates the enrolled `device_id` and device token before node methods run. |

OAuth mode exposes `/.well-known/oauth-authorization-server`,
`/.well-known/oauth-protected-resource`, `/jwks`, `/register`, `/authorize`,
`/token`, `/auth/login`, `/auth/session`, `/auth/logout`, and
`/auth/google/callback`. Lab stores Google tokens server-side and issues Lab JWTs for
API/MCP clients.

Gateway OAuth is separate from Lab login OAuth. Upstreams configured with
`[upstream.oauth]` use `/v1/gateway/oauth/start`, `/auth/upstream/callback`,
`/v1/gateway/oauth/status`, and `/v1/gateway/oauth/clear`; credentials are encrypted in
Lab's auth store and shared by the web UI, CLI, and MCP gateway actions under
the shared Gateway subject `gateway`. Protected MCP routes can publish these
OAuth upstreams at public paths such as `https://mcp.example.com/syslog` while
Lab validates public clients with Lab OAuth and separately authenticates to the
upstream server with the stored upstream OAuth credential.

Callback relay helpers cover split-browser/device flows:

```bash
labby oauth relay-local --machine dookie --port 38935
labby oauth relay-local --forward-base http://node.internal.example:38935/callback/dookie --port 38935
```

The same local relay can be started through the node runtime with
`POST /v1/nodes/oauth/relay/start`. Relays forward the final callback request only; they
do not mint tokens, store PKCE state, or complete the OAuth exchange.

When binding HTTP to a non-loopback host, configure bearer or OAuth auth. The server warns
and refuses unsafe exposed configurations. See [docs/OAUTH.md](./docs/runtime/OAUTH.md),
[docs/TRANSPORT.md](./docs/surfaces/TRANSPORT.md), and [docs/GATEWAY.md](./docs/services/GATEWAY.md) for
the full auth contract.

## CLI

Top-level commands are defined in [crates/lab/src/cli.rs](./crates/lab/src/cli.rs):

| Command | Purpose |
| --- | --- |
| `labby serve` | Start the hosted HTTP runtime |
| `labby mcp` | Start the stdio MCP server for local MCP clients |
| `lab help` | Print the generated service and action catalog |
| `labby doctor` | Run comprehensive health audits |
| `labby health` | Run quick reachability checks |
| `labby nodes` | Query nodes from the configured controller |
| `labby logs` | Search fleet or local-master logs |
| `lab plugins` | Open the Ratatui plugin manager |
| `labby marketplace` | Manage Claude Code, Codex, MCP Registry, and ACP Registry marketplace entries |
| `labby gateway` | Manage proxied upstream MCP gateways |
| `labby oauth` | Run local OAuth callback relay helpers |
| `labby extract` | Scan local or SSH appdata paths and extract service credentials |
| `labby audit` | Audit service onboarding against the repo contract |
| `labby scaffold` | Generate a new service onboarding scaffold |
| `labby install` / `labby uninstall` | Patch `.mcp.json` service entries |
| `labby init` | Run first-time setup |
| `lab completions` | Generate shell completions |

Feature-gated services also expose CLI subcommands such as `lab radarr`, `lab unifi`,
`lab qdrant`, and `labby deploy`.

CLI output is human-readable by default. Use global `--json` for machine-readable output
and `--color auto|always|never` for human output styling. See [docs/CLI.md](./docs/surfaces/CLI.md)
and [docs/design/CLI_DESIGN_SYSTEM.md](./docs/design/CLI_DESIGN_SYSTEM.md).

Destructive CLI operations require confirmation. Non-interactive callers use `-y` or
`--yes` where the subcommand exposes it; dry-run capable paths document `--dry-run`.

## MCP Server

Core Lab services use one MCP tool per registered service. Gateway and upstream features
can add healthy upstream tools plus optional `tool_search` and `tool_invoke` helpers.
Core service tool input is:

```json
{
  "action": "movie.search",
  "params": { "query": "Inception" }
}
```

Discovery surfaces:

| Surface | Purpose |
| --- | --- |
| `lab://catalog` MCP resource | Full generated catalog |
| `lab://<service>/actions` MCP resource | Per-service action list |
| `help` action | Per-tool action catalog |
| `schema` action | Parameter schema for one action |

Upstream MCP resources may be merged into the resource list when upstream gateway
proxying is configured.

Destructive MCP actions use elicitation when the client supports it. Headless clients pass
`"confirm": true` inside `params`; otherwise the tool returns `confirmation_required`.

See [docs/MCP.md](./docs/surfaces/MCP.md), [docs/RMCP.md](./docs/surfaces/RMCP.md), and
[docs/TRANSPORT.md](./docs/surfaces/TRANSPORT.md).

## HTTP API

`labby serve` mounts the HTTP API under `/v1` and MCP over HTTP at `/mcp` in hosted mode.
Protected routes accept configured auth: static bearer tokens, OAuth JWT bearer tokens,
and browser session cookies on `/v1` routes. Loopback development can run without auth
when neither bearer nor OAuth auth is configured; non-loopback binds require configured auth.

Unauthenticated routes include:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Liveness |
| `GET` | `/ready` | Readiness |
| `POST` | `/v1/nodes/hello` | Public node self-registration |
| `GET` | `/v1/nodes/ws` | Node WebSocket upgrade; JSON-RPC session validates enrollment |
| `GET` | `/` and SPA paths | Labby web UI when exported assets are available |
| `GET` / `POST` | OAuth metadata and auth paths | Mounted when OAuth auth state is configured |

Protected routes include:

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/v1/{service}/actions` | List generated actions for a service |
| `POST` | `/v1/{service}` | Dispatch one action with `action` and `params` |
| `GET` | `/v1/openapi.json` | OpenAPI 3.1 spec |
| `GET` | `/v1/docs` | Scalar API docs UI |
| `GET` / `POST` | `/v0.1/*` | MCP Registry compatibility routes when the feature is enabled |

Example:

```bash
curl -s -X POST http://127.0.0.1:8765/v1/radarr \
  -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"movie.search","params":{"query":"Inception"}}'
```

Destructive HTTP actions require `"confirm": true` in `params`. Missing confirmation
returns `422` with `kind: "confirmation_required"`. Error responses use the shared
structured envelope and stable `kind` vocabulary from [docs/ERRORS.md](./docs/dev/ERRORS.md).

Middleware order is request id, tracing, request-id propagation, timeout, compression, and
CORS. Loopback origins are allowed by default; add more origins with `LAB_CORS_ORIGINS` or
`[api].cors_origins`.

See [docs/TRANSPORT.md](./docs/surfaces/TRANSPORT.md), [docs/OAUTH.md](./docs/runtime/OAUTH.md),
[docs/ERRORS.md](./docs/dev/ERRORS.md), and [docs/OBSERVABILITY.md](./docs/dev/OBSERVABILITY.md).

## Service Catalogs

Do not maintain action lists by hand in this README. The source-of-truth catalog is generated
from the registry and action specs:

```bash
lab help --json
lab help
curl -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" \
  http://127.0.0.1:8765/v1/radarr/actions
```

Action-level coverage docs live under [docs/coverage](./docs/coverage). The
complete coverage index is [docs/coverage/README.md](./docs/coverage/README.md):

| Service | Coverage Doc |
| --- | --- |
| ACP Registry | [docs/coverage/acp_registry.md](./docs/coverage/acp_registry.md) |
| AdGuard | [docs/coverage/adguard.md](./docs/coverage/adguard.md) |
| Apprise | [docs/coverage/apprise.md](./docs/coverage/apprise.md) |
| Arcane | [docs/coverage/arcane.md](./docs/coverage/arcane.md) |
| ByteStash | [docs/coverage/bytestash.md](./docs/coverage/bytestash.md) |
| Dozzle | [docs/coverage/dozzle.md](./docs/coverage/dozzle.md) |
| FreshRSS | [docs/coverage/freshrss.md](./docs/coverage/freshrss.md) |
| Glances | [docs/coverage/glances.md](./docs/coverage/glances.md) |
| Gotify | [docs/coverage/gotify.md](./docs/coverage/gotify.md) |
| Immich | [docs/coverage/immich.md](./docs/coverage/immich.md) |
| Jellyfin | [docs/coverage/jellyfin.md](./docs/coverage/jellyfin.md) |
| Linkding | [docs/coverage/linkding.md](./docs/coverage/linkding.md) |
| LoggiFly | [docs/coverage/loggifly.md](./docs/coverage/loggifly.md) |
| MCP Registry | [docs/coverage/mcpregistry.md](./docs/coverage/mcpregistry.md) |
| Memos | [docs/coverage/memos.md](./docs/coverage/memos.md) |
| Navidrome | [docs/coverage/navidrome.md](./docs/coverage/navidrome.md) |
| Neo4j | [docs/coverage/neo4j.md](./docs/coverage/neo4j.md) |
| NotebookLM | [docs/coverage/notebooklm.md](./docs/coverage/notebooklm.md) |
| OpenACP | [docs/coverage/openacp.md](./docs/coverage/openacp.md) |
| OpenAI | [docs/coverage/openai.md](./docs/coverage/openai.md) |
| Overseerr | [docs/coverage/overseerr.md](./docs/coverage/overseerr.md) |
| Paperless | [docs/coverage/paperless.md](./docs/coverage/paperless.md) |
| Pi-hole | [docs/coverage/pihole.md](./docs/coverage/pihole.md) |
| Plex | [docs/coverage/plex.md](./docs/coverage/plex.md) |
| Prowlarr | [docs/coverage/prowlarr.md](./docs/coverage/prowlarr.md) |
| qBittorrent | [docs/coverage/qbittorrent.md](./docs/coverage/qbittorrent.md) |
| Qdrant | [docs/coverage/qdrant.md](./docs/coverage/qdrant.md) |
| Radarr | [docs/coverage/radarr.md](./docs/coverage/radarr.md) |
| SABnzbd | [docs/coverage/sabnzbd.md](./docs/coverage/sabnzbd.md) |
| Scrutiny | [docs/coverage/scrutiny.md](./docs/coverage/scrutiny.md) |
| Sonarr | [docs/coverage/sonarr.md](./docs/coverage/sonarr.md) |
| Stash | [docs/coverage/stash.md](./docs/coverage/stash.md) |
| Tailscale | [docs/coverage/tailscale.md](./docs/coverage/tailscale.md) |
| Tautulli | [docs/coverage/tautulli.md](./docs/coverage/tautulli.md) |
| TEI | [docs/coverage/tei.md](./docs/coverage/tei.md) |
| UniFi | [docs/coverage/unifi.md](./docs/coverage/unifi.md) |
| Unraid | [docs/coverage/unraid.md](./docs/coverage/unraid.md) |
| Uptime Kuma | [docs/coverage/uptime_kuma.md](./docs/coverage/uptime_kuma.md) |

## Environment Reference

Service credentials follow the pattern `{SERVICE}_URL`, `{SERVICE}_API_KEY`,
`{SERVICE}_TOKEN`, `{SERVICE}_USERNAME`, and `{SERVICE}_PASSWORD`. Multi-instance
services insert the label before the suffix: `UNRAID_NODE2_URL`,
`UNRAID_NODE2_API_KEY`.

The complete generated service env inventory is
[docs/generated/env-reference.md](./docs/generated/env-reference.md). Its JSON
contract is [docs/generated/env-reference.json](./docs/generated/env-reference.json).

Server and runtime env:

| Variable | Purpose |
| --- | --- |
| `LAB_MCP_HTTP_TOKEN` | Static bearer token for protected HTTP routes and HTTP MCP |
| `LAB_MCP_TRANSPORT` | Default transport, `http` or `stdio` |
| `LAB_MCP_HTTP_HOST` / `LAB_MCP_HTTP_PORT` | Hosted runtime bind address |
| `LAB_MCP_SESSION_TTL_SECS` / `LAB_MCP_STATEFUL` | HTTP MCP session behavior |
| `LAB_MCP_ALLOWED_HOSTS` | Additional DNS-rebinding allowed hosts |
| `LAB_CORS_ORIGINS` | Additional browser CORS origins |
| `LAB_AUTH_MODE` | `bearer` or `oauth` |
| `LAB_PUBLIC_URL` | Public base URL for OAuth metadata, JWT issuer/audience, callback construction, and allowed-host derivation |
| `LAB_GOOGLE_CLIENT_ID` / `LAB_GOOGLE_CLIENT_SECRET` | Google OAuth credentials for OAuth mode |
| `LAB_GOOGLE_CALLBACK_PATH` | Optional Google callback path override |
| `LAB_AUTH_ALLOWED_REDIRECT_URIS` | Optional non-loopback MCP OAuth callback allowlist |
| `LAB_AUTH_ADMIN_EMAIL` | Bootstrap admin Google email; required for OAuth mode (fail-closed default) |
| `LAB_WEB_ASSETS_DIR` | Static Labby export directory override |
| `LAB_WEB_UI_AUTH_DISABLED` | Development-only bypass for Labby browser auth. `LAB_WEB_UI_DISABLE_AUTH` is accepted as a legacy alias. |
| `LAB_LOG` / `LAB_LOG_FORMAT` | Tracing filter and text/json log format |
| `LAB_ADMIN_ENABLED` | Runtime opt-in for the `lab_admin` MCP tool |

## Feature Flags

`lab` defaults to `all`. `lab-apis` defaults to no optional upstream services.

Feature-gated upstream integrations:

`radarr`, `sonarr`, `prowlarr`, `overseerr`, `plex`, `tautulli`, `sabnzbd`,
`qbittorrent`, `tailscale`, `unraid`, `unifi`, `arcane`, `linkding`, `memos`,
`bytestash`, `paperless`, `gotify`, `apprise`, `openai`, `qdrant`, `tei`,
`deploy`, `mcpregistry`, `acp_registry`, `fs`, `lab-admin`.

Always-on product/capability services include `extract`, `gateway`, `doctor`, `logs`,
`device`, `marketplace`, and `acp`. `lab_admin` is feature-enabled by `all` but
runtime-gated.

Build a subset:

```bash
cargo build -p labby --no-default-features --features radarr,sonarr,plex
```

## Development

Use the `just` aliases when possible:

```bash
just check            # cargo check --workspace --all-features
just test             # cargo nextest run --workspace --all-features
just test-integration # cargo nextest run --workspace --all-features -- --ignored
just lint             # cargo clippy --workspace --all-features -- -D warnings; cargo fmt --all -- --check
just deny             # cargo deny check
just build            # cargo build --workspace --all-features
just build-release    # cargo build --workspace --all-features --release; install bin/labby
just web-build        # cd apps/gateway-admin && pnpm build
just web-watch        # rebuild Labby static assets on frontend changes
just run -- help      # cargo run --all-features -- <args>
just chat-local       # local Labby chat workflow with auth disabled for development
just dev-up           # bring up the docker dev container (first-time start)
just dev              # release rebuild + hot-swap binary into the dev container
just dev-debug        # nightly+cranelift debug rebuild + hot-swap (3x faster compile)
just fmt              # cargo fmt --all
just clean            # cargo clean
just release          # cargo release
just mcp-token        # rotate LAB_MCP_HTTP_TOKEN in .env
```

The dev container (`docker-compose.yml` + `docker-compose.dev.yml`) pre-installs the three ACP adapters (`claude-agent-acp`, `codex-acp`, `gemini`) into the image at fixed versions, with an npm `overrides` entry floating `@anthropic-ai/claude-agent-sdk` forward of the version `claude-agent-acp` pins. This eliminates per-spawn `npx` overhead and avoids credential/binary-version mismatches that otherwise SIGILL the bundled Claude Code binary. Bumping any adapter version requires rebuilding the image (`docker compose build labby-master`); changing only the labby binary uses `just dev` or `just dev-debug` and is immediate.

Driving the UI as automation while OAuth is enabled: pass the static bearer token as a header. The `/auth/session` endpoint recognizes the same token and returns a synthetic admin session, so the AuthBootstrap treats the caller as logged in:

```bash
TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" .env | cut -d= -f2)
agent-browser open http://localhost:8765/chat \
  --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
```

Authoritative verification is all-features:

```bash
cargo check --workspace --all-features
cargo clippy --workspace --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo build --workspace --all-features
```

Use `cargo test` only for narrow local slices or when a tool specifically requires it.
The repo-level test contract is [docs/TESTING.md](./docs/dev/TESTING.md).

## Docs Map

Start at [docs/README.md](./docs/README.md). Topic ownership:

| Doc | Owns |
| --- | --- |
| [docs/ARCH.md](./docs/ARCH.md) | Crate split, runtime surfaces, shared contracts, runtime flow |
| [docs/TECH.md](./docs/TECH.md) | Stack choices, toolchain, feature posture, verification surfaces, release tooling |
| [docs/CONVENTIONS.md](./docs/CONVENTIONS.md) | Locked engineering rules, async style, HTTP, testing, docs, privacy |
| [docs/SERVICES.md](./docs/dev/SERVICES.md) | Service inventory, feature gates, metadata, multi-instance model |
| [docs/SERVICE_ONBOARDING.md](./docs/dev/SERVICE_ONBOARDING.md) | End-to-end checklist for adding a service |
| [docs/SCAFFOLD_AND_AUDIT.md](./docs/dev/SCAFFOLD_AND_AUDIT.md) | `labby scaffold service` and `labby audit onboarding` contract |
| [docs/CLI.md](./docs/surfaces/CLI.md) | CLI behavior, command rules, confirmations, operator commands |
| [docs/design/CLI_DESIGN_SYSTEM.md](./docs/design/CLI_DESIGN_SYSTEM.md) | Human-readable CLI output language and color policy |
| [docs/design/CLI_OUTPUT_THEME_API.md](./docs/design/CLI_OUTPUT_THEME_API.md) | Proposed Rust API for CLI semantic styling |
| [docs/MCP.md](./docs/surfaces/MCP.md) | MCP transport model, one-tool-per-service design, discovery, envelopes |
| [docs/RMCP.md](./docs/surfaces/RMCP.md) | RMCP SDK integration contract |
| [docs/TRANSPORT.md](./docs/surfaces/TRANSPORT.md) | Stdio and streamable HTTP transport, sessions, CORS, DNS rebinding |
| [docs/OAUTH.md](./docs/runtime/OAUTH.md) | Bearer vs OAuth auth, Google flow, JWTs, JWKS, metadata, callback forwarding |
| [docs/CONFIG.md](./docs/runtime/CONFIG.md) | Env/TOML ownership, load order, secrets, instance naming |
| [docs/ENV.md](./docs/runtime/ENV.md) | Deployment-ready env examples and auth mode variables |
| [docs/ERRORS.md](./docs/dev/ERRORS.md) | Stable error taxonomy, envelopes, status mapping |
| [docs/design/SERIALIZATION.md](./docs/design/SERIALIZATION.md) | Serde ownership, stable envelopes, output-boundary rules |
| [docs/DISPATCH.md](./docs/dev/DISPATCH.md) | Surface-neutral dispatch ownership and adapter direction |
| [docs/SERVICE_LAYER_MIGRATION.md](./docs/dev/SERVICE_LAYER_MIGRATION.md) | Migration phases for shared dispatch/service layer |
| [docs/OBSERVABILITY.md](./docs/dev/OBSERVABILITY.md) | Logging boundaries, required fields, correlation, redaction, verification |
| [docs/OPERATIONS.md](./docs/OPERATIONS.md) | Repo helpers, doctor/health workflows, CI, releases, updates |
| [docs/CICD.md](./docs/runtime/CICD.md) | GitHub Actions check matrix and release behavior |
| [docs/TESTING.md](./docs/dev/TESTING.md) | Test runner contract and verification expectations |
| [docs/EXTRACT.md](./docs/services/EXTRACT.md) | Bootstrap credential extraction and `.env` merge semantics |
| [docs/GATEWAY.md](./docs/services/GATEWAY.md) | Upstream MCP gateway CRUD, reload/test flows, exposure policy |
| [docs/UPSTREAM.md](./docs/services/UPSTREAM.md) | Upstream MCP proxy setup, tool merging, circuit breaker, resources |
| [docs/MARKETPLACE.md](./docs/services/MARKETPLACE.md) | Marketplace service, plugin workspace mirrors, save/deploy flows |
| [docs/MCPREGISTRY_METADATA.md](./docs/services/MCPREGISTRY_METADATA.md) | Lab-owned metadata layered onto MCP Registry entries |
| [docs/acp/README.md](./docs/acp/README.md) | ACP service architecture and chat boundary |
| [docs/acp/design.md](./docs/acp/design.md) | ACP design details |
| [docs/acp/research-findings.md](./docs/acp/research-findings.md) | ACP research notes |
| [docs/DEVICE_RUNTIME.md](./docs/runtime/DEVICE_RUNTIME.md) | Master/non-master runtime roles and device inventory |
| [docs/NODES.md](./docs/runtime/NODES.md) | Node-facing CLI/API behavior |
| [docs/NODE_RUNTIME_CONTRACT.md](./docs/runtime/NODE_RUNTIME_CONTRACT.md) | Controller/node split and node artifact rules |
| [docs/FLEET_METHODS.md](./docs/runtime/FLEET_METHODS.md) | Fleet WebSocket JSON-RPC method contract |
| [docs/FLEET_LOGS.md](./docs/runtime/FLEET_LOGS.md) | Fleet log ingestion, queueing, search, storage limits |
| [docs/LOCAL_LOGS.md](./docs/services/LOCAL_LOGS.md) | Local-master runtime log store, `/v1/logs`, SSE streaming |
| [docs/DEPLOY.md](./docs/runtime/DEPLOY.md) | Device-runtime deployment topology and rollout guidance |
| [docs/DEPLOY_SERVICE.md](./docs/runtime/DEPLOY_SERVICE.md) | Deploy service action/API contract |
| [docs/MONITORS.md](./docs/services/MONITORS.md) | Claude Code monitor definitions and `labby deploy monitor` |
| [docs/TUI.md](./docs/surfaces/TUI.md) | Ratatui plugin manager behavior and `.mcp.json` patching |
| [apps/gateway-admin/README.md](./apps/gateway-admin/README.md) | Labby frontend workflow and static export model |
| [docs/design/component-development.md](./docs/design/component-development.md) | Labby component workflow and browser verification |
| [docs/design/design-system-contract.md](./docs/design/design-system-contract.md) | Labby Aurora design-system contract |
| [docs/design/CLAUDE_CODE_AURORA_THEME.md](./docs/design/CLAUDE_CODE_AURORA_THEME.md) | Aurora theme mapping for Claude Code-like surfaces |

Supporting directories:

| Path | Purpose |
| --- | --- |
| [docs/coverage](./docs/coverage) | Per-service coverage and action mapping |
| [docs/upstream-api](./docs/upstream-api/README.md) | Upstream specs and reference material |
| [docs/features](./docs/features/FEATURE_BRIEF.md) | Product feature briefs and implementation notes |
| [docs/reviews](./docs/reviews) | Review artifacts |
| [docs/reports](./docs/reports) | Verification and audit reports |
| [docs/sessions](./docs/sessions) | Historical session notes |
| [docs/superpowers/plans](./docs/superpowers/plans) | Historical implementation plans |
| [docs/superpowers/specs](./docs/superpowers/specs) | Historical implementation specs |
| [docs/mockups](./docs/mockups) | Static UI mockups |

## Design Highlights

- One binary exposes many integrations without exploding the MCP tool list.
- The generated action catalog drives CLI help, MCP discovery, HTTP action listings, and UI surfaces.
- CLI, MCP, and HTTP dispatch share action semantics instead of duplicating business logic.
- Destructive actions have a single metadata flag and separate confirmations per surface.
- Structured error envelopes use stable `kind` tags across agent-facing surfaces.
- Multi-instance service selection is a config-layer concern, not hardcoded per service.
- Labby is served from the same hosted runtime when static assets exist; the separate Next dev server is only for frontend development.
- No telemetry or analytics are built in; network calls are explicit service/API operations.

## License

Workspace metadata declares `MIT OR Apache-2.0`.
