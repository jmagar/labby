---
name: repo-context-triage
description: Quick repository context pass using filesystem, Lumen, Octocode, GitHub, and time
tags: [repo, triage, research]
inputs:
  repo_path:
    type: string
    default: /home/jmagar/workspace/lab
    required: false
    description: Local repository path
  owner:
    type: string
    default: jmagar
    required: false
    description: GitHub owner
  repo:
    type: string
    default: lab
    required: false
    description: GitHub repository
  topic:
    type: string
    default: Code Mode
    required: false
    description: Search topic
  max_results:
    type: integer
    default: 5
    required: false
    description: Per-source result limit
---

# Repo Context Triage

Use this snippet when you need a quick orientation pass for a repo topic. It combines local file reads, Lumen semantic search, Octocode local code search, and GitHub issue/file lookups.

## Tutorial: How This Snippet Is Built

This snippet is a repository-orientation checklist. It collects local context, semantic context, code-search hits, GitHub issues, and one remote file in one run.

| Step | Tool | Why it is included | Parameters the user fills |
|---|---|---|---|
| Timestamp | `time::get_current_time` | Records when the triage pass ran | `timezone` |
| Local doc | `filesystem::read_file` | Reads the local snippet docs as context | `path` |
| Semantic search | `lumen::semantic_search` | Finds related indexed workspace knowledge | `query`, `limit` |
| Local code search | `octocode::localSearchCode` | Finds exact code/text matches in the repo | `queries[].path`, `queries[].pattern`, `maxResults` |
| GitHub issues | `github::search_issues` | Finds remote issue context | `query`, `perPage` |
| GitHub file | `github::get_file_contents` | Compares or retrieves a canonical remote file | `owner`, `repo`, `path` |

The calls are independent, so they run in parallel. The only transformation logic is output cleanup: long tool responses are previewed, GitHub issues are reduced to title/state/URL, and the result keeps enough handles for follow-up work.

## Why The Inputs Exist

- `repo_path` tells Octocode where to search locally.
- `owner` and `repo` are used by GitHub calls.
- `topic` becomes the semantic/code/issues search target.
- `max_results` bounds Lumen, Octocode, and GitHub output.

The snippet also has two fixed internal paths:

- `localDoc` defaults to the local snippets README.
- `remoteDoc` defaults to the same path in GitHub.

Those are normal builder defaults: the generated snippet can include fixed params where a workflow always wants the same file.

## What Validation Should Catch

The builder should validate both simple and nested schemas:

- `filesystem::read_file.path` must be a string.
- `lumen::semantic_search.limit` must be an integer.
- `octocode::localSearchCode.queries` must be an array of objects with `path` and `pattern`.
- `github::search_issues.perPage` must be an integer.
- `github::get_file_contents.owner`, `repo`, and `path` must be strings.

This example is useful for teaching nested fields: users should be able to add an Octocode query row in the UI instead of hand-writing `queries: [{ path, pattern }]`.

Live smoke-tested tools before authoring:

- `time::get_current_time`
- `filesystem::read_file`
- `lumen::semantic_search`
- `octocode::localSearchCode`
- `github::search_issues`
- `github::get_file_contents`

Run with:

```bash
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/repo-context-triage.md)"
```

```js
async (overrides = {}) => {
  const input = {
    repoPath: overrides.repo_path ?? "/home/jmagar/workspace/lab",
    owner: overrides.owner ?? "jmagar",
    repo: overrides.repo ?? "lab",
    topic: overrides.topic ?? "Code Mode",
    localDoc: "/home/jmagar/workspace/lab/docs/snippets/README.md",
    remoteDoc: "docs/snippets/README.md",
    maxResults: overrides.max_results ?? 5,
    ...overrides
  };

  const preview = (value, limit = 1400) => {
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
      "local_doc",
      "filesystem::read_file",
      { path: input.localDoc },
      (result) => preview(result.content || result, 1000)
    ),
    timed(
      "semantic_search",
      "lumen::semantic_search",
      { query: `${input.repo} ${input.topic}`, limit: input.maxResults },
      (result) => preview(result)
    ),
    timed(
      "local_code_search",
      "octocode::localSearchCode",
      { queries: [{ path: input.repoPath, pattern: input.topic }], maxResults: input.maxResults },
      (result) => preview(result)
    ),
    timed(
      "github_issues",
      "github::search_issues",
      { query: `repo:${input.owner}/${input.repo} ${input.topic}`, perPage: input.maxResults },
      (result) => ({
        total_count: result.total_count,
        issues: (result.items || []).slice(0, input.maxResults).map((issue) => ({
          number: issue.number,
          title: issue.title,
          state: issue.state,
          url: issue.html_url
        }))
      })
    ),
    timed(
      "github_file",
      "github::get_file_contents",
      { owner: input.owner, repo: input.repo, path: input.remoteDoc },
      (result) => preview(result, 1000)
    )
  ]);

  return {
    snippet: "repo_context_triage",
    input,
    ok: calls.every((call) => call.ok),
    calls,
    next_steps: [
      "Use the returned file paths, issue URLs, and semantic matches as follow-up targets.",
      "For exact symbol navigation, add Octocode LSP tools after smoke-testing the target language server path."
    ]
  };
}
```
