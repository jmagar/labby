# Current Lab Surfaces

Do not maintain a hand-written full service list here. The current source of
truth is generated from code:

- `docs/generated/service-catalog.md`
- `docs/generated/action-catalog.md`
- `docs/generated/cli-help.md`

Regenerate generated docs with `labby docs generate` or `just docs-generate`.
Verify tracked generated docs with `labby docs check` or `just docs-check`.

## Stable Operator Surfaces

Use these surfaces for current Lab operations:

| Surface | Purpose | Discovery command |
| --- | --- | --- |
| `doctor` | Read-only auth, proxy, service, and system audits | `labby doctor --help` |
| `health` | Quick configured-service reachability check | `labby health --help` |
| `serve` | HTTP MCP/API/web runtime | `labby serve --help` |
| `mcp` | Stdio MCP runtime | `labby mcp --help` |
| `gateway` | Upstream MCP gateway control plane | `labby gateway --help` |
| `marketplace` | Plugin and registry-backed marketplace actions | `labby marketplace --help` |
| `registry` | MCP Registry install path into the gateway | `labby registry --help` |
| `setup` | Plugin setup, connectivity, env sync, local repair | `labby setup --help` |
| `nodes` | Fleet node inventory and enrollment operations | `labby nodes --help` |
| `logs` | Fleet and local-master log search | `labby logs --help` |
| `stash` | Component versioning and deployment store | `labby stash --help` |
| `deploy` | Build/push/verify release binary on SSH targets | `labby deploy --help` |
| `docs` | Generate and verify code-owned docs | `labby docs --help` |

## Current Catalog Notes

In the current repo line, the generated service catalog is operator-focused.
Common entries include `acp`, `deploy`, `device`, `doctor`, `fs`, `gateway`,
`lab_admin`, `logs`, `marketplace`, `mcpregistry`, `setup`, and `stash`. Check
`Cargo.toml` and `labby --version` separately when version identity matters,
because PATH installs and local build artifacts can lag the workspace version.

Treat the generated catalogs as the boundary for Labby-owned services. For
external capabilities, route through gateway upstreams, marketplace-installed
plugins, or another current operator surface.

## Action Discovery

For action-based surfaces, read `docs/generated/action-catalog.md` and
`docs/generated/mcp-help.md`.

For MCP/API dispatch, call the service tool with `help` or `schema` before
invoking an action with complex params.

## Gateway Schema Resources

Agents can inspect connected upstream MCP servers without relying only on
search:

- `lab://gateway/servers` lists registered upstream servers and counts for
  exposed tools, prompts, resources, health, and last error.
- `lab://gateway/<name>/schema` returns the exposed tool catalog for one
  upstream, including each tool's verbatim `input_schema` and `meta`.

The action-dispatch mirrors are:

```json
{ "action": "gateway.servers", "params": {} }
```

```json
{ "action": "gateway.schema", "params": { "name": "<upstream>" } }
```

Use Code Mode `codemode` for intent-based discovery and execution across the gateway. Use the
`lab://gateway/*` resources or `gateway.servers`/`gateway.schema` actions when
the caller needs a complete schema for a known connected server.

## Code Mode Contract

The public Code Mode MCP tool is `codemode`.

`codemode` accepts:

```json
{
  "code": "async () => { ... }",
  "upstreams": ["optional-upstream-allowlist"],
  "tools": ["optional-tool-or-id-allowlist"]
}
```

Only `code` is required. `upstreams` and `tools` are Labby `codemode`
arguments, not upstream tool params. Some client-rendered schemas may lag the
implementation, so use this contract when handling Code Mode recovery.

Inside `code`, call upstream MCP tools either way:

```js
async () => {
  const hits = await codemode.search({ query: "github issues", limit: 1 });
  const docs = await codemode.describe(hits.results[0].path);
  const a = await callTool("github::search_issues", { q: "bug" });
  const b = await codemode.github.search_issues({ q: "fix" });
  return { tool: docs.path, a, b };
}
```

`callTool` is the escape hatch and always takes the canonical upstream ID.
`codemode.<upstream>.<tool>` helpers are generated from the live catalog; use
them only after `codemode.search` confirms the sanitized helper name.

Destructive upstream tools:

- The MCP `codemode` tool does not expose a public top-level `confirm` field.
- Put confirmation exactly where the upstream tool's own live schema requires it.
- Do not accept `allow_destructive_actions`; that is an internal
  `CodeModeSurface` flag and older error text may leak it.
- Execute-capable OAuth scopes authorize Code Mode but do not confirm
  destructive effects.

Error recovery:

- `missing_param`, `invalid_param`, `validation_failed`: inspect the `codemode.search`
  schema and fix params.
- `confirmation_required`: inspect the upstream schema and provide the
  confirmation field exactly where that tool expects it.
- `unknown_tool`: rerun `codemode.search`; Code Mode only accepts `<upstream>::<tool>` IDs,
  not `lab::...` action IDs.
- `tool_call_limit_exceeded`: stale legacy error text; current Code Mode is
  bounded by wall-clock time, sandbox resources, output caps, and host-side
  tool policy.
- `timeout`: split work across executions or reduce upstream calls.

For Code Mode `codemode`, follow the live schema exactly for tool params. For
example, Windows MCP `Wait` currently uses `{ "duration": 2 }`, not
`{ "seconds": 2 }`.

Discovery chooser:

| Need | Use |
| --- | --- |
| Unknown intent across all servers | Code Mode `search` then `execute` |
| Connected upstream inventory | MCP `list_resources` + `read_resource lab://gateway/servers`, or `gateway.servers` |
| Full schema for one known upstream | MCP `read_resource lab://gateway/<name>/schema`, or `gateway.schema` |
| Runtime gateway health/status | `labby gateway mcp list --json` or `gateway.mcp.list` |
| Exposed tools/resources/prompts for one gateway | `gateway.discovered_tools`, `gateway.discovered_resources`, `gateway.discovered_prompts` |
| Local MCP client config discovery | `labby gateway discover --json` then `labby gateway import -y ...` |

Schema resources are cache-backed and exposure-policy filtered. If a tool is
absent, check the upstream `expose_tools` policy and refresh gateway state
before assuming the upstream server does not provide it:

```bash
labby gateway reload
labby gateway mcp list --json
labby gateway list --json
```

`gateway.servers` and `gateway.schema` over HTTP/action dispatch require admin
authorization; check auth/scope before treating a failed schema action as an
empty gateway.

For full gateway workflows, read `gateway-operations.md`. For Code Mode
execution details, read `code-mode.md`.
