# lab — Binary Crate

The `lab` binary: one CLI + MCP server + HTTP API exposing 21 feature-gated homelab services.

This crate builds the `lab` binary itself. See [`lab-apis`](../lab-apis/) for the pure SDK that implements all service clients.

## Quick Start

### Build

```bash
cargo build --release --all-features
```

Binary lives at `target/release/labby`.

### Run CLI

```bash
lab radarr movie.search --query "The Matrix"
labby extract scan
labby --help
```

### Run MCP Server

```bash
labby serve
```

Server listens on stdio by default. Connect with any MCP client configured for this binary.

### Run HTTP API

```bash
labby api
```

Starts axum server on `127.0.0.1:3000` (configurable). Routes:

- `POST /v1/<service>` — dispatch action: `{"action":"movie.search","params":{"query":"..."}}`
- `GET /health` — liveness probe
- `GET /ready` — readiness probe

## Configuration

Config loaded from (in order):
- `.env` (CWD — errors silent, for dev)
- `~/.lab/.env` (secrets: URLs, API keys)
- `~/.config/lab/config.toml` (preferences)

Env var format:
- `{SERVICE}_URL` — base URL
- `{SERVICE}_API_KEY` — API key auth
- `{SERVICE}_TOKEN` — bearer/token auth
- `{SERVICE}_USERNAME` / `{SERVICE}_PASSWORD` — basic auth

Multi-instance: `{SERVICE}_{LABEL}_URL`, `{SERVICE}_{LABEL}_API_KEY`, etc.

Example:
```bash
RADARR_URL=http://radarr:7878
RADARR_API_KEY=abc123
UNRAID_URL=http://unraid.local
UNRAID_NODE2_URL=http://unraid-node2.local
UNRAID_NODE2_API_KEY=def456
```

## Observability

### Logging

```bash
LAB_LOG=labby=info,lab_apis=debug cargo run
LAB_LOG_FORMAT=json cargo run  # newline-delimited JSON
```

Default: `labby=info,lab_apis=warn` with ANSI colors on TTY.

### Health Check

```bash
labby doctor                      # full audit: env, reachability, auth, versions
labby doctor --json              # JSON output for CI
```

## Architecture

This binary depends on [`lab-apis`](../lab-apis/) — a pure Rust SDK with all HTTP clients.

Four surfaces wrap the SDK:

| Surface | Entry | Style | Use |
|---------|-------|-------|-----|
| **CLI** | `cli.rs` | Typed `clap` subcommands (tier 1) or MCP-passthrough stubs (tier 2) | Human users |
| **MCP** | `mcp.rs` | One tool per service, `action`+`params` dispatch | Claude Code, agents |
| **HTTP API** | `api.rs` | Axum router, mirrors MCP dispatch | Programmatic clients |
| **TUI** | `tui.rs` | Ratatui plugin manager (stub) | Service discovery, enable/disable |

All surfaces delegate business logic to `lab-apis` clients. Logic lives in SDK, not in surface shims.

### Service Coverage

**Fully typed CLI (tier 1):**
- `radarr` — movie search, add, queue
- `extract` — scan hosts for service credentials
- `bytestash` — note storage
- `unifi` — network management

**MCP-passthrough (tier 2, work in progress):**
- Media: sonarr, prowlarr, plex, tautulli, sabnzbd, qbittorrent, overseerr
- Network: tailscale, unraid, arcane
- Notes: linkding, memos
- Notifications: gotify, apprise
- AI: openai, qdrant, tei

## MCP Dispatch

Each service exposes one MCP tool. Operations dispatch via flat `action.verb` + free-form `params`:

```jsonc
radarr({
  "action": "movie.search",
  "params": { "query": "The Matrix" }
})

radarr({
  "action": "help"  // built-in: list all actions for this service
})

radarr({
  "action": "schema",
  "params": { "action": "movie.add" }  // per-action JSON schema
})
```

All services also accept:
- `{"action": "help"}` — list valid actions
- `{"action": "schema", "params": {"action": "X"}}` — schema for action X

See [`docs/MCP.md`](../docs/MCP.md) for error envelopes and elicitation.

## Development

See [`Justfile`](../../Justfile) for dev commands:

```bash
just check          # cargo check --workspace
just test           # cargo nextest run (unit tests)
just lint           # clippy + fmt --check
just build          # cargo build --workspace --all-features
just build-release  # release build
just run            # cargo run --all-features -- <args>
```

### Adding a Service

1. Implement client in [`lab-apis`](../lab-apis/src/)
2. Create MCP dispatch in `src/mcp/services/<service>.rs`
3. Create CLI subcommands in `src/cli/<service>.rs`
4. Register in `src/mcp/registry.rs`, `src/cli.rs`, and `src/catalog.rs`
5. Add feature flag to `Cargo.toml`

See [`docs/SERVICE_ONBOARDING.md`](../../docs/SERVICE_ONBOARDING.md) for the full checklist.

## Documentation

- **Architecture:** [`docs/ARCH.md`](../../docs/ARCH.md)
- **Services:** [`docs/SERVICES.md`](../../docs/SERVICES.md)
- **CLI:** [`docs/CLI.md`](../../docs/CLI.md)
- **MCP:** [`docs/MCP.md`](../../docs/MCP.md)
- **Config:** [`docs/CONFIG.md`](../../docs/CONFIG.md)
- **Observability:** [`docs/OBSERVABILITY.md`](../../docs/OBSERVABILITY.md)
- **Errors:** [`docs/ERRORS.md`](../../docs/ERRORS.md)
- **Docs index:** [`docs/README.md`](../../docs/README.md)

## Features

All flags 1:1 passthrough to [`lab-apis` features](../lab-apis/Cargo.toml).

```bash
# Build with specific services
cargo build --features radarr,sonarr,unraid

# Build all services
cargo build --all-features
```

Default features: `radarr`, `sonarr`, `prowlarr`, `plex`, `sabnzbd`, `qbittorrent`.

## License

See root workspace.
