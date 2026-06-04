# Code Mode

Use this reference when invoking upstream MCP tools through Labby's public Code
Mode tools: `search` and `execute`.

## Public Tools

`search` filters the live upstream MCP catalog inside a sandbox:

```js
async () => tools
  .filter(t => t.upstream.includes("github"))
  .map(t => ({ id: t.id, signature: t.signature, dts: t.dts }))
```

`execute` runs a JavaScript async function and lets that function call upstream
MCP tools:

```js
async () => {
  const issues = await callTool("upstream::github::search_issues", { q: "bug" });
  return issues.items?.length ?? 0;
}
```

Always run `search` before `execute`. The live catalog is the authority for
tool IDs, schemas, output schemas, signatures, and helper names.

## Search Catalog Entries

Each `search` entry contains:

| Field | Meaning |
| --- | --- |
| `id` | Canonical `upstream::<upstream>::<tool>` ID for `callTool`. |
| `upstream` | Upstream gateway name. |
| `name` | Upstream tool name. |
| `description` | Sanitized tool description. |
| `schema` | Input JSON schema. |
| `output_schema` | Output JSON schema when provided. |
| `signature` | Compact callable signature. |
| `dts` | TypeScript declaration for the `codemode.*` helper. |

The catalog injected into `search` is complete and in-sandbox; only your
filtered return value enters the model context.

## Execute Arguments

Top-level `execute` arguments:

```json
{
  "code": "async () => { ... }",
  "upstreams": ["optional-upstream-allowlist"],
  "tools": ["optional-tool-or-id-allowlist"],
  "max_tool_calls": 10,
  "confirm": true
}
```

Only `code` is required. The rest are Labby `execute` arguments:

- `upstreams`: allow only named upstreams for this run.
- `tools`: allow only raw tool names or `upstream::<upstream>::<tool>` IDs.
- `max_tool_calls`: cap brokered tool calls for this execution; clamped by gateway config.
- `confirm`: permit destructive upstream tools for this execution.

Do not place these fields inside upstream tool params.

## Calling Tools

Use `callTool` when selecting dynamically or when helper sanitization is
unclear:

```js
async () => {
  return await callTool("upstream::github::search_issues", { q: "fix" });
}
```

Use `codemode.<upstream>.<tool>` only after `search` confirms the helper name:

```js
async () => {
  return await codemode.github.search_issues({ q: "fix" });
}
```

The host validates params against the upstream input schema before dispatching.

## Destructive Tools

Destructive upstream tools require top-level `confirm` on `execute`:

```json
{
  "code": "async () => { return await callTool(\"upstream::x::delete\", { id: \"1\" }); }",
  "tools": ["upstream::x::delete"],
  "confirm": true
}
```

Rules:

- `lab` or `lab:admin` scope authorizes execution but does not confirm effects.
- `confirm` belongs on the top-level Labby `execute` call.
- `allow_destructive_actions` is internal-only. Do not use it as a public param.
- If the error says `confirmation_required`, retry `execute` with top-level
  `"confirm": true`.

## Return Shape

Successful `execute` returns:

```json
{
  "result": {},
  "calls": [
    { "id": "upstream::name::tool", "ok": true, "elapsed_ms": 12 }
  ],
  "logs": []
}
```

Upstream result unwrapping:

- Prefer upstream `structuredContent`.
- Else join all text content and parse JSON when possible.
- Else return text or the full mixed MCP result shape.
- Per-call result payloads are not copied into `calls`.

Oversized final responses are replaced with a truncation marker. Reduce data in
the sandbox before returning large values.

## Error Recovery

Tool-call errors reject only that promise. Catch them locally when you want the
run to continue:

```js
async () => {
  const settled = await Promise.allSettled([
    callTool("upstream::a::one", {}),
    callTool("upstream::b::two", {})
  ]);
  return settled.map(r => r.status === "fulfilled" ? r.value : JSON.parse(String(r.reason.message)));
}
```

Common error kinds:

| Kind | Recovery |
| --- | --- |
| `missing_param` | Read `search` schema and include the required field. |
| `invalid_param` | Fix type/shape against the schema. |
| `validation_failed` | Fix nested schema validation errors. |
| `confirmation_required` | Retry top-level `execute` with `"confirm": true`. |
| `unknown_tool` | Rerun `search`; use `upstream::...` IDs only. |
| `tool_call_limit_exceeded` | Reduce fan-out or set top-level `max_tool_calls`. |
| `timeout` | Split work into smaller executions. |
| `oauth_needs_reauth` | Check `labby gateway mcp auth status <upstream> --json`. |

## Runtime And Limits

Implementation facts that affect operation:

- `search` uses a 15s sandbox timeout and cannot call tools.
- `execute` uses root `[code_mode]` config for timeout, tool-call, response,
  token, and log limits.
- The runner process starts with a cleared environment and temp cwd.
- The parent host brokers all tool calls, validates schemas, applies
  confirmations, and terminates runaway executions.
- CLI `labby gateway code exec` is operator-driven and permits destructive
  upstream tools; MCP `execute` requires top-level `confirm`.

Current config defaults:

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

`gateway.code_mode.set` currently updates only:

- `timeout_ms`
- `max_tool_calls`
- `max_response_bytes`
- `max_response_tokens`

Edit `config.toml` for other Code Mode config fields unless the generated
action catalog has changed.

## CLI Code Mode

CLI execution:

```bash
labby gateway code exec --code 'async () => ({ ok: true })' --json
labby gateway code exec --file ./snippet.js --json
```

The CLI mirrors execution only; there is no CLI `gateway code search`
subcommand. Use MCP `search` for catalog filtering.

## Safe Execution Pattern

1. Run `search` and return only the candidate IDs/signatures needed.
2. Choose a narrow `upstreams` or `tools` allowlist.
3. Set `max_tool_calls` for bounded fan-out.
4. Use `Promise.allSettled` when independent calls may partially fail.
5. Return a compact result object rather than raw large payloads.
