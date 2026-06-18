# Code Mode — Agent Contract

This document describes the current Lab gateway Code Mode surface exposed to MCP agents.

## Advertised Tools

When gateway-wide `[code_mode].enabled = true`, Lab hides raw upstream tools and advertises the single synthetic `codemode` MCP tool:

| Tool | Scope | Purpose |
|------|-------|---------|
| `codemode` | `lab` or `lab:admin` | Run JavaScript in the Code Mode sandbox, discover upstream tools with `codemode.search()` / `codemode.describe()`, and broker upstream `callTool()` calls. |

There are no advertised compatibility Code Mode MCP tools. Clients must use `codemode`.

## Discover Inside Codemode

Call `codemode` and use in-sandbox discovery before making upstream calls:

```json
{ "code": "async () => { const hits = await codemode.search('github issues'); return hits.results; }" }
```

`codemode.search()` uses a reduced per-execution catalog with `id`, `path`, `upstream`, `name`, `description`, and `signature`.

## Codemode Calls

`codemode` accepts:

```json
{
  "code": "async () => callTool('github::search_issues', { \"q\": \"repo:jmagar/lab gateway\" })",
  "upstreams": ["github"],
  "tools": ["github::search_issues"]
}
```

`upstreams` and `tools` are optional allowlists for that execution.

Inside `codemode`, every visible upstream MCP tool is callable two ways:

```ts
declare function callTool<T = unknown>(
  id: `${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;

declare namespace codemode {
  namespace github {
    function search_issues(params: { q: string }): Promise<unknown>;
  }
}
```

The `codemode.<upstream>.<tool>()` helpers are generated from the live catalog. Use `codemode.search()` and `codemode.describe()` to inspect exact helper names before relying on them.

## IDs

Code Mode IDs are exactly:

```text
<upstream>::<tool>
```

Examples:

- `github::search_issues`
- `cortex::cortex`
- `agent-os_windows-mcp::screenshot`

Do not prefix IDs with `upstream::`; the gateway already knows these are upstream tools. `lab::...` IDs are reserved and rejected because Lab built-in service actions are not available inside Code Mode.

## Runtime Contract

- The JavaScript must be an async arrow function or async function expression.
- The sandbox has no Node, Deno, network, filesystem, or environment access.
- All host access goes through `callTool()` / `codemode.*` and is brokered by the parent gateway.
- Failed `callTool()` calls reject their own promise. Use `Promise.allSettled()` for best-effort fan-out.
- Results are capped by `[code_mode]` response and log limits.
- Timeouts and fuel exhaustion are surfaced as structured errors.

## Config

`[code_mode].enabled` controls whether the MCP gateway advertises `codemode`, `search`, and `execute`.

The remaining `[code_mode]` fields control execution limits:

```toml
[code_mode]
timeout_ms = 30000
max_tool_calls = 1000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

There is no separate `[gateway.search_and_execute]` enable field; `[code_mode].enabled` is the switch for this MCP surface.
