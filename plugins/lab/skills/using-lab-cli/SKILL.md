---
name: using-lab-cli
description: This skill should be used when the user wants to run labby CLI commands, operate any homelab service through the labby binary (Radarr, Sonarr, UniFi, Unraid, Linkding, Gotify, SABnzbd, Qdrant, Prowlarr, Bytestash, Apprise, TEI), manage the labby MCP server (labby serve, labby install, labby uninstall), configure credentials in ~/.lab/.env, scan for credentials with labby extract, check service health with labby doctor, scaffold a new service with labby scaffold, or perform any action dispatch against a homelab service using the action + params pattern.
---

# Using the `lab` CLI

`lab` is a pluggable homelab CLI + MCP server. One binary, 21 services, runtime MCP tool selection.

## Quick Start

```bash
lab help               # Full service + action catalog
labby doctor             # Audit all configured services (health, auth, reachability)
labby health             # Quick reachability check
lab <service> --help   # Per-service subcommands
labby --json <command>   # Machine-readable output for any command
```

## Top-Level Commands

| Command | Purpose |
|---------|---------|
| `labby serve` | Start MCP server (stdio or HTTP transport) |
| `labby doctor` | Full health audit: env vars, reachability, auth, version |
| `labby health` | Quick reachability check for all configured services |
| `lab plugins` | Open plugin manager TUI |
| `labby audit onboarding` | Audit service onboarding against repo contract |
| `labby install <service>` | Install service into `.mcp.json` |
| `labby uninstall <service>` | Remove service from `.mcp.json` |
| `labby init` | First-time setup wizard |
| `lab help` | Service + action catalog |
| `labby scaffold service <name>` | Generate new service onboarding scaffold |
| `lab completions` | Generate shell completions |

## Available Services

For current service status, see [references/service-catalog.md](references/service-catalog.md).

**Active services** (fully implemented): `extract`, `radarr`, `prowlarr`, `sabnzbd`, `linkding`, `bytestash`, `unraid`, `unifi`, `gotify`, `qdrant`, `tei`, `apprise`

**Stub services** (not yet implemented): `sonarr`, `plex`, `tautulli`, `qbittorrent`, `tailscale`, `memos`, `arcane`, `overseerr`, `openai`

If asked to use a stub service, inform the user it is not yet implemented and suggest `labby doctor` to see what is actually configured.

## CLI vs MCP Naming

**CLI subcommands use kebab-case**: `movie-list`, `movie-lookup`, `bookmark-list`

**MCP action strings use `resource.verb` dot notation**: `movie.search`, `movie.add`, `bookmark.list`

They map to the same underlying operations — the surface determines the form.

## Common Patterns

### Querying a service

```bash
lab radarr movie-list
lab radarr movie-lookup --query "The Matrix"
lab radarr calendar-list --json

lab unifi client-list
lab unraid system-status

lab linkding bookmark-list --tag homelab
```

### Destructive operations require `--yes`

```bash
lab radarr movie-delete --id 42 --yes
lab sabnzbd queue-purge --yes
labby extract apply --yes          # writes to ~/.lab/.env (backs up first)
```

`extract apply` merges found credentials into `~/.lab/.env`. It backs up the file before writing. Use `--force` to overwrite on key conflicts instead of the default skip-and-warn.

### Multi-instance services

Some services support multiple instances (e.g. multiple Unraid nodes). Select via `--instance`:

```bash
lab unraid system-status --instance node2
```

Instances are configured in `~/.lab/.env` with a label prefix:
```
UNRAID_URL=http://tower.local
UNRAID_NODE2_URL=http://tower2.local
```

### JSON output

```bash
lab radarr movie-list --json | jq '.[].title'
labby doctor --json          # CI-friendly audit
```

## Scaffolding a New Service

When onboarding a new service, always scaffold first and audit second:

```bash
labby scaffold service <name>    # generates module stubs in the correct locations
labby audit onboarding           # checks all services against the repo contract
```

`scaffold` produces the required files (`client.rs`, `types.rs`, `error.rs`, module declaration, CLI shim, MCP dispatch) in the right crate locations. `audit onboarding` verifies the scaffold matches the contract before wiring it into the build.

## Configuration

Config lives in `~/.lab/.env`. For full env-var reference, see [references/config-reference.md](references/config-reference.md).

Each service uses:

```
{SERVICE}_URL=http://...
{SERVICE}_API_KEY=...        # API key auth
{SERVICE}_TOKEN=...          # Bearer token auth
{SERVICE}_USERNAME=...       # Basic auth
{SERVICE}_PASSWORD=...
```

**Bootstrap from existing configs:**

```bash
labby extract scan              # Find credentials in local config files
labby extract scan --ssh user@host  # Scan remote host
labby extract apply --yes       # Write found credentials to ~/.lab/.env
```

## MCP Server Mode

```bash
labby serve                     # stdio (for Claude Desktop, claude.ai)
labby serve --http              # HTTP with bearer auth
labby install radarr            # Add radarr tool to .mcp.json
labby install --all             # Install all available services
```

Each service exposes one MCP tool with `action` + `params` dispatch:

```json
{ "action": "movie.search", "params": { "query": "The Matrix" } }
{ "action": "help" }
{ "action": "schema", "params": { "action": "movie.add" } }
```

## Dev Commands (inside the lab repository)

```bash
just build      # cargo build --workspace --all-features
just test       # cargo nextest run
just lint       # clippy + fmt check
just check      # cargo check --workspace
just run        # cargo run --all-features -- <args>
```

## Troubleshooting

- **Service not found**: set `{SERVICE}_URL` in `~/.lab/.env`, then run `labby doctor`
- **Auth errors**: set `{SERVICE}_API_KEY` or `{SERVICE}_TOKEN` for the service
- **Stub service**: not yet implemented — inform the user and run `labby doctor` to show what is configured
- **All services**: run `labby doctor` for a comprehensive health report; exit code reflects worst severity
