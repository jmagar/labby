---
name: "gh-fix-ci"
description: "Use when a user asks to debug or fix failing GitHub PR checks / GitHub Actions runs (e.g., 'CI is red', 'why is this PR failing', 'fix the failing checks', 'PR checks broken'). Inspects checks via gh, summarizes failure context, and implements when the user has asked for a fix."
---

# gh-fix-ci — Fix Failing GitHub PR Checks

## Overview

Use `gh` to locate failing PR checks, fetch GitHub Actions logs for actionable failures, summarize the failure, then implement when the user has asked for a fix. If the user only asked what failed, stop after the summary. If a plan-oriented skill (e.g. `create-plan`) is available and the fix is broad or risky, use it; otherwise keep the plan inline.

**Scope:** GitHub Actions only. For external providers (Buildkite, CircleCI, etc.), report the `detailsUrl` and stop — don't attempt to inspect them.

**Prereq:** authenticate the GitHub CLI once (`gh auth login`), confirm with `gh auth status`. Typically needs `repo` + `workflow` scopes.

## Inputs

Operates on the current branch's PR by default. Accepts an explicit PR number or URL if provided.

## Workflow

1. **Verify gh auth.** `gh auth status`. If unauthenticated, ask the user to run `gh auth login` before proceeding.

2. **Resolve the PR.** Prefer the current branch: `gh pr view --json number,url`. Otherwise use the provided number/URL.

3. **List failing checks.**
   - `gh pr checks <pr> --json name,state,bucket,link,startedAt,completedAt,workflow`
   - If a field is rejected (gh field drift), retry with the available fields gh reports, or fall back to bare `gh pr checks <pr>`.
   - Filter for `state == "failure"` or `bucket == "fail"`.

4. **For each failing GitHub Actions check, fetch logs.**
   - Extract the run id from `detailsUrl` (e.g. `.../actions/runs/<run_id>/...`).
   - `gh run view <run_id> --json name,workflowName,conclusion,status,url,event,headBranch,headSha`
   - `gh run view <run_id> --log` (full log)
   - If the log indicates the run is still in progress or a job log is needed directly:
     - Get failing job ids: `gh run view <run_id> --json jobs -q '.jobs[] | select(.conclusion=="failure") | .databaseId'`
     - Then: `gh api "/repos/<owner>/<repo>/actions/jobs/<job_id>/logs" > <path>`

5. **Scope non-Actions checks.** If `detailsUrl` is not a GitHub Actions URL, label external and report only the URL.

6. **Summarize for the user.** Failing check name, run URL, concise log snippet, and explicit callouts for missing logs.

7. **Plan.** Use `create-plan` if available for broad or risky fixes; otherwise inline a concise plan. Continue directly when the user has asked to fix CI. Pause only when the intended change is destructive, ambiguous, outside the repo, or materially broader than the CI failure.

8. **Implement.** Apply the plan, summarize diffs/tests, and ask about opening a follow-up commit/PR only if the user has not already requested that workflow.

9. **Recheck.** Suggest re-running local tests and `gh pr checks <pr>` to confirm green.
