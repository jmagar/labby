---
name: mcp-gateway-tools
description: Use when invoking upstream MCP tools through the Lab gateway Code Mode surface — i.e. when the exposed MCP tools are `search` and `execute`, and real upstream tools are hidden behind the gateway. Trigger on "search", "execute", "gateway tools", "find a tool for...", "call X through the gateway", "what tools do I have", or whenever `list_tools` shows the synthetic `search` + `execute` pair. Not for designing or implementing new MCP tools, building MCP servers, or generic "what can you do" questions unrelated to the gateway.
---

# Gateway Tools: Search Then Execute

When the Lab gateway is in gateway search/execute mode, the MCP catalog collapses to two synthetic tools:

- `search` — runs a JavaScript async arrow function against the live upstream tool catalog.
- `execute` — runs a JavaScript async arrow function in the Code Mode sandbox and brokers `callTool()` calls to upstream MCP tools.

Everything else is hidden until you discover it through `search`. Your job is to drive that loop cleanly.

## Tool Name Resolution

In Claude Code / Codex the callable tool names are namespaced by the gateway's MCP server name. Match the suffix:

- Plain MCP server: `mcp__lab__search`, `mcp__lab__execute`
- Plugin registration: `mcp__plugin_labby_lab__search`, `mcp__plugin_labby_lab__execute`

If your `list_tools` shows exactly one `*__search` and one `*__execute`, and the descriptions mention Code Mode or `const tools = [...]`, use them.

## Tool Routing

Every connected upstream — homelab services (dozzle, plex, sonarr, unraid, cortex, axon, …) AND general-purpose servers (github, context7, gmail, google-calendar, google-drive, …) — is hidden behind this gateway. None of them appear as direct `mcp__lab__<service>` or `mcp__<server>__<tool>` entries in your tool list.

If a skill or request names a capability and you don't see a matching tool, do NOT conclude it's unavailable. Translate it:

1. `search` the catalog for the capability.
2. `execute` it via `callTool("<upstream>::<tool>", params)`.

Exception: a server with its own native MCP endpoint registered directly in the client (e.g. dozzle per its skill) — use that directly, not the gateway.

## The Loop

1. **Search.** Call `search` with JavaScript that filters the injected `tools` array.
2. **Read IDs and signatures.** Each entry has `id`, `upstream`, `name`, `description`, `schema`, `output_schema`, `signature`, and `dts`.
3. **If nothing fits**, re-search with a different predicate before giving up.
4. **Execute.** Call `execute` with JavaScript that uses `callTool(id, params)` or `codemode.<upstream>.<tool>(params)`.

## Search Shape

`search` takes `{ code: string }`. The code must be an async arrow function. The sandbox injects `const tools = [...]`.

```js
async () => tools
  .filter(t => t.upstream === "github" && /issue/i.test(t.description))
  .map(t => ({ id: t.id, signature: t.signature, dts: t.dts }))
```

Good search snippets return only what you need. Avoid dumping the entire catalog unless you are debugging catalog completeness.

## Execute Shape

`execute` takes `{ code: string, upstreams?: string[], tools?: string[] }`.

```js
async () => {
  const result = await callTool("github::search_issues", {
    q: "repo:jmagar/lab gateway"
  });
  return result;
}
```

IDs are always `<upstream>::<tool>`, for example `cortex::cortex` or `github::search_issues`. The old `upstream::<server>::<tool>` form is invalid.

The generated helper form is also available after `search` confirms the helper name:

```js
async () => codemode.github.search_issues({ q: "repo:jmagar/lab gateway" })
```

## Common Error Envelopes

The gateway returns structured errors as JSON text. Check `kind`.

| `kind` | What it means | What to do |
|---|---|---|
| `unknown_tool` | The upstream/tool id did not match a visible tool | Re-run `search` and use the returned `id` exactly |
| `invalid_code_mode_id` | The id is not `<upstream>::<tool>` or uses a reserved namespace | Fix the id shape |
| `validation_failed` | Params failed the upstream input schema | Use the `signature`, `dts`, or `schema` returned by `search` |
| `forbidden` | Missing scope | Surface to the user |
| `confirmation_required` | Destructive action without confirmation | Ask the user before retrying with confirmation if supported |
| `timeout` / `code_mode_timeout` | Execution exceeded the configured wall-clock limit | Split the work or reduce fan-out |
| `code_mode_fuel_exhausted` | JavaScript execution burned through fuel | Reduce local processing |
| `rate_limited` | Upstream throttling | Honor `retry_after_ms` when present |
| `upstream_error` | The upstream returned an error | Report the upstream message |

Surface unknown `kind` values verbatim.

## What Not To Do

- Do not call legacy `code_mode` / `tool_execute` unless those are the only names actually listed by the client; canonical names are `search` and `execute`.
- Do not use the old `tool_execute({ name, arguments })` shape with `execute`; current `execute` runs JavaScript.
- Do not guess IDs. Use `search` in the current session and copy the returned `id`.
- Do not use `upstream::` as a prefix; all gateway tools are upstream tools, so IDs start with the actual server name.
- Do not try to call Lab built-in service actions from inside Code Mode. Code Mode brokers upstream MCP tools only.

## Quick Reference

```js
// Search
search({ code: "async () => tools.filter(t => /logs/i.test(t.description))" })

// Execute
execute({ code: "async () => callTool('cortex::cortex', { action: 'help', params: {} })" })
```
