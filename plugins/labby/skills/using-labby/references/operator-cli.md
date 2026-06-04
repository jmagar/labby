# Operator CLI

Use this reference when operating Labby from the shell rather than through MCP
tools or HTTP action dispatch.

## Current Top-Level Commands

Current generated CLI help exposes these top-level commands:

| Command | Use |
| --- | --- |
| `labby serve` | Start the HTTP/API/web runtime. |
| `labby mcp` | Start stdio MCP transport. |
| `labby doctor` | Audit configured services, auth, proxy, and system state. |
| `labby docs` | Generate or verify code-owned docs. |
| `labby nodes` | Query node inventory and enrollment state. |
| `labby health` | Quick reachability check. |
| `labby setup` | First-run wizard, plugin setup, repair, plugin sync/export/connectivity. |
| `labby completions` | Generate shell completions. |
| `labby gateway` | Manage upstream MCP gateways and Code Mode settings. |
| `labby oauth` | Run local OAuth callback relay helpers. |
| `labby logs` | Search fleet or local-master logs. |
| `labby marketplace` | Manage plugins and registry-backed MCP marketplace actions. |
| `labby registry` | MCP Registry lookup/install path when enabled. |
| `labby stash` | Component versioning and deployment store. |
| `labby deploy` | Build/push/verify release binary on SSH targets when enabled. |

Use `labby --help` and `labby <command> --help` for the current surface. Treat
`docs/generated/cli-help.md` as the checked-in source for generated CLI help.

## Discovery Commands

```bash
labby --help
labby gateway --help
labby setup --help
labby marketplace --help
labby docs --help
```

Use generated docs for service/action catalogs:

```bash
sed -n '1,140p' docs/generated/service-catalog.md
sed -n '1,220p' docs/generated/action-catalog.md
sed -n '1,220p' docs/generated/mcp-help.md
```

`labby help` is normal Clap command help in the current CLI, not a replacement
for `docs/generated/action-catalog.md`.

## Setup Workflows

Use `setup` for local Labby installation and plugin support:

```bash
labby setup check --json
labby setup repair -y
labby setup plugin-connectivity --json
labby setup plugin-sync -y --json
labby setup plugin-export --json
labby setup installed-plugins --json
labby setup services-status --json
labby setup install-plugin <service> -y
labby setup uninstall-plugin <service> -y
```

Destructive setup actions require `-y` in non-interactive shells. Prefer
`--dry-run` when available before mutating plugin/env state.

## Health And Doctor

Use quick checks before deeper debugging:

```bash
labby health --json
labby doctor --json
labby doctor auth --json
labby doctor proxy --json
labby doctor system --json
labby doctor services --json
```

Use `doctor service <name>` only for a service that appears in current generated
catalogs.

## Docs

Generated docs are code-owned artifacts:

```bash
labby docs generate
labby docs check
```

Use `labby docs check` before trusting hand-written guidance after CLI/action
surface edits. Regenerate generated docs after changing Clap commands, action
catalogs, service metadata, OpenAPI routes, or MCP help.

## Logs

Use logs for current runtime evidence:

```bash
labby logs --help
labby logs local --help
labby logs local tail --help
```

True live log streaming is exposed over HTTP SSE at `/v1/logs/stream`; bounded
CLI follow-up uses local log subcommands.

## Marketplace And Registry

Use marketplace for plugin and MCP registry-backed workflows:

```bash
labby marketplace --help
labby marketplace mcp --help
labby registry --help
```

Marketplace action dispatch includes plugin listing/install/deploy/uninstall,
registry MCP list/get/install/uninstall/sync/validate, source management, and
metadata operations. Check `docs/generated/action-catalog.md` before invoking a
destructive marketplace action.

## JSON Output

Most operator commands accept global `--json` and `--color`:

```bash
labby --json doctor
labby gateway list --json
labby setup check --json
```

When scripting, prefer `--json` over parsing human-readable output.

## Verification Pattern

1. Confirm the binary and command surface:
   ```bash
   command -v labby
   labby --version
   labby --help
   ```
2. Read generated docs for the relevant service/action.
3. Run the narrow command with `--json`.
4. For destructive actions, use `--dry-run` if available, then `-y` only after
   approval or a clear operator request.
