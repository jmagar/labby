# Code Mode — Implementation Specification

> **THIS IS THE SOURCE OF TRUTH.**  
> If this document conflicts with anything in `code_execute_description.md`, any research
> agent output, any prior bead comment, or any LLM-generated suggestion, **this document
> wins.** Update the other thing.

---

## What Code Mode Is

Code Mode exposes a **single MCP tool named `code`** that lets an LLM write JavaScript
to orchestrate multiple upstream tool calls in one round-trip.

The LLM receives the **typed catalog of every available upstream tool as TypeScript
function signatures**, injected directly into the sandbox at execution time. The model
reads the types, writes an async function that calls those typed helpers, and the sandbox
routes each call back to the host broker which dispatches to the real upstream MCP server.

**One tool. One trip. Self-contained. No separate discovery step.**

This is the Cloudflare Code Mode model. See:
- https://developers.cloudflare.com/agents/api-reference/codemode/
- https://blog.cloudflare.com/code-mode-mcp/

---

## The Cloudflare Reference (What We Are Matching)

Cloudflare `createCodeTool` does exactly this:

1. Takes an existing MCP server (or OpenAPI spec)
2. Generates TypeScript type definitions for every available operation
3. Advertises **one tool** to the model: `code` (write-code tool)
4. The model writes `async () => { const result = await codemode.listMovies({...}); return result; }`
5. A `Proxy` intercepts `codemode.*` calls and routes them to the host via Workers RPC
6. The host dispatches to the real backend; the sandbox never makes direct network calls
7. The script's final return value is surfaced to the model as the tool result

**Cloudflare's actual tool surface for Code Mode: `code`. One tool. Done.**

Their separate `search` and `execute` tools are for the **OpenAPI/Tool Search mode** — 
a different mode that brokered discovery and invocation without code execution. That maps
directly to our Tool Search mode (`search` + `execute`). These are **two separate, mutually
exclusive modes.** The plan is correct. The modes are:

| Mode | Tools Advertised | Purpose |
|------|-----------------|---------|
| Tool Search mode | `search`, `execute` | Semantic discovery + brokered invocation |
| Code Mode | `code` | Typed JS orchestration with inline typed catalog |

---

## The Typed Catalog — This Is The Discovery Mechanism

The current `code_search` approach injects `const tools = [<JSON catalog>]` and then runs
user JS against it. That is a stopgap. The target is:

**Generate TypeScript type definitions from the catalog and inject them as the sandbox preamble.**

This means the sandbox receives something like:

```typescript
// AUTO-GENERATED — do not modify
// Available upstream tools as of <timestamp>
// Catalog: <N> tools across <M> upstreams

declare namespace codemode {
  namespace radarr {
    function movie_search(params: { query: string; limit?: number }): Promise<{
      movies: Array<{ id: number; title: string; year: number; imdbId: string }>;
    }>;
    function movie_add(params: { tmdbId: number; qualityProfileId: number; rootFolderPath: string }): Promise<void>;
    // ...
  }
  namespace sonarr {
    function series_search(params: { term: string }): Promise<Array<{ id: number; title: string }>>;
    // ...
  }
  // ... all upstreams
}

// callTool is still available for cases where typed helpers don't cover a tool
declare function callTool<T = unknown>(
  id: `upstream::${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

Tool names use snake_case (Cloudflare-parity): separators (`.`, `-`, `/`, `:`) in the
upstream tool id all become `_`. The original id is preserved in the `callTool` escape
hatch (`upstream::radarr::movie.search`) so the dispatcher routes correctly.

The model reads the types and writes:

```typescript
async () => {
  const movies = await codemode.radarr.movie_search({ query: "The Matrix" });
  const series = await codemode.sonarr.series_search({ term: "Breaking Bad" });
  return { movies: movies.movies.slice(0, 3), series: series.slice(0, 3) };
}
```

**This is the whole point.** The model does not call `code_search` first. It does not need
to. It already has the types. It writes correct, type-safe code on the first attempt.

### What This Replaces

The following are **dead patterns** after this implementation:

- `code_search`: Runs JS against JSON catalog to filter tools. **Gone.** The typed
  preamble replaces it entirely — the model has the catalog already.
- The two-tool round-trip: model calls `code_search` to discover, then `code_execute`
  to run. **Gone.** Single `code` call with typed preamble.
- `code_execute_description.md` line 1: "Use IDs returned by `code_search`." **Gone.**
  Circular reference to a tool that no longer exists.

---

## Execution Architecture

```
MCP client
    │
    ▼
mcp/server.rs  ← receives code tool call, checks scope, dispatches
    │
    ▼
dispatch/gateway/code_mode.rs  ← CodeModeBroker::execute()
    │
    ├─ generates TypeScript typed preamble from catalog
    ├─ prepends preamble to user code
    ├─ spawns sandboxed subprocess (current_exe() "internal code-mode-runner")
    │   ├─ env_clear()
    │   ├─ temp cwd
    │   ├─ kill_on_drop(true)
    │   └─ stdin/stdout JSON protocol
    │
    ├─ sandbox runs user code in Boa/WASM JS engine
    ├─ sandbox calls codemode.* → host intercepts → dispatches to GatewayManager
    ├─ GatewayManager → upstream MCP server → returns result → back to sandbox
    │
    └─ final return value of the async function → CodeModeExecutionResponse
```

**The sandbox never makes direct network calls. The host broker is the only path to
upstream tools.** This is the load-bearing security invariant. Do not break it.

---

## The `code` Tool Interface

### MCP Registration

```
name: "code"
description: <see docs/contracts/CODE_NODE_CONTRACT_FOR_RETARD_AGENTS.md>
```

Advertised **only** when `code_mode.enabled = true` in config.  
**Never** advertised alongside `search`/`execute`.

### Input Parameters

```typescript
{
  code: string;          // JavaScript async arrow function: async () => { ... }
  max_tool_calls?: number; // default: config.max_tool_calls (default 1000, high safety ceiling); the 30s timeout is the meaningful bound
}
```

### Output Shape

`CodeModeExecutionResponse` — the current struct is missing the final script result.
**Add it:**

```rust
pub struct CodeModeExecutionResponse {
    pub result: Option<Value>,   // ADD THIS — the final return value of the async function
    pub calls: Vec<CodeModeExecutedCall>,
    pub logs: Vec<String>,       // ADD THIS — captured console.log/warn/error output
}
```

This matches Cloudflare's `ExecuteResult: { result: unknown; error?: string; logs?: string[] }`.
Without `result`, the model cannot reduce a large multi-call computation inside the sandbox —
it must return all intermediate results, which wastes tokens and defeats half the purpose.

---

## Scope Model

| Scope | Can call `code`? | Can use typed helpers (read catalog)? | Can execute callTool? |
|-------|-----------------|--------------------------------------|----------------------|
| `lab:read` | Yes (read path only) | Yes | **No** |
| `lab` | Yes | Yes | Yes |
| `lab:admin` | Yes | Yes | Yes |

The `code` tool has **two scope levels inside it**, gated per-subaction:

- Catalog preamble generation (read path) → `CodeModeCaller::can_read()`  
- `callTool` / `codemode.*` dispatch → `CodeModeCaller::can_execute()`

**The MCP scope guard at `call_tool` in `server.rs` is the outer gate.**  
`CodeModeBroker` applies the inner per-operation gates.  
Both gates must exist. Neither replaces the other.

---

## Mutual Exclusion With Tool Search Mode

Code Mode and Tool Search mode are **mutually exclusive**. Both enabled simultaneously
is a config error. The enforcement must exist in:

1. `LabConfig::validate()` — fires on TOML load at startup
2. `config_mutation.lock()` in `manager.rs` — fires on runtime API mutation

The error is `ToolError::InvalidParam` with a message naming both modes and telling the
operator to choose one. Log at `WARN` level (operator/caller error, not internal error).

The `gateway_code_mode_enabled()` function in `mcp/catalog.rs` currently delegates to
`manager.tool_search_enabled()` — **this is wrong** and must be fixed before any of this
works. It must read `cfg.code_mode.enabled`.

---

## Config

```toml
[code_mode]
enabled = false              # mutually exclusive with [tool_search].enabled = true
timeout_ms = 30000           # valid: 1..=60000 (Cloudflare-parity default)
max_tool_calls = 1000        # valid: 1..=10000 (high safety ceiling; the 30s timeout is the real bound)
max_response_bytes = 24576   # valid: 1024..=1048576 (24KB default)
max_response_tokens = 6000   # valid: 256..=256000
```

---

## What Is Explicitly NOT In This Implementation

The following were suggested by research agents and are **wrong for this implementation**.
Do not implement them.

### ❌ Keep `code_search` + `code_execute` as separate MCP tools

The entire point is **one tool**. Two tools means the model needs a discovery round-trip
before execution. That is the pattern Code Mode eliminates.

### ❌ Merge `code_search` + `code_execute` into one tool via an "action discriminator" in params

> (Suggested by pattern-recognition-specialist: "single code MCP tool needs a discriminator
> in params — action: 'search' | 'execute'")

No. This is two tools wearing a trench coat. The model still has to decide which subaction
to call, which means it still has a discovery step. The typed preamble replaces `search`
entirely. There is no discriminator. The `code` tool takes `code: string`. That's it.

### ❌ "Staged minimal form" — start with `const tools = [...]` JSON injection, defer typed helpers

> (Suggested by code-simplicity-reviewer as YAGNI)

Wrong. The typed helpers **are the feature**. Without them, Code Mode is just a more
complicated version of `code_execute` with extra ceremony. The JSON `tools` array is
useful as a fallback when type generation fails or the catalog is too large for types,
but it is not the primary interface. Ship typed helpers.

The `const tools = [...]` injection currently used by `code_search` is the stopgap this
implementation is replacing. Keep it as a fallback for catalog-too-large cases. Make typed
`codemode.*` helpers the primary interface.

### ❌ Embed `deno_core` instead of subprocess

> (Suggested by best-practices-researcher as a security improvement)

Valid security improvement but out of scope for this epic. The subprocess boundary
(`current_exe() internal code-mode-runner`) is the established architecture. The Boa
engine handles in-process search; the subprocess handles execution. Do not change this
as part of lab-inyc7. File a separate bead if you want to pursue deno_core embedding.

### ❌ Seccomp BPF / Landlock / capabilities drop on the subprocess

> (Suggested by best-practices-researcher)

Correct hardening but out of scope for lab-inyc7. The current `env_clear() + temp cwd +
kill_on_drop(true)` boundary is the baseline. Harden it in a dedicated security bead.
Do not block Code Mode on kernel-level sandbox work.

### ❌ `GatewayExposureMode` as a new standalone enum

> (Suggested by simplicity-reviewer and architecture-strategist)

`ToolSearchVisibility` already exists in `mcp/catalog.rs` with `Raw`, `RootSynthetic`,
`InProcessPeer`. Add a `CodeMode` variant to it. Do not create a second enum for the
same concept.

### ❌ TypeScript discriminated union at the gateway-admin client level

> (Suggested by kieran-typescript-reviewer for gateway.ts types)

The backend delivers `tool_search` config and `code_mode` config via separate actions
and separate SWR keys. Don't collapse them into a discriminated union at the client
level — that requires a new combined fetcher, a new SWR key, and a schema migration.
Fix the exclusivity at the input boundary and the `handleToggle` guard. A discriminated
union is only appropriate if a unified `gateway.mode.set` action is ever introduced.

---

## Files Owned By This Spec

| File | Role |
|------|------|
| `crates/lab/src/dispatch/gateway/code_mode.rs` | All broker logic — `CodeModeBroker`, typed preamble generation, sandbox dispatch |
| `crates/lab/src/mcp/server.rs` | MCP adapter only — registration, envelope, scope check at call_tool boundary |
| `docs/specs/CODE_MODE_SPEC_FOR_RETARD_AGENTS.md` | This file — the spec |
| `docs/contracts/CODE_NODE_CONTRACT_FOR_RETARD_AGENTS.md` | The agent-facing contract (what the model sees) |
| `crates/lab/src/config.rs` | `CodeModeConfig`, `LabConfig::validate()` cross-check |
| `crates/lab/src/dispatch/gateway/config.rs` | `validate_config()` cross-check |
| `crates/lab/src/dispatch/gateway/manager.rs` | Mutual exclusion inside `config_mutation.lock()` |
| `crates/lab/src/mcp/catalog.rs` | `ToolSearchVisibility` — add `CodeMode` variant; fix `gateway_code_mode_enabled()` |

---

## Implementation Order (lab-inyc7 children)

1. **lab-inyc7.1** — Fix `gateway_code_mode_enabled()` delegation bug + add mutual
   exclusion to `LabConfig::validate()` + `config_mutation.lock()`. Nothing else works
   until this is done.

2. **lab-inyc7.3** — Replace `code_search` + `code_execute` with single `code` tool.
   Add TypeScript typed preamble generation. Add `result` and `logs` to
   `CodeModeExecutionResponse`. Fix `LAB_ACTION_UNKNOWN_TOOL_HINT` constant. Rewrite
   tool description per `docs/contracts/CODE_NODE_CONTRACT_FOR_RETARD_AGENTS.md`.

3. **lab-inyc7.2** — Rename `tool_search`→`search`, `tool_execute`→`execute`. Add
   legacy aliases with `tracing::warn!`. Update scope guards on all alias arms.

4. **lab-inyc7.4** — Docs, config.example.toml, GATEWAY.md, gateway-admin TS fixes.

5. **lab-inyc7.5** — Snapshot tests per mode, absence assertions for old names.
