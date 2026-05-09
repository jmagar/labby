# CLI

The CLI is the human-facing surface for `lab`. It must remain thin, predictable, and strongly aligned with the underlying service clients.

## Design Rules

- command parsing belongs in `lab`
- service logic belongs in `lab-apis`
- output formatting belongs in the output layer
- destructive commands require explicit confirmation

## Top-Level Commands

The CLI includes:

- one subcommand per service
- `mcp`
- `nodes`
- `logs`
- `serve`
- `gateway`
- `marketplace`
- `stash`
- `plugins`
- `setup`
- `install`
- `uninstall`
- `init`
- `health`
- `doctor`
- `audit`
- `scaffold`
- `extract`
- `oauth`
- `help`
- `completions`

Representative command tree:

```text
lab
├── <service> ...
├── mcp
├── nodes
├── logs
├── serve
├── gateway
├── marketplace
├── stash
├── plugins
├── setup
├── install
├── uninstall
├── init
├── health
├── doctor
├── audit
├── scaffold
├── extract
├── oauth
├── help
└── completions
```

## Per-Service Commands

Each service subcommand must expose operations in a way that mirrors the service model cleanly.

Examples:

- `lab radarr movie-lookup --params '{"query":"The Matrix"}'`
- `lab sonarr series.list`
- `lab plex library.list`
- `lab unraid system.array`
- `labby openai models`
- `lab qdrant collections.list`
- `labby marketplace mcp.meta.set --params '{"name":"io.github.user/server","metadata":{"curation":{"featured":true},"trust":{"reviewed":true}}}'`

The CLI must not invent a second semantic model that drifts from MCP or the SDK.

## Output Formats

Supported output modes are:

- human-readable terminal output
- JSON

The canonical serialization and output-boundary contract lives in [design/SERIALIZATION.md](./design/SERIALIZATION.md).
The canonical human-readable output language and color policy live in [design/CLI_DESIGN_SYSTEM.md](./design/CLI_DESIGN_SYSTEM.md).

Rules:

- human-readable output is the default for interactive TTY use
- JSON is the machine-readable mode for pipes and automation
- `lab-apis` types stay presentation-free
- CLI wrappers or local row types handle human rendering

## Color and TTY Behavior

- use `owo-colors`
- disable color automatically when stdout is not a TTY
- honor `NO_COLOR`
- expose a shared `--color=auto|plain|color` policy rather than per-command color toggles

Examples:

```bash
# default interactive behavior
labby doctor

# force plain text even on a TTY
labby doctor --color=plain

# force styling for pagers like less -R
labby doctor --color=color | less -R

# pipes stay plain by default in auto mode
labby doctor | jq

# NO_COLOR still disables styling unless the user explicitly forces color
NO_COLOR=1 labby doctor
```

Rules:

- `--json` remains unstyled machine output
- `--color=auto` is the default and must remain pipe-safe
- `--color=plain` is the deterministic script and CI escape hatch
- `--color=color` is the explicit operator override

## Destructive Operations

Destructive commands use interactive confirmation by default.

Relevant flags:

- `-y` / `--yes`
- `--no-confirm`
- `--dry-run`

Policy knobs may also exist via env, but non-interactive shells must still refuse destructive work unless confirmation has been made explicit.

The CLI reads the same destructive flag from `ActionSpec` that MCP uses for elicitation.

## Multi-Instance Services

The CLI must support explicit instance selection where relevant:

```bash
lab unraid array status --instance shart
```

If there is a clear default instance, that can be used implicitly. Otherwise the command must fail loudly and ask for an instance.

## `labby doctor`

`labby doctor` is a read-only audit command.

It checks:

- env presence
- URL validity
- connectivity
- auth
- service version visibility

It must support:

- all services
- one service
- machine-readable output
- a quicker validation mode

Exit semantics:

- `0` for OK
- `1` for warnings
- `2` for failures

## `labby health`

`labby health` is the product-level health-check surface. It is distinct from repo-level shell helpers.

It must expose normalized service health results using the shared `ServiceStatus` model.

## `labby serve`

`labby serve` is the node runtime entrypoint on every fleet machine.

Rules:

- local hostname plus `[node].controller` decide whether the process is controller or non-controller
- the controller exposes the Web UI, MCP, `/v1/{service}`, `/v1/gateway`, and `/v1/nodes/*`
- a non-controller node exposes only `/health`, `/ready`, and `/v1/nodes/*`
- non-controller startup queues metadata and bootstrap logs, then opens a long-lived fleet websocket session to the controller

## `labby nodes`

`labby nodes` is the fleet inventory command group. It routes to the configured controller.

Commands:

- `labby nodes list`
- `labby nodes get <node_id>`
- `labby nodes enrollments list`
- `labby nodes enrollments approve <node_id> [--note <text>]`
- `labby nodes enrollments deny <node_id> [--reason <text>]`

## `labby logs`

`labby logs` now has two additive paths:

- fleet search routed to the configured controller
- controller-local log search and bounded follow-up queries against the embedded runtime store

Commands:

- `labby logs search <node_id> <query>`
- `labby logs local search [--subsystem <name>] [--level <level>] [--text <needle>] [--limit <n>]`
- `labby logs local tail [--after-ts <unix_ms>] [--since-event-id <id>] [--limit <n>]`
- `labby logs local stats`
- `labby logs local stream` — exits with guidance to use `GET /v1/logs/stream` or `labby logs local tail`

Rules:

- `labby logs search <node_id> <query>` keeps the existing fleet behavior and continues to use `POST /v1/nodes/logs/search`
- `labby logs local *` is strictly controller-local and uses the shared `dispatch::logs` contract
- true live streaming is not a CLI capability in v1; operators should use `GET /v1/logs/stream` or the gateway-admin `/logs` page
- CLI local log commands stay thin adapters; normalization, retention, search, and tail semantics are owned by `dispatch::logs`

## Install and Uninstall

`labby install` and `labby uninstall` handle:

- env validation and prompting
- `.mcp.json` patching
- service enablement changes

These commands are operationally sensitive and must use atomic file writes and backup behavior.

Expected `.mcp.json` behavior:

1. locate the file
2. parse or initialize it
3. compute the updated `--services` list
4. support dry-run diffing
5. back up before mutation
6. write atomically
7. verify the rewritten file parses

## Marketplace MCP metadata

Lab-owned MCP Registry metadata now lives under the unified Marketplace surface.

Commands:

- `labby marketplace mcp.meta.get --params '{"name":"io.github.user/server","version":"latest"}'`
- `labby marketplace mcp.meta.set --params '{"name":"io.github.user/server","metadata":{"curation":{"featured":true},"trust":{"reviewed":true}}}'`
- `labby marketplace mcp.meta.delete --params '{"name":"io.github.user/server","version":"latest"}'`

Rules:

- use Marketplace `mcp.*` actions instead of a standalone `mcpregistry` command
- pass metadata as JSON params; validation remains in the shared dispatch layer
- the CLI sets a stable audit actor label when writing metadata
- metadata validation is enforced by the shared dispatch layer, not by ad hoc CLI checks

See [MCPREGISTRY_METADATA.md](./MCPREGISTRY_METADATA.md) for the contract and allowed fields.

## Shell Completions

The CLI must generate completions rather than hand-maintaining shell-specific assets.

## `labby oauth relay-local`

`labby oauth relay-local` is a browser-side transport helper for OAuth clients that redirect to a
loopback callback but keep the real OAuth listener on another machine.

Supported forms:

```bash
labby oauth relay-local --machine dookie --port 38935
labby oauth relay-local --forward-base http://node.internal.example:38935/callback/dookie --port 38935
```

Flags:

| Flag | Description |
| --- | --- |
| `--machine <id>` | Resolve the forwarding target from `[oauth.machines.<id>]` in `config.toml`. |
| `--forward-base <url>` | Forward to an explicit callback base URL without a named machine config. |
| `--port <port>` | Loopback port to bind on the browser machine. Required. |
| `--json` | Global flag; emit JSON instead of human-readable tables where applicable. |

Rules:

- exactly one of `--machine` or `--forward-base` is required
- the listener binds only to `127.0.0.1:<port>`
- `--machine` resolves the target from `[oauth.machines.*]` in `config.toml`
- `--forward-base` is for ad hoc use when no named machine exists yet
- the command only forwards the final callback request; it does not mint tokens or run PKCE logic
- the remote callback listener must already be active before the browser callback arrives

Example named-machine config:

```toml
[oauth.machines.dookie]
target_url = "http://node.internal.example:38935/callback/dookie"
description = "dookie Codex callback listener"
default_port = 38935
```

Runtime behavior:

- incoming callback requests are accepted only on loopback
- the helper forwards the original method, query string, request body, and most headers
- hop-by-hop headers are stripped before forwarding
- successful forwarding returns the upstream response as-is
- failures return transport-oriented HTTP errors on the local loopback callback:
  - unreachable upstream target -> `502`
  - upstream timeout -> `504`
  - unsupported method -> `405`

The node runtime also exposes this relay capability remotely through `POST /v1/nodes/oauth/relay/start`.
