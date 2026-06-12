---
name: repo-status
description: Audit the current Git checkout, open worktrees, local branches, stale or merged cleanup candidates, merge readiness, conflicts, PR/CI/test state, blockers, and safest merge order. Use when the user asks for repo status, branch/worktree cleanup candidates, stale branch review, conflict investigation, merge readiness, or what must be done before open branches can merge.
---

# Repo Status

## Core Rule

Start from live Git evidence. Do not infer branch cleanliness, mergeability, CI state, or stale status from memory alone.

Keep the work read-only unless the user explicitly asks to merge, delete, rebase, clean, stage, commit, push, or run a destructive cleanup.

## Evidence Sweep

Run the bundled context collector from the current repository root and treat its output as the initial context for the analysis:

```bash
<skill-dir>/scripts/repo_context.sh
```

The collector emits `pwd`, `git rev-parse --show-toplevel`, `git status --short --branch`, `git status --porcelain=v2 --branch`, `git branch --show-current`, `git worktree list --porcelain`, structured per-worktree status, `git branch --all --verbose --no-abbrev`, machine-readable branch inventory, per-branch base/diff/risk fields, `git remote -v`, default base detection with rationale, and `git fetch --all --prune --dry-run`, plus command labels and nonzero exit codes.

Useful collector modes:

```bash
<skill-dir>/scripts/repo_context.sh --no-fetch
<skill-dir>/scripts/repo_context.sh --json
<skill-dir>/scripts/repo_context.sh --branch <branch>
<skill-dir>/scripts/repo_context.sh --include-gh
<skill-dir>/scripts/repo_context.sh --json --output /tmp/repo-status.json
<skill-dir>/scripts/repo_context.sh --json --output /tmp/repo-status.json --force-output
<skill-dir>/scripts/repo_context.sh --max-branches 20
```

The fetch dry-run is read-only but may contact remotes. Use `--no-fetch` or set `REPO_STATUS_NO_FETCH=1` when the user needs an offline snapshot. Use `--json` when you need stable machine-readable context for follow-up parsing. Use `--output` to save an evidence artifact to a temp path or user-requested path; it fails if the file already exists unless `--force-output` is provided. Use `--branch` for focused investigation after the initial inventory. Use `--include-gh` when GitHub PR/CI evidence is needed and `gh` is configured; this collects per-branch PR and run evidence for local branches and open PR heads. Use `--max-branches` in repos with many local branches to cap expensive per-branch diff/PR collection; the collector prioritizes the current branch, open worktree branches, focused branch, open PR heads, and recently updated branches before applying the cap. Rerun with `--branch` for branches that need deeper review. When capped, the JSON keeps lightweight placeholder rows for uncollected branches and marks them with `limited: true`.

For a first-pass table from saved JSON, run:

```bash
<skill-dir>/scripts/summarize_context.py /tmp/repo-status.json
```

Treat the summary as a triage aid, not a final answer. See `<skill-dir>/examples/context.json` and `<skill-dir>/examples/summary.md` for a small sanitized schema and summary example.

If the script is unavailable, run those commands manually and preserve the same evidence in your notes.

If GitHub is the remote and `gh` is available, also gather PR and CI evidence:

```bash
gh pr list --state open --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,updatedAt,url
gh run list --limit 20 --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url
```

For branches with open PRs or unclear CI/review state, gather per-branch evidence:

```bash
gh pr view <branch-or-number> --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,reviews,comments,updatedAt,url
gh run list --branch <branch> --limit 10 --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url
```

If another forge is in use, use its CLI/API if locally configured. If no forge evidence is available, say CI state is unverified rather than guessing.

## Risk Signals

For every active branch, scan the changed files and patch for merge-readiness risk signals before classifying it as ready:

- `TODO`, `FIXME`, `WIP`, `XXX`, temporary debug code, or commented-out code.
- Skipped, ignored, deleted, or weakened tests.
- Lockfile-only, generated, vendored, or large binary changes.
- Migrations, config, secrets, auth, CI, deployment, or shared API files.
- Overlap with other open branches' changed files.

Do not treat these signals as automatic blockers. Use them to decide what evidence or tests are needed.

Use the collector's `stale_evidence` fields as inputs, not conclusions:

- `merged_into_base`
- `days_since_last_commit`
- `has_unique_commits`
- `upstream_branch_exists`
- `same_named_remote_exists`
- `worktree_missing_or_prunable`

In `stale_evidence`, `null` means the field was not evaluated or evidence was unavailable. `false` means the check was evaluated and did not match.

A branch may be old without being stale, or merged while still needing cleanup of a worktree or remote.

## Per Worktree And Branch Review

Treat `main`, `master`, `trunk`, `develop`, and the repository default branch from `origin/HEAD` as primary candidates. Review every other local branch unless the user narrows scope. If unsure whether a branch is primary, include it and mark its role as unknown.

For each worktree from `git worktree list --porcelain` and each reviewed local branch:

1. Record path, branch, HEAD SHA, upstream, ahead/behind counts, dirty status, and last commit date.
2. Check whether the worktree path still exists and whether it is locked.
3. Inspect the branch diff against its intended base:

```bash
git -C <worktree-path> status --short --branch
git -C <worktree-path> status --porcelain=v2 --branch
git -C <worktree-path> log --oneline --decorate --max-count=8
git -C <worktree-path> diff --stat <base>...HEAD
git -C <worktree-path> diff --name-only <base>...HEAD
```

Choose the base in this order:

1. The PR base branch, if a PR exists.
2. The repository default branch from `origin/HEAD`.
3. `origin/main`, `origin/master`, `main`, or `master`, whichever exists.

Use the branch upstream to compute push/pull state, not as the integration base unless it is clearly the intended integration branch. Use `git merge-base --is-ancestor`, `git for-each-ref`, and ahead/behind counts to distinguish merged, stale, and active branches.

For active branches, inspect the patch at a summary level with `git diff <base>...HEAD -- <changed files>` or targeted file reads. Identify implementation scope, test coverage, risky shared files, generated files, and TODO/WIP markers before assigning `ready_to_merge`.

## Mergeability And Conflict Check

Run mergeability probes after the initial context snapshot. Do not run the context collector and merge helper in parallel, because the snapshot may observe the helper's temporary worktree while it is initializing.

Check mergeability without changing the user's worktree. Prefer the bundled helper:

```bash
<skill-dir>/scripts/check_mergeability.sh <base> <branch>
```

The helper performs a merge in a temporary worktree, cleans up with traps, and prints `mergeable`, `failure_kind`, and conflicted files. If the helper is unavailable, use this equivalent pattern:

Temporary worktree probes briefly mutate Git worktree metadata even though they do not change the user's checkout. Run them sequentially with evidence collection and confirm cleanup before final reporting.

```bash
tmp_parent=$(mktemp -d)
tmp="$tmp_parent/worktree"
cleanup() {
  git worktree remove --force "$tmp" >/dev/null 2>&1 || true
  rmdir "$tmp_parent" >/dev/null 2>&1 || true
}
trap cleanup EXIT
git worktree add --detach "$tmp" "$base"
if ! git -C "$tmp" merge --no-commit --no-ff "$branch"; then
  git -C "$tmp" diff --name-only --diff-filter=U
fi
```

If the merge reports conflicts, list the conflicted files and inspect enough of each side to explain what must be reconciled. Do not resolve conflicts unless the user asks.

When using a temporary worktree, clean it up before final response. If cleanup fails, report the leftover path.

## Readiness Classification

Classify every non-primary branch/worktree as exactly one of:

- `ready_to_merge`: clean worktree, branch is not behind base or has been checked against current base, merge check passes, tests/CI are passing or explicitly not required, and reviews are satisfied if PR evidence exists.
- `needs_work`: active branch with useful commits but missing tests, failing tests/CI, draft PR, unresolved review feedback, dirty worktree, unclear base, or incomplete implementation.
- `conflicted`: merge check fails or forge marks it conflicting. Include exact files and required reconciliation.
- `stale_cleanup_candidate`: branch is already merged, abandoned, superseded, very old with no unique useful commits, or its worktree path is gone. Recommend cleanup commands but do not run them.
- `unknown`: evidence is insufficient. State exactly what evidence is missing.

Do not mark a branch ready just because it has no conflicts. Readiness also requires status checks/tests and review state when those systems exist.

## Tests And CI

Use repository conventions for tests. Prefer project docs and common task runners:

```bash
just --list
make help
npm test
cargo test --workspace
```

For each branch, report whether tests were:

- Already passing in CI, with source.
- Run locally by you, with command and result.
- Not run, with a concrete reason.

Avoid expensive full-suite runs on every branch unless the user asked for exhaustive verification. For initial status, use existing CI plus targeted local checks; then recommend the exact tests needed to graduate `needs_work` to `ready_to_merge`.

## Merge Order

When multiple branches/worktrees are open, propose a merge order using these rules:

1. Ready branches before needs-work branches.
2. Independent low-risk changes before broad refactors.
3. Foundational/shared API changes before branches that depend on them.
4. Branches touching overlapping files should be ordered to minimize conflict churn.
5. Stale cleanup candidates should be removed before active merge work if they create confusing worktrees or branch names.

If order depends on unknowns, list the dependency or missing evidence.

## Final Report Shape

Lead with a compact status summary:

- Current checkout: branch, clean/dirty, ahead/behind.
- Open worktrees/branches: count and high-level classification.
- Merge order: ordered list or "none ready".

Then provide a table:

| Branch/worktree | Status | Evidence | Next action |
|---|---|---|---|

For every `needs_work` or `conflicted` item, include a short checklist of what must happen to make it mergeable.

End with verification notes: commands run, tests/CI checked, and any evidence you could not access.
