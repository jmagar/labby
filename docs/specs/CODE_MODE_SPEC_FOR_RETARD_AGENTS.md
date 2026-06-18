# Code Mode — Implementation Specification

This specification describes the current Lab implementation. Older design notes that mention a single advertised `code` tool are historical and do not match the shipped surface.

## Current Surface

Code Mode is exposed through the gateway `codemode` MCP tool when `[code_mode].enabled = true`.

| Tool | Input | Behavior |
|------|-------|----------|
| `codemode` | `{ "code": string, "upstreams"?: string[], "tools"?: string[] }` | Runs a JavaScript async arrow function in the Javy/QuickJS sandbox, supports in-sandbox discovery, and brokers upstream calls. |

The gateway does not advertise compatibility Code Mode MCP tools.

## Catalog Discovery

Discovery happens inside the `codemode` sandbox:

- `await codemode.search({ query, limit })` returns compact candidate hits.
- `await codemode.describe(path)` returns full schema, output schema, signature, and TypeScript declaration details for an exact target.

The expected agent flow is:

1. Call `codemode`.
2. Use `codemode.search()` for a small candidate list.
3. Use `codemode.describe()` for the full contract of the chosen tool.
4. Call the tool with `callTool(id, params)` or a generated `codemode.<upstream>.<tool>()` helper.

Example:

```js
async () => {
  const hits = await codemode.search({ query: "github issues", limit: 5 });
  const docs = await codemode.describe(hits.results[0].path);
  return { path: docs.path, schema: docs.schema, signature: docs.signature };
}
```

## Execution

`codemode` receives JavaScript and wraps it in the Code Mode runner. The runner exposes:

```ts
declare function callTool<T = unknown>(
  id: `${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

It also injects generated `codemode.<upstream>.<tool>()` helpers for visible upstream tools when the live catalog can be built within limits.

Example:

```js
async () => {
  const result = await callTool("github::search_issues", {
    q: "repo:jmagar/lab gateway"
  });
  return result;
}
```

## IDs

Valid Code Mode IDs are:

```text
<upstream>::<tool>
```

`upstream::<server>::<tool>` is invalid. `lab::<service>` is reserved and rejected because Lab built-in service actions are not available inside the Code Mode sandbox.

## Config Ownership

`[code_mode]` controls MCP visibility and execution limits:

```toml
[code_mode]
enabled = true
timeout_ms = 30000
max_tool_calls = 1000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

There is no search-ranking config. `codemode.search()` is an in-sandbox helper
over the live catalog; callers control narrowing in their own code.

## Enforcement

- `codemode` requires `lab` or `lab:admin`.
- `codemode` has no filesystem, environment, host network, Node, or Deno APIs.
- Host calls are brokered by the parent gateway and retain upstream visibility, auth, destructive-action, schema-validation, and response-budget checks.
- `timeout_ms` kills runaway executions.
- `max_tool_calls` is enforced in the parent before each brokered upstream call.
- response and console output are truncated according to `[code_mode]` limits.

## Error Kinds

Code Mode specific failures use stable `kind` values including:

- `invalid_code_mode_id`
- `validation_failed`
- `code_mode_timeout`
- `code_mode_fuel_exhausted`
- `timeout`

General gateway and upstream failures continue to use the shared error envelope described in [docs/dev/ERRORS.md](../dev/ERRORS.md).
Unavailable or overlarge upstream schemas may be omitted from Code Mode metadata; generated signatures fall back to `unknown`.
