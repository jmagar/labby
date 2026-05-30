---
name: work-it
description: Use when the user asks to "work it", execute a superpowers executing-plans document in a worktree, create a PR as soon as the implemented plan is green, or run a complete review-and-fix loop over all touched files.
---

# Work It

## Overview

Use this skill to run a plan to completion in an isolated `.worktrees/` checkout. Treat the whole worktree as owned for the duration: pre-existing failures, stale tests, lint issues, review findings, and PR comments in that worktree must be fixed before claiming completion.

## Non-Negotiables

- Create and work inside a `.worktrees/<slug>` checkout unless the user explicitly names an existing worktree.
- Read the requested plan file before implementation, then dispatch a dedicated implementation agent inside the worktree to execute it with the repo's `superpowers:executing-plans` workflow.
- Do not implement the plan directly in the coordinator session unless the user explicitly overrides this skill. If agent dispatch is unavailable, report the workflow as blocked rather than silently self-implementing.
- Keep all implementation, review fixes, verification, commits, PR creation, and PR comment resolution inside the worktree.
- Fix every issue surfaced by verification, `lavra-review`, three `code_simplifier` passes, all available `pr-review-toolkit` agents, and `gh-fetch-comments`.
- Create the PR immediately after the plan is fully implemented and the worktree is green of all known issues, including pre-existing issues. This starts external review from CodeRabbit, Copilot, cubic-dev, and similar reviewers while later review waves run.
- Do not resolve GitHub comments until the matching code or documentation change is committed or the comment is proven obsolete with evidence.
- Execute `vibin:save-to-md` before the final `git add .` so the session note is captured and can be included in the final commit when the repo expects it.
- Before final completion, run `git add .`, commit all remaining worktree changes, and push to the worktree branch's remote. If the branch has no upstream, set it with `git push -u origin HEAD` or the repo's correct remote.
- Do not finish while background jobs or review agents are still running.

## Workflow

1. **Prepare the isolated checkout**
   - Inspect the current repo state with `git status --short --branch`, `git branch --show-current`, and `git remote -v`.
   - Create a branch and worktree under `.worktrees/`, for example:

     ```bash
     git worktree add -b <branch> .worktrees/<slug> HEAD
     ```

   - Enter the worktree for all remaining commands.
   - Re-check status in the worktree and record the base branch.

2. **Load the plan**
   - Read the plan path supplied by the user, such as `docs/superpowers/plans/<date>-<name>.md`.
   - Confirm the `superpowers:executing-plans` skill is available for the implementation agent and include that requirement in the dispatch prompt.
   - Convert the plan into a coordinator checklist for tracking the implementation agent handoff and later review phases.
   - Treat ambiguous plan items as requirements to clarify through repo evidence before editing.

3. **Dispatch implementation agent to green**
   - Dispatch one implementation agent whose working directory is the worktree root.
   - Instruct the agent to invoke `superpowers:executing-plans` and execute the plan file from inside the worktree.
   - Give the agent the plan path, base branch, worktree path, branch name, repo validation hints, and this constraint: all implementation, focused tests, full verification, and any repair work must happen inside the worktree.
   - Require the agent to make scoped changes that satisfy every plan item, include any pre-existing worktree failures in the repair scope, and iterate until the entire worktree is green: tests, lint, formatting, build, typecheck, generated artifacts, or any repo-specific gates.
   - Require the agent to return a concise handoff with changed files, plan items completed, verification commands and results, remaining risks, and whether the worktree is clean or dirty.
   - When the agent returns, inspect `git status --short`, review the changed files enough to understand the implementation, and run or re-run the reported verification before proceeding.

4. **Create the PR immediately**
   - Commit as soon as the plan is fully implemented and the worktree is green of all known issues, including pre-existing failures.
   - Push the branch and create a PR with `gh pr create`.
   - Include the plan summary, implemented changes, and verification evidence in the PR body.
   - Keep the PR open while the remaining review waves run so external reviewers have time to produce comments.

5. **Run first independent review wave**
   - Run `lavra-review` in the worktree.
   - Address every finding, regardless of severity.
   - Re-run relevant verification after each fix batch and push follow-up commits to the PR.

6. **Run three simplification passes**
   - Dispatch three `code_simplifier` agents against all touched files.
   - Give each agent the touched-file list, the plan path, and the instruction to report concrete issues or patch directly when the environment supports it.
   - When they finish, address every issue from all three passes.
   - Re-run verification after simplifier-driven changes and push follow-up commits to the PR.

7. **Run full PR review toolkit sweep**
   - Dispatch every available `pr-review-toolkit` agent against all touched files in the PR.
   - Ask for a systematic sweep of correctness, tests, silent failures, type design, comments, and simplification where the toolkit offers those roles.
   - Address every finding.
   - Re-run the complete verification gate and push follow-up commits.

8. **Resolve PR comments**
   - Run `gh-fetch-comments` or the repo-local equivalent for the PR just created.
   - Address every open comment in the worktree.
   - Push fixes, verify again, then resolve comments with the repo's accepted resolver only after the fix is present remotely.
   - Repeat fetch, fix, verify, push, and resolve until there are zero unresolved actionable comments.

9. **Save the session**
   - Execute `vibin:save-to-md` in the worktree after step 8 (PR comment resolution) and before step 10's final `git add .`.
   - Save the markdown note in the repo's normal session location, usually `docs/sessions/`, unless the user provides a different path.
   - Include concrete repo context: branch, HEAD, worktree path, PR URL, verification commands, review waves run, comments resolved, remaining risks, and open questions.
   - If the save-to-md skill or command is unavailable, manually create the same markdown artifact and state the substitution in the final report.

10. **Final publish**
   - Run `git status --short` and review exactly what will be included.
   - Run `git add .` from the worktree root.
   - If required deliverables are ignored, such as session notes under `docs/sessions/`, force-add them explicitly with `git add -f <path>`.
   - Commit the staged changes with a message that reflects the completed plan or final review/comment cleanup.
   - Push to the worktree branch's configured upstream. If no upstream exists, repair tracking and push with `git push -u origin HEAD` or the repo's correct remote.

11. **Final gate**
   - Confirm `git status --short` is clean except for intentionally untracked ignored artifacts.
   - Confirm all required validation commands pass in the worktree.
   - Confirm review/comment queues are empty or explicitly non-actionable with evidence.
   - Report the worktree path, branch, PR URL, commits, validation commands, and review/comment resolution status.

## Agent Dispatch Guidance

Use agents when the current runtime supports them and the user has asked for this full workflow. Keep ownership explicit:

- Implementation agent: execute the plan with `superpowers:executing-plans` inside the worktree and return only after the plan is implemented and verification is green.
- `code_simplifier` pass 1: touched implementation files.
- `code_simplifier` pass 2: touched tests and fixtures.
- `code_simplifier` pass 3: touched docs, config, generated surfaces, and cross-file consistency.
- `pr-review-toolkit` agents: dispatch every available toolkit role with the PR number, branch, touched-file list, and verification commands.

If an exact named review agent or command is unavailable, use the closest repo-local skill, script, or CLI equivalent and state the substitution in the final report. This fallback does not apply to the implementation agent: if no agent-dispatch mechanism exists, stop and report the implementation phase as blocked.

## Completion Standard

Completion means all plan items are implemented, all pre-existing and newly introduced worktree issues are fixed, the worktree is green, the PR exists, independent review waves have no outstanding actionable findings, and fetched PR comments are resolved. Anything less is blocked, not done.
