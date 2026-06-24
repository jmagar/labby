# Lab

`lab` is the Rust workspace behind **Labby**, a local-first control plane for
agent tooling and homelab operations. One binary, `labby`, exposes the same
operator capabilities through a CLI, an MCP server, an HTTP API, and the Labby
web UI.

The root README is the public entrypoint. The topic docs in
[docs/](./docs/README.md) own the detailed contracts; when this file and a topic
doc disagree, fix the topic doc first and then refresh this summary.

## Contents

- [What Lab Does](#what-lab-does)
- [Quick Start](#quick-start)
- [Core Workflows](#core-workflows)
- [Runtime Surfaces](#runtime-surfaces)
- [Configuration](#configuration)
- [Current Catalogs](#current-catalogs)
- [Architecture](#architecture)
- [Development](#development)
- [Documentation](#documentation)

## What Lab Does

Labby is centered on the current gateway/operator surface:

- **MCP gateway** - connect HTTP and stdio upstream MCP servers, inspect their
  tools/resources/prompts, apply exposure filters, publish protected MCP routes,
  and optionally collapse the upstream catalog into Code Mode `search` and
  `execute`.
- **Marketplace and registry** - browse Claude/Codex plugin marketplaces, the
  official MCP Registry, and the ACP Agent Registry; install plugins, MCP
  servers, and ACP providers through explicit target-aware workflows.
- **Stash workspaces** - mirror installable artifacts into `~/.lab/stash`, edit
  and version component snapshots, preview deployment diffs, and deploy saved
  artifacts back to configured targets.
- **ACP chat** - run provider-backed Agent Client Protocol sessions, stream and
  persist events, expose the `/chat` web UI, and keep the backend service named
  `acp`.
- **Fleet, setup, logs, and deployment** - run `labby serve` as a controller or
  node, enroll devices, search local/fleet logs, audit setup health, and deploy
  the local release binary to SSH targets.
- **Generated discovery** - publish code-owned service, action, environment,
  API route, OpenAPI, MCP help, CLI help, and feature-matrix artifacts under
  [docs/generated](./docs/generated/README.md).

Lab no longer exposes the old Radarr/Sonarr/Plex-style service catalog in this
branch. Use the generated catalogs below for the current surface instead of
copying command or action lists by hand.

## Quick Start

### Install A Release

Linux/macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh
labby setup
labby serve --host 127.0.0.1 --port 8765
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.ps1 | iex
labby setup
labby serve --host 127.0.0.1 --port 8765
```

The install scripts download the requested GitHub Release asset, verify its
checksum, and install `labby` onto the user PATH. Override with
`LAB_INSTALL_DIR`, `LAB_INSTALL_VERSION`, or `LAB_INSTALL_REPO`.

### Build From Source

Prerequisites:

- Rust 1.92 or newer. CI/release currently verifies with Rust 1.94.1.
- `just` for repo commands.
- `cargo-nextest` for the main test suite.
- `pnpm 9.15.9` for the Labby web UI. The repo pins this in
  [.mise.toml](./.mise.toml) and
  [apps/gateway-admin/package.json](./apps/gateway-admin/package.json).
- `openssl` if you want to generate a bearer token manually.

```bash
git clone https://github.com/jmagar/lab.git
cd lab
just install
just web-build
labby serve --host 127.0.0.1 --port 8765
```

`just install` builds the all-features release binary and symlinks it to
`~/.local/bin/labby`.

### First Run

For loopback development, `labby serve` can bootstrap a missing bearer token for
you. If `LAB_MCP_HTTP_TOKEN` is absent and `LAB_AUTH_MODE` is not `oauth`, it
generates a token, writes a minimal `~/.lab/.env`, reloads it into the running
process, prints the setup URL, and continues. The token itself is stored in
`~/.lab/.env` rather than printed.

For explicit setup:

```bash
mkdir -p ~/.lab
printf 'LAB_AUTH_MODE=bearer\nLAB_MCP_HTTP_TOKEN=%s\n' "$(openssl rand -hex 32)" > ~/.lab/.env
chmod 600 ~/.lab/.env
labby setup
labby serve --host 127.0.0.1 --port 8765
```

Open `http://127.0.0.1:8765/setup` or `http://127.0.0.1:8765/`.
Build static Labby assets with `just web-build` first when running from a source
checkout.

## Core Workflows

### Start Labby

```bash
labby serve --host 127.0.0.1 --port 8765
labby mcp
```

`labby serve` starts the hosted HTTP runtime: `/v1` product APIs, `/mcp`
streamable HTTP MCP, auth routes, node runtime endpoints, and static Labby web
assets when an export is available. `labby mcp` is the stdio MCP entrypoint for
local MCP clients.

### Manage Upstream MCP Gateways

```bash
labby gateway add \
  --name github \
  --url https://example.com/mcp \
  --bearer-token-env GITHUB_MCP_TOKEN \
  -y

labby gateway reload
labby gateway list
```

Stdio upstreams execute local commands when tested or reconciled, so gateway
tests and config mutations use the shared destructive-action confirmation gate.
The stdio spawn guard allows known runtimes such as `npx`, `uvx`, `docker`,
`node`, `python`, `python3`, `deno`, `pipx`, and `dnx`; customize it in
`[gateway]` inside `config.toml`.

### Use Code Mode

When `[code_mode].enabled = true`, Lab hides raw proxied upstream tools from MCP
`list_tools()` and exposes the canonical synthetic `codemode` tool.

```bash
labby gateway code status
labby gateway code enable
labby gateway code exec --code 'async () => tools.length'
```

MCP call shapes:

```json
{ "code": "async () => (await codemode.search(\"github issues\")).results" }
```

```json
{ "code": "async () => callTool(\"github::search_issues\", {\"query\":\"repo:jmagar/lab gateway\"})" }
```

```json
{ "code": "async () => codemode.run(\"gateway-summary\", {\"includeHealth\": true})" }
```

Code Mode can call exposed upstream MCP tools only. It cannot call Lab actions
from inside the sandbox.

### Browse And Install Agent Tooling

```bash
labby marketplace sources.list --json
labby marketplace plugins.list --params '{"runtime":"claude"}'
labby marketplace mcp.list --params '{"search":"postgres","limit":10}'
labby marketplace agent.list
```

Destructive install/update/deploy operations require explicit confirmation:

```bash
labby marketplace mcp.install \
  --params '{"name":"io.github.user/server","gateway_ids":["default"],"confirm":true}' \
  -y
```

Marketplace actions cover Claude/Codex plugins, MCP Registry servers, ACP agents,
artifact fork/update flows, and device-aware installation targets.

### Work With Stash

```bash
labby stash help
```

The `stash` service manages versioned components, provider metadata, target
config, import/export, diffs, and deploy previews for Lab-managed artifact
workspaces.

### Operate The Fleet And Logs

```bash
labby doctor system
labby nodes list
labby logs search dookie oauth
labby deploy plan dookie
```

Every supported node runs `labby serve`. One node acts as controller; other nodes
report status, inventory, and logs back through `/v1/nodes/*`.

### Drive The API

Generic action dispatch:

```bash
curl -s -X POST http://127.0.0.1:8765/v1/gateway \
  -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"action":"gateway.list","params":{}}'
```

Dedicated product routes also exist for catalog discovery, ACP sessions/events,
setup, stash, logs, gateway OAuth, auth allowlists, OpenAPI, and compatibility
routes. See [generated API routes](./docs/generated/api-routes.md) and
[OpenAPI](./docs/generated/openapi.json).

## Runtime Surfaces

| Surface | Entry Point | Notes |
| --- | --- | --- |
| CLI | `labby <command>` | Current commands are generated in [docs/generated/cli-help.md](./docs/generated/cli-help.md). Use `--json` for machine-readable output and `--color auto|plain|color` for human output styling. |
| MCP stdio | `labby mcp` | Local editor/desktop MCP clients. |
| MCP HTTP | `labby serve` plus `/mcp` | Streamable HTTP MCP with bearer or OAuth JWT auth. |
| HTTP API | `labby serve` plus `/v1/*` | Generic `POST /v1/{service}` action dispatch plus dedicated product routes. |
| Web UI | `labby serve` plus exported assets | Main routes include `/setup`, `/marketplace`, `/gateways`, `/logs`, `/activity`, `/chat`, `/settings`, `/docs`, and `/design-system`; `/registry` redirects to marketplace. |

MCP service tools use the shared action shape:

```json
{
  "action": "mcp.list",
  "params": { "search": "postgres", "limit": 10 }
}
```

Every service tool also supports `help` and `schema` through the shared
dispatcher. Destructive MCP actions use elicitation when the client supports it;
headless clients pass `"confirm": true` inside `params`.

## Configuration

Configuration is split deliberately:

| Data | Location | Examples |
| --- | --- | --- |
| Secrets and endpoint values | `~/.lab/.env` | `LAB_MCP_HTTP_TOKEN`, `LAB_GOOGLE_CLIENT_SECRET`, upstream bearer token env values |
| Preferences | `config.toml` | transport, CORS, auth mode, workspace root, gateway spawn guard, registry URLs |

`config.toml` is searched in this order:

1. `./config.toml`
2. `~/.lab/config.toml`
3. `~/.config/lab/config.toml`

Startup loads the first `config.toml`, initializes tracing, then loads
`~/.lab/.env` and a CWD `.env` if present. Runtime precedence is:

1. CLI flags
2. Environment variables
3. `config.toml`
4. Built-in defaults

Useful environment variables:

| Variable | Purpose |
| --- | --- |
| `LAB_MCP_HTTP_TOKEN` | Static bearer token for protected admin/API/MCP routes. |
| `LAB_AUTH_MODE` | `bearer` or `oauth`. |
| `LAB_PUBLIC_URL` | Public base URL for OAuth metadata, issuer/audience, callbacks, and allowed-host derivation. |
| `LAB_GOOGLE_CLIENT_ID` / `LAB_GOOGLE_CLIENT_SECRET` | Google OAuth credentials for OAuth mode. |
| `LAB_AUTH_ADMIN_EMAIL` | Bootstrap admin email; required in OAuth mode. |
| `LAB_OAUTH_ENCRYPTION_KEY` | Base64 32-byte key required for encrypted upstream OAuth credentials. Rotation requires reauthorizing affected upstreams. |
| `LAB_WEB_ASSETS_DIR` | Override static Labby export directory. |
| `LAB_WEB_UI_AUTH_DISABLED` | Development-only browser auth bypass. |
| `LAB_LOG` / `LAB_LOG_FORMAT` / `LAB_LOG_COLOR` | Tracing filter, text/json format, and non-TTY color policy. |
| `LAB_LOG_DIR` | Optional rolling JSON file log directory. |
| `LAB_LOCAL_LOGS_STORE_PATH` | SQLite store path for the local activity log service. |
| `LAB_ACTOR_KEY_SECRET` | Stable secret for redacted actor correlation in logs. |
| `LAB_ADMIN_ENABLED` | Runtime opt-in for the `lab_admin` tool. |

Bearer auth is an operator/admin shortcut for Lab routes. Public protected MCP
routes validate route-scoped Lab OAuth JWTs; do not treat `LAB_MCP_HTTP_TOKEN` as
a public resource credential.

When driving the web UI with automation while OAuth is enabled, pass the bearer
token as a same-origin header. `/auth/session` recognizes that token and returns a
synthetic admin session:

```bash
TOKEN=$(awk -F= '/^LAB_MCP_HTTP_TOKEN=/{print $2}' ~/.lab/.env)
agent-browser open http://127.0.0.1:8765/chat \
  --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
```

See [runtime configuration](./docs/runtime/CONFIG.md),
[environment variables](./docs/runtime/ENV.md), and
[OAuth](./docs/runtime/OAUTH.md).

## Current Catalogs

Do not maintain action, feature, env, or coverage inventories by hand in this
README. The generated artifacts are authoritative for the current branch:

| Artifact | Purpose |
| --- | --- |
| [service-catalog.md](./docs/generated/service-catalog.md) | Registered services, exposure, features, categories, and surfaces. |
| [action-catalog.md](./docs/generated/action-catalog.md) | Per-service actions and destructive metadata. |
| [env-reference.md](./docs/generated/env-reference.md) | Env vars generated from service metadata. |
| [api-routes.md](./docs/generated/api-routes.md) | Mounted HTTP routes. |
| [openapi.json](./docs/generated/openapi.json) | OpenAPI 3.1 schema. |
| [feature-matrix.md](./docs/generated/feature-matrix.md) | Cargo feature invariants. |
| [mcp-help.md](./docs/generated/mcp-help.md) | MCP help projection. |
| [cli-help.md](./docs/generated/cli-help.md) | Clap command help snapshot. |

Refresh and verify them with:

```bash
just docs-generate
just docs-check
```

`docs-check` verifies generated-artifact freshness and invariants. It is not a
Markdown link checker, live health check, or onboarding policy audit.

## Architecture

The workspace uses Rust 2024, resolver 3, and a single workspace version.

| Path | Role |
| --- | --- |
| [crates/labby-apis](./crates/labby-apis) | Pure SDK/domain crate for shared models, auth primitives, metadata, registry clients, ACP types, setup/doctor/stash/marketplace/device/deploy types. |
| [crates/labby-auth](./crates/labby-auth) | OAuth/JWT/session middleware, route support, and upstream OAuth runtime. |
| [crates/labby-runtime](./crates/labby-runtime) | Surface-neutral contracts and helpers: `ToolError`, gateway config DTOs, dispatch helpers, redaction, path safety, and security helpers. |
| [crates/labby-codemode](./crates/labby-codemode) | Client-neutral Code Mode runner kernel, broker, result shaping, snippets, and TypeScript descriptor generation. |
| [crates/labby-gateway](./crates/labby-gateway) | Gateway manager, upstream MCP proxy pool, Code Mode host adapter, discovery/imports, virtual servers, protected routes, and OAuth lifecycle. |
| [crates/labby-web](./crates/labby-web) | Embedded/filesystem web asset serving with symlink escape defense. |
| [crates/labby](./crates/labby) | Product binary crate: CLI, MCP, HTTP API, config loading, product dispatch, ACP orchestration, logs, setup, and output rendering. |
| [crates/labby-winjob](./crates/labby-winjob) | Windows Job Object process-tree support, isolated so the main workspace can keep `unsafe_code = "forbid"`. |
| [apps/gateway-admin](./apps/gateway-admin/README.md) | Labby web UI, statically exported and served by `labby serve`. |
| [plugins](./plugins) | Claude/Codex plugin assets, skills, hooks, and monitor definitions. |
| [docs](./docs/README.md) | Topic documentation and generated inventories. |

Shared behavior belongs in the shared execution layer. Upstream/domain logic
belongs in `labby-apis`; reusable gateway/runtime/code-mode behavior belongs in
the extracted `labby-*` crates; product dispatch belongs in
`crates/labby/src/dispatch`; CLI, MCP, HTTP, and web adapters stay thin. See
[Architecture](./docs/ARCH.md) and [Dispatch](./docs/dev/DISPATCH.md).

## Development

Prefer the `just` aliases:

```bash
just check            # cargo check --workspace --all-features
just test             # cargo nextest run --workspace --all-features
just test-integration # cargo nextest run --workspace --all-features --run-ignored ignored-only
just lint             # skill drift + cargo wrapper smoke + clippy -D warnings + fmt check
just deny             # cargo deny check
just build            # cargo build --workspace --all-features
just build-release    # release build, bin/labby install, ~/.local/bin symlink
just host-service-install # install/start labby.service under systemd --user
just host-sync        # release-fast rebuild + install ~/.local/bin/labby + restart host service
just host-service-status # inspect the host Labby gateway service
just dev-container    # explicit Docker compatibility/prod-like smoke path
just dev-container-debug # explicit Docker debug binary path
just web-build        # cd apps/gateway-admin && pnpm build
just web-watch        # rebuild web assets when frontend files change
just run -- help      # cargo run --all-features -- <args>
just chat-local       # local Labby chat workflow with browser auth disabled
just dev-up           # start the explicit Docker compatibility stack
just dev              # alias for just dev-container
just dev-debug        # alias for just dev-container-debug
just install          # build-release + symlink ~/.local/bin/labby
just prod-run         # local prod-like image smoke on port 18765
just mcp-token        # rotate LAB_MCP_HTTP_TOKEN in .env
```

Authoritative Rust verification is all-features:

```bash
cargo check --workspace --all-features
cargo clippy --workspace --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo build --workspace --all-features
```

CI uses the same posture and runs nextest with its CI profile. Use `cargo test`
only for narrow local slices or when a tool specifically requires it.

Frontend changes should also run the relevant `pnpm` scripts under
`apps/gateway-admin`, and `just web-build` when exported assets matter.

### Host Gateway Runtime

The default local and dookie gateway runtime is the host user service:
`~/.local/bin/labby serve` managed by `systemd --user` as `labby.service`.
This keeps stdio MCP tools, SSH config, local binaries, agent caches, and
credentials in the same namespace as the gateway. Use `just host-service-install`
once, then `just host-sync` for ordinary Rust changes. Docker remains available
for prod-like image smoke and adapter-container work, but it is no longer the
preferred agent gateway runtime.

### Dev Container

The Compose stack is a trusted local operator environment, not a hardened
generic deployment. It bind-mounts host Lab state, agent credentials/plugin
caches, the repo, and built web assets; secrets are loaded from the mounted
`/home/lab/.lab/.env`. The image pre-installs ACP adapters
(`claude-agent-acp`, `codex-acp`, `gemini`) so session spawns use deterministic
local binaries rather than repeated `npx` installs. Rebuild the image when
changing Dockerfiles or adapter versions; use `just dev` or `just dev-debug`
for ordinary Labby binary swaps.

### Releases

Release prep is version/changelog first, then tag:

1. Bump the workspace version in [Cargo.toml](./Cargo.toml).
2. Update `CHANGELOG.md` when present.
3. Regenerate docs and web assets when relevant.
4. Push a `vX.Y.Z` tag.

The release workflow builds Linux and Windows archives with checksums, publishes
the GitHub Release, pushes GHCR images, and includes the generated marketplace
artifact.

### ACP Runtime Notes

The Rust ACP SDK is pinned exactly in
[crates/labby/Cargo.toml](./crates/labby/Cargo.toml) as
`agent-client-protocol = "=0.13.1"` with the `unstable` feature. Model/config
discovery depends on reading `session_config_options()` from the raw
`NewSessionResponse` before `attach_session`, and model switching uses
`SetSessionConfigOptionRequest`. Re-check those APIs before upgrading the SDK.

### Plugin Setup Hooks

The `plugins/labby` plugin ships skills, commands, and MCP config, not a
`labby` binary. SessionStart runs `labby setup plugin-hook --no-repair` when
`labby` is on PATH and prints an install pointer otherwise. ConfigChange runs
`labby setup plugin-hook` to sync settings. Hooks should stay advisory: no binary
bundling, no auto-install, and no Docker/systemd bootstrap.

## Documentation

Start at [docs/README.md](./docs/README.md). High-value entrypoints:

- [Architecture](./docs/ARCH.md)
- [Configuration](./docs/runtime/CONFIG.md)
- [Environment](./docs/runtime/ENV.md)
- [OAuth](./docs/runtime/OAUTH.md)
- [Transport](./docs/surfaces/TRANSPORT.md)
- [Gateway](./docs/services/GATEWAY.md)
- [Marketplace](./docs/services/MARKETPLACE.md)
- [ACP](./docs/acp/README.md)
- [Testing](./docs/dev/TESTING.md)
- [Operations](./docs/OPERATIONS.md)

## License

Workspace metadata declares `MIT OR Apache-2.0`.
