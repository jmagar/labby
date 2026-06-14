# Code Mode Snippets

Code Mode snippets are reusable JavaScript workflows for Labby's `search` and `execute` tools. They let an agent run many upstream MCP calls from one controlled async function, combine the results, and return a structured answer that is easier to reuse than a one-off chat transcript.

This document is only about snippets that run inside Code Mode.

## What A Snippet Is

A snippet is a Markdown or JavaScript file that contains an async arrow function passed to Code Mode `execute`.

The simple version:

1. Pick one or more gateway tools.
2. Fill in the parameters those tools already declare in their typed schemas.
3. Decide whether the calls run in parallel or one after another.
4. Return the useful parts of each tool result as JSON.

That is it. A snippet is not a new plugin system, a new MCP server, or a special agent language. It is a saved recipe for calling existing MCP tools with known arguments.

Executable Markdown snippets should begin with frontmatter:

```yaml
---
name: homelab-readonly-pulse
description: Read-only homelab pulse across core services
tags: [homelab, readonly, ops]
---
```

The filename stem is the stable id. If frontmatter is present, `name` must match that id and `description` must be non-empty.

Inside that function, the sandbox can call upstream MCP tools through:

```js
await callTool("<upstream>::<tool>", params)
```

or through the generated helper object:

```js
await codemode.<upstream>.<tool>(params)
```

Tool ids use the live Code Mode catalog shape: `<upstream>::<tool>`. For example, Axon's single MCP tool is:

```js
await callTool("axon::axon", { action: "search", query: "mcp-ui rust" })
```

Before writing or running a snippet, use Code Mode `search` to inspect the live catalog. Search returns tool ids, descriptions, input schemas, output schemas, TypeScript signatures, and focused DTS blocks. A snippet should be written against those returned ids and schemas, not against guessed tool names.

## How Users Should Build Snippets

The snippet builder should make authoring feel like assembling a small checklist, not writing JavaScript.

### 1. Search the live gateway tools

The gateway already knows every connected upstream tool. Each catalog entry includes:

- `id`, such as `time::get_current_time` or `axon::axon`
- `upstream`, such as `time` or `axon`
- `name`, such as `get_current_time` or `axon`
- `description`
- input `schema`
- output schema when the upstream provides one
- generated TypeScript signature and DTS help text

The user should search this live catalog by service, tool name, or description, then select the tools they want the snippet to call.

### 2. Render a form from each selected tool schema

For each selected tool, the builder should read the upstream JSON schema and render the available fields:

- strings become text inputs
- booleans become toggles
- numbers and integers become numeric inputs
- enums become selects
- arrays and objects become structured editors
- required fields are clearly marked
- descriptions and defaults are shown inline

The user should not need to know the parameter names ahead of time. The schema tells Labby what fields exist and what values are valid.

### 3. Validate before saving

Every call step should be validated against the same upstream schema that Code Mode uses at runtime. Invalid params should fail before a snippet is saved or executed.

Example validation failures should be concrete:

```text
axon::axon.params.action is required
time::get_current_time.params.timezone must be a string
github::search_repositories.params.perPage must be an integer
```

### 4. Choose parallel or chained execution

Most snippets are one of two shapes:

- **Parallel:** call independent tools at the same time, then combine results.
- **Chained:** call one tool, use its result to fill parameters for the next tool.

The first builder pass can make parallel snippets easy and reserve advanced chaining for a later step. Even a parallel-only builder covers many useful workflows: health pulses, docs briefs, repo triage, and multi-source search.

### 5. Save a readable Markdown snippet

The saved file should still be plain Markdown. A user should be able to open it, understand the selected tools and parameters, and edit it by hand if they want.

The builder can generate the JavaScript body, but the source of truth stays simple:

```yaml
---
name: my-docs-brief
description: Search docs and GitHub for one topic
tags: [docs, research]
inputs:
  topic:
    type: string
    default: Model Context Protocol Rust SDK
    required: false
tools:
  - time::get_current_time
  - context7::query-docs
  - github::search_repositories
---
```

Then the code is just the generated call plan: call each selected tool with validated params and return the results.

## Why Snippets Matter

Snippets turn repeated agent behavior into a durable workflow. Instead of asking the model to remember a multi-step process every time, the process lives in a small program with explicit inputs, bounded fan-out, timing, error capture, source selection, and output shape.

They are powerful because they can:

- Fan out independent calls in parallel with `Promise.all`.
- Chain results from one tool into follow-up calls.
- Query several upstream MCP servers in one execution.
- Normalize messy tool outputs into a consistent result object.
- Record timing and failure data for each call.
- Keep discovery, ranking, synthesis inputs, and follow-up recommendations in one reusable place.
- Be exposed as MCP prompts later without changing the workflow itself.

The prompt can stay simple: name the snippet, collect arguments, and tell the model what output to expect. The snippet carries the operational logic.

## Execution Contract

`labby snippets exec` and the MCP/API `snippets.exec` action expect snippet code to evaluate to an async arrow function:

```js
async (input) => {
  return { ok: true };
}
```

The returned value must be JSON-serializable. The sandbox has `callTool` and, when proxy generation succeeds, `codemode`. The host validates `callTool` params against the upstream tool input schema before dispatching the call.

CLI execution passes repeated `--param key=value` flags as the `input` object:

```bash
labby snippets exec homelab-readonly-pulse --param host=dookie
```

MCP and API callers pass the same shape through `params`:

```json
{
  "action": "snippets.exec",
  "params": {
    "name": "homelab-readonly-pulse",
    "params": { "host": "dookie" }
  }
}
```

Snippet code should provide defaults for omitted optional fields:

```js
async (input) => {
  const host = input.host || "dookie";
  return { ok: true, host };
}
```

Snippets can also declare typed inputs in frontmatter. When inputs are declared,
`snippets.exec` merges caller params with defaults, rejects unknown params,
validates types, and fails fast when a required param is missing.

```yaml
inputs:
  host:
    type: string
    default: dookie
    required: false
    description: Host alias to inspect
  limit:
    type: integer
    default: 5
    required: false
```

`labby snippets create` validates the body before saving. User-created Markdown gets frontmatter automatically when the input body does not already include it.

Use `labby snippets validate <name>` to validate an existing snippet without
executing it, or pass `--file` / `--code` to validate an unsaved body:

```bash
labby snippets validate draft --file draft-snippet.md
```

Use `labby snippets test <name>` to execute one snippet as a smoke test, or
`labby snippets test --all` to run every listed snippet with its declared
defaults. MCP/API callers use `snippets.test` with `{ "all": true }` for the
same all-snippet check.

`snippets.list`, `help`, and `schema` are read-only discovery actions. Actions
that expose snippet bodies or execute/manage snippets require `lab:admin`.

Successful upstream MCP results are unwrapped before reaching snippet code when possible. Structured content is returned as the value; all-text content is parsed as JSON when possible; mixed content keeps its MCP content shape.

## Artifact-First Output

Code Mode snippets should return compact execution receipts and write large composed outputs as artifacts.

Use this pattern whenever a snippet creates markdown, source tables, screenshots, crawl manifests, or follow-up snippets:

```js
async () => {
  const markdown = renderMarkdownReport(data);
  const artifact = await writeArtifact("reports/example.md", markdown, {
    contentType: "text/markdown"
  });

  return {
    summary: "Generated report",
    artifact,
    timings
  };
}
```

The final return value is still subject to `[code_mode].max_response_bytes` and `[code_mode].max_response_tokens`. Each `writeArtifact` path is validated (non-empty, relative, no `..`, no symlinked-ancestor escape) and the content — capped at 1 MiB — is written into a per-run directory under `$LAB_HOME/code-mode-artifacts/<run_id>/`. The receipt includes the path, byte count, content type, and SHA-256 digest. Artifact writes count against `max_tool_calls`, and `contentType` defaults to `text/plain` when omitted.

## Basic Pattern

Start every reusable snippet with an explicit input block, small helpers, and a bounded call plan.

```js
async () => {
  const input = {
    topic: "implementing mcp-ui in rust",
    maxEvidenceUrls: 4
  };

  const axon = (args) => callTool("axon::axon", args);

  const timed = async (label, fn) => {
    const started = Date.now();
    try {
      return {
        label,
        ok: true,
        ms: Date.now() - started,
        result: await fn()
      };
    } catch (error) {
      return {
        label,
        ok: false,
        ms: Date.now() - started,
        error: String(error)
      };
    }
  };

  const firstPass = await Promise.all([
    timed("search", () => axon({ action: "search", query: input.topic })),
    timed("research", () => axon({ action: "research", query: input.topic })),
    timed("query", () => axon({ action: "query", query: input.topic }))
  ]);

  return {
    input,
    first_pass: firstPass,
    next_action: "Select evidence URLs from first_pass and run targeted follow-up calls."
  };
}
```

## Authoring Rules

Use these rules when creating snippets:

- Discover first with Code Mode `search`; use returned ids such as `axon::axon`.
- Keep the top-level snippet as an async arrow function.
- Put all user-editable parameters in a single `input` object near the top.
- Bound fan-out with limits like `maxEvidenceUrls`, `maxResults`, or `maxToolCalls`.
- Use `Promise.all` only for independent calls.
- Chain calls when later params depend on earlier results.
- Wrap each tool call with timing and error capture.
- Return structured JSON, not prose-only text.
- Include enough raw evidence handles, URLs, ids, or labels for follow-up.
- Prefer compact summaries of large results; Code Mode responses are still budgeted.
- Generate a follow-up snippet when the current result exposes gaps that need another targeted pass.

## Result Shape

A good snippet result should be easy for an agent or prompt wrapper to consume:

```js
return {
  snippet: "axon_research_brief",
  input,
  answer,
  evidence,
  gaps,
  followup_calls,
  followup_snippet,
  timings
};
```

Use stable field names. Avoid hiding important data inside long prose. The agent can always turn structured output into prose after the snippet finishes.

## Fan-Out And Chaining

Fan-out is for independent calls:

```js
const results = await Promise.all([
  callTool("axon::axon", { action: "search", query }),
  callTool("axon::axon", { action: "query", query }),
  callTool("axon::axon", { action: "research", query })
]);
```

Chaining is for dependent calls:

```js
const search = await callTool("axon::axon", { action: "search", query });
const urls = selectUrls(search);
const pages = await Promise.all(
  urls.map((url) => callTool("axon::axon", { action: "scrape", url }))
);
```

Most useful snippets combine both: a broad first pass, a scoring or selection step, then targeted second-pass calls.

## Prompt Exposure

MCP prompts should expose snippets, not reimplement them.

A prompt wrapper should define:

- snippet name
- required arguments
- optional arguments
- expected output fields
- short guidance for when to use it

The prompt should not duplicate the internals. If the workflow changes, update the snippet and keep the prompt as a thin interface.

## Snippet Quality Checklist

A snippet is ready to reuse when:

- It uses real Code Mode ids from `search`.
- It runs as an `async () => { ... }` function.
- It has explicit inputs and bounded fan-out.
- It captures per-call timing and errors.
- It returns structured JSON.
- It preserves enough references for verification and follow-up.
- It avoids one-off assumptions that only apply to a single page, repo, or result.
- It can be wrapped by an MCP prompt without changing its internal workflow.

## Current Snippets

- [`axon-fanout.md`](./axon-fanout.md) defines reusable Axon research fan-out workflows for Code Mode.
- [`cross-server-docs-brief.md`](./cross-server-docs-brief.md) combines Context7, SearXNG, Cloudflare docs, GitHub, Axon, and time into a compact documentation brief.
- [`repo-context-triage.md`](./repo-context-triage.md) combines local file reads, Lumen semantic search, Octocode local search, and GitHub issue/file lookups for repo orientation.
- [`repo-status-gh-pulse.md`](./repo-status-gh-pulse.md) collects the GitHub PR/CI side of a repo-status evidence sweep and returns equivalent `gh` commands for shell parity.
- [`homelab-readonly-pulse.md`](./homelab-readonly-pulse.md) combines Dozzle, Cortex, Unraid, Gotify, and time for a read-only homelab status pulse.
- [`cross-server-smoke-tests.md`](./cross-server-smoke-tests.md) records the live catalog count and the tool/action smoke-test results used to choose the snippets.
