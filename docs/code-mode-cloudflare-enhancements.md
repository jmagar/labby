# Code Mode — Lab vs Cloudflare: Enhancement Analysis

> Session 2026-05-31. Compares Lab's Code Mode
> (`crates/lab/src/dispatch/gateway/code_mode.rs`, `code_mode_preamble.rs`,
> `projection.rs`) against Cloudflare's **current** Code Mode source — the
> `cloudflare/agents` monorepo `packages/codemode/` package (git `fac4463`),
> which has evolved well past the `blog.cloudflare.com/code-mode` post.

## Evidence basis

Everything below was read from source on both sides (file:line cited inline).
The Cloudflare blog describes an earlier state; the cloned repo is the source of
truth for what CF actually ships today. Key correction to any blog-based reading:
**`packages/agents/src/codemode/ai.ts` is now a 5-line throwing stub** — the real
implementation is the standalone `@cloudflare/codemode` package.

## PR 85 follow-up status

The current implementation has moved several items in this comparison from
"recommended" to "present":

- `search` entries now include `output_schema`, `signature`, and `dts`, so typed
  discovery is delivered progressively through `search` rather than injected as a
  monolithic execute preamble. Missing or unsupported schemas degrade to
  `unknown`.
- Successful upstream calls are unwrapped before they reach sandbox JavaScript:
  `structuredContent` wins, all-text content is parsed as JSON when possible, and
  mixed/non-text content keeps its MCP JSON shape.
- `execute` supports `upstreams` and `tools` capability filters. When present,
  they must be JSON string arrays; non-array values are ignored as absent filters,
  and empty strings are dropped.
- Destructive upstream calls are host-gated. MCP `execute` can confirm the run
  with top-level `confirm: true`. Execute-capable scopes (`lab` or `lab:admin`)
  authorize Code Mode execution, but do not implicitly confirm destructive
  upstream effects. Unconfirmed MCP destructive calls receive
  `confirmation_required`. CLI execution permits destructive upstream calls.
- The runner has a tagged base64 codec for JavaScript `ArrayBuffer` and typed
  array values crossing the parent/runner boundary. Mixed or binary MCP content
  blocks that are not unwrapped as structured/all-text content still remain JSON
  MCP content.

## The single most important finding

Before PR 85, we **deliberately removed typed signatures** from Code Mode in the
search+execute pivot, citing "Cloudflare parity" — but Cloudflare *keeps* typing
in **every** one of its paths. The implementation now restores typed discovery
through `search` (`signature`/`dts`) while keeping the execute sandbox runtime-only.
The older `execute_sandboxed` comment said:

> `// Cloudflare-parity: no typed TypeScript preamble is injected.`

That comment is now **factually behind Cloudflare's own code.** Git history:
`82832562` added `{{types}}` injection → `780c67d3` "restore Cloudflare-parity
search+execute; remove code tool" deleted it → `da593edc` removed the
`code_mode.enabled` flag. We modeled ourselves on CF's *OpenAPI* server shape
(search+execute) while dropping the typing that CF's OpenAPI **and** MCP servers
both retain.

Cloudflare ships three entry points, all typed:

| CF entry point | File | Shape | How types reach the model |
|---|---|---|---|
| `createCodeTool` (AI SDK) | `tool.ts` | single `code` tool | `generateTypes` (Zod/AI-SDK) → `{{types}}` in description |
| `codeMcpServer` (wraps an MCP server) | `mcp.ts:135` | single `code` tool | `generateTypesFromJsonSchema` → `{{types}}` in description |
| `openApiMcpServer` | `mcp.ts:408` | `search` + `execute` | `OpenApiSpec` TS interface + `codemode.spec()` returns the spec |

Lab's current `search` returns raw JSON schemas plus TypeScript signatures and
focused DTS for each tool. `execute` still injects only the runtime JS proxy
(`generate_js_proxy`) and expects callers to discover types with `search` first.
That keeps progressive disclosure while avoiding a giant typed preamble.

**Current `main` is internally honest about this** — the `CODE_EXECUTE_DESCRIPTION`
constant tells the agent to discover tool ids and TypeScript signatures with `search`
first, and states that the runtime helpers match the signatures returned by
`search.dts`. The execute sandbox still has no TypeScript typechecker or schema
introspection API; typing is a discovery-time contract for agents.

(Note: an *older deployed* build's execute description claimed tools were
"pre-declared as a typed TypeScript helper — read the types" before that was true.
The current wording now matches the implemented search-delivered type surface.)

**Why our base is still the right one:** CF's MCP path loads the *entire* tool API
into one tool description. At our scale (many upstreams, hundreds of tools, a 256KB
catalog soft-cap that already truncates — code_mode.rs:538) that doesn't fit. Our
`search` is the progressive-disclosure mechanism CF's blog calls future work. So the
fix was **not** "go back to a typed monolith" — it was **make `search` emit
types**, keeping progressive disclosure. That puts us ahead of CF's described
state instead of behind it.

---

## Ranked Enhancements And Status

| # | Enhancement | Status | Notes |
|---|---|---|---|
| 1 | Emit TypeScript signatures + JSDoc from `search` | Present | `signature` and `dts` are emitted per catalog entry |
| 2 | Unwrap the MCP result envelope | Present | Sandbox receives payload values, not raw successful `CallToolResult` envelopes |
| 3 | Thread `outputSchema` → typed returns | Present | `output_schema` feeds generated output types, falling back to `unknown` |
| 4 | AST-based code normalization | Present | Boa parsing handles exports, function declarations, and trailing expressions |
| 5 | Arg validation against inputSchema before dispatch | Present | Host-side validation returns `missing_param` / `invalid_param` before upstream dispatch |
| 6 | Binary-safe value codec across the sandbox boundary | Present | JavaScript `ArrayBuffer` and typed-array values use tagged base64 |
| 7 | Explicit, tested network/fs/require deny invariant | Pending | Still worth keeping as a verification target |
| 8 | Per-execution capability narrowing (`upstreams`/`tools` filter) | Present | Filters narrow proxy generation and direct `callTool` resolution |
| 9 | Latency: cache/pool the wasmtime module + instances | Pending | Optimization, not a correctness blocker |
| 10 | Fix stale `docs/dev/CODE_MODE.md` | Present | Root Code Mode docs now describe search+execute and search-delivered typing |

---

### 1. Emit TypeScript signatures + JSDoc from `search` — present

**CF reference (ready to port):** `json-schema-types.ts` is a complete, dependency-free
JSON-Schema→TypeScript emitter. `generateTypesFromJsonSchema(tools)` (line 334) produces:

```ts
type MovieSearchInput = {
  /** Search query */
  query: string;
  /** Release year filter */
  year?: number;
};
type MovieSearchOutput = unknown;
declare const codemode: {
  /**
   * Search the catalog for movies.
   * @param input.query - Search query
   */
  movie_search: (input: MovieSearchInput) => Promise<MovieSearchOutput>;
};
```

`jsonSchemaToTypeString` (line 66) already handles everything messy MCP schemas throw
at it: `$ref` (internal JSON-pointer resolve), `anyOf`/`oneOf` (unions),
`allOf` (intersection), `enum`/`const`, tuples (`prefixItems` + draft-07 array
`items`), `additionalProperties` (index signatures), OpenAPI `nullable`, type arrays
(`["string","null"]`), a **maxDepth=20 guard** and a **circular-ref guard** (line 80).
`utils.ts` supplies `sanitizeToolName`, `toPascalCase`, `quoteProp`, `escapeJsDoc`,
`escapeStringLiteral`.

**Why it's low-risk for us:** the data is already there. `sanitize_schema`
(projection.rs:117) is **permissive** — it does NOT drop schema fields. It recurses the
JSON value, redacting secret-looking strings and truncating string values to 2048 chars
(projection.rs:120-122), and drops a whole schema only if it serializes over
`MAX_SCHEMA_BYTES = 16_384` (line 139). Every codegen-relevant field —
`type, properties, required, items, enum, const, description, format, default,
additionalProperties, anyOf, oneOf, allOf` — survives untouched. So each
`CodeModeCatalogEntry.schema` already carries enough to type from; we just never run an
emit step. Two caveats to handle in the emitter: (a) a schema dropped for exceeding 16KB
arrives as `None` → emit `unknown` (CF's exact fallback); (b) long `description`s are
truncated to 2048 chars before they reach JSDoc — acceptable, but note it.

**Status:** implemented through Rust-side schema-to-TypeScript generation.
`CodeModeCatalogEntry` carries `signature` and `dts` alongside `schema` and
`output_schema`. The execute proxy remains runtime JavaScript; callers should use
`search` first to retrieve the focused declaration block they need.

**Constraint-aware (heterogeneous MCP vs CF's owned tools):** CF's `try/catch`
fallback (line 380) degrades a bad schema to `type X = unknown` — copy that exactly.
Our `tool_name_to_snake` already matches CF's `sanitizeToolName` semantics (verified:
hyphen/dot/slash/colon→`_`, leading-digit prefix, reserved-word suffix), so generated
method names will line up with the runtime proxy.

### 2. Unwrap the MCP result envelope — present

**Lab today:** `call_upstream_tool` unwraps successful `CallToolResult` values before
returning to sandbox JavaScript. `structuredContent` wins; all-text content is joined
and parsed as JSON when possible; empty content returns `null`; mixed/non-text content
keeps its JSON MCP representation.

**CF:** `unwrapMcpResult` (mcp.ts:70) gives the sandbox a **plain value**, in priority
order: compat `toolResult` → **throw** on `isError` → `structuredContent` (authoritative
typed value) → all-text content `JSON.parse`'d (or raw) → mixed/binary returned as-is.

This now matches the shipped tool description that promises successful returns are
payloads, not raw MCP envelopes. Upstream `isError` results still become structured
ToolErrors before reaching sandbox code.

### 3. Thread `outputSchema` → typed returns — present

`CodeModeCatalogEntry` carries `output_schema`, and the TypeScript emitter generates
`${Type}Output` from it when available. Missing output schemas fall back to `unknown`.

### 4. AST-based code normalization — present

**Lab:** `normalize_user_code` strips markdown fences, parses module/script forms
with Boa, unwraps default exports, wraps named function declarations, and returns a
trailing expression from loose multi-statement snippets.

**CF:** `normalizeCode` (normalize.ts) parses with **acorn** and is far more forgiving:
arrow passthrough; `export default` expression/anonymous-fn/anonymous-class unwrap;
named function declaration → wrap + call; **trailing expression statement → splice in
`return`**; any other multi-statement body → wrap in `async () => { ... }`; empty →
`async () => {}`; parse failure → wrap-and-hope.

The practical gap called out in the original review is closed: an LLM emitting
`const x = await codemode.foo(); x.items` is normalized into an async function that
returns the trailing expression.

### 5. Input schema arg validation before dispatch — present

CF validates tool args against the input schema before invoking
(`tool.ts:extractFns` → `asSchema(...).validate`, throwing on failure). Lab now
validates host-side against the preserved input schema before upstream dispatch,
surfacing precise `missing_param` or `invalid_param` errors to the agent's
`try/catch` instead of a generic upstream rejection.

### 6. Binary-safe value codec — present

CF base64-wraps `Uint8Array`/`ArrayBuffer`/views across the sandbox boundary with a
tagged codec (`executor.ts` `BINARY_TAG`/`SANDBOX_CODEC`). Lab now uses a tagged
base64 codec for JavaScript `ArrayBuffer` and typed-array values crossing the runner
boundary. MCP binary content blocks still follow their JSON MCP representation unless
an upstream exposes bytes through structured content that the codec can carry.

### 7. Explicit, tested network/fs/require deny invariant

`docs/dev/CODE_MODE.md` claims the Javy runner has "no Node, Deno, Bun, fetch, or
require globals," and we `env_clear()` + run in a temp dir (code_mode.rs:627). CF makes
isolation a **runtime-enforced, documented guarantee** (`globalOutbound: null` →
`fetch`/`connect` throw; executor.ts:233). Convert our incidental absence into a tested
invariant: assert in the runner test suite that `fetch`, `connect`, `XMLHttpRequest`,
`require`, dynamic `import`, and fs globals are `undefined` in both the Boa and Javy
paths. Cheap, and it turns a claim into a guarantee.

### 8. Per-execution capability narrowing — present

CF's capability set per run = the providers/bindings handed to that execution
(executor.ts `ResolvedProvider[]`, namespaced). Lab now accepts optional `execute`
params `upstreams: [...]` and `tools: [...]` that narrow both injected proxy generation
and direct `callTool` resolution for that run. Valid filters are JSON arrays of strings;
non-array values reject as `invalid_param`.

### 9. Latency — cache/pool the wasmtime path

CF: fresh V8 isolate per call, ms startup, no pooling (cheap on workerd). Lab spawns a
**subprocess** per `execute` (`labby internal code-mode-runner`, code_mode.rs:625) —
stronger isolation, slower start. The wasmtime engine skeleton already exists
(`wasm_runner`, code_mode.rs:1202, fuel + epoch interruption). A compiled-module cache
+ instance pool on the wasm path would cut per-call latency while keeping per-call state
isolation. Optimization, not correctness — do last.

### 10. Fix stale `docs/dev/CODE_MODE.md` — present

The root Code Mode doc now describes the current search+execute surface, focused
TypeScript delivered through `search`, capability filters, result unwrapping, binary
codec behavior, and destructive upstream confirmation semantics.

---

## Parity already achieved — do not re-fix

- **snake_case tool naming** — `tool_name_to_snake` (code_mode_preamble.rs:79) matches
  CF's `sanitizeToolName` (separators→`_`, leading-digit prefix, reserved-word suffix).
- **Sandbox reaches MCP only via the host bridge; secrets never enter the sandbox** —
  `callTool`/proxy is the only channel, same posture as CF bindings, different mechanism.
- **Intermediate results stay local; only the return value comes back** — inherent to
  running one arrow function per sandbox pass; matches CF's core efficiency claim.
- **Catchable structured errors** — `settle_code_mode_tool_promise` (code_mode.rs:1994)
  rejects with a JSON-encoded `{kind,message}` so `JSON.parse(String(e.message))` works;
  fixed in both Boa and Javy paths (commit `8f4032bb`). This is parity-plus.
- **Computed-only runs allowed** — no minimum `callTool` requirement (code_mode.rs:786).
- **Markdown-fence / `export default` / `function main` normalization** exists (the
  *coverage* is narrower than CF's — see #4 — but the intent is matched).

## Where Lab is already ahead of Cloudflare

- **`search` = progressive type disclosure.** Lab exposes typed tools on-demand at
  a scale CF's "whole-API-in-one-description" can't reach.
- **Per-`sub` OAuth attribution + explicit destructive confirmation**
  (`CodeModeCaller`, `oauth_subject`, `destructive_permitted`) — finer-grained
  authz than CF's `filterTools`/`needsApproval` static drop.
- **Machine-actionable envelope** — `calls[]` metadata + token-budget truncation
  (`truncate_execution_response`) + canonical error taxonomy
  (`code_mode_canonical_error_kind`) beats CF's `console.log` capture.
- **Multi-engine portability** (Boa in-process / Javy subprocess / wasmtime) + a wasm
  build, vs CF's workerd-only substrate.
- **Process-group kill + stderr-drain anti-deadlock** (code_mode.rs:632, 660) —
  production hardening CF gets "for free" from the platform and we implement explicitly.

## Cloudflare ideas that do NOT port (and why)

- **Worker Loader API / V8-isolate-per-call** — different substrate; our subprocess +
  wasmtime already give per-call isolation. Don't chase it.
- **"Load the entire API into one tool description"** — anti-pattern at our scale;
  adopt the *typing* (#1), not the delivery.
- **`iframe-executor.ts` browser sandbox** — CF added a browser execution path; n/a to a
  Rust gateway.
- **Durable-Object / `ctx.exports` RPC plumbing** — platform-specific; our gateway
  manager + OAuth-subject attribution is the equivalent.

## Remaining sequencing

Most original correctness/truth items are implemented. The remaining follow-ups are:

1. **#7** — add explicit runner tests for denied network/fs/module globals.
2. **#9** — tune wasmtime compile/module reuse if execution latency becomes material.
