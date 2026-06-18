# Code Mode — Agent Contract

This document describes the current Lab gateway Code Mode surface exposed to MCP agents.

## Advertised Tools

When gateway-wide `[code_mode].enabled = true`, Lab hides raw upstream tools and advertises the primary synthetic `codemode` MCP tool plus compatibility tools:

| Tool | Scope | Purpose |
|------|-------|---------|
| `codemode` | `lab` or `lab:admin` | Run JavaScript in the Code Mode sandbox, discover upstream tools with `codemode.search()` / `codemode.describe()`, and broker upstream `callTool()` calls. |
| `search` | `lab:read`, `lab`, or `lab:admin` | Compatibility tool: run JavaScript against the full live upstream tool catalog. |
| `execute` | `lab` or `lab:admin` | Compatibility tool: run JavaScript in the Code Mode sandbox and broker upstream `callTool()` calls. |

There is no advertised `code` MCP tool. There is no `code_search` or `code_execute` tool. New clients must use `codemode`.

## Discover Inside Codemode

Call `codemode` and use in-sandbox discovery before making upstream calls:

```json
{ "code": "async () => { const hits = await codemode.search('github issues'); return hits.results; }" }
```

`codemode.search()` uses a reduced per-execution catalog with `id`, `path`, `upstream`, `name`, `description`, and `signature`.

## Compatibility Search

Call `search` before `execute` to discover the live IDs and TypeScript signatures.

`search` accepts:

```json
{ "code": "async () => tools.filter(t => t.upstream === \"github\")" }
```

The search sandbox injects:

```ts
const tools: Array<{
  id: string;
  upstream: string;
  name: string;
  description: string;
  schema: unknown;
  output_schema: unknown;
  signature: string;
  dts: string;
}>;
```

Good search snippets return a small filtered projection:

```ts
async () => tools
  .filter(t => /issue/i.test(t.description))
  .map(t => ({ id: t.id, signature: t.signature, dts: t.dts }))
```

## Codemode / Compatibility Execute

`codemode` and compatibility `execute` accept:

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

`[code_mode].enabled` controls whether the MCP gateway advertises `search` and `execute`.

`[code_mode]` controls execution limits only:

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

There is no `[code_mode].enabled` field.
