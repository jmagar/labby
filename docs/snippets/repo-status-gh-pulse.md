---
name: repo-status-gh-pulse
description: Read-only GitHub PR and CI pulse for repository status checks
tags: [repo, github, ci, readonly]
inputs:
  owner:
    type: string
    default: jmagar
    required: false
    description: GitHub repository owner
  repo:
    type: string
    default: lab
    required: false
    description: GitHub repository name
  branch:
    type: string
    default: ""
    required: false
    description: Optional branch or PR head ref for focused evidence
  run_limit:
    type: integer
    default: 20
    required: false
    description: Maximum recent workflow runs to include in equivalent gh commands
  pr_limit:
    type: integer
    default: 20
    required: false
    description: Maximum open PRs to request
  include_workflow_runs:
    type: boolean
    default: true
    required: false
    description: Include shell-only workflow-run commands in the returned evidence gap
tools:
  - github::search_issues
---

# Repo Status GH Pulse

Use this snippet when the repo-status pass needs the GitHub half of the evidence sweep: open PRs, review/check summaries where the GitHub tool exposes them, recent workflow runs, and focused branch follow-up.

It mirrors the shell chain from the `vibin:repo-status` skill:

```bash
gh pr list --state open --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,updatedAt,url
gh run list --limit 20 --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url
gh pr view <branch-or-number> --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,reviews,comments,updatedAt,url
gh run list --branch <branch> --limit 10 --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url
```

## What It Returns

- `open_prs` uses `github::search_issues` with `repo:<owner>/<repo> is:pr is:open`.
- `focused_prs` runs when `branch` is set and searches `head:<owner>:<branch>`.
- `workflow_runs` is explicit shell-only parity because the current GitHub Code Mode catalog does not expose a workflow-run tool.
- `gh_equivalent_commands` keeps the exact CLI commands that a human or shell-capable agent can run for parity.

The snippet is read-only. It is meant to complement the local Git evidence from `repo_context.sh --include-gh`, not replace the local status, worktree, diff, or mergeability checks.

```js
async (overrides = {}) => {
  const input = {
    owner: overrides.owner ?? "jmagar",
    repo: overrides.repo ?? "lab",
    branch: overrides.branch ?? "",
    runLimit: overrides.run_limit ?? 20,
    prLimit: overrides.pr_limit ?? 20,
    includeWorkflowRuns: overrides.include_workflow_runs ?? true
  };

  const compact = (value, limit = 5000) => {
    const text = typeof value === "string" ? value : JSON.stringify(value);
    if (!text || text.length <= limit) return value;
    return `${text.slice(0, limit)}...`;
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
        params,
        result: transform(result)
      };
    } catch (error) {
      return {
        label,
        id,
        ok: false,
        ms: Date.now() - started,
        params,
        error: String(error)
      };
    }
  };

  const normalizeSearchIssues = (result) => {
    if (!result || typeof result !== "object" || !Array.isArray(result.items)) {
      throw new Error(`github::search_issues returned malformed result: ${compact(result, 1000)}`);
    }

    return {
      total_count: typeof result.total_count === "number" ? result.total_count : result.items.length,
      items: result.items.map((item) => ({
        number: item.number,
        title: item.title,
        state: item.state,
        url: item.html_url,
        updated_at: item.updated_at,
        user: item.user?.login
      }))
    };
  };

  const repoSlug = `${input.owner}/${input.repo}`;
  const openPrQuery = `repo:${repoSlug} is:pr is:open sort:updated-desc`;
  const focusedPrQuery = input.branch
    ? `repo:${repoSlug} is:pr is:open head:${input.owner}:${input.branch}`
    : null;

  const calls = [
    timed(
      "open_prs",
      "github::search_issues",
      { query: openPrQuery, perPage: input.prLimit },
      normalizeSearchIssues
    )
  ];

  if (focusedPrQuery) {
    calls.push(
      timed(
        "focused_prs",
        "github::search_issues",
        { query: focusedPrQuery, perPage: Math.min(input.prLimit, 10) },
        normalizeSearchIssues
      )
    );
  }

  const results = await Promise.all(calls);
  const openPrs = results.find((call) => call.label === "open_prs");
  const focusedPrs = results.find((call) => call.label === "focused_prs");
  const requiredCallsOk = results.every((call) => call.ok);
  const workflowRunCommands = [
    `gh run list --repo ${repoSlug} --limit ${input.runLimit} --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url`,
    input.branch
      ? `gh run list --repo ${repoSlug} --branch ${input.branch} --limit 10 --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url`
      : null
  ].filter(Boolean);
  const workflowRuns = input.includeWorkflowRuns
    ? {
        available: false,
        status: "shell_only",
        reason: "The current GitHub Code Mode catalog does not expose a workflow-run tool.",
        gh_commands: workflowRunCommands
      }
    : {
        available: false,
        status: "not_requested",
        gh_commands: []
      };

  return {
    snippet: "repo_status_gh_pulse",
    input,
    ok: requiredCallsOk && !input.includeWorkflowRuns,
    status: requiredCallsOk
      ? input.includeWorkflowRuns
        ? "degraded"
        : "ok"
      : "error",
    open_prs: openPrs,
    focused_prs: focusedPrs ?? null,
    workflow_runs: workflowRuns,
    gh_equivalent_commands: [
      `gh pr list --repo ${repoSlug} --state open --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,updatedAt,url`,
      ...workflowRunCommands,
      input.branch
        ? `gh pr view ${input.branch} --repo ${repoSlug} --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,reviews,comments,updatedAt,url`
        : null
    ].filter(Boolean),
    next_steps: [
      "Pair this GitHub pulse with local git status, worktree, diff, and mergeability evidence.",
      "Run the workflow-run gh commands from a shell-capable context when CI evidence is required."
    ]
  };
}
```
