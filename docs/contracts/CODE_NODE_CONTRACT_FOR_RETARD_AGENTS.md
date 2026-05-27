# Code Mode — Agent Contract

> **THIS IS THE SOURCE OF TRUTH for what the `code` MCP tool exposes to agents.**  
> `code_execute_description.md` is deprecated and will be deleted.  
> The tool description loaded by `mcp/server.rs` must match this document exactly.  
> If this document and the tool description diverge, this document wins.

---

## The `code` Tool

```
name: "code"
scope required: lab:read (catalog only) | lab / lab:admin (execution)
mode: Code Mode only — mutually exclusive with Tool Search mode (search + execute)
```

### What it does

You write an async JavaScript function. The sandbox runs it. Every upstream MCP tool
available on this gateway is pre-declared as a typed TypeScript helper in the sandbox
namespace `codemode`. You call those helpers like typed functions. The host dispatches
each call to the real upstream server. You get the result back as the function's return
value.

**You do not call `code_search` first. It does not exist.**  
**You do not call `callTool` with a string ID to discover tools. You read the types.**  
**The typed catalog IS the discovery. It is already there.**

---

## Typed Catalog — What Is Injected Into Your Sandbox

The typed preamble is injected by the server into the sandbox before your code runs —
not fetched by you via a discovery call. The `code` tool description stays static and
short. The preamble content is built server-side from the live upstream catalog and
prepended to your code at execution time.

At execution time, before your code runs, the sandbox receives a preamble of TypeScript
declarations for every upstream tool currently connected to this gateway. It looks like
this (content varies by connected upstreams):

```typescript
// Auto-generated catalog — <N> tools across <M> upstreams
// Generated: <ISO timestamp>
// If a tool is missing, the upstream may be disconnected. Check gateway status.

declare namespace codemode {
  namespace radarr {
    /** Search for movies by title query. Returns up to limit results. */
    function movieSearch(params: {
      query: string;
      limit?: number;
    }): Promise<{
      movies: Array<{
        id: number;
        title: string;
        year: number;
        imdbId: string;
        overview: string;
        monitored: boolean;
      }>;
    }>;

    /** Add a movie to Radarr for monitoring and download. */
    function movieAdd(params: {
      tmdbId: number;
      qualityProfileId: number;
      rootFolderPath: string;
      monitored?: boolean;
    }): Promise<{ id: number; title: string }>;

    // ... all radarr tools
  }

  namespace sonarr {
    /** Search for TV series by term. */
    function seriesSearch(params: { term: string }): Promise<Array<{
      id: number;
      title: string;
      seasons: number;
      status: string;
    }>>;
    // ... all sonarr tools
  }

  // ... all connected upstreams
}
```

**Read the namespace. Call the functions. The types tell you everything.**

---

## How To Write Code

### The basics

Your code must be an async arrow function expression or an async function expression.
The return value of your function is what the tool returns.

```typescript
// CORRECT — async arrow function
async () => {
  const result = await codemode.radarr.movieSearch({ query: "The Matrix" });
  return result.movies[0];
}

// CORRECT — async function expression
async function main() {
  const [movies, series] = await Promise.all([
    codemode.radarr.movieSearch({ query: "Breaking Bad" }),
    codemode.sonarr.seriesSearch({ term: "Breaking Bad" }),
  ]);
  return { movies, series };
}
main
```

### Parallel calls — always use Promise.all for independent reads

```typescript
async () => {
  // GOOD — fires all three simultaneously, waits for all
  const [movies, series, music] = await Promise.all([
    codemode.radarr.movieSearch({ query: "Dune" }),
    codemode.sonarr.seriesSearch({ term: "Dune" }),
    codemode.navidrome.albumSearch({ query: "Dune" }),
  ]);
  return { movies, series, music };
}

// BAD — serial, three times slower for no reason
async () => {
  const movies = await codemode.radarr.movieSearch({ query: "Dune" });
  const series = await codemode.sonarr.seriesSearch({ term: "Dune" });
  const music = await codemode.navidrome.albumSearch({ query: "Dune" });
  return { movies, series, music };
}
```

### Reducing results in the sandbox — this is why Code Mode exists

```typescript
async () => {
  // Fetch a big result set, reduce it here, return only what matters
  const result = await codemode.radarr.movieSearch({ query: "", limit: 500 });
  return result.movies
    .filter(m => m.year >= 2020 && !m.monitored)
    .sort((a, b) => b.year - a.year)
    .slice(0, 10)
    .map(m => ({ title: m.title, year: m.year, imdbId: m.imdbId }));
}
```

Without Code Mode you would get 500 movies back to the model context, use all your
tokens on data you don't want, and have to ask again. With Code Mode you get 10 movies.

---

## callTool — The Escape Hatch

If a tool is not in the typed `codemode` namespace (e.g., the catalog was truncated, or
you need to call a tool by dynamic ID), use `callTool`:

```typescript
declare function callTool<T = unknown>(
  id: `upstream::${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

Tool IDs have the form `upstream::<server>::<tool>`. Example:
`upstream::radarr::movie.search`. You can find IDs in the codemode namespace or by
inspecting `__catalog__` if it is present.

**Prefer typed `codemode.*` helpers. Use `callTool` only when necessary.**

---

## Scope Requirements

| Operation | Required scope |
|-----------|---------------|
| Read the typed catalog | `lab:read`, `lab`, or `lab:admin` |
| Execute `codemode.*` calls or `callTool` | `lab` or `lab:admin` |

If your token has `lab:read` scope and you call `codemode.radarr.movieSearch(...)`,
you will receive a `forbidden` error. This is not a bug in your code. Your token does
not have execution scope. Get a token with `lab` scope.

---

## Error Handling

Every `codemode.*` call and `callTool` call throws a `CodeModeError` on failure.
Catch it, parse it, react to the `kind`.

```typescript
type CodeModeError = {
  kind:
    // Terminal — do not retry, your code cannot fix these
    | "unknown_tool"        // tool ID does not exist on any connected upstream
    | "unknown_action"      // upstream knows the server but not this action
    | "auth_failed"         // upstream rejected the request — auth issue, not scope
    | "server_error"        // upstream returned 5xx
    | "internal_error"      // gateway-side failure
    | "decode_error"        // upstream returned unparseable response

    // Fix and retry — change your code or params
    | "missing_param"       // required param not provided
    | "invalid_param"       // param provided but invalid value
    | "validation_failed"   // upstream rejected the input
    | "confirmation_required" // destructive action requires explicit confirmation

    // Retry with backoff
    | "rate_limited"        // honor retry_after_ms before retrying
    | "timeout"             // upstream call timed out — retry once, then give up
    | "network_error"       // transient network issue — retry once

    // Resource budget — split your work
    | "tool_call_limit_exceeded"   // hit max_tool_calls for this execution
    | "code_mode_timeout";         // total execution time exceeded timeout_ms

  message: string;
  valid?: string[];        // for unknown_tool/unknown_action: valid options
  hint?: string;           // human-readable recovery hint
  retry_after_ms?: number; // for rate_limited
};
```

### Recovery pattern

```typescript
async () => {
  try {
    return await codemode.radarr.movieSearch({ query: "Matrix" });
  } catch (e) {
    const err: CodeModeError = JSON.parse(String(e.message));
    switch (err.kind) {
      case "rate_limited":
        // Honor the backoff — but you can't sleep in the sandbox, so return
        // a structured error for the host to handle
        return { error: err.kind, retry_after_ms: err.retry_after_ms };
      case "missing_param":
      case "invalid_param":
        // Your code has a bug — don't retry, surface it
        throw e;
      case "tool_call_limit_exceeded":
      case "code_mode_timeout":
        // You did too much — return what you have so far
        return { partial: true, error: err.kind };
      default:
        throw e;
    }
  }
}
```

---

## Response Shape

The tool returns:

```typescript
type CodeModeResult = {
  result: unknown;          // the return value of your async function
  calls: Array<{
    id: string;             // tool ID called
    result: unknown;        // what that call returned
  }>;
  logs: string[];           // console.log/warn/error output from your code
};
```

`result` is the primary output. Prefer returning a computed value from your function
rather than relying on `calls` — `calls` is there for debugging, not for primary data.

---

## Limits

| Limit | Default | Config key | Range |
|-------|---------|------------|-------|
| Execution timeout | 5 000 ms | `code_mode.timeout_ms` | 1..=60 000 |
| Max tool calls | 8 | `code_mode.max_tool_calls` | 1..=50 |
| Response bytes | 24 576 (24 KB) | `code_mode.max_response_bytes` | 1 024..=1 048 576 |
| Response tokens | 6 000 | `code_mode.max_response_tokens` | 256..=256 000 |

### Result truncation

If a single `callTool` / `codemode.*` call returns a result larger than the response
budget, it is replaced with a truncation marker:

```typescript
{
  truncated: true,
  original_size: number,     // bytes
  original_tokens: number,   // estimated tokens
  preview: string,           // first ~500 chars of the result
  next_action: string        // hint on how to get the full result
}
```

Use the sandbox to reduce data before returning it. That is the whole point.

---

## What To Do When A Tool Is Missing

If `codemode.<upstream>.<tool>` does not exist in the typed namespace:

1. **The upstream may be disconnected.** Call `codemode.__meta__.upstreams()` (if
   available) to see connected upstreams. Or just try; you'll get `unknown_tool`.

2. **The catalog may have been truncated.** If `__catalog__` appears in the namespace,
   the catalog exceeded the 256KB soft cap and some tools were dropped. Use the Tool
   Search mode (`search` tool) from outside Code Mode to find specific tools, then come
   back into Code Mode with their IDs and use `callTool` directly.

3. **You cannot call Lab actions (`lab::*` tool IDs).** Code Mode handles upstream MCP
   tools only. For Lab built-in actions (radarr service dispatch, gateway management,
   etc.), exit Code Mode and use the `execute` tool in Tool Search mode.

---

## What Code Mode Is NOT

| Thing | Answer |
|-------|--------|
| Can I call Lab actions (radarr, sonarr, etc.) directly as `lab::radarr::...`? | No. Use upstream connections through `codemode.*`. Lab actions are internal dispatch, not MCP tools. |
| Can I make HTTP requests from inside the sandbox? | No. The sandbox has no network access. All calls go through `codemode.*` / `callTool` to the host broker. |
| Can I use `require()` or `import`? | No. The sandbox is a constrained JS runtime (Boa engine). No module system. |
| Can I use Node.js APIs (fs, path, crypto, etc.)? | No. No Node APIs. Pure JS only plus `codemode.*` and `callTool`. |
| Can I use `setTimeout` / `setInterval`? | No. |
| Can I call `code_search` to discover tools first? | No. `code_search` does not exist. Read the typed namespace. |
| Is this available when Tool Search mode is active? | No. Code Mode and Tool Search mode are mutually exclusive. |
