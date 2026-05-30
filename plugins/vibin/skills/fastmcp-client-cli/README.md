# fastmcp-client-cli

Query MCP servers from the shell using FastMCP 3.x client commands: `fastmcp discover`, `fastmcp list`, and `fastmcp call`.

## What it does

Lets a shell-capable agent (or a human) inspect MCP servers without a full MCP host:
- Discover servers configured in `fastmcp.json`
- List tools / resources / prompts from any reachable server
- Invoke tools and read resources by name
- Handle multiple transports (stdio, SSE, streamable HTTP) and auth methods

## Invoke

Triggers: "list MCP tools", "call MCP tool from CLI", "test an MCP server", "debug MCP", "what tools does <server> expose".

## Prerequisites

`fastmcp` 3.x installed and on PATH.

## Files

- `SKILL.md` — agent instructions + canonical command reference
