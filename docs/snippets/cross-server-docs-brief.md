---
name: cross-server-docs-brief
description: Build a compact docs brief from Context7, web search, GitHub, Axon, and time
tags: [docs, research, cross-server]
inputs:
  topic:
    type: string
    default: Model Context Protocol Rust SDK
    required: false
    description: Main research topic
  library_name:
    type: string
    default: tokio
    required: false
    description: Context7 library search name
  library_id:
    type: string
    default: /websites/rs_tokio
    required: false
    description: Concrete Context7 library id
  max_results:
    type: integer
    default: 3
    required: false
    description: Per-source result limit
---

# Cross-Server Docs Brief

Use this snippet when you want a quick documentation brief from several independent sources. It combines Context7 library docs, SearXNG web search, Cloudflare docs, GitHub repository search, Axon search, and the time server.

## Tutorial: How This Snippet Is Built

This snippet is a small parallel research checklist. Each selected tool answers a different evidence question:

| Step | Tool | Why it is included | Parameters the user fills |
|---|---|---|---|
| Timestamp | `time::get_current_time` | Marks when the brief was generated | `timezone` |
| Library discovery | `context7::resolve-library-id` | Finds matching Context7 library ids | `libraryName`, `query` |
| Library docs | `context7::query-docs` | Pulls focused docs from a known library id | `libraryId`, `query`, `tokens` |
| Web search | `searxng::searxng_web_search` | Finds fresh public pages | `query`, `count` |
| Cloudflare docs | `docs-mcp-cloudflare-com::search_cloudflare_documentation` | Adds a concrete vendor-doc example | `query`, `limit` |
| GitHub repos | `github::search_repositories` | Finds related code and libraries | `query`, `perPage` |
| Axon search | `axon::axon` | Searches and indexes through the local RAG stack | `action`, `query`, `limit` |

In the builder, a user should not write these parameter objects manually. They should search for each tool, select it, and get a form generated from that tool's schema. For example, selecting `github::search_repositories` should show `query` and `perPage`; selecting `context7::query-docs` should show `libraryId`, `query`, and `tokens`.

The calls are independent, so the snippet runs them with `Promise.all`. Nothing in the GitHub query depends on the Context7 result, and nothing in the time call depends on Axon. That is the main authoring decision.

## Why The Inputs Exist

- `topic` becomes the generic docs/web/Axon search query.
- `library_name` is used only for Context7 library discovery.
- `library_id` is the exact Context7 id used for the docs query. The default is concrete because `query-docs` needs an id, not just a search phrase.
- `max_results` bounds the web, Cloudflare, GitHub, and Axon result volume.

If a user omits every input, the snippet still runs with defaults. If they only change `topic`, most calls follow that new topic while Context7 still uses the default library until `library_name` / `library_id` are changed.

## What Validation Should Catch

The builder should validate every selected call against its schema before saving:

- `context7::query-docs.libraryId` must be a string.
- `context7::query-docs.tokens` must be numeric when provided.
- `searxng::searxng_web_search.count` and `github::search_repositories.perPage` must be integers.
- `axon::axon.action` must be present because Axon is an action-dispatched tool.

That validation is what makes the workflow approachable: the user picks fields from forms instead of remembering tool-specific argument names.

Live smoke-tested tools before authoring:

- `time::get_current_time`
- `context7::resolve-library-id`
- `context7::query-docs`
- `searxng::searxng_web_search`
- `docs-mcp-cloudflare-com::search_cloudflare_documentation`
- `github::search_repositories`
- `axon::axon` with `action: "search"`

Run with:

```bash
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/cross-server-docs-brief.md)"
```

```js
async (overrides = {}) => {
  const input = {
    topic: overrides.topic ?? "Model Context Protocol Rust SDK",
    libraryName: overrides.library_name ?? "tokio",
    libraryId: overrides.library_id ?? "/websites/rs_tokio",
    libraryQuestion: "spawn blocking task",
    cloudflareQuery: "workers durable objects",
    githubRepoQuery: "modelcontextprotocol rust sdk",
    maxResults: overrides.max_results ?? 3,
    ...overrides
  };

  const preview = (value, limit = 1200) => {
    const text = typeof value === "string" ? value : JSON.stringify(value);
    return text.length > limit ? `${text.slice(0, limit)}...` : text;
  };

  const timed = async (label, id, params, transform = (x) => x) => {
    const started = Date.now();
    try {
      const result = await callTool(id, params);
      return {
        label,
        id,
        ok: true,
        ms: Date.now() - started,
        result: transform(result)
      };
    } catch (error) {
      return {
        label,
        id,
        ok: false,
        ms: Date.now() - started,
        error: String(error)
      };
    }
  };

  const calls = await Promise.all([
    timed("timestamp", "time::get_current_time", { timezone: "America/New_York" }),
    timed(
      "context7_library_candidates",
      "context7::resolve-library-id",
      { libraryName: input.libraryName, query: input.libraryName },
      (result) => preview(result)
    ),
    timed(
      "context7_docs",
      "context7::query-docs",
      { libraryId: input.libraryId, query: input.libraryQuestion, tokens: 900 },
      (result) => preview(result)
    ),
    timed(
      "searxng_web",
      "searxng::searxng_web_search",
      { query: input.topic, count: input.maxResults },
      (result) => preview(result)
    ),
    timed(
      "cloudflare_docs",
      "docs-mcp-cloudflare-com::search_cloudflare_documentation",
      { query: input.cloudflareQuery, limit: input.maxResults },
      (result) => preview(result)
    ),
    timed(
      "github_repositories",
      "github::search_repositories",
      { query: input.githubRepoQuery, perPage: input.maxResults },
      (result) => ({
        total_count: result.total_count,
        repositories: (result.items || []).slice(0, input.maxResults).map((repo) => ({
          full_name: repo.full_name,
          description: repo.description,
          language: repo.language,
          stars: repo.stargazers_count,
          url: repo.html_url
        }))
      })
    ),
    timed(
      "axon_search",
      "axon::axon",
      { action: "search", query: input.topic, limit: input.maxResults },
      (result) => preview(result)
    )
  ]);

  return {
    snippet: "cross_server_docs_brief",
    input,
    ok: calls.every((call) => call.ok),
    calls,
    notes: [
      "Context7 query_docs needs a concrete libraryId; update input.libraryId when changing libraryName.",
      "Axon search may enqueue crawls and can return warnings if the running Axon binary is stale."
    ]
  };
}
```
