# Code Mode Snippets

Code Mode snippets are reusable JavaScript workflows for Labby's `search` and `execute` tools. They let an agent run many upstream MCP calls from one controlled async function, combine the results, and return a structured answer that is easier to reuse than a one-off chat transcript.

This document is only about snippets that run inside Code Mode.

## What A Snippet Is

A snippet is an `async () => { ... }` JavaScript function passed to Code Mode `execute`.

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

Code Mode `execute` expects the submitted code to evaluate to a function:

```js
async () => {
  return { ok: true };
}
```

The returned value must be JSON-serializable. The sandbox has `callTool` and, when proxy generation succeeds, `codemode`. The host validates `callTool` params against the upstream tool input schema before dispatching the call.

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
- [`homelab-readonly-pulse.md`](./homelab-readonly-pulse.md) combines Dozzle, Cortex, Unraid, Gotify, and time for a read-only homelab status pulse.
- [`cross-server-smoke-tests.md`](./cross-server-smoke-tests.md) records the live catalog count and the tool/action smoke-test results used to choose the snippets.
