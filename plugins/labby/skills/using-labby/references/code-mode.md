# Code Mode

Use this reference when invoking upstream MCP tools through Labby's public Code
Mode tool: `codemode`.

## Public Tools

`codemode.search()` filters the live upstream MCP catalog inside a sandbox:

```js
async () => {
  const hits = await codemode.search({ query: "github issues", limit: 5 });
  return hits.results.map(t => ({ path: t.path, id: t.id, signature: t.signature }));
}
```

`codemode` runs a JavaScript async function and lets that function call upstream
MCP tools with `callTool()` or generated `codemode.<upstream>.<tool>()` helpers:

```js
async () => {
  const issues = await callTool("github::search_issues", { q: "bug" });
  return issues.items?.length ?? 0;
}
```

Always run `codemode.search()` before calling an upstream, then call
`codemode.describe()` for the exact target when you need parameter details. The
live catalog is the authority for tool IDs, signatures, helper names, and
generated TypeScript parameter docs.

## Complete Working Examples

Search for candidate tools and return only compact catalog fields:

```json
{
  "code": "async () => {\n    const hits = await codemode.search({ query: \"axon\", limit: 5 });\n    return hits.results.map(t => ({ path: t.path, id: t.id, signature: t.signature }));\n  }"
}
```

Call a discovered tool by raw ID. Prefer this when the upstream/tool is selected
from search results:

```json
{
  "code": "async () => {\n    const help = await callTool(\"axon::axon\", { action: \"help\" });\n    return { ok: true, actions: help.actions ?? help };\n  }",
  "tools": ["axon::axon"]
}
```

Call an action-dispatched upstream using the shape from `codemode.search()` and
`codemode.describe()`. Axon uses flat action fields:

```json
{
  "code": "async () => {\n    const result = await callTool(\"axon::axon\", {\n      action: \"search\",\n      query: \"Labby Code Mode examples\",\n      limit: 5\n    });\n    const results = result.data?.data?.results ?? [];\n    return { count: results.length, results };\n  }",
  "upstreams": ["axon"]
}
```

Use a generated helper after `codemode.search()` confirms the exact helper path:

```json
{
  "code": "async () => {\n    const help = await codemode.rustarr.sonarr({ action: \"help\" });\n    return { ok: true, help_type: typeof help };\n  }",
  "upstreams": ["rustarr"]
}
```

Fan out independent reads without throwing away partial successes:

```json
{
  "code": "async () => {\n    const calls = await Promise.allSettled([\n      callTool(\"rustarr::sonarr\", { action: \"help\" }),\n      callTool(\"rustarr::radarr\", { action: \"help\" }),\n      callTool(\"rustarr::prowlarr\", { action: \"help\" })\n    ]);\n    return calls.map((r, index) => r.status === \"fulfilled\"\n      ? { index, ok: true, type: typeof r.value }\n      : { index, ok: false, error: JSON.parse(String(r.reason.message)) });\n  }",
  "upstreams": ["rustarr"]
}
```

Call a Windows helper through the live-confirmed helper path:

```json
{
  "code": "async () => {\n    const result = await codemode.agent_os_windows_mcp.PowerShell({\n      command: \"$PSVersionTable.PSVersion.ToString()\"\n    });\n    return { ok: true, result };\n  }",
  "tools": ["agent-os_windows-mcp::PowerShell"]
}
```

## Search Catalog Entries

Each `codemode.search()` entry contains:

| Field | Meaning |
| --- | --- |
| `id` | Canonical `<upstream>::<tool>` ID for `callTool`. |
| `namespace` | Upstream gateway name. |
| `name` | Upstream tool name. |
| `description` | Sanitized tool description. |
| `signature` | Compact callable signature. |
| `kind` | `tool` or `snippet`. |
| `tags` | Snippet tags when present. |

The catalog searched by `codemode.search()` is complete and in-sandbox; only
your filtered return value enters the model context. Use `codemode.describe()`
for exact target docs, including generated TypeScript parameter declarations
for tools.

## Codemode Arguments

Top-level `codemode` arguments:

```json
{
  "code": "async () => { ... }",
  "upstreams": ["optional-upstream-allowlist"],
  "tools": ["optional-tool-or-id-allowlist"]
}
```

Only `code` is required. The rest are Labby `codemode` arguments:

- `upstreams`: allow only named upstreams for this run.
- `tools`: allow only raw tool names or `<upstream>::<tool>` IDs.

Do not place these fields inside upstream tool params.

## Calling Tools

Use `callTool` when selecting dynamically or when helper sanitization is
unclear:

```js
async () => {
  return await callTool("github::search_issues", { q: "fix" });
}
```

Use `codemode.<upstream>.<tool>` only after `codemode.search()` confirms the helper name:

```js
async () => {
  return await codemode.github.search_issues({ q: "fix" });
}
```

The host validates params against the upstream input schema before dispatching.

## Action-Dispatched Upstreams

Many upstreams expose a single action-dispatched tool instead of one tool per
operation — `axon`, and the rmcp family (`unraid`, `unifi`, `sonarr`, `radarr`,
`cortex`, ...). They all take an `action`, but the rest of the envelope is
upstream-specific. Do not guess the envelope shape from memory.

- Discover operations with the tool's own `{ "action": "help" }`, or read the
  compact docs returned by `codemode.search()` and `codemode.describe()`.
- Put operation arguments exactly where the upstream schema expects them:

```js
// Axon search uses flat action fields.
async () => callTool("axon::axon", {
  action: "search",
  query: "mcpb",
  limit: 5
});

// Wrong for Axon: guessed nested params rejects with `invalid_param`
//   ("... must match exactly one schema").
async () => callTool("axon::axon", {
  action: "search",
  params: { query: "mcpb", limit: 5 }
});
```

An `invalid_param` that mentions `must match exactly one schema` means the
envelope matched no action variant. Re-read the schema and move arguments to the
expected fields. It is not a bug in the upstream tool.

## Destructive Tools

The MCP `codemode` tool currently accepts top-level `code`, `upstreams`, and
`tools`. It does not accept a public top-level `confirm` field.

Rules:

- `lab` or `lab:admin` scope authorizes execution but does not confirm effects.
- If a call returns `confirmation_required`, inspect the upstream schema and
  retry with the confirmation field exactly where that upstream expects it.
- `allow_destructive_actions` is internal-only. Do not use it as a public param.

## Return Shape

Successful `codemode` returns a trace envelope:

```json
{
  "result": {},
  "calls": [
    { "id": "name::tool", "ok": true, "elapsed_ms": 12 }
  ],
  "logs": []
}
```

Upstream result unwrapping:

- Prefer upstream `structuredContent`.
- Else join all text content and parse JSON when possible.
- Else return text or the full mixed MCP result shape.
- Per-call result payloads are not copied into `calls`.

> **Reading the value back.** `codemode` returns the envelope in the tool's text
> content block and a copy in `structuredContent` carrying both `result` and a
> compact `result_shape`; shaped runs also include `result_shaping` metadata.
> Most MCP clients (Claude Code included) surface `structuredContent` over text.
> If `result` comes back as a truncation marker — an object with `"truncated":
> true`, plus `preview` and `next_action` — the value exceeded the response
> budget (24 KB / 6000 tokens). Reduce the data inside the sandbox before
> returning, or write large payloads to an artifact and read them back — do not
> rely on a large `result` reaching the model verbatim.

Oversized final responses are replaced with a truncation marker. Reduce data in
the sandbox before returning large values.

## Final Result Shaping

`[code_mode].result_shape_policy` defaults to `"off"`. When set to
`"truncate"`, Labby shapes only the successful completed final `result` after
the sandbox returns and after the `__ui` compatibility unwrap. It then applies
the normal envelope budget and builds MCP text JSON plus `structuredContent`
from that same shaped response.

This does not change values seen inside the sandbox through `callTool()` or
`codemode.<upstream>.<tool>()`, and it does not add raw-result audit retention.
Use `writeArtifact()` for large detailed payloads. Truncation bounds output; it
is not redaction and must not be used to sanitize secrets.

## Error Recovery

Tool-call errors reject only that promise. Catch them locally when you want the
run to continue:

```js
async () => {
  const settled = await Promise.allSettled([
    callTool("a::one", {}),
    callTool("b::two", {})
  ]);
  return settled.map(r => r.status === "fulfilled" ? r.value : JSON.parse(String(r.reason.message)));
}
```

Common error kinds:

| Kind | Recovery |
| --- | --- |
| `missing_param` | Read `codemode.search()` / `codemode.describe()` output and include the required field. |
| `invalid_param` | Fix type/shape against the schema. |
| `validation_failed` | Fix nested schema validation errors. |
| `confirmation_required` | Inspect the upstream schema and provide confirmation where that upstream expects it. |
| `unknown_tool` | Rerun `codemode.search()`; use `<upstream>::<tool>` IDs only. |
| `call_budget_exceeded` | Reduce fan-out or split the work. |
| `result_too_large` | Reduce the upstream payload or write large data to an artifact. |
| `timeout` | Split work into smaller executions. |
| `oauth_needs_reauth` | Check `labby gateway mcp auth status <upstream> --json`. |

## Runtime And Limits

Implementation facts that affect operation:

- `codemode.search()` runs inside the sandbox and cannot call tools.
- `codemode` uses root `[code_mode]` config for timeout, response, token, log,
  and final-result shaping limits.
- Host-side env knobs also bound runner pool overflow, artifact size/retention,
  per-run `callTool` fan-out, and per-call result size.
- The runner process starts with a cleared environment and temp cwd.
- The parent host brokers all tool calls, validates schemas, applies
  confirmations, and terminates runaway executions.
- CLI `labby gateway code exec` is operator-driven and has its own policy for
  destructive upstream tools; MCP `codemode` exposes only `code`, `upstreams`,
  and `tools` as top-level arguments.

Current config defaults:

```toml
[code_mode]
enabled = true
trace_params = true
result_shape_policy = "off"
timeout_ms = 30000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

`gateway.code_mode.set` accepts the public fields in the generated action
catalog, including `result_shape_policy`.

## CLI Code Mode

CLI execution:

```bash
labby gateway code exec --code 'async () => ({ ok: true })' --json
labby gateway code exec --file ./snippet.js --json
```

The CLI mirrors execution only; there is no CLI `gateway code search`
subcommand. Use in-sandbox `codemode.search()` for catalog filtering.

## Safe Execution Pattern

1. Run `codemode.search()` and return only the candidate IDs/signatures needed.
2. Choose a narrow `upstreams` or `tools` allowlist.
3. Use `Promise.allSettled` when independent calls may partially fail.
4. Return a compact result object rather than raw large payloads.
