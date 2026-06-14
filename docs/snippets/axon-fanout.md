---
name: axon-fanout
description: Axon fan-out research workflows for briefs and smoke tests
tags: [axon, research, docs]
inputs:
  topic:
    type: string
    default: implementing mcp-ui in rust
    required: false
    description: Research topic
  focus:
    type: string
    default: Rust rmcp server with MCP Apps UI resources, concrete APIs, metadata keys, MIME types, and implementation steps
    required: false
    description: Specific evidence focus
  seed_url:
    type: json
    required: false
    description: Optional seed URL to scrape and summarize
  max_evidence_urls:
    type: integer
    default: 4
    required: false
    description: Maximum evidence URLs to inspect
  include_ask:
    type: boolean
    default: false
    required: false
    description: Include an Axon ask synthesis call
---

# Axon Fan-Out Snippets

Reusable Axon workflow snippets. Treat these as the source of truth. MCP prompts should only expose a snippet name, arguments, and output expectations.

## Tutorial: How This Snippet Is Built

This snippet uses one upstream tool, `axon::axon`, several times with different `action` values. That is common for action-dispatched MCP servers: the selected tool is the same, but the parameters choose the operation.

| Step | Tool | Action | Why it is included | Parameters the user fills |
|---|---|---|---|---|
| Fresh search | `axon::axon` | `search` | Finds current web results and starts passive indexing | `query` |
| Research synthesis | `axon::axon` | `research` | Runs search, extraction, and an evidence summary | `query` |
| Vector search | `axon::axon` | `query` | Searches already indexed knowledge | `query` |
| Seed scrape | `axon::axon` | `scrape` | Reads one known URL when supplied | `url` |
| Seed summary | `axon::axon` | `summarize` | Summarizes one known URL when supplied | `url` |
| RAG answer | `axon::axon` | `ask` | Optional slower synthesis over indexed context | `query` |
| Evidence scrape | `axon::axon` | `scrape` | Reads selected high-score URLs | `url` |
| Evidence summary | `axon::axon` | `summarize` | Produces compact notes for selected URLs | `url` |

The snippet has two phases:

1. **Fan-out:** run independent first-pass Axon actions in parallel.
2. **Chaining:** score URLs from the first pass, then scrape/summarize the selected evidence URLs.

This is the most advanced built-in snippet because it shows both shapes. The builder can still make it understandable by presenting the first phase as selected tool calls and the second phase as "use URLs from step 1 as the `url` field for step 2."

## Why The Inputs Exist

- `topic` becomes the base query for `search`, `research`, and `query`.
- `focus` sharpens the optional `ask` prompt and the evidence scoring terms.
- `seed_url` adds known evidence to the first pass without requiring search to find it.
- `max_evidence_urls` bounds how many URLs are selected for the second pass.
- `include_ask` keeps the slower `ask` call opt-in.

Defaults are intentionally useful. A user can run the snippet unchanged for a smoke test, or change only `topic` and get a relevant research pass.

## What Validation Should Catch

Because every call uses `axon::axon`, validation has to look at the `action` field and the action-specific parameters:

- `action: "search"` requires a string `query`.
- `action: "research"` requires a string `query`.
- `action: "query"` requires a string `query`.
- `action: "scrape"` and `action: "summarize"` require a string `url`.
- `include_ask` must be boolean.
- `max_evidence_urls` must be an integer.

The builder should make action-dispatched tools feel like normal tools by showing the selected action first, then showing only the fields that action needs.

## `axon_research_brief`

Purpose: turn a topic into a useful engineering brief by combining fresh web discovery, Axon's indexed knowledge, and targeted page summaries.

Fast path:
- `search` for fresh web results.
- `research` for search + source synthesis.
- `query` for indexed semantic matches.
- optional seed `scrape` / `summarize` when a URL is provided.
- targeted `scrape` / `summarize` for selected evidence URLs.

Deferred / optional:
- `ask` only when `includeAsk` is true.
- `suggest` only as a follow-up.
- no `stats`.
- no `extract`; it is async and too slow for this workflow.

Output contract:
- concise answer
- implementation recipe
- minimal code/config shape when evidence supports it
- evidence table
- gaps and risks
- follow-up Axon calls
- generated follow-up Code Mode snippet for missing or weak evidence
- timing by action

### Code Mode Snippet

Paste into Labby Code Mode `execute`. Edit the `input` object at the top.

```js
async (overrides = {}) => {
  const input = {
    topic: overrides.topic ?? "implementing mcp-ui in rust",
    focus:
      overrides.focus ?? "Rust rmcp server with MCP Apps UI resources, concrete APIs, metadata keys, MIME types, and implementation steps",
    seedUrl: overrides.seed_url ?? null,
    maxEvidenceUrls: overrides.max_evidence_urls ?? 4,
    includeAsk: overrides.include_ask ?? false,
    maxAnswerChars: 5000,
    maxSourceSummaryChars: 900,
    maxMarkdownChars: 12000,
    includeFollowupSnippet: true,
    includeSourceSummaries: false,
    includeDebugFields: false,
    ...overrides
  };

  const axon = (args) => callTool("axon::axon", args);

  const parseTool = (result) => {
    const text = result?.content?.[0]?.text;
    if (typeof text !== "string") return result;
    try {
      return JSON.parse(text);
    } catch {
      return { raw: text };
    }
  };
  const truncateText = (text, maxChars) => {
    if (typeof text !== "string") return text;
    if (!Number.isFinite(maxChars) || maxChars <= 0 || text.length <= maxChars) return text;
    return `${text.slice(0, maxChars)}\n\n[truncated ${text.length - maxChars} chars]`;
  };

  const timed = async (label, args) => {
    const started = Date.now();
    try {
      const result = await axon(args);
      const parsed = parseTool(result);
      return {
        label,
        ok: parsed?.ok ?? !result?.isError,
        ms: Date.now() - started,
        action: parsed?.action,
        subaction: parsed?.subaction,
        args,
        key_fields: parsed?.data?.key_fields,
        shape: parsed?.data?.shape,
        artifact:
          parsed?.data?.artifact?.path ??
          parsed?.data?.artifact_handle?.path ??
          parsed?.artifact?.path,
        data: parsed?.data?.data,
        error: parsed?.error?.message
      };
    } catch (error) {
      return {
        label,
        ok: false,
        ms: Date.now() - started,
        args,
        error: String(error)
      };
    }
  };

  const firstPass = [
    ["search", { action: "search", query: input.topic }],
    ["research", { action: "research", query: input.topic }],
    ["query", { action: "query", query: input.topic }]
  ];

  if (input.seedUrl) {
    firstPass.push(
      ["seed.scrape", { action: "scrape", url: input.seedUrl }],
      ["seed.summarize", { action: "summarize", url: input.seedUrl }]
    );
  }

  if (input.includeAsk) {
    firstPass.push([
      "ask",
      {
        action: "ask",
        query: `For ${input.topic}, what are the concrete APIs, crates, examples, compatibility constraints, and implementation steps? ${input.focus}`
      }
    ]);
  }

  const started = Date.now();
  const firstPassResults = await Promise.all(firstPass.map(([label, args]) => timed(label, args)));

  const sourceCandidates = [];
  const isUsableUrl = (url) =>
    typeof url === "string" &&
    /^https?:\/\//i.test(url) &&
    !url.startsWith("<") &&
    !url.includes("<string");
  const addUrl = (url, reason, quality = "unknown") => {
    if (!isUsableUrl(url) || sourceCandidates.some((candidate) => candidate.url === url)) return;
    sourceCandidates.push({ url, reason, quality });
  };

  for (const result of firstPassResults) {
    const searchSamples = result.shape?.search_results?.sample ?? result.data?.results ?? [];
    for (const item of searchSamples) {
      addUrl(item.url, `${result.label} result: ${item.title ?? item.source ?? "untitled"}`);
    }

    const querySamples = result.shape?.results?.sample ?? [];
    for (const item of querySamples) {
      addUrl(item.url ?? item.source, `${result.label} indexed match`);
    }

    const researchSources = result.shape?.extractions?.sample ?? [];
    for (const item of researchSources) {
      addUrl(item.url, `${result.label} source: ${item.title ?? "untitled"}`, item.source_reputation);
    }
  }

  const queryTerms = new Set(
    `${input.topic} ${input.focus}`
      .toLowerCase()
      .split(/[^a-z0-9_:-]+/)
      .filter((term) => term.length > 3)
  );

  const scoreEvidenceFit = (candidate) => {
    const url = candidate.url.toLowerCase();
    const reason = candidate.reason.toLowerCase();
    let score = 0;

    // Source quality: prefer official docs, SDK docs, examples, and package docs.
    if (url.includes("modelcontextprotocol.io")) score += 100;
    if (url.includes("github.com/modelcontextprotocol")) score += 95;
    if (url.includes("github.com/") && url.includes("/examples")) score += 80;
    if (url.includes("github.com/") && url.includes("/docs")) score += 75;
    if (url.includes("docs.rs") || url.includes("crates.io")) score += 70;
    if (url.includes("mcpui.dev")) score += 65;
    if (url.includes("github.com/")) score += 55;

    // Specificity: spend follow-up calls on pages whose URL/title matches the task.
    for (const term of queryTerms) {
      if (url.includes(term)) score += 12;
      if (reason.includes(term)) score += 8;
    }

    // Broad pages are often useful for context but poor follow-up targets.
    if (url.endsWith("/index.html")) score -= 20;
    if (/github\.com\/[^/]+\/[^/]+\/?$/.test(url)) score -= 25;
    if (url.includes("/latest/") && ![...queryTerms].some((term) => url.includes(term))) {
      score -= 10;
    }

    // Avoid spending follow-up calls on personal mirrors, generated reference
    // blobs, and line-fragment text dumps when public docs/examples exist.
    if (/github\.com\/jmagar\//.test(url)) score -= 80;
    if (url.includes("/docs/references/")) score -= 60;
    if (url.includes(".txt#l")) score -= 50;
    if (url.includes("llms.txt")) score -= 50;

    if (url.includes("blog") || url.includes("medium.com")) score -= 20;
    return score;
  };

  const evidenceUrls = sourceCandidates
    .map((candidate) => ({ ...candidate, score: scoreEvidenceFit(candidate) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, input.maxEvidenceUrls);

  const evidenceCalls = evidenceUrls.flatMap((candidate, index) => [
    [
      `evidence.${index + 1}.scrape`,
      { action: "scrape", url: candidate.url }
    ],
    [
      `evidence.${index + 1}.summarize`,
      { action: "summarize", url: candidate.url }
    ]
  ]);

  const evidenceResults = await Promise.all(
    evidenceCalls.map(([label, args]) => timed(label, args))
  );

  const facts = evidenceResults
    .filter((result) => result.ok && result.key_fields?.summary)
    .map((result) => ({
      label: result.label,
      url: result.args.url,
      summary: truncateText(result.key_fields.summary, input.maxSourceSummaryChars)
    }));

  const followupQueries = [
    `${input.topic} ${input.focus}`,
    `${input.topic} official docs examples`,
    `${input.topic} API reference concrete code`,
    `${input.topic} compatibility caveats`
  ];

  const followupSeedUrls = [
    ...evidenceUrls.map((candidate) => candidate.url),
    ...sourceCandidates
      .map((candidate) => ({ ...candidate, score: scoreEvidenceFit(candidate) }))
      .filter((candidate) => !evidenceUrls.some((selected) => selected.url === candidate.url))
      .sort((a, b) => b.score - a.score)
      .slice(0, 4)
      .map((candidate) => candidate.url)
  ];

  const followupSnippet = `async () => {
  const input = ${JSON.stringify(
    {
      topic: input.topic,
      focus: input.focus,
      queries: followupQueries,
      seedUrls: followupSeedUrls,
      maxEvidenceUrls: 5
    },
    null,
    2
  )};

  const axon = (args) => callTool("axon::axon", args);
  const parseTool = (result) => {
    const text = result?.content?.[0]?.text;
    if (typeof text !== "string") return result;
    try { return JSON.parse(text); } catch { return { raw: text }; }
  };
  const timed = async (label, args) => {
    const started = Date.now();
    try {
      const result = await axon(args);
      const parsed = parseTool(result);
      return {
        label,
        ok: parsed?.ok ?? !result?.isError,
        ms: Date.now() - started,
        args,
        key_fields: parsed?.data?.key_fields,
        shape: parsed?.data?.shape,
        artifact: parsed?.data?.artifact?.path ?? parsed?.data?.artifact_handle?.path,
        data: parsed?.data?.data,
        error: parsed?.error?.message
      };
    } catch (error) {
      return { label, ok: false, ms: Date.now() - started, args, error: String(error) };
    }
  };

  const discoveryCalls = input.queries.flatMap((query, index) => [
    ["search." + (index + 1), { action: "search", query }],
    ["query." + (index + 1), { action: "query", query }]
  ]);

  const started = Date.now();
  const discovery = await Promise.all(discoveryCalls.map(([label, args]) => timed(label, args)));

  const candidates = [];
  const isUsableUrl = (url) =>
    typeof url === "string" &&
    /^https?:\/\//i.test(url) &&
    !url.startsWith("<") &&
    !url.includes("<string");
  const addCandidate = (url, reason) => {
    if (!isUsableUrl(url) || candidates.some((candidate) => candidate.url === url)) return;
    candidates.push({ url, reason });
  };

  for (const url of input.seedUrls) addCandidate(url, "seed URL from prior run");
  for (const result of discovery) {
    const searchSamples = result.shape?.search_results?.sample ?? result.data?.results ?? [];
    for (const item of searchSamples) addCandidate(item.url, result.label + ": " + (item.title ?? item.source ?? "untitled"));

    const querySamples = result.shape?.results?.sample ?? [];
    for (const item of querySamples) addCandidate(item.url ?? item.source, result.label + ": indexed match");
  }

  const queryTerms = new Set(
    (input.topic + " " + input.focus)
      .toLowerCase()
      .split(/[^a-z0-9_:-]+/)
      .filter((term) => term.length > 3)
  );

  const scoreEvidenceFit = (candidate) => {
    const url = candidate.url.toLowerCase();
    const reason = candidate.reason.toLowerCase();
    let score = 0;
    if (url.includes("modelcontextprotocol.io")) score += 100;
    if (url.includes("github.com/modelcontextprotocol")) score += 95;
    if (url.includes("github.com/") && url.includes("/examples")) score += 80;
    if (url.includes("github.com/") && url.includes("/docs")) score += 75;
    if (url.includes("docs.rs") || url.includes("crates.io")) score += 70;
    if (url.includes("mcpui.dev")) score += 65;
    if (url.includes("github.com/")) score += 55;
    for (const term of queryTerms) {
      if (url.includes(term)) score += 12;
      if (reason.includes(term)) score += 8;
    }
    if (url.endsWith("/index.html")) score -= 20;
    if (/github\\.com\\/[^/]+\\/[^/]+\\/?$/.test(url)) score -= 25;
    if (url.includes("/latest/") && ![...queryTerms].some((term) => url.includes(term))) score -= 10;
    if (/github\\.com\\/jmagar\\//.test(url)) score -= 80;
    if (url.includes("/docs/references/")) score -= 60;
    if (url.includes(".txt#l")) score -= 50;
    if (url.includes("llms.txt")) score -= 50;
    if (url.includes("blog") || url.includes("medium.com")) score -= 20;
    return score;
  };

  const selectedSources = candidates
    .map((candidate) => ({ ...candidate, score: scoreEvidenceFit(candidate) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, input.maxEvidenceUrls);

  const evidenceCalls = selectedSources.flatMap((candidate, index) => [
    ["evidence." + (index + 1) + ".scrape", { action: "scrape", url: candidate.url }],
    ["evidence." + (index + 1) + ".summarize", { action: "summarize", url: candidate.url }]
  ]);

  const evidence = await Promise.all(evidenceCalls.map(([label, args]) => timed(label, args)));

  return {
    workflow: "axon_research_brief_followup",
    total_ms: Date.now() - started,
    input,
    selected_sources: selectedSources,
    timings: [...discovery, ...evidence].map((result) => ({
      label: result.label,
      ok: result.ok,
      ms: result.ms,
      artifact: result.artifact,
      error: result.error
    })),
    summaries: evidence
      .filter((result) => result.ok && result.key_fields?.summary)
      .map((result) => ({
        label: result.label,
        url: result.args.url,
        summary: result.key_fields.summary
      }))
  };
}`;

  const researchSummary =
    firstPassResults.find((result) => result.label === "research" && result.key_fields?.summary)
      ?.key_fields.summary ?? "";
  const askSummary =
    firstPassResults.find((result) => result.label === "ask" && result.key_fields?.answer)
      ?.key_fields.answer ?? "";
  const primaryAnswer = truncateText(
    askSummary || researchSummary || facts.map((fact) => fact.summary).join("\n\n"),
    input.maxAnswerChars
  );
  const evidenceTable = evidenceUrls.map((candidate, index) => ({
    n: index + 1,
    url: candidate.url,
    reason: candidate.reason,
    score: candidate.score
  }));
  const followupCalls = followupQueries.map((query) => ({
    action: "search/query",
    query
  }));
  const timings = [...firstPassResults, ...evidenceResults].map((result) => ({
    label: result.label,
    ok: result.ok,
    ms: result.ms,
    artifact: result.artifact,
    error: result.error
  }));
  const markdown = truncateText([
    `# ${input.topic}`,
    "",
    "## Answer",
    primaryAnswer || "No synthesized answer was returned. Inspect the evidence summaries and artifacts below.",
    "",
    "## Evidence",
    evidenceTable.length
      ? [
          "| # | Source | Reason | Score |",
          "| ---: | --- | --- | ---: |",
          ...evidenceTable.map(
            (source) => `| ${source.n} | ${source.url} | ${source.reason} | ${source.score} |`
          )
        ].join("\n")
      : "No evidence URLs were selected.",
    "",
    "## Source Summaries",
    facts.length
      ? facts
          .map(
            (fact, index) =>
              `### ${index + 1}. ${fact.url}\n\n${fact.summary}`
          )
          .join("\n\n")
      : "No source summaries were produced.",
    "",
    "## Gaps And Risks",
    [
      evidenceUrls.length < input.maxEvidenceUrls
        ? `Only ${evidenceUrls.length} evidence URL(s) were selected.`
        : null,
      facts.length < evidenceUrls.length
        ? "Some selected sources did not produce summaries."
        : null,
      "Verify code examples against the current crate versions before copying into production."
    ]
      .filter(Boolean)
      .map((line) => `- ${line}`)
      .join("\n"),
    "",
    "## Follow-Up Calls",
    followupCalls.map((call) => `- ${call.action}: ${call.query}`).join("\n"),
    "",
    "## Follow-Up Code Mode Snippet",
    input.includeFollowupSnippet
      ? ["```js", followupSnippet, "```"].join("\n")
      : "Set `includeFollowupSnippet: true` to include the executable follow-up snippet.",
    "",
    "## Timings",
    [
      "| Call | Status | Time | Artifact |",
      "| --- | --- | ---: | --- |",
      ...timings.map(
        (timing) =>
          `| ${timing.label} | ${timing.ok ? "ok" : "error"} | ${timing.ms}ms | ${timing.artifact ?? ""} |`
      )
    ].join("\n")
  ].join("\n"), input.maxMarkdownChars);

  const slug = (value) =>
    String(value)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 80) || "brief";
  // Artifact-first: write the (potentially large) brief to disk and return only
  // the receipt, keeping the markdown out of the final response payload.
  const artifact = await writeArtifact(
    `axon/${slug(input.topic)}.md`,
    markdown,
    { contentType: "text/markdown" }
  );

  const debugFields = input.includeDebugFields
    ? {
        evidence_table: evidenceTable,
        gaps_and_risks: [
          evidenceUrls.length < input.maxEvidenceUrls
            ? `Only ${evidenceUrls.length} evidence URL(s) were selected.`
            : null,
          facts.length < evidenceUrls.length
            ? "Some selected sources did not produce summaries."
            : null,
          "Verify code examples against the current crate versions before copying into production."
        ].filter(Boolean),
        followup_calls: followupCalls,
        selected_sources: evidenceUrls,
        timings,
        source_summaries: input.includeSourceSummaries ? facts : null
      }
    : {};

  return {
    workflow: "axon_research_brief",
    total_ms: Date.now() - started,
    input,
    artifact,
    followup_snippet: input.includeFollowupSnippet
      ? {
          purpose:
            "Included in the markdown artifact under `Follow-Up Code Mode Snippet`."
        }
      : null,
    ...debugFields,
    output_format: [
      "Answer",
      "Implementation Recipe",
      "Minimal Shape",
      "Evidence Table",
      "Gaps And Risks",
      "Follow-Up Calls"
    ]
  };
}
```

### MCP Prompt Wrapper

An MCP prompt should not duplicate the workflow. It should expose this snippet by name:

```text
Run snippet `axon_research_brief` with:
- topic: {{topic}}
- focus: {{focus}}
- seedUrl: {{url}}

Read the artifact named in the snippet output's `artifact` receipt (its `absolute_path`) as the primary answer. Use structured fields only for follow-up or verification.
Do not add `extract` or `stats`.
```

## `axon_fanout_url`

Purpose: gather broad intel about a single URL.

Recommended calls:
- `scrape`
- `map`
- `summarize`
- `brand`
- `screenshot`
- `query`
- optional `ask`
- optional `crawl` when the user wants background indexing

Avoid in the fast path:
- `extract`
- `stats`

## `axon_health_snapshot`

Purpose: quickly understand whether Axon is available and what it already knows.

Recommended calls:
- `help`
- `status`
- `doctor`
- `domains`
- `sources`

Avoid unless debugging storage/index internals:
- `stats`
