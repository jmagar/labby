---
name: using-lab-cli
description: This skill should be used when the user wants to run labby CLI commands, manage the labby MCP stdio server or HTTP/API server, configure credentials in ~/.lab/.env, scan or apply credentials with labby extract, check Lab health with labby doctor, manage gateway or marketplace surfaces, install Lab plugins through labby setup, or perform action + params dispatch against Lab operator services.
---

# Using the `labby` CLI

`labby` is the Lab binary. Treat generated help and `docs/` as source of truth when this skill and the repo disagree.

## Quick Start

```bash
labby help                 # Service + action catalog
labby doctor               # Full health/config audit
labby health               # Quick availability check
labby --json doctor        # Machine-readable output
labby completions bash     # Generate shell completions
```

Use `labby`, not the old `lab` command name.

## Top-Level Surfaces

| Command | Purpose |
|---------|---------|
| `labby mcp` | Start the MCP server over stdio |
| `labby serve` | Start the HTTP/API server |
| `labby doctor` | Audit config, auth, and runtime health |
| `labby health` | Quick availability check |
| `labby setup` | First-run/setup and plugin install flows |
| `labby setup install-plugin <name>` | Install a Lab plugin |
| `labby gateway ...` | Manage proxied upstream MCP gateways |
| `labby marketplace ...` | Manage marketplace/plugin metadata |
| `labby registry ...` | MCP Registry install/search when enabled |
| `labby extract [URI]` | Scan appdata for credentials |
| `labby extract [URI] --diff` | Preview `.env` changes |
| `labby extract [URI] --apply [-y]` | Merge discovered credentials into `.env` |
| `labby logs ...` | Search/tail Lab logs |
| `labby stash ...` | Component versioning/deployment metadata |

Do not suggest top-level `labby install`, `labby uninstall`, or `labby init`; those legacy stubs are intentionally unsupported. Use `setup`, `marketplace`, or `registry` instead.

## CLI vs MCP

The MCP surface exposes one tool per runtime service with flat action strings:

```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "gateway.reload" } }
{ "action": "tool.search", "params": { "query": "radarr queue" } }
```

For direct MCP stdio use, run `labby mcp`. For browser/API/admin workflows, run `labby serve`.

## Extract Credentials

`extract` scans local or SSH appdata roots. `--apply` and `--diff` require a targeted URI.

```bash
labby extract /mnt/user/appdata
labby extract squirts:/mnt/user/appdata --diff
labby extract squirts:/mnt/user/appdata --apply -y
labby extract squirts:/mnt/user/appdata --apply -y --force
```

`--apply` uses the canonical `.env` merge path: backup, atomic write, key dedupe, comment preservation, conflict warnings, and secure file permissions. Do not pass `-y` for a destructive operation unless the user explicitly approved it.

## Configuration

Config lives in `~/.lab/.env` and `config.toml` using Lab's documented load order. Common env keys:

```bash
{SERVICE}_URL=http://...
{SERVICE}_API_KEY=...
{SERVICE}_TOKEN=...
{SERVICE}_USERNAME=...
{SERVICE}_PASSWORD=...
```

Multi-instance services add a label before the suffix, for example `UNRAID_NODE2_URL`.

## Dev Commands

Inside the Lab repo, default verification is all-features:

```bash
just check
just test
just lint
just build
just run -- help
```

If you run a narrow command for speed, treat the result as provisional until the all-features path is checked.

## Troubleshooting

- Check current commands with `labby --help` or `labby <command> --help`.
- Use `labby doctor --json` when you need structured evidence.
- For MCP stdio problems, verify `labby mcp`; for HTTP/browser problems, verify `labby serve`.
- For stale docs, refresh generated docs before editing hand-written guidance.
