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
the entire upstream catalog into one tool description.

`execute` accepts optional `upstreams` and `tools` arrays to narrow the per-run
capability set. The injected proxy only includes allowed tools, and direct
`callTool` IDs outside the allowlist reject as `unknown_tool`.

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

Binary-like JavaScript return values (`ArrayBuffer` and typed-array views) are
encoded as tagged base64 JSON:

```json
{ "__labBinary": "base64", "type": "Uint8Array", "data": "AQL/" }
```

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

Destructive upstream tools are blocked unless the surface or caller scope permits
destructive actions.

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
