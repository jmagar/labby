# Code Mode

Code Mode exposes a **single** MCP tool — `code` — when `[code_mode] enabled = true` in
`config.toml`. This is mutually exclusive with Tool Search mode (`search` + `execute`).

The `code` tool uses an `action` discriminator:

- `action: "search"` (scope: `lab:read`) — fetch the current upstream MCP tool catalog
  as a typed TypeScript preamble. The server injects this preamble into the sandbox
  before your code runs; you do not call it manually.
- `action: "execute"` (scope: `lab` or `lab:admin`) — run a JavaScript snippet against
  the sandbox. The typed `codemode.*` namespace (built from the catalog) is available.
  Each `codemode.<helper>(params)` call dispatches to the real upstream server via
  `callTool` under the hood.

**Legacy aliases** (`code_search`, `code_execute`) remain callable for backward
compatibility but are hidden from `list_tools` and emit a `tracing::warn!` with
`legacy_alias` and `canonical` fields.

Lab actions are intentionally not exposed through Code Mode. Use the normal
`execute` (Tool Search mode) or CLI surface for Lab service actions.

## Catalog Budget

The inline catalog has a 256KB soft cap and 512KB hard cap. Over the soft cap,
the catalog is stably pruned and a `__truncated__` sentinel entry is appended.
Over the hard cap, `code(search)` returns `invalid_param`.

## Execute Response Budget

`code(execute)` returns a capped envelope. Defaults:

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
