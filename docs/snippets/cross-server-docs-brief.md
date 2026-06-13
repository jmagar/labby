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
