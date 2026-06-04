---
name: using-labby
description: "This skill should be used when the user mentions labby, the labby CLI, the Lab gateway, or any Lab operator surface. Triggers include: \"run labby doctor\", \"check labby health\", \"start the labby MCP server\", \"configure ~/.lab/.env\", \"search upstream MCP tools with Code Mode\", \"use labby gateway to import servers\", \"manage the Labby marketplace\", \"reload the gateway\", or any request to run labby CLI commands, inspect gateway upstreams, or dispatch an action against a Lab service."
---

# Using the `labby` CLI

`labby` is the Lab binary. Treat generated help and `docs/` as source of truth when this skill and the repo disagree.

## Quick Start

```bash
labby help                 # CLI command help
labby doctor               # Full health/config audit
labby health               # Quick availability check
labby --json doctor        # Machine-readable output
labby completions bash     # Generate shell completions
```

Use `labby`, not the old `lab` command name.

`labby help` is Clap command help in the current CLI. For service/action catalogs,
read `docs/generated/service-catalog.md`, `docs/generated/action-catalog.md`, or
use service `help`/`schema` actions through MCP/API dispatch.

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
| `labby gateway discover` | Scan local MCP client configs for upstream servers |
| `labby gateway import [-y]` | Import discovered MCP servers into the gateway |
| `labby logs ...` | Search/tail Lab logs |
| `labby stash ...` | Component versioning/deployment metadata |

Use only current top-level commands from `labby --help`. Prefer `setup`,
`gateway`, `marketplace`, or `registry` for operator workflows.

For command details and workflows, read:

- `references/operator-cli.md` for top-level CLI, setup, docs, doctor, logs, deploy, and marketplace workflows.
- `references/gateway-operations.md` for gateway add/update/import/OAuth/protected routes/runtime operations.
- `references/code-mode.md` for `search`/`execute`, schemas, confirmations, limits, and error recovery.
- `references/config-reference.md` for `~/.lab/.env`, `config.toml`, and mutable gateway settings.
- `references/service-catalog.md` for generated catalog sources and action-dispatch discovery.

## CLI vs MCP

The MCP surface exposes one tool per runtime service with flat action strings:

```json
{ "action": "help" }
{ "action": "schema", "params": { "action": "gateway.reload" } }
{ "action": "gateway.servers", "params": {} }
{ "action": "gateway.schema", "params": { "name": "github" } }
```

For direct MCP stdio use, run `labby mcp`. For browser/API/admin workflows, run `labby serve`.

## Code Mode Gotchas

Labby exposes the public Code Mode tools as `search` and `execute`. Use `search` first to inspect live upstream tool IDs, schemas, output schemas, TypeScript signatures, and `codemode.*` helper names. Do not guess parameter names from memory or older examples.

Use `execute` with JavaScript that evaluates to an async function:

```js
async () => {
  const result = await callTool("upstream::github::search_issues", { q: "bug" });
  return result.items?.length ?? 0;
}
```

Prefer `callTool("upstream::<upstream>::<tool>", params)` when dynamically selecting tools. Use `codemode.<upstream>.<tool>(params)` only after `search` confirms the helper name.

Destructive upstream tools require top-level confirmation on the Labby `execute` call:

```json
{ "code": "async () => { ... }", "upstreams": ["agent-os_windows-mcp"], "confirm": true }
```

Do not put `confirm` inside upstream tool params, and do not try `allow_destructive_actions`; that is an internal flag surfaced by older error text, not the public MCP `execute` parameter. Scopes such as `lab` or `lab:admin` authorize execution but do not confirm destructive effects. If a call returns `confirmation_required`, retry the same top-level Labby `execute` call with `"confirm": true`.

Use `upstreams` or `tools` allowlists to narrow risky executions:

```json
{
  "code": "async () => { return await codemode.agent_os_windows_mcp.Wait({ duration: 2 }); }",
  "upstreams": ["agent-os_windows-mcp"],
  "max_tool_calls": 3
}
```

`max_tool_calls` is a top-level `execute` budget override clamped by gateway config. It is not an upstream tool param.

For `Wait`, use the live schema field:

```js
await codemode.agent_os_windows_mcp.Wait({ duration: 2 });
```

Do not use `{ seconds: ... }` unless the current `search` result explicitly shows that field.

For deeper Code Mode details, read `references/code-mode.md`.

## Configuration

Config lives in `~/.lab/.env` and `config.toml` using Lab's documented load order. Common env keys:

```bash
LAB_MCP_HTTP_TOKEN=...
LAB_GW_<NAME>_AUTH_HEADER=Bearer ...
```

Labby-owned config is operator/gateway config. Use generated env docs and
gateway service-config actions for current fields.

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
