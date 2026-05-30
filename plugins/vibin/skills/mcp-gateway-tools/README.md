# mcp-gateway-tools

How to invoke MCP tools through the Lab gateway when it's running in **tool-search mode** — i.e. the only exposed MCP tools are a `*__tool_search` + `*__tool_execute` pair and all real upstream tools (radarr, sonarr, github, paperless, etc.) are hidden behind them.

## What it does

Encodes the gateway's name-resolution + invocation contract for agents:
- Map a user intent to an exact tool name via `tool_search`, then call it via `tool_execute`.
- Pick the right argument shape (built-in vs upstream) — the #1 failure mode.
- Recognize and respond to the canonical error envelope (`unknown_action`, `index_warming`, etc.) without guessing-and-retrying.
- Tune search queries when the wrong tool is surfacing first.

## Invoke

Automatically — if `list_tools` shows only `*__tool_search` + `*__tool_execute`, this skill applies. Also triggers on phrases like "find a tool for...", "call X through the gateway".

## Files

- `SKILL.md` — error table, anti-patterns, quick reference
