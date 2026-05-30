---
name: quick-push
description: Create the save-to-md session doc before staging, git add all, commit with Claude co-authorship trailer, and push to current/new feature branch — including version bump and changelog update. Use when the user says "quick push", "push my changes", "commit and push", "ship this", "push to a new branch", or any request to wrap up local work and get it on the remote. Accepts optional `--no-bump` argument to skip the version bump.
allowed-tools: Bash, Read, Edit, Write, TodoWrite
argument-hint: [--no-bump]
disable-model-invocation: true
---

## Context

- Current branch: !`git branch --show-current`
- Git status: !`git status --short`
- Remote info: !`git remote -v | head -1`
- Change scope: !`git rev-parse --verify HEAD > /dev/null 2>&1 && git diff --stat HEAD || echo "(no commits yet)"`
- Recent commits: !`git rev-parse --verify HEAD > /dev/null 2>&1 && git log --oneline -5 || echo "(no commits yet)"`

## Your task

Work through these steps in order. The arguments string `$ARGUMENTS` may contain `--no-bump`.

### 1. Orient
- If the working tree is clean (nothing to commit), stop and report — nothing to do
- If on main/master, create a new feature branch with a descriptive name based on the changes

### 2. Bump version (before staging)

Detect the project type and bump the version based on the nature of the changes in context.

**Bump rules** (based on what you observe in the diff):
- Breaking API/behavior change → **major** (X+1.0.0)
- New feature or capability → **minor** (X.Y+1.0)
- Everything else (fix, chore, refactor, test, docs, etc.) → **patch** (X.Y.Z+1)

**Process:**
1. Read the current version from the primary manifest (first match: `Cargo.toml`, `package.json`, `pyproject.toml`)
2. Determine bump type from the changes in context (the commit prefix you'll write in step 5 should match)
3. Calculate the new version
4. **Update the version in ALL version-bearing files that exist in the repo:**
   - `Cargo.toml` — `version = "X.Y.Z"` in `[package]` (or `[workspace.package]` for Rust workspaces)
   - every tracked `package.json` with a top-level `"version"` field, including app packages such as `apps/*/package.json`; skip dependency folders such as `node_modules`, build output, and vendored third-party packages
   - `pyproject.toml` — `version = "X.Y.Z"` in `[project]`
   - `.claude-plugin/plugin.json` — `"version": "X.Y.Z"`
   - `.codex-plugin/plugin.json` — `"version": "X.Y.Z"`
   - `README.md` version line/badge when present, for common forms like `Version: X.Y.Z`, `version-X.Y.Z`, or `vX.Y.Z`
5. If a repo has a release checklist or other explicit version-sync contract, follow it before committing. Prefer repo-local tests/scripts for this if present.
6. If Rust: run `cargo check` to update `Cargo.lock` (it records the version) — if `cargo check` fails, stop and report the error
7. Verify version sync before staging: `git grep -F "<old_version>" -- '*.toml' '*.json' '*.md' '*.yml' '*.yaml'`. Review hits and fix any that represent the *current* project version (not historical release notes / changelog entries / dependency pins on third-party packages).
8. Report: `Version: X.Y.Z → A.B.C (bump type)` and list which files were updated

**Skip conditions:**
- Version is `0.0.0` or `0.0.1` (project not yet versioned)
- No manifest file found
- `--no-bump` appears in `$ARGUMENTS`

### 3. Update CHANGELOG.md (before staging)
If a `CHANGELOG.md` exists in the repo root:
- This step documents *prior* commits. Do not add the current push's own entry here because its commit SHA is not known yet.
- If the version was bumped, ensure the changelog has a release section for the new version, e.g. `## [A.B.C] - YYYY-MM-DD`, and move any current `## [Unreleased]` content under it when the repo uses Keep a Changelog style.
- Find the most recently documented commit in the changelog (look for commit hashes in the table)
- Run `git log --oneline <last_documented_sha>..HEAD` to get undocumented commits
- If there are new commits, update the changelog:
  - Add new rows to the commit summary table (newest first)
  - Update the Highlights section with grouped summaries
  - Keep the existing structure and style
- If the changelog format is unrecognizable (no commit hash table, no clear anchor), skip rather than guess
- If no CHANGELOG.md exists, skip this step

### 4. Save session context before staging
Invoke the `save-to-md` skill before staging anything so the session document can be included in the commit.

Because quick-push stages the whole worktree immediately afterward, constrain this invocation to session documentation:
- Write the session document
- Perform read-only maintenance checks needed to document the session
- Create or update beads only when directly required to track remaining work from this session
- Do not move plan files, delete branches/worktrees, or update unrelated stale docs during quick-push; record those as follow-up work instead

After `save-to-md` finishes:
- Record the final absolute path it prints; if no path is printed, stop before staging or committing
- Check whether the file is ignored with `git -C "$(git rev-parse --show-toplevel)" check-ignore -q -- <session-doc-path>`
- If the file is ignored by repo rules (common for `docs/sessions/`), stage it explicitly with `git add -f -- <session-doc-path>` during the staging step below
- If the save fails, report the error and stop before staging or committing

### 5. Stage, commit, and push
- Get the repo root with `repo_root=$(git rev-parse --show-toplevel)`
- Stage all changes from the repo root with `git -C "$repo_root" add .`
- If the generated session document was ignored, also run `git -C "$repo_root" add -f -- <session-doc-path>`
- Create a meaningful commit message following the repo's conventions
- Always include Claude's co-authorship trailer:
  ```text
  Co-authored-by: Claude <noreply@anthropic.com>
  ```
- Push to remote:
  - New branch: `git push -u origin <branch>`
  - Existing branch: `git push`
  - If push is rejected (remote has new commits): run `git pull --rebase`, resolve any conflicts, then retry the push

---

**Notes:**
- If creating a new branch, name it based on the changes (e.g., `feat/add-user-auth`, `fix/navbar-styling`)
- The changelog update is part of the commit — it goes in the same commit as the other changes
- The session document is part of the commit — save it before staging, then explicitly force-add it if repo ignore rules would hide it
- The saved session document captures pre-commit HEAD metadata; do not amend the session document after the push unless the user explicitly asks for final-HEAD metadata
- End with a summary of what was pushed and the branch name
- List all unfinished tasks in the session and next steps for the user to consider
- If any step fails (e.g., version bump, changelog update, push), report the error and stop the process to avoid partial commits
- Never force push or delete branches without explicit user instruction
