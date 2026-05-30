# mcporter

Use the `mcporter` CLI to discover, inspect, and call MCP servers from the shell — and to write repeatable smoke-test scripts that exercise an MCP server's tools and resources.

## What it does

Encodes the working patterns for `mcporter` (the MCP CLI):

- Discover configured servers (`mcporter list --json`)
- Inspect a server's tools / resources (`mcporter inspect`)
- Call a tool ad-hoc with the right arg form (`--args '{...}'` vs positional `k=v`)
- Generate a CLI binding for a server when you'll script against it repeatedly
- Write a smoke-test harness using `scripts/smoke.sh` as a template
- Recognize the common failure modes (transport vs tool error, auth state, schema mismatches) and respond without guessing

## Invoke

Triggers: "test an MCP server", "smoke-test these tools", "automate MCP testing", "call an MCP tool from the shell", "list MCP tools", "exercise the gateway tools".

Not for designing new MCP servers, writing server-side handlers, or generic API testing.

## Prerequisites

- `mcporter` installed and on PATH
- `jq` (used by the smoke-test template)
- For configured-server flow: an `mcporter.json` with at least one server registered

## Usage — smoke test

```bash
cp ~/.agents/src/skills/mcporter/scripts/smoke.sh tests/mcp-smoke.sh
# edit the TOOLS / RESOURCES arrays for the server under test
./tests/mcp-smoke.sh <server-name>
```

The script exits 2 if both arrays are empty (so a copy-and-forget doesn't silently report "0 passed").

## Files

- `SKILL.md` — agent instructions, argument-form rules, failure-modes table, anti-patterns
- `scripts/smoke.sh` — copy-and-edit smoke-test template with auth/offline preflight and failure summary
