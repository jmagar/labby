# Code Mode

Code Mode is the JavaScript execution surface behind the MCP `codemode` tool. It
lets an agent discover upstream MCP tools, inspect compact docs, and run one async
JavaScript function in a sandbox that can call those upstream tools.

Lab actions are intentionally not exposed through Code Mode. Call Lab built-in
service tools directly when raw tools are visible, or use the native gateway
management/API surfaces for Lab actions.

## Surface

Code Mode's primary MCP surface is `codemode({ code })`. The code runs as one
async JavaScript function in the sandbox. Discovery, focused compact docs,
upstream calls, fan-out, filtering, and final result shaping all happen inside
that same execution.

Inside the sandbox:

- `await codemode.search("GitHub pull requests")` searches the reduced
  in-execution catalog.
- `await codemode.describe("github.list_pull_requests")` returns compact docs
  for an exact tool or snippet target.
- `await codemode.run("gateway-summary", input)` resolves and runs a snippet
  inside the same sandbox runtime.
- `await codemode.github.list_pull_requests(params)` calls the generated helper.
- `await callTool("github::list_pull_requests", params)` calls the raw bridge.

Example:

```ts
async () => {
  const matches = await codemode.search({ query: "GitHub pull requests", limit: 1 });
  const docs = await codemode.describe(matches.results[0].path);
  const pulls = await codemode.github.list_pull_requests({ state: "open" });
  return {
    docs: docs.path,
    open: pulls.items.map(pr => ({ number: pr.number, title: pr.title }))
  };
}
```

`Promise.all([...])` and `Promise.allSettled([...])` fan out independent upstream
calls. A failed `callTool` rejects only that promise; catch locally when partial
success is useful.

The gateway exposes only `codemode`. Discovery, schema inspection, tool calls,
and intermediate values stay inside one sandbox execution.

## Snippets

Snippet metadata appears in `codemode.search()` and `codemode.describe()` for
trusted-local or `lab:admin` callers. Snippets are listed as `kind: "snippet"`
and are invoked through the single helper:

```ts
async () => {
  const found = await codemode.search("snippet gateway");
  const docs = await codemode.describe(found.results[0].id);
  const summary = await codemode.run("gateway-summary", { includeHealth: true });
  await writeArtifact("gateway-summary.json", JSON.stringify(summary, null, 2), {
    contentType: "application/json"
  });
  return { docs: docs.path, summary };
}
```

`codemode.run()` lazily resolves snippet source through the host, then evaluates
`return await (<snippet-code>)(input)` inside the same Javy/QuickJS runtime as the
caller. A snippet can call `codemode.<upstream>.<tool>()`, `callTool()`,
`writeArtifact()`, and other snippets, bounded by the same Code Mode timeout plus
per-run snippet depth/count/byte budgets.

Snippet execution is admin/trusted-local only. Route-scoped Code Mode catalogs do
not expose user snippets, and host-side snippet resolution repeats the permission
check because discovery is not a security boundary.

Successful Code Mode executions return an `execution_id`. Admin callers can
promote the live process's retained source into a user snippet through the
`snippets` service:

```json
{
  "action": "snippets.promote",
  "params": {
    "execution_id": "01JEXAMPLE",
    "name": "gateway-summary",
    "description": "Summarize gateway health",
    "confirm": true
  }
}
```

Promotion source is deliberately ephemeral and live-gateway scoped. It is stored
only in memory, is evicted by retention limits, and disappears after restart,
deploy, or a different gateway process handles the promotion request. Promoted
source is written as plaintext executable snippet content under the user snippet
directory and may contain anything the original Code Mode source contained.

## Tool IDs and Helpers

Upstream tool IDs use:

```text
<upstream-name>::<tool-name>
```

`codemode` injects a runtime proxy generated from the live readable catalog, so
`codemode.github.search_issues(params)` calls the same bridge as:

```ts
callTool("github::search_issues", params)
```

Legacy `search` entries include both raw JSON Schemas and generated TypeScript:

- `schema` — input JSON Schema.
- `output_schema` — output JSON Schema when the upstream tool declares one.
- `signature` — one-line TypeScript call signature.
- `dts` — focused TypeScript declarations with JSDoc for that tool.

The `codemode.search` helper uses a reduced in-execution catalog (`kind`, `id`,
`path`, `upstream`, `name`, `description`, and `signature`) so normal runs do not
inject full schema, output schema, dts payloads, or snippet source. When a schema
is missing or too complex for the TypeScript emitter, generated signatures fall
back to `unknown`.

## Catalog Freshness

Code Mode does not build or read a durable vector, lexical, or RRF index. Each
`codemode` execution projects a transient catalog from the gateway runtime and
refreshes enabled upstream tool metadata through the gateway manager before
building the local discovery helpers and runtime proxy. Legacy `search` uses the
same catalog source, so helper visibility and direct `callTool` routing stay
aligned.

`gateway.reload` swaps in a freshly seeded lazy upstream pool. The next Code Mode
`codemode`, `search`, or `execute` call reprobes the relevant live upstreams and should see
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

2. Code Mode `codemode` proxy:

   ```ts
   async () => Object.keys(codemode.agent_os_windows_mcp).sort()
   ```

   For agent-os, the list should include `PowerShell`, `FileSystem`, `Snapshot`,
   and `Wait`.

3. Direct callability:

   ```ts
   async () => callTool("agent-os_windows-mcp::PowerShell", {
     command: "Write-Output MCP_OK"
   })
   ```

   If this succeeds while search is stale, the upstream is callable and the
   issue is catalog visibility rather than tool execution.

4. MCP legacy `search` injected catalog:

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

`codemode` accepts optional `upstreams` and `tools` arrays to narrow the per-run
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

`codemode` returns a capped envelope with:

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

## MCP Apps (mcp-ui) widgets

An upstream tool can return a native MCP Apps (mcp-ui) widget by carrying
`_meta.ui.resourceUri` (a `ui://<upstream>/...` URI served as
`text/html;profile=mcp-app`). Inside `execute`, the unwrapped `callTool` payload
drops that envelope metadata, so a widget would otherwise collapse to plain JSON.

When a snippet calls a widget-bearing upstream tool, `codemode` surfaces the most
recent captured widget metadata on the final tool result. The caller can also
return an object with a `__ui` key to unwrap a specific payload shape while
rendering the captured widget:

```ts
async () => {
  const dashboard = await codemode.axon.status_dashboard({});
  return { __ui: dashboard };   // optional: render the widget; surface `dashboard` as the result
}
```

Semantics:

- **Last-wins.** The broker records the most recent widget-bearing upstream call
  during the run; that link is the one surfaced. If the final return value uses
  `{ __ui: <result> }`, `<result>` is unwrapped into the execute `result` field
  so the model still sees the payload.
- **Native URIs.** The widget's `ui://<upstream>/...` URI is preserved verbatim.
  The gateway routes a `resources/read` of that URI to the owning upstream peer
  via catalog reverse-lookup (it is **not** rewritten to `lab://upstream/...`).
  `ui://lab/code-mode/*` remains reserved for Lab's own Code Mode app resources.
- **Identical mirroring.** The execute `CallToolResult` carries the upstream's
  `_meta.ui` object verbatim, so the host renders the widget identically to a
  direct connector. The widget itself is driven by the `ui://` resource read, not
  by inline content, so the execute trace content is left intact.
- The `CodeModeExecutionResponse` gains an optional `ui` field when a
  widget-bearing upstream result was captured.

### Widget → host callbacks

While the synthetic `codemode` surface is active, raw upstream tools stay hidden
from `list_tools`. Compatibility `search` and `execute` remain listed during the
migration window. MCP App tools that carry `_meta.ui.resourceUri` may still be
advertised so the host can render the widget.

A rendered MCP App can call back to its server only through host
`callServerTool` / `tools/call`. Lab allows those callback calls through Code
Mode's raw-tool gate only when all of these are true:

- the requested tool is an exposed upstream tool, not a Lab built-in service;
- the upstream is routable and allowed by the current protected route scope;
- the same upstream exposes at least one MCP App UI tool;
- the requested tool is not destructive.

The callback exemption changes callability only. It does not put sibling tools
back into `list_tools`, so the model-facing surface remains collapsed.
Destructive sibling callbacks return `confirmation_required`; callers should use
the `codemode` tool with `confirm:true` for destructive upstream actions.

`LAB_CODE_MODE_WIDGET_CALLBACKS=1` remains as a broader legacy operator bypass.
With that variable set, any known exposed non-destructive upstream tool may pass
the raw-tool gate while Code Mode is enabled. It does not bypass destructive
confirmation. Leave it off unless a legacy widget depends on callbacks that
cannot be represented by the same-upstream MCP App sibling rule.

## Error Contract

Tool errors reject with a JSON-encoded string that can be decoded in the sandbox:

```ts
try {
  await callTool("github::search_issues", {});
} catch (e) {
  const env = JSON.parse(String(e.message));
  return env.kind;
}
```

Canonical recovery buckets:

- Retry-safe: `rate_limited`, `timeout`, `network_error`
  - The live budget kind is `timeout`: the Javy/QuickJS runner's wall-clock
    backstop interrupts an over-running snippet and the host normalizes the
    trap to `timeout`. (`code_mode_fuel_exhausted` is **not** emitted on the
    live path — it belongs to the dead Wasmtime reference engine; see
    [Runner Architecture](#runner-architecture).)
- Fix-and-retry: `missing_param`, `invalid_param`, `validation_failed`,
  `confirmation_required`
- Terminal: `unknown_tool`, `unknown_action`, `auth_failed`, `server_error`,
  `internal_error`

Destructive upstream tools are gated by host-side metadata before dispatch. In
the MCP `codemode` surface, callers can explicitly confirm the whole Code Mode
run with top-level `confirm: true`. Execute-capable scopes (`lab` or
`lab:admin`) authorize Code Mode execution, but do not implicitly confirm
destructive upstream effects. Unconfirmed MCP destructive calls fail as
`confirmation_required`. CLI Code Mode execution permits destructive upstream calls because it is
operator-driven.

## Scope

- `lab` or `lab:admin` can use `codemode`.

OAuth callers retain their subject attribution when Code Mode calls upstream tools.
Trusted local callers use the shared gateway subject.

## Runner Architecture

The stdio parent-broker protocol is:

1. Parent starts (or reuses a pooled) `labby internal code-mode-runner` process.
2. Parent sends a `start` line; the child builds a FRESH QuickJS runtime and
   evaluates the normalized async function.
3. Child emits `tool_call` lines for `callTool` requests.
4. Parent dispatches through the gateway broker and replies with `tool_result` or
   `tool_error`.
5. Child settles pending promises and emits `done`.
6. The child then resets and parks for the next `start` (warm-runner pool).

### Warm-runner pool

The runner **process** is pooled and long-lived; the **JS runtime is rebuilt for
every execution**. Pooling amortizes the dominant fixed cost (process fork +
startup) without ever sharing JS state across callers — a brand-new runtime has
no globals, no leftover pending tool calls, and no captured data from a prior
run, so isolation holds by construction.

- **Process reuse, fresh runtime.** A pooled runner loops: read `start` → build a
  fresh `javy::Runtime` → run → emit `done`/`error` → reset and read the next
  `start`. It exits only when the parent closes stdin.
- **Per-execution isolation.** Each run resets the `callTool` sequence counter and
  creates a fresh, empty per-execution working-directory jail (removing the prior
  one), so a long-lived process never accumulates JS or filesystem state across
  callers. The 64 MiB heap, 30 s wall-clock timeout, and stack limit are enforced
  per execution.
- **Bounded pool, one execution per runner.** `N` runners serve `N` concurrent
  executions. When all are busy, an extra request is served by a bounded
  ephemeral (overflow) runner rather than queueing unboundedly.
- **Robustness.** A runner that crashes, times out, or violates the protocol is
  killed and replaced (the failing run surfaces a clean error — `timeout` on
  wall-clock expiry — never a hang). A pooled runner is also recycled
  (killed + respawned) after a fixed number of executions as cheap insurance
  against native-side leaks.
- **Configuration / kill switch** (environment, read at startup):
  - `LAB_CODE_MODE_POOL_SIZE` — number of pooled runners (default `2`, clamped to
    `16`). **`LAB_CODE_MODE_POOL_SIZE=0` disables pooling entirely**, falling back
    to spawn-per-execution with behavior identical to the pre-pool path.
  - `LAB_CODE_MODE_POOL_RECYCLE_AFTER` — executions before a runner is recycled
    (default `100`).
  - `LAB_CODE_MODE_POOL_MAX_OVERFLOW` — cap on simultaneous ephemeral overflow
    runners (default `8`).

  The conservative default (`size = 2`) keeps idle memory bounded while absorbing
  typical `codemode` bursts. The security invariants (`env_clear`,
  process-group/Job-Object reaping, `kill_on_drop`, `PR_SET_DUMPABLE`) are set
  once at spawn and therefore hold for the pooled process's whole lifetime.

Code Mode always uses Javy/QuickJS for snippet execution — it is the **sole live
engine**, with no Boa fallback and no `code_mode_wasm` feature. `codemode` runs
in the Javy/QuickJS child runner over stdio. The Javy toolchain is pulled in by
the `gateway` feature.

The runner starts with an empty environment in a temporary directory. It does not
provide Node, Deno, Bun, `fetch`, `connect`, `XMLHttpRequest`, `require`, or host
module `import()` access. `callTool` is the only host bridge exposed to user code.

> **Wasmtime is dead reference code, not a live path.** `wasm_runner.rs` is an
> unused engine skeleton retained only for reference; nothing on the live Code
> Mode path constructs or runs it. Its fuel/epoch-interruption design would
> normalize fuel and timeout traps to `code_mode_fuel_exhausted` and
> `code_mode_timeout`, but because the skeleton never executes, **neither kind is
> emitted today.** The only budget kind a caller observes on the live
> Javy/QuickJS path is `timeout` (the wall-clock backstop). Treat
> `code_mode_fuel_exhausted` / `code_mode_timeout` as reserved-for-the-dead-path
> and do not switch-case on them as live outcomes.

Loose JavaScript snippets are normalized before execution. Already-formed
function expressions pass through, while statement blocks such as
`const x = await callTool(...); x.items` are wrapped as `async () => { ... }` and
the trailing expression is returned.
