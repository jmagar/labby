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

## The single most important finding

We **deliberately removed typed signatures** from Code Mode in the search+execute
pivot, citing "Cloudflare parity" — but Cloudflare *keeps* typing in **every** one
of its paths. Our `execute_sandboxed` (code_mode.rs:611) literally says:

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

Lab's `search` returns the raw JSON `schema` per tool but **no TypeScript**, and
`execute` injects only a runtime JS proxy (`generate_js_proxy`) with **no types and
no JSDoc**. We are the only one of the four designs that gives the model zero typed
surface.

**Current `main` is internally honest about this** — the `CODE_EXECUTE_DESCRIPTION`
constant (`crates/lab/src/mcp/server.rs:43`) tells the agent the truth: *"schemas are not
injected into this sandbox, so `search` is how you learn what arguments a tool takes,"* and
the `codemode.<upstream>.<tool>()` helpers are *"callable, but UNTYPED — there is no schema
in this sandbox to introspect"* (server.rs:64-66). So this is a capability gap, not a
broken contract: the design correctly works around the missing types by routing the agent
through `search` for raw schema. Enhancement #1 closes the gap — once `search` emits real
TypeScript, that "UNTYPED / run search first" caveat can be replaced with delivered types,
and the agent gets IDE-grade signatures instead of having to read raw JSON Schema.

(Note: an *older deployed* build's execute description claimed tools were "pre-declared as
a typed TypeScript helper — read the types" — which *was* untrue. Current `main` already
fixed the wording to match reality. #1 makes the stronger, typed version true.)

**Why our base is still the right one:** CF's MCP path loads the *entire* tool API
into one tool description. At our scale (many upstreams, hundreds of tools, a 256KB
catalog soft-cap that already truncates — code_mode.rs:538) that doesn't fit. Our
`search` is the progressive-disclosure mechanism CF's blog calls future work. So the
fix is **not** "go back to a typed monolith" — it's **make `search` emit types**,
keeping progressive disclosure. That puts us ahead of CF's described state instead of
behind it.

---

## Ranked enhancements

| # | Enhancement | Impact | Effort | Confidence |
|---|---|---|---|---|
| 1 | Emit TypeScript signatures + JSDoc from `search` (port `jsonSchemaToType`) | ★★★ | M | High — input data already preserved |
| 2 | Unwrap the MCP result envelope (parity with `unwrapMcpResult`) | ★★★ | S | High — confirmed contradicts our own contract |
| 3 | Thread `outputSchema` → typed returns | ★★ | M | High |
| 4 | AST-based code normalization (splice `return`, wrap loose statements) | ★★ | M | High |
| 5 | In-sandbox arg validation against inputSchema before dispatch | ★★ | M | Med |
| 6 | Binary-safe value codec across the sandbox boundary | ★ | M | Med |
| 7 | Explicit, tested network/fs/require deny invariant | ★★ | S | High |
| 8 | Per-execution capability narrowing (`upstreams`/`tools` filter) | ★★ | M | Med |
| 9 | Latency: cache/pool the wasmtime module + instances | ★ | M | Med |
| 10 | Fix stale `docs/dev/CODE_MODE.md` (documents removed design) | ★★ | S | High |

---

### 1. Emit TypeScript signatures + JSDoc from `search` — the headline

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

**Proposal:** port `jsonSchemaToType` to Rust (a focused ~250-line module). Add a
`signature: String` and/or `dts: String` field to `CodeModeCatalogEntry`
(code_mode.rs:198) so a `search` filter can return real signatures, not just raw
schema. Optionally also prepend the generated `declare const codemode` block into the
`execute` proxy preamble (`generate_js_proxy`) as comments, so an agent that goes
straight to `execute` still sees signatures.

**Constraint-aware (heterogeneous MCP vs CF's owned tools):** CF's `try/catch`
fallback (line 380) degrades a bad schema to `type X = unknown` — copy that exactly.
Our `tool_name_to_snake` already matches CF's `sanitizeToolName` semantics (verified:
hyphen/dot/slash/colon→`_`, leading-digit prefix, reserved-word suffix), so generated
method names will line up with the runtime proxy.

### 2. Unwrap the MCP result envelope — confirmed correctness gap

**Lab today:** `call_upstream_tool` (code_mode.rs:1017) returns the **entire**
`CallToolResult` via `serde_json::to_value(result)`. Sandbox code therefore receives
`{ content: [{type:"text", text:"..."}], structuredContent, isError }` and has to
dig the payload out itself.

**CF:** `unwrapMcpResult` (mcp.ts:70) gives the sandbox a **plain value**, in priority
order: compat `toolResult` → **throw** on `isError` → `structuredContent` (authoritative
typed value) → all-text content `JSON.parse`'d (or raw) → mixed/binary returned as-is.

**This contradicts our own shipped tool description, verbatim.**
`TOOL_EXECUTE_DESCRIPTION` (`crates/lab/src/mcp/server.rs:73-74`) tells every agent:
*"Successful return: the upstream tool's structuredContent if present, else the parsed
text of the first content[0] block. **Never the raw MCP envelope.**"* Our code returns
the raw envelope. There is even a test asserting that description string is present
(server.rs:3011) — so we test that we *promise* the behavior, but not that we *do* it.
CF's `unwrapMcpResult` (mcp.ts:70) is the exact target: implement the same unwrap in
`call_upstream_tool` before `serde_json::to_value`. (We already throw on `is_error` via
`code_mode_upstream_error_info` at line 998 — that half matches the contract; the
success-path unwrap is the missing half.) This is a clean correctness fix: make code
match the contract two other parts of the codebase already assert.

### 3. Thread `outputSchema` → typed returns

`CodeModeCatalogEntry` only carries the input `schema` (code_mode.rs:204) and
`code_search_catalog` only passes `tool.input_schema` (line 516). MCP now supports
`outputSchema`; CF's emitter already generates `${Type}Output` from it
(json-schema-types.ts:347) and falls back to `unknown`. Thread `output_schema` through
the catalog and into #1's emitter so agents can chain `r.map(x => x.field)` against a
real type instead of `unknown`. Compounds with #1 and #2 (a typed return is only
useful once the envelope is unwrapped).

### 4. AST-based code normalization

**Lab:** `normalize_user_code` (code_mode.rs:73) is string-prefix based: strips
markdown fences, parenthesizes `function main`/`export default function`, passes arrows
through. Anything else must already be a bare async-arrow expression or it fails the
`typeof __codeModeMain === 'function'` check (CODE_MODE_MAIN_SHAPE_ERROR).

**CF:** `normalizeCode` (normalize.ts) parses with **acorn** and is far more forgiving:
arrow passthrough; `export default` expression/anonymous-fn/anonymous-class unwrap;
named function declaration → wrap + call; **trailing expression statement → splice in
`return`**; any other multi-statement body → wrap in `async () => { ... }`; empty →
`async () => {}`; parse failure → wrap-and-hope.

The practical gap: an LLM emitting `const x = await codemode.foo(); x.items` (no
explicit `return`, multi-statement) **works in CF, fails for us.** That's a recurrent
real-world LLM output shape. Porting equivalent logic (a small JS-statement classifier;
we don't need full acorn — even a "if it doesn't start with `async (`/`(` , wrap it and
splice a return into the last statement" heuristic captures most cases) materially cuts
spurious `code_execute` contract errors.

### 5. In-sandbox arg validation before dispatch

CF validates tool args against the input schema before invoking
(`tool.ts:extractFns` → `asSchema(...).validate`, throwing on failure). Lab passes
`params` straight to the upstream (code_mode.rs:991-995). Validating against the
already-preserved input schema in `call_upstream_tool` would surface a precise
`invalid_param` to the agent's `try/catch` instead of a generic upstream rejection —
better fan-out ergonomics. Lower priority than #1–#4.

### 6. Binary-safe value codec

CF base64-wraps `Uint8Array`/`ArrayBuffer`/views across the sandbox boundary with a
tagged codec (`executor.ts` `BINARY_TAG`/`SANDBOX_CODEC`). Our boundary is
JSON-over-stdio, so non-UTF8/binary tool results round-trip only as far as JSON allows.
MCP already base64-encodes binary content blocks in-spec, so this is **lower impact for
us than for CF** — but worth a note for tools returning raw bytes in `structuredContent`.

### 7. Explicit, tested network/fs/require deny invariant

`docs/dev/CODE_MODE.md` claims the Javy runner has "no Node, Deno, Bun, fetch, or
require globals," and we `env_clear()` + run in a temp dir (code_mode.rs:627). CF makes
isolation a **runtime-enforced, documented guarantee** (`globalOutbound: null` →
`fetch`/`connect` throw; executor.ts:233). Convert our incidental absence into a tested
invariant: assert in the runner test suite that `fetch`, `connect`, `XMLHttpRequest`,
`require`, dynamic `import`, and fs globals are `undefined` in both the Boa and Javy
paths. Cheap, and it turns a claim into a guarantee.

### 8. Per-execution capability narrowing

CF's capability set per run = the providers/bindings handed to that execution
(executor.ts `ResolvedProvider[]`, namespaced). Lab's `build_code_mode_proxy`
(code_mode.rs:576) exposes the **entire** readable catalog every time. Add optional
`execute` params — `upstreams: [...]` / `tools: [...]` — that narrow the injected proxy
**and** `callTool` resolution for that run. Wins: least-privilege, clearer agent intent,
and a smaller injected type surface (directly shrinks #1's context cost). The capability
model to enforce it (`CodeModeCaller`, `destructive_permitted`) is already in place.

### 9. Latency — cache/pool the wasmtime path

CF: fresh V8 isolate per call, ms startup, no pooling (cheap on workerd). Lab spawns a
**subprocess** per `execute` (`labby internal code-mode-runner`, code_mode.rs:625) —
stronger isolation, slower start. The wasmtime engine skeleton already exists
(`wasm_runner`, code_mode.rs:1202, fuel + epoch interruption). A compiled-module cache
+ instance pool on the wasm path would cut per-call latency while keeping per-call state
isolation. Optimization, not correctness — do last.

### 10. Fix stale `docs/dev/CODE_MODE.md`

The doc describes the **removed** design: "exposes a **single** MCP tool — `code`",
"`[code_mode] enabled = true`", "mutually exclusive with Tool Search mode", and a typed
`codemode.*` "TypeScript preamble" that "declares `__catalog__` as `string | undefined`."
All of that was deleted (`780c67d3`, `da593edc`). Today the surface is search+execute
with a runtime-only proxy. Rewrite the doc to match — and once #1 lands, document the
restored (search-delivered) typing. Ironically the stale doc proves the typed-preamble
machinery already existed once; #1 is partly a *re-introduction* through the better
delivery channel.

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

- **`search` = progressive type disclosure.** Once #1 ships, we expose typed tools
  on-demand at a scale CF's "whole-API-in-one-description" can't reach.
- **Per-`sub` OAuth attribution + scope-gated destructive actions** (`CodeModeCaller`,
  `oauth_subject`, `destructive_permitted`) — finer-grained authz than CF's
  `filterTools`/`needsApproval` static drop.
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

## Suggested sequencing

1. **#10 + #2** — fix the stale doc and the envelope-unwrap bug (both are
   correctness/truth fixes, small, and #2 unblocks the value of typed returns).
2. **#1 + #3** — port the JSON-Schema→TS emitter, wire `dts`/`signature` into `search`,
   thread `outputSchema`. The headline; compounds with #2.
3. **#4 + #7** — robust normalization + the tested isolation invariant.
4. **#5 + #8** — arg validation and capability narrowing.
5. **#6 + #9** — binary codec and wasmtime pooling, last.
