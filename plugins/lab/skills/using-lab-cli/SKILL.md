---
name: using-lab-cli
description: Operate the current Lab/Labby CLI and MCP server. Use when the user wants to run `labby` commands, start or inspect `labby serve`/`labby mcp`, check health with `doctor` or `health`, manage gateway/tool-search/protected-route/registry/marketplace/setup/stash/deploy workflows, configure or inspect `~/.lab/.env` and `~/.config/lab/config.toml`, extract credentials with `labby extract`, or dispatch Lab actions through CLI/MCP action + params patterns.
---

# Using the Lab CLI

Use `labby` as the canonical binary. The older `lab` command may exist on this
machine, but it can be an older compatibility binary with a different command
tree. Do not use `lab <service> ...` unless the user explicitly asks for that
binary or you first verify the exact subcommand with `lab --help`.

## Start With Live Help

Treat the installed binary and generated docs as the source of truth:

```bash
command -v labby
labby --version
labby --help
labby <command> --help
labby help --json
```

For repo docs, prefer generated artifacts over static lists:

- `docs/generated/cli-help.md` for current clap help
- `docs/generated/service-catalog.md` for available services and surfaces
- `docs/generated/action-catalog.md` for action names, params, and destructive flags
- `docs/generated/env-reference.md` for config/env metadata

Read [references/service-catalog.md](references/service-catalog.md) when deciding which current surfaces to use. Read [references/config-reference.md](references/config-reference.md) before writing or advising on env/config files.

## Command Patterns

Top-level operator commands currently include:

```bash
labby doctor              # read-only health/auth/system audit
labby health              # quick configured-service health check
labby serve               # HTTP MCP/API/web runtime
labby mcp                 # stdio MCP runtime
labby gateway list        # upstream MCP gateway control plane
labby marketplace help    # plugin/marketplace action surface
labby stash help          # component stash action surface
labby deploy config-list  # deployment target inventory
labby docs check          # verify generated docs are fresh
```

Some commands are present but intentionally incomplete in this checkout. Verify
before relying on `labby install`, `labby uninstall`, or `labby init`; if they
return `not yet implemented`, use the owning setup/gateway/marketplace workflow
instead of trying to force them.

## Actions and Params

Several command groups are thin CLI adapters over action catalogs:

```bash
labby marketplace plugins.list --params '{"installed":true}' --json
labby stash components.list --json
labby stash component.get id=01aryz6s41tpz5x11k39dv3r2g --json
```

MCP calls use one tool per service with an `action` plus `params` envelope:

```json
{ "tool": "gateway", "input": { "action": "gateway.list", "params": {} } }
{ "tool": "marketplace", "input": { "action": "plugins.list", "params": { "installed": true } } }
```

Use `help` and `schema` actions for discovery before constructing non-trivial
params.

## Gateway Workflows

Use `labby gateway` for upstream MCP servers and protected MCP routes:

```bash
labby gateway list
labby gateway get <name>
labby gateway discover
labby gateway test --name <name>
labby gateway add --name <name> --url https://example.com/mcp --bearer-token-env EXAMPLE_TOKEN
labby gateway add --name local-tools --command local-mcp-server --allow-stdio
labby gateway tool-search status
labby gateway tool-search enable --top-k-default 20 --max-tools 8000
labby gateway mcp list
labby gateway protected-route list
```

Stdio gateways can execute local commands during test/add/update. Only pass
`--allow-stdio` after the operator intentionally approves that local execution.
Gateway secrets should be referenced by env var name; do not put raw token
values in TOML, logs, or chat.

### Gateway Tool Search

Tool-search mode is gateway-wide. When enabled, MCP clients should see the
synthetic `scout` and `invoke` tools instead of the full raw upstream catalog.
Older clients or traces may still use the compatibility aliases `tool_search`,
`tool_execute`, or `tool_invoke`; prefer `scout` and `invoke` for new work.

Use the CLI to inspect or change the setting:

```bash
labby gateway tool-search status --json
labby gateway tool-search enable --top-k-default 10 --max-tools 5000
labby gateway tool-search disable
```

Use the gateway action API names when dispatching through the `gateway` tool:

```json
{ "action": "gateway.scout.get", "params": {} }
```

```json
{ "action": "gateway.scout.set", "params": { "enabled": true, "top_k_default": 10, "max_tools": 5000 } }
```

When operating through MCP, first search with two to four intent words, then
invoke an exact result:

```json
{ "tool": "scout", "input": { "query": "docker container restart", "top_k": 10, "include_schema": false } }
```

```json
{ "tool": "invoke", "input": { "name": "arcane::container_restart", "arguments": { "id": "..." } } }
```

Set `include_schema: true` when you need the exact argument schema before
calling `invoke`. Search results include `name`, `description`, `upstream`, and
`score`; if a bare tool name is ambiguous, retry `invoke` with one of the
fully-qualified names from the `ambiguous_tool.valid` list, using
`upstream::tool_name`.

Use gateway schema resources when you already know the server or need the full
catalog in one read instead of search-ranked snippets:

```text
lab://gateway/servers
lab://gateway/<name>/schema
```

`lab://gateway/servers` lists registered upstream MCP servers plus tool,
prompt, resource, health, and last-error counts. `lab://gateway/<name>/schema`
returns that upstream's exposed tools with verbatim `input_schema` and `meta`.
The mirrored gateway actions are:

```json
{ "action": "gateway.servers", "params": {} }
```

```json
{ "action": "gateway.schema", "params": { "name": "<upstream>" } }
```

Choose the discovery path deliberately:

| Need | Use |
| --- | --- |
| Unknown intent across all servers | `scout` then `invoke` |
| Connected upstream inventory | MCP `list_resources`, then read `lab://gateway/servers`; or call `gateway.servers` |
| Full schema for one known upstream | MCP `read_resource` on `lab://gateway/<name>/schema`; or call `gateway.schema` |
| Runtime gateway health/status | `labby gateway mcp list --json` or `gateway.mcp.list` |
| Exposed tools/resources/prompts for one gateway | `gateway.discovered_tools`, `gateway.discovered_resources`, or `gateway.discovered_prompts` |

For MCP resources, first call the client's resource-list operation and look for
`lab://gateway/servers` plus one `lab://gateway/<name>/schema` entry per
upstream. Then call the client's resource-read operation with the exact URI.
Do not call `scout` just to recover a whole known server schema.

Use `scout` for intent-based discovery across all servers. Use schema resources
when inspecting a specific connected server or preparing an exact `invoke`
call. If a tool is missing from `scout` results or a schema document, check the
upstream `expose_tools` policy before assuming the upstream lacks that tool.
Schema resources respect exposure policy and return resource-not-found when the
upstream pool is unavailable or the server name is unknown.

HTTP/action-dispatch access to `gateway.servers` and `gateway.schema` requires
gateway admin authorization. If a schema action fails, check auth/scope before
treating the result as proof that no server exists. MCP resource reads depend on
the active MCP session and configured gateway pool.

If `scout` returns weak or empty results, check gateway state before guessing:

```bash
labby gateway tool-search status --json
labby gateway list --json
labby gateway mcp list --json
labby gateway reload
```

Tool search and schema resources reflect the current upstream pool/cache. If
results look stale after config changes or upstream restarts, reload the
gateway, inspect `labby gateway mcp list --json`, then retry `list_resources`,
`read_resource`, or `scout`.

Configuration lives at root `[tool_search]` in `~/.config/lab/config.toml`.
Legacy per-upstream `[[upstream]].tool_search` blocks are migration input only;
do not add new per-upstream tool-search config.

## Extract and Config

`extract` takes an optional URI and flags, not `scan` or `apply` subcommands:

```bash
labby extract                         # fleet discovery
labby extract ~/appdata               # targeted read-only scan
labby extract host:/mnt/user/appdata --diff
labby extract host:/mnt/user/appdata --apply --yes
labby extract ~/appdata --apply --dry-run
```

`--apply` writes to `~/.lab/.env` by default. It backs up first, writes
atomically, preserves existing conflicting values unless `--force` is used, and
should be treated as destructive.

## Safety Rules

- Prefer `--json` for automation and parsing.
- Use `--color=plain` for deterministic text in CI/log captures.
- Confirm destructive operations explicitly with `-y`/`--yes` only when the user intent is clear.
- For gateway and registry installs, pass env var names such as `FOO_TOKEN`, never raw secret values.
- If a command shape is uncertain, run `<command> --help` before invoking it.
- If live help and bundled references disagree, trust live help and generated docs.

## Dev Verification

Inside the repo, generated docs and all-features checks are the normal truth:

```bash
just docs-check
just check
just test
just build
```

`just test` and `just build` are expected to target the all-features workspace
path per repo instructions.
