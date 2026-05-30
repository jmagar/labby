---
name: mcp-gateway-tools
description: Use when invoking MCP tools through the Lab gateway in tool-search mode — i.e. when the only exposed MCP tools are a `*__tool_search` + `*__tool_execute` pair and the real upstream tools (radarr, sonarr, github, paperless, etc.) are hidden behind them. Trigger on "tool_search", "tool_execute", "the gateway", "find a tool for…", "is there a tool that…", "search for a tool", "call X through the gateway", "what tools do I have", or whenever `list_tools` shows that synthetic pair. Not for designing or implementing new MCP tools, building MCP servers, or generic "what can you do" questions unrelated to the gateway.
---

# Gateway tools: search → execute

When the Lab gateway is in **tool-search mode**, your MCP catalog collapses to two synthetic tools:

- `tool_search` — full-text + score search across every healthy upstream tool *and* every built-in Lab service action.
- `tool_execute` — invoke one tool by name with its arguments.

Everything else (radarr, sonarr, paperless, github, etc.) is hidden until you discover it via `tool_search`. Your job is to drive that loop cleanly.

## Tool name resolution (first thing to get right)

In Claude Code / Codex the names you actually call are namespaced by the gateway's MCP server name. The prefix varies; the suffix doesn't:

- Plain MCP server: `mcp__lab__tool_search`, `mcp__lab__tool_execute`
- Shipped as a plugin: `mcp__plugin_lab_lab__tool_search`, `mcp__plugin_lab_lab__tool_execute`

**Match on the suffix.** If your `list_tools` shows exactly one `*__tool_search` and one `*__tool_execute`, that's the gateway — use them.

## The loop

1. **Search.** Call `tool_search` with a short natural-language query describing what you want to do — not just keywords. Two or three content words beats one.
2. **Read results.** Each hit has `name`, `description`, `upstream`, `score`, and (if you asked) `input_schema`. Higher `score` is better. `upstream: "lab"` is a built-in Lab service; anything else is a proxied upstream MCP server.
3. **If nothing fits**, re-search with a different phrasing before giving up. The server's own error hint on a missed `tool_execute` is literally "Call tool_search to discover available tools" — take the hint.
4. **Execute.** Call `tool_execute` with `{ name, arguments }` using the **right argument shape** (see next section).
5. **On `ambiguous_tool` errors**, the envelope includes a `valid: [...]` list of fully-qualified names. Pick one and retry.

## Argument shape — this is where things go wrong

`tool_execute` always takes `{ name, arguments }`, but `arguments` looks different for the two kinds of tools:

### Built-in Lab service (`upstream: "lab"`)

Action-style. The service name is the tool name; the work it does is selected by `action`:

```json
{
  "name": "radarr",
  "arguments": {
    "action": "movie.search",
    "params": { "query": "Inception" }
  }
}
```

The first dozen or so visible actions (cap is server-side) for a built-in service are appended directly to its `description` field in every search hit — you don't need `include_schema: true` just to see action names. For full per-action parameter shapes, either pass `include_schema: true` and read the `input_schema`, or call the service's `help` action: `arguments: { "action": "help" }`.

### Upstream MCP server tool (anything else)

The tool's own raw schema. Pass `arguments` as that tool expects it. Use `include_schema: true` on the search call so you know the field shape before you invoke:

```json
{
  "name": "search_issues",
  "arguments": { "query": "repo:jmagar/lab tool_search", "limit": 10 }
}
```

## Calling `tool_search` well

Schema: `{ query: string (≤500 chars), top_k?: int 1..=50 (default 10), include_schema?: bool (default false) }`.

- **Use `include_schema: true` whenever you intend to execute an upstream tool** — saves a round trip and avoids guessing field names. Schemas above a server-side size cap (~16 KB at time of writing) are dropped; if `input_schema` is missing from a hit you wanted, fall back to a tool-specific `help` action or just try a minimal call and read the error.
- **Default `top_k` is 10.** Bump to 20–30 for fuzzy "is there anything that does X" exploration; keep low for targeted lookups.
- **Queries are scored on name + description tokens** with a prefix boost and token-boundary matching (splits on `_` and `-`). Be concrete: `"github issue search"` > `"issue"`. `"movie search radarr"` > `"radarr"`.
- **Don't paginate by re-querying with weird offsets** — there's no pagination. Re-phrase instead.

## Common error envelopes

The gateway returns structured errors as JSON text. Check `kind` (canonical set from `crates/lab/src/mcp/server.rs`):

| `kind` | What it means | What to do |
|---|---|---|
| `unknown_tool` | `name` didn't match anything | Re-run `tool_search` with a different query |
| `ambiguous_tool` | Multiple upstreams expose this name | Pick from `valid: [...]` and retry with the fully-qualified name |
| `unknown_action` | Built-in service doesn't expose that action on MCP | Use `valid: [...]` from the envelope, or call `action: "help"` |
| `not_found` | Service exists but isn't enabled on the MCP surface | Surface to user; can't fix from the tool call |
| `forbidden` | Missing scope (need `lab` or `lab:admin`) | Surface to user; can't fix from the tool call |
| `confirmation_required` | Destructive built-in action without confirm | Re-call with `params: { ..., "confirm": true }` after the user agrees |
| `index_warming` | Search index is rebuilding | Honor `retry_after_ms` from the envelope (currently ~2000); built-in hits may still come back during warm-up |
| `invalid_param` | Bad query/argument shape | Read the `param` field and fix it |
| `rate_limited` | Upstream throttling | Back off; don't hammer |
| `upstream_error` | Upstream server returned an error or is disconnected | Report the message; don't blindly retry — it usually won't fix itself |
| `internal_error` | Gateway-side bug | Surface verbatim to user; not something to retry around |

If you see a `kind` not in this table, surface it verbatim — don't paper over an unknown failure mode.

## What NOT to do

- **Don't guess tool names.** If you haven't seen a name come back from `tool_search` in this session, search first. The hidden upstream catalog can be hundreds of tools — guessing burns user trust. Even if you've used a tool with this name in a previous session, exposure is per-gateway-config and may have changed.
- **Don't loop `tool_execute` with the same name** when it returns `unknown_tool`. The next call will fail the same way. Search instead.
- **Don't batch parallel `tool_execute` calls with guessed names** hoping one works. Wasteful, noisy in logs, and you still don't know which call did what when the results come back.
- **Don't dump raw search JSON at the user** unless they asked or you're explicitly debugging the gateway itself. Summarize: "Found 3 candidates — `github__search_issues`, `gitea__list_issues`, `linear__search`. Going with `github__search_issues` because [reason]." Then invoke.
- **Don't forget the argument-shape split.** A built-in radarr call with `{query: "Inception"}` at the top of `arguments` (instead of inside `params`) will fail with `unknown_action: ""`.
- **Don't skip the search just because the user named a tool.** "Use github" → still search for `"github"` first to find the actual exposed tool name in this user's gateway. Tool names depend on which upstreams are wired up.

## Quick reference

```
search:   tool_search({ query, top_k?, include_schema? })
execute:  tool_execute({ name, arguments })

built-in args: { action: "name.sub", params: { ... } }
upstream args: <whatever the upstream tool's input_schema says>
```
