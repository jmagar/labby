---
name: "fastmcp-client-cli"
description: "Query MCP servers from the shell using fastmcp discover, list, and call. Use when you need to discover configured servers, list tools/resources/prompts, call tools, read resources, get prompts, or bridge MCP servers into shell-based workflows."
metadata:
  doc_type: "skill"
  status: "active"
  owner: "fastmcp-client-cli"
  audience:
    - "agents"
    - "maintainers"
  scope: "v0"
  source_of_truth: false
  upstream_refs:
    - "docs/references/fastmcp/docs/client.mdx"
    - "docs/references/fastmcp/docs/overview.mdx"
    - "docs/references/fastmcp/docs/running.mdx"
    - "docs/references/fastmcp/repos/prefecthq-fastmcp.xml"
  related: []
  last_reviewed: "2026-05-13"
  last_modified: "2026-05-13"
  modified_on_branch: "main"
  modified_at_version: "0.1.0"
  modified_at_commit: "unborn"
  review_basis: "cross-referenced against local docs/references snapshot"
---

# FastMCP CLI: List and Call

Use `fastmcp discover`, `fastmcp list`, and `fastmcp call` to interact with MCP servers from the command line. These commands are FastMCP 3.x client commands and are useful for development, debugging, scripting, and shell-capable agents that do not have native MCP support.

## Listing Tools

```bash
# Remote server
fastmcp list http://localhost:8000/mcp

# Local Python file (runs via fastmcp run automatically)
fastmcp list server.py

# MCPConfig with multiple servers
fastmcp list mcp.json

# Stdio command (npx, uvx, etc.)
fastmcp list --command 'npx -y @modelcontextprotocol/server-github'

# Include full input/output schemas
fastmcp list server.py --input-schema --output-schema

# Machine-readable JSON
fastmcp list server.py --json

# Include resources and prompts
fastmcp list server.py --resources --prompts
```

Default output shows tool signatures and descriptions. Use `--input-schema` or `--output-schema` to include full JSON schemas, `--json` for structured output. With `--json`, resources and prompts are included only when `--resources` or `--prompts` is also passed.

## Calling Tools

```bash
# Key=value arguments (auto-coerced to correct types)
fastmcp call server.py greet name=World
fastmcp call server.py add a=3 b=4

# Single JSON object for complex/nested args
fastmcp call server.py create_item '{"name": "Widget", "tags": ["a", "b"]}'

# --input-json with key=value overrides
fastmcp call server.py search --input-json '{"query": "hello", "limit": 5}' limit=10

# JSON output for scripting
fastmcp call server.py add a=3 b=4 --json
```

Type coercion is schema-driven: integers, numbers, booleans, arrays, and objects are coerced from strings based on the target tool's input schema. Booleans accept `true`/`false`, `yes`/`no`, and `1`/`0`. Arrays and objects must be valid JSON.

`fastmcp call` can also read a resource URI or get a prompt:

```bash
# Resource URI targets are detected by ://
fastmcp call server.py resource://docs/readme

# Prompt targets require --prompt
fastmcp call server.py summarize --prompt topic=FastMCP
```

## Server Targets

All commands accept the same server targets:

| Target | Example |
|--------|---------|
| HTTP/HTTPS URL | `http://localhost:8000/mcp` |
| Python file | `server.py` |
| MCPConfig JSON | `mcp.json` (must have `mcpServers` key) |
| Stdio command | `--command 'npx -y @mcp/server'` |
| Discovered name | `weather` or `source:name` |

Python file targets are launched through `fastmcp run <file> --no-banner`; code in an `if __name__ == "__main__"` block is not used by `fastmcp run`. JSON targets for `list` and `call` must be MCPConfig-style files with an `mcpServers` key. A `fastmcp.json` server config is not a direct client target; start it first with `fastmcp run`.

Servers configured in editor configs (Claude Desktop, Claude Code, Cursor, Goose) or the current directory's `./mcp.json` can be referenced by name. Bare names are rejected if the same name appears in multiple sources. Use `source:name` (e.g. `claude-code:my-server`, `cursor:weather`) to target a specific source. Run `fastmcp discover` to see available names.

For SSE servers, pass `--transport sse`:

```bash
fastmcp list http://localhost:8000/mcp --transport sse
```

HTTP URLs default to Streamable HTTP. The `--transport sse` override is for older SSE-only servers.

## Auth

HTTP targets automatically use OAuth unless disabled. If the server does not require auth, OAuth setup is a no-op. Disable auth or pass a bearer token with `--auth`:

```bash
fastmcp call http://server/mcp tool --auth none
fastmcp list http://server/mcp --auth "Bearer sk-..."
```

Top-level auth is not applied to MCPConfig JSON targets; those transports are resolved per configured server.

## Discovering Configured Servers

```bash
# See all MCP servers in editor/project configs
fastmcp discover

# Filter by source
fastmcp discover --source claude-code

# JSON output
fastmcp discover --json
```

Scans Claude Desktop, Claude Code, Cursor, Goose, and `./mcp.json`. Sources: `claude-desktop`, `claude-code`, `cursor`, `goose`, `project`. Claude Code discovery includes global `~/.claude.json` servers and project-scoped entries matching the current working directory.

## Workflow Pattern

Discover tools first, then call them:

```bash
# 1. See what servers are configured
fastmcp discover

# 2. See what tools a server has
fastmcp list weather

# 3. Call a tool
fastmcp call weather get_forecast city=London
```

If you call a nonexistent tool, FastMCP suggests close matches.

For shell-agent integration, prefer `fastmcp list --json` to gather tool schemas and `fastmcp call --json` to parse content blocks, `is_error`, and structured content reliably.
