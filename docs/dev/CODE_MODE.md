# Code Mode

Code Mode is the JavaScript execution surface behind the MCP `search` and `execute`
tools. It lets an agent discover upstream MCP tools with a small catalog query, then
run one async JavaScript function in a sandbox that can call those upstream tools.

Lab actions are intentionally not exposed through Code Mode. For Lab built-in actions,
use the normal Tool Search `execute` shape with `name=<service>` and
`arguments={ action, params }`.

## Surface

Code Mode has two MCP tools:

- `search` — runs a JavaScript async arrow function over a projected catalog:
  `const tools = [...]`. Use it to find upstream tool IDs and inspect their input
  schemas.
- `execute` — runs a JavaScript async arrow function in the Code Mode sandbox.
  The sandbox can call upstream tools with `callTool(id, params)` or with the
  generated `codemode.<upstream>.<tool>(params)` helper.

Example search:

```ts
async () => tools
  .filter(t => /issue/i.test(t.description))
  .map(t => ({ id: t.id, schema: t.schema }))
```

Example execute:

```ts
async () => {
  const issues = await callTool("upstream::github::search_issues", { q: "bug" });
  return issues.items.length;
}
```

`Promise.all([...])` and `Promise.allSettled([...])` fan out independent upstream
calls. A failed `callTool` rejects only that promise; catch locally when partial
success is useful.

## Tool IDs and Helpers

Upstream tool IDs use:

```text
upstream::<upstream-name>::<tool-name>
```

`execute` injects a runtime proxy generated from the live readable catalog, so
`codemode.github.search_issues(params)` calls the same bridge as:

```ts
callTool("upstream::github::search_issues", params)
```

Search entries include both raw JSON Schemas and generated TypeScript:

- `schema` — input JSON Schema.
- `output_schema` — output JSON Schema when the upstream tool declares one.
- `signature` — one-line TypeScript call signature.
- `dts` — focused TypeScript declarations with JSDoc for that tool.

The execute proxy is runtime JavaScript; the TypeScript surface is delivered by
`search` so agents can request only the declarations they need instead of loading
the entire upstream catalog into one tool description. When a schema is missing or
too complex for the TypeScript emitter, the generated declaration falls back to
`unknown`.

## Catalog Freshness

Code Mode does not build or read a durable vector, lexical, or RRF index. Each
`search` call projects a transient catalog from the gateway runtime and refreshes
enabled upstream tool metadata through the gateway manager before evaluating the
caller JavaScript. The `execute` proxy is generated from the same refreshed
catalog source, so helper visibility and direct `callTool` routing stay aligned.

`gateway.reload` swaps in a freshly seeded lazy upstream pool. The next Code Mode
`search` or `execute` call reprobes the relevant live upstreams and should see
tool-list changes such as the agent-os Windows-MCP `PowerShell`, `FileSystem`,
`Snapshot`, and `Wait` tools without requiring a process restart.

## Catalog Drift Diagnostics

When search results do not match live execution, check the layers in order:

1. Gateway runtime:

   ```bash
   labby gateway list --json
   ```

   Confirm the upstream reports the expected discovered tool count and is not
   carrying a tools-capability error.

2. Code Mode execute proxy:

   ```ts
   async () => Object.keys(codemode.agent_os_windows_mcp).sort()
   ```

   For agent-os, the list should include `PowerShell`, `FileSystem`, `Snapshot`,
   and `Wait`.

3. Direct callability:

   ```ts
   async () => callTool("upstream::agent-os_windows-mcp::PowerShell", {
     command: "Write-Output MCP_OK"
   })
   ```

   If this succeeds while search is stale, the upstream is callable and the
   issue is catalog visibility rather than tool execution.

4. MCP search injected catalog:

   ```ts
   async () => tools
     .filter(t => t.upstream === "agent-os_windows-mcp")
     .map(t => t.name)
     .sort()
   ```

   Missing `PowerShell`, `FileSystem`, or `Snapshot` here after layers 1-3 are
   fresh indicates Code Mode catalog freshness drift in the active MCP session.
   Run `gateway.reload` once to swap the runtime pool; if the same MCP session
   still sees stale search results while execute is fresh, reconnect that MCP
   client session so it receives the current gateway manager state.

`execute` accepts optional `upstreams` and `tools` arrays to narrow the per-run
capability set. When present, each filter must be a JSON array of strings; other
shapes reject with `invalid_param`. Empty strings are ignored. The injected proxy only
includes allowed tools, and direct `callTool` IDs outside the allowlist reject as
`unknown_tool`.

## Result Contract

Successful upstream tool calls resolve to the payload, never the raw MCP
`CallToolResult` envelope:

1. `structuredContent` when present.
2. Otherwise the first text content block, parsed as JSON when possible.
3. Otherwise raw text, `null`, or non-text content blocks as JSON.

`execute` itself returns a capped envelope with:

- `result` — the JavaScript function return value.
- `calls[]` — lightweight per-call metadata: `id`, `ok`, `elapsed_ms`, and
  `error_kind` on failure.
- `logs[]` — sandbox console output when available.

Binary-like JavaScript values crossing the runner boundary use a tagged base64
codec. JavaScript return values (`ArrayBuffer` and typed-array views) are encoded
as JSON:

```json
{ "__labBinary": "base64", "type": "Uint8Array", "data": "AQL/" }
```

Tagged binary values received from the parent bridge are decoded back to
`ArrayBuffer` or `Uint8Array` inside the sandbox. Mixed or binary MCP content
blocks that are not unwrapped as `structuredContent` or all-text content remain in
their JSON MCP representation.

Defaults:

- `max_response_bytes = 24576`
- `max_response_tokens = 6000`

When the envelope is too large, the final `result` is replaced with a truncation
marker containing `truncated`, `original_size`, `original_tokens`, `preview`, and
`next_action`. Logs are trimmed after result truncation if needed.

## Error Contract

Tool errors reject with a JSON-encoded string that can be decoded in the sandbox:

```ts
try {
  await callTool("upstream::github::search_issues", {});
} catch (e) {
  const env = JSON.parse(String(e.message));
  return env.kind;
}
```

Canonical recovery buckets:

- Retry-safe: `rate_limited`, `timeout`, `network_error`
- Fix-and-retry: `missing_param`, `invalid_param`, `validation_failed`,
  `confirmation_required`
- Terminal: `unknown_tool`, `unknown_action`, `auth_failed`, `server_error`,
  `internal_error`

Destructive upstream tools are gated by host-side metadata before dispatch. In
the MCP `execute` surface, callers can explicitly confirm the whole Code Mode run
with top-level `confirm: true`. Execute-capable scopes (`lab` or `lab:admin`)
authorize Code Mode execution, but do not implicitly confirm destructive upstream
effects. Unconfirmed MCP destructive calls fail as `confirmation_required`. CLI
Code Mode execution permits destructive upstream calls because it is
operator-driven.

## Scope

- `lab:read` can use `search`.
- `lab` or `lab:admin` can use `execute`.

OAuth callers retain their subject attribution when Code Mode calls upstream tools.
Trusted local callers use the shared gateway subject.

## Runner Architecture

The stdio parent-broker protocol is:

1. Parent starts `labby internal code-mode-runner`.
2. Child evaluates the normalized async function.
3. Child emits `tool_call` lines for `callTool` requests.
4. Parent dispatches through the gateway broker and replies with `tool_result` or
   `tool_error`.
5. Child settles pending promises and emits `done`.

With `code_mode_wasm` enabled, the child runner uses Javy/QuickJS for snippet
execution. Without it, the runner keeps the Boa fallback implementation for
development builds that do not include the Javy/Wasmtime dependencies.

The runner starts with an empty environment in a temporary directory. It does not
provide Node, Deno, Bun, `fetch`, `connect`, `XMLHttpRequest`, `require`, or host
module `import()` access. `callTool` is the only host bridge exposed to user code.

The Wasmtime engine skeleton uses fuel and epoch interruption. Fuel and timeout
traps are normalized to `code_mode_fuel_exhausted` and `code_mode_timeout` so
callers can recover programmatically as the Wasmtime path grows. The Wasmtime path
shares one configured engine and caches compiled modules by source to avoid paying
compile cost on repeated executions while keeping per-call stores and instances
isolated.

Loose JavaScript snippets are normalized before execution. Already-formed
function expressions pass through, while statement blocks such as
`const x = await callTool(...); x.items` are wrapped as `async () => { ... }` and
the trailing expression is returned.
