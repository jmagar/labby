Execute JavaScript in the Code Mode sandbox against proxied upstream MCP tools.

Use IDs returned by `code_search`. `Promise.all([...])` dispatches `callTool`
requests in parallel, so batch independent read-only calls instead of awaiting
them serially.

```ts
type CodeModeToolId = `upstream::${string}::${string}`;

type CodeModeError = {
  kind:
    | "unknown_tool"
    | "unknown_action"
    | "missing_param"
    | "invalid_param"
    | "validation_failed"
    | "confirmation_required"
    | "auth_failed"
    | "rate_limited"
    | "network_error"
    | "server_error"
    | "decode_error"
    | "internal_error"
    | "timeout"
    | "tool_call_limit_exceeded"
    | "code_mode_timeout"
    | "code_mode_fuel_exhausted";
  message: string;
  valid?: string[];
  hint?: string;
  retry_after_ms?: number;
};

declare function callTool<T = unknown>(
  id: CodeModeToolId,
  params: Record<string, unknown>
): Promise<T>;

// Successful return: the upstream tool's structuredContent if present,
// else the parsed text of the first content[0] block. Never the raw MCP envelope.
// To recover: const env: CodeModeError = JSON.parse(String(e.message));
//             switch (env.kind) { ... }
// Retry-safe:    rate_limited (honor retry_after_ms), timeout, network_error, code_mode_timeout
// Fix-and-retry: missing_param, invalid_param, validation_failed, confirmation_required, code_mode_fuel_exhausted
// Terminal:      unknown_tool, unknown_action, auth_failed, server_error, internal_error, decode_error
```

Results are capped to the configured Code Mode envelope budget, defaulting to
24KB and roughly 6000 tokens. Oversized per-call results are replaced with a
truncation marker containing `truncated`, `original_size`, `original_tokens`,
`preview`, and `next_action`.

Fuel budget guidance:
- Base overhead: about 100K fuel for the JS module and promise scheduler.
- Per `callTool` boundary: about 2K fuel for promise plumbing and host dispatch.
- Default 50M fuel is intended for heavy fan-out plus moderate result processing.
- Hitting `code_mode_fuel_exhausted` means split the work across calls or reduce
  local processing over large result arrays.
