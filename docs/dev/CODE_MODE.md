# Code Mode

Code Mode exposes two MCP tools when gateway tool search is enabled:

- `code_search` injects the current upstream MCP tool catalog as `const tools = [...]`
  inside a constrained JavaScript search sandbox. Each catalog entry contains
  `id`, `upstream`, `name`, `description`, and sanitized `schema`.
- `code_execute` runs JavaScript snippets that call `callTool(id, params)` for
  upstream MCP tool IDs returned by `code_search`.

Lab actions are intentionally not exposed through Code Mode. Use the normal
`invoke`/`tool_execute` surface for Lab service actions.

## Catalog Budget

The inline catalog has a 256KB soft cap and 512KB hard cap. Over the soft cap,
the catalog is stably pruned and a `__truncated__` sentinel entry is appended.
Over the hard cap, `code_search` returns `invalid_param` and callers should use
`scout` for RRF discovery.

## Execute Response Budget

`code_execute` returns a capped envelope. Defaults:

- `max_response_bytes = 24576`
- `max_response_tokens = 6000`

When the envelope is too large, oversized per-call results are replaced with a
truncation marker containing `truncated`, `original_size`, `original_tokens`,
`preview`, and `next_action`.

## Runner Architecture

The stdio parent-broker protocol is unchanged:

1. Parent starts `labby internal code-mode-runner`.
2. Child emits `tool_call` lines for `callTool` requests.
3. Parent dispatches through the gateway broker and replies with `tool_result`.
4. Child emits `done` after all promises settle.

With `code_mode_wasm` enabled, the child runner uses Javy/QuickJS for snippet
execution. `callTool` returns a real JavaScript promise, so `Promise.all`
fan-out emits independent tool calls before awaiting results. `console.log` and
`console.error` are routed to stderr, and the runner process starts with an
empty environment in a temporary directory with no Node, Deno, Bun, fetch, or
require globals.

Without `code_mode_wasm`, the runner keeps the Boa fallback implementation for
development builds that do not include the Javy/Wasmtime dependencies.

The same feature also initializes the Wasmtime engine skeleton with fuel and
epoch interruption enabled. Fuel and timeout traps are normalized to
`code_mode_fuel_exhausted` and `code_mode_timeout` so callers can recover
programmatically as the Wasmtime path grows.
