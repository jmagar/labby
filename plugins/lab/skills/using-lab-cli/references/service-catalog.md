# Current Lab Surfaces

Do not maintain a hand-written full service list here. The current source of
truth is generated from code:

- `docs/generated/service-catalog.md`
- `docs/generated/action-catalog.md`
- `docs/generated/cli-help.md`
- `labby help --json`

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
| `extract` | Credential and URL discovery from appdata/SSH/fleet | `labby extract --help` |
| `nodes` | Fleet node inventory and enrollment operations | `labby nodes --help` |
| `logs` | Fleet and local-master log search | `labby logs --help` |
| `stash` | Component versioning and deployment store | `labby stash --help` |
| `deploy` | Build/push/verify release binary on SSH targets | `labby deploy --help` |
| `docs` | Generate and verify code-owned docs | `labby docs --help` |

## Current Catalog Notes

In the current repo line, the generated service catalog is operator-focused.
Common entries include `acp`, `deploy`, `device`, `doctor`, `extract`, `fs`,
`gateway`, `lab_admin`, `logs`, `marketplace`, `mcpregistry`, `setup`, and
`stash`. Check `Cargo.toml` and `labby --version` separately when version
identity matters, because PATH installs and local build artifacts can lag the
workspace version.

Do not assume old upstream-service subcommands such as `labby radarr ...`,
`labby unifi ...`, or `labby qdrant ...` exist. If the user asks for an older
service-style command, first run:

```bash
labby <service> --help
```

If that fails, inspect `docs/generated/service-catalog.md` and route through
gateway, marketplace, setup, or another current operator surface.

## Action Discovery

For action-based surfaces:

```bash
labby help --json
labby marketplace help --json
labby stash help --json
```

For MCP, call the service tool with `help` or `schema` before invoking an action
with complex params.

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

Use `scout`/`invoke` for intent-based search across the gateway. Use the
`lab://gateway/*` resources or `gateway.servers`/`gateway.schema` actions when
the caller needs a complete schema for a known connected server.

Discovery chooser:

| Need | Use |
| --- | --- |
| Unknown intent across all servers | `scout` then `invoke` |
| Connected upstream inventory | MCP `list_resources` + `read_resource lab://gateway/servers`, or `gateway.servers` |
| Full schema for one known upstream | MCP `read_resource lab://gateway/<name>/schema`, or `gateway.schema` |
| Runtime gateway health/status | `labby gateway mcp list --json` or `gateway.mcp.list` |
| Exposed tools/resources/prompts for one gateway | `gateway.discovered_tools`, `gateway.discovered_resources`, `gateway.discovered_prompts` |

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
