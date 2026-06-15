---
name: creating-snippets
description: Use when creating, editing, validating, testing, running, explaining, or removing Labby Code Mode snippets; when a user wants a reusable workflow made from gateway MCP tools; or when building schema-backed snippets from upstream tool ids, JSON schemas, params, inputs, defaults, artifacts, and MCP/CLI snippet actions.
---

# Creating Snippets

## Overview

Labby snippets are saved Code Mode workflows: pick gateway MCP tools, fill their schema-typed params, call them from one async JavaScript arrow function, and return structured JSON. Keep snippet business logic in the snippet body; use Labby's snippets dispatch/CLI/MCP actions to store, validate, test, and execute it.

## First Checks

Use `vibin:mcp-gateway-tools` before authoring any snippet that calls upstream tools. Search the live catalog and copy the returned `id`, `schema`, `output_schema`, `signature`, and `dts`; never guess tool ids or params.

Useful local references:

- `/home/jmagar/workspace/lab/docs/snippets/README.md`
- `/home/jmagar/workspace/lab/docs/snippets/*.md`
- `/home/jmagar/workspace/lab/crates/lab/src/dispatch/snippets/`

## Snippet Shape

Use Markdown for reusable snippets. The filename stem is the id, and frontmatter `name` must match it.

````markdown
---
name: my-workflow
description: Brief human-readable purpose
tags: [research, readonly]
inputs:
  topic:
    type: string
    required: true
    description: Topic to search
  limit:
    type: integer
    default: 5
    required: false
tools:
  - axon::axon
  - github::search_issues
---

## Tutorial: How This Snippet Is Built

Explain the selected tools, params, validation, execution order, and output.

```js
async (input) => {
  const topic = input.topic;
  const limit = input.limit ?? 5;
  const axon = (params) => callTool("axon::axon", params);

  const timed = async (label, fn) => {
    const started = Date.now();
    try {
      return { label, ok: true, ms: Date.now() - started, result: await fn() };
    } catch (error) {
      return { label, ok: false, ms: Date.now() - started, error: String(error) };
    }
  };

  const results = await Promise.all([
    timed("web-search", () => axon({ action: "search", query: topic, limit })),
    timed("rag-query", () => axon({ action: "query", query: topic, limit }))
  ]);

  return { snippet: "my-workflow", input: { topic, limit }, results };
}
```
````

Raw JavaScript is allowed, but Markdown with frontmatter and a tutorial is preferred.

## Inputs And Defaults

Use frontmatter `inputs` for user-configurable values. Supported types are `string`, `integer`, `number`, `boolean`, `object`, `array`, and `json`.

Rules:

- Give optional inputs defaults when possible so snippets still run with sparse params.
- Mark genuinely required values with `required: true`.
- Keep unknown input rejection useful: declared inputs cause `snippets.exec` to reject unexpected caller params.
- Mirror upstream schemas in the generated call params. Snippet inputs describe user-facing knobs; upstream schemas validate each MCP tool call.

## Authoring Workflow

1. List existing snippets: `labby snippets list --json`.
2. Search gateway tools with `search` and inspect schemas/signatures.
3. Pick tools and decide parallel vs chained execution.
4. Draft Markdown with frontmatter, tutorial text, declared inputs, and one `js`/`javascript` fenced block.
5. Validate without saving: `labby snippets validate my-workflow --file draft.md`.
6. Save as a user snippet: `labby snippets create my-workflow --file draft.md --description "..."`.
7. Smoke-test execution: `labby snippets test my-workflow --param topic="mcp-ui rust"`.
8. Run normally: `labby snippets exec my-workflow --param topic="mcp-ui rust" --max-tool-calls 10`.

Use `--force` only when intentionally replacing a user snippet.

## MCP And Dispatch Actions

Snippets are also available through the shared dispatch layer and MCP/API service:

```json
{ "action": "snippets.list", "params": {} }
{ "action": "snippets.get", "params": { "name": "my-workflow" } }
{ "action": "snippets.validate", "params": { "name": "my-workflow", "body": "..." } }
{ "action": "snippets.create", "params": { "name": "my-workflow", "body": "...", "description": "...", "force": false } }
{ "action": "snippets.exec", "params": { "name": "my-workflow", "params": { "topic": "mcp-ui rust" }, "max_tool_calls": 10 } }
{ "action": "snippets.test", "params": { "name": "my-workflow", "params": { "topic": "mcp-ui rust" } } }
{ "action": "snippets.test", "params": { "all": true } }
{ "action": "snippets.remove", "params": { "name": "my-workflow" } }
```

`remove` is destructive and only removes user snippets. Built-ins are read-only.

## Execution Patterns

- Use `Promise.all` only for independent calls.
- Chain calls when later params depend on earlier results.
- Wrap each call with timing and error capture.
- Return stable JSON fields: `snippet`, `input`, `summary`, `results`, `evidence`, `gaps`, `followup_calls`, `timings`.
- Keep responses compact; large Markdown, tables, screenshots, or manifests should be written with `writeArtifact("relative/path.md", content, { contentType })`.
- Include enough ids, URLs, labels, and raw evidence handles for follow-up verification.

## Validation Checklist

Before calling the work done:

- `name` is slug-like and matches filename/frontmatter.
- Description is non-empty.
- Body contains exactly the intended async arrow function.
- All upstream tool ids came from live gateway `search`.
- Tool params match upstream schemas.
- Optional inputs have defaults or code fallbacks.
- Required inputs fail fast with clear validation.
- Fan-out is bounded by limits and `max_tool_calls`.
- `labby snippets validate` passes.
- `labby snippets test` passes for one snippet, or `labby snippets test --all` passes when changing shared built-ins.
