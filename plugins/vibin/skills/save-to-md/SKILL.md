---
name: save-to-md
description: Save session documentation to a markdown file with full context — date, branch, HEAD, session ID, and git state pre-injected — then stage, commit, and push only the generated session artifact. Use when the user says "save session", "save to md", "document this session", "write up what we did", "save session notes", or asks to capture the current conversation as a session log. Pass `--html` or a `.html` path to render a rich Aurora-styled HTML artifact instead.
allowed-tools: Write, Read, Bash
argument-hint: "[--html] [path]"
---

## Context

- Date: !`TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'`
- Repo: !`git remote get-url origin`
- Branch: !`git branch --show-current`
- HEAD: !`git rev-parse --short HEAD`
- Recent commits: !`git log --oneline -5`
- Files currently dirty: !`git status --short`
- Files in recent commits: !`git log --oneline --name-only -10`
- Transcript: !`ls -t ~/.claude/projects/$(pwd | sed 's|/|-|g')/*.jsonl 2>/dev/null | head -1`
- Active plan: !`cat .claude/current-plan 2>/dev/null || echo "none"`
- Working directory: !`pwd`
- Repo root: !`git rev-parse --show-toplevel 2>/dev/null || pwd`
- Worktree: !`git worktree list | grep $(pwd) | head -1`
- Active PR: !`gh pr view --json number,title,url 2>/dev/null || echo "none"`
- Beads recent issues: !`bd list --all --sort updated --reverse --limit 100 --json 2>/dev/null || echo "[]"`
- Beads recent interactions: !`tail -200 .beads/interactions.jsonl 2>/dev/null || echo "none"`
- Registered worktrees: !`git worktree list --porcelain 2>/dev/null || echo "none"`
- Local branches: !`git branch -vv 2>/dev/null || echo "none"`
- Remote branches: !`git branch -r -vv 2>/dev/null || echo "none"`
- Plans: !`find docs/plans -maxdepth 2 -type f 2>/dev/null | sort || echo "none"`

# Save Session Documentation

Document the **entire conversation session** (not just recent work) as a markdown or HTML file at `$ARGUMENTS`. If the injected `Transcript` path above is non-empty, read it to recover the full session (the current context window may be truncated). If no path is provided, save to `docs/sessions/YYYY-MM-DD-description.md` under the repo root.

## Output format

The default format is markdown (`.md`). To produce a rich Aurora-styled HTML artifact instead, the user (or caller) can:

- Pass `--html` as the first argument, with or without a path. Default path becomes `docs/sessions/YYYY-MM-DD-description.html`.
- Pass a path that ends in `.html`. Format follows the extension.

Otherwise default to markdown. Never silently switch formats.

If the resolved format is HTML, follow the **HTML rendering** section near the bottom of this file instead of the markdown structure below. All other rules (Repository Maintenance Pass, content quality, path rules, final-path print) still apply identically.

Path rules:
- Relative paths resolve from the repo root (not CWD).
- Keep this workflow in-repo. If the resolved target is outside the repo root, stop and report the path issue.
- Check whether the target directory exists (`[ -d <dir> ]`) before creating it — only run `mkdir -p` if the check fails.
- If the target filename already exists, do not overwrite. Append a suffix like `-v2`, `-v3`, etc.

## Repository Maintenance Pass

Before writing the session note, perform a repo maintenance pass and document exactly what happened. Keep this pass evidence-driven and safe; do not hide skipped, blocked, or uncertain cleanup.

1. **Plans**: Find completed plan files under `docs/plans/` and move only clearly completed plans to `docs/plans/complete/`. Create `docs/plans/complete/` only if needed. Do not move active, partial, draft, or ambiguous plans; list them in **Open Questions** or **Next Steps**.
2. **Beads**: Run the relevant `bd` reads before changing tracker state. Create, edit, comment on, claim, assign, or close all beads that are directly relevant to the session and remaining work. Close completed beads only when the work and verification are observed. Create follow-up beads for known remaining work instead of burying it only in prose.
3. **Worktrees and branches**: Inspect `git worktree list --porcelain`, local branches, remote branches, and merge ancestry before cleanup. Remove stale worktrees or branches only when they are proven safe, for example merged into the protected base branch or otherwise explicitly obsolete. Do not delete dirty worktrees, unmerged branches, unknown backup refs, active PR branches, or anything with unclear ownership; document why each was left alone.
4. **Stale docs**: Review documentation touched by or contradicted by the session. Update stale docs when the current implementation or workflow proves them wrong. If the stale-doc pass is too broad to complete safely, create/update beads and list precise docs follow-ups.
5. **Transparency**: Record every maintenance action, no-op, skipped item, blocked item, and assumption in the session note. Include the command or evidence used for each cleanup decision.

## Documentation Requirements

Start the file with a metadata block populated from the injected context above:

```yaml
date: YYYY-MM-DD HH:MM:SS EST
repo: <remote URL>
branch: <current branch>
head: <HEAD commit SHA>
plan: <path/to/plan.md> (if applicable, otherwise omit)
session id: <UUID filename of the transcript, e.g. cef54ead-b02d-4c3e-a833-a8672fa20523> (omit if transcript injection was empty)
transcript: <full path to the .jsonl transcript file> (omit if transcript injection was empty)
working directory: <pwd>
worktree: <worktree path if applicable, otherwise omit>
pr: <PR number, title, and URL if applicable, otherwise omit>
beads: <IDs of beads created, closed, edited, or worked on during this session; omit only if none>
```

Then include these sections:
1. **User Request**: The original prompt or goal that initiated the session — one or two sentences verbatim or paraphrased
2. **Session Overview**: Brief summary of what was accomplished
3. **Sequence of Events**: Chronological breakdown of major activities (no timestamps — order only)
4. **Key Findings**: Important discoveries with file paths and line numbers where relevant
5. **Technical Decisions**: Reasoning behind implementation choices
6. **Files Changed**: List every file created, modified, renamed, or deleted. Prefer a table with `status | path | previous path | purpose | evidence`, where `status` is `created`, `modified`, `renamed`, or `deleted`
7. **Beads Activity**: List every bead created, closed, edited, claimed, assigned, commented on, or otherwise worked during the session. Include bead ID, title, action(s), final status, and why it mattered. Use the injected `Beads recent issues`, `Beads recent interactions`, transcript, and command output; do not omit a bead just because it is already closed
8. **Repository Maintenance**: Summarize completed-plan moves, bead updates, worktree/branch cleanup, stale-docs updates, no-ops, skipped items, blocked items, and the evidence behind each decision
9. **Tools and Skills Used**: List every tool category used during the session: shell commands, file tools, MCP servers/tools, skills, plugins, subagents/agents, browser tools, and external CLIs. Include the purpose for each category and any issues encountered, including failures, degraded behavior, missing permissions, bad outputs, retries, and workarounds. If only shell/file reads were used, state that explicitly and list whether any issues were observed
10. **Commands Executed**: Critical bash commands and their results
11. **Errors Encountered**: What failed, root cause, and how it was resolved — omit if no errors occurred
12. **Behavior Changes (Before/After)**: User-visible or system-visible behavior changes caused by this session
13. **Verification Evidence**: Table with `command | expected | actual | status` — omit if no verification commands were run
14. **Risks and Rollback**: Concise risk notes and rollback path for non-trivial changes — omit if no risk
15. **Decisions Not Taken**: Alternatives considered but rejected, with brief rationale — omit if none
16. **References**: Docs, PRs, issues, or URLs consulted during the session — omit if none
17. **Open Questions**: Unresolved items or assumptions that need follow-up — omit if none
18. **Next Steps**: Clear, actionable guidance for how to proceed. Distinguish unfinished work from this session, follow-on tasks not yet started, blocked tasks, and recommended immediate next commands

After writing the file, print the final absolute path.

## Session File Commit and Push

Immediately after writing the session artifact, stage, commit, and push **only that generated file**. This is part of the `save-to-md` contract, not a caller responsibility.

Rules:
- Resolve the final artifact path to one absolute path under the repo root and store it as the session artifact path.
- Stage only that path with `git add -f -- <session-artifact-path>`. Use `-f` because `docs/sessions/` is commonly ignored.
- Commit only that path with `git commit -m "docs: save session log" --only -- <session-artifact-path>`. This path-limited commit is mandatory so pre-existing staged or dirty files are not included.
- If the current branch has no upstream, push with `git push -u origin HEAD`; otherwise use `git push`.
- Do not run `git add .`, `git add -A`, broad pathspecs, or any command that stages or commits non-session files.
- If `git commit -m "docs: save session log" --only -- <session-artifact-path>` reports there is nothing to commit because the generated content is unchanged, do not create an empty commit. Report that no session-file commit was needed, then continue to push only if the branch is ahead of its upstream.
- If the target is outside a git repository, write the file and report that the commit/push step was skipped because no repo was available.
- If push fails, diagnose and retry after non-destructive fixes. Do not use force push.
- After the push, verify the committed file set with `git show --name-only --format= --stat HEAD` or `git diff-tree --no-commit-id --name-only -r HEAD`. The only path in the session-file commit must be the generated artifact path. If any other path appears, report it as a workflow failure.

Content quality rules:
- Facts only. Do not infer values that were not observed in tool/command output.
- If something is uncertain, place it in **Open Questions** instead of stating it as fact.
- Treat Beads activity as mandatory session context. If no bead activity occurred, state `No bead activity observed`; otherwise list every observed bead action even if it seems administrative.
- Treat repository maintenance as mandatory session closeout context. If no maintenance action was needed or safe, state that explicitly and give the observed evidence.
- Do not present cleanup as complete unless plan files, beads, worktrees/branches, and stale docs were all checked or explicitly marked out of scope with a reason.
- Keep sections concise (target max 5 bullets per section), but exceed when needed to preserve material implementation details, critical evidence, or safety context.
- Use file:line references (e.g., `server.ts:45`) for code-specific findings.

## HTML rendering (when output is `.html`)

When the resolved output format is HTML, render an Aurora-styled artifact using the template at `references/html-template.html` (sibling to this SKILL.md). The template lives in `references/` per progressive-disclosure — do not inline it here.

Workflow:

1. **Load the template.** Read `references/html-template.html` from the skill directory.
2. **Fill every `{{TOKEN}}` placeholder.** Token list and rendering rules below.
3. **Drop empty sections.** For sections marked "omit if none" in the markdown structure above (Errors, Verification, Risks, Decisions Not Taken, References, Open Questions), if there is no content, delete the entire `<section class="panel" id="…">…</section>` block AND the matching `<a href="#…">` entry in the sticky ToC `<nav>`. Do not render an empty panel.
4. **Sentence-case copy.** Match the Aurora voice rule — no marketing tone, no emoji anywhere.
5. **Write the file**, then print the final absolute path (same contract as the markdown path).

### Token reference

Metadata / hero:

| Token | Source |
|---|---|
| `{{TITLE}}` | Short human title (sentence case, no trailing period). Reuse the leading `#` heading you'd write in markdown |
| `{{DATE}}` | Injected `Date` value |
| `{{REPO_URL}}` | Injected `Repo` value |
| `{{BRANCH}}` | Injected `Branch` value |
| `{{HEAD_SHA}}` | Injected `HEAD` |
| `{{WORKING_DIR}}` | Injected `Working directory` |
| `{{WORKTREE}}` | Injected `Worktree` (or `—` if absent) |
| `{{PLAN_PATH}}` | Injected `Active plan` (or `—` if `none`) |
| `{{PR}}` | Injected `Active PR` rendered as `#NN title (URL)` (or `—`) |
| `{{SESSION_ID}}` | UUID from `Transcript` filename, or omit the entire `.item` block if transcript is empty |
| `{{TRANSCRIPT_PATH}}` | Injected `Transcript` (or omit the `.item`) |
| `{{BEADS_IDS}}` | Comma-separated bead IDs touched this session, or `none` |
| `{{HERO_SUMMARY_HTML}}` | One- or two-sentence elevator pitch of the session. Wrap key nouns in `<strong>` |

At-a-glance stat tiles — each is a small integer plus a one-line subtitle. Use `0` and a meaningful sub if a value is genuinely zero. Drop the whole `.stat` block only if the metric is meaningless for this session (e.g. no PR-related work → keep Files/Commits stats; but if there were zero commits made *by* this session, render `0` not omit):

| Token | Suggested value |
|---|---|
| `{{STAT_FILES}}` / `{{STAT_FILES_SUB}}` | Files changed count / `N created · N modified` breakdown |
| `{{STAT_COMMITS}}` / `{{STAT_COMMITS_SUB}}` | Commits made this session / commit SHAs (`abc123, def456`) |
| `{{STAT_COMMANDS}}` / `{{STAT_COMMANDS_SUB}}` | Commands executed count / short label (e.g. `git/build/tests`) |
| `{{STAT_BEADS}}` / `{{STAT_BEADS_SUB}}` | Bead actions count / `N created · N closed` |
| `{{STAT_VERIFY}}` / `{{STAT_VERIFY_SUB}}` | Verification rows count / `N pass · N fail` |
| `{{STAT_ERRORS}}` / `{{STAT_ERRORS_SUB}}` | Error count / `recoverable` / `blocking` etc. |

Body sections — each maps to the same-named markdown section. Render content as semantic HTML (`<p>`, `<ul>`, `<table>`, `<code>`, `<strong>`):

| Token | Renders as |
|---|---|
| `{{USER_REQUEST}}` | Plain text inside the existing `<blockquote class="request-quote"><p>…</p></blockquote>` |
| `{{SESSION_OVERVIEW_HTML}}` | One or more `<p>` paragraphs |
| `{{SEQUENCE_OF_EVENTS_LIST}}` | `<li>` items inside the existing `<ol class="timeline">`. Lead each item with `<strong>Step title.</strong>` then the detail |
| `{{KEY_FINDINGS_LIST}}` | `<li>` items |
| `{{TECHNICAL_DECISIONS_LIST}}` | `<li>` items |
| `{{FILES_CHANGED_ROWS}}` | `<tr>` rows. Status cell uses `<span class="badge created"><svg><use href="#i-plus"/></svg>created</span>`. Status→icon map: `created`→`#i-plus`, `modified`→`#i-pencil`, `renamed`→`#i-arrows-right-left`, `deleted`→`#i-trash`. Previous-path cell shows `—` when absent. Path and evidence wrap in `<code>` |
| `{{BEADS_ACTIVITY_HTML}}` | Either a `<div class="table-wrap"><table>…</table></div>` listing bead actions (id, title, action, status, why), or `<p>No bead activity observed.</p>` |
| `{{REPO_MAINTENANCE_HTML}}` | Free-form `<p>` / `<ul>` blocks for plans, beads, worktrees/branches, stale docs, transparency. Use `<h3 class="sub">` for the sub-headings |
| `{{TOOLS_AND_SKILLS_LIST}}` | `<li>` items, one per tool category. Lead with `<strong>Category.</strong>` |
| `{{COMMANDS_EXECUTED_ROWS}}` | `<tr><td><code>cmd</code></td><td>result</td></tr>` rows |
| `{{ERRORS_ENCOUNTERED_LIST}}` | `<li>` items (omit whole section if none) |
| `{{BEHAVIOR_CHANGES_ROWS}}` | `<tr><td>area</td><td>before</td><td>after</td></tr>` rows |
| `{{VERIFICATION_ROWS}}` | `<tr>` rows. Status cell uses `pass`/`fail`/`warn` badge with matching icon: `<span class="badge pass"><svg><use href="#i-check-circle"/></svg>pass</span>` (omit whole section if none) |
| `{{RISKS_AND_ROLLBACK_HTML}}` | `<p>` / `<ul>` blocks (omit whole section if none) |
| `{{DECISIONS_NOT_TAKEN_LIST}}` | `<li>` items (omit whole section if none) |
| `{{REFERENCES_LIST}}` | `<li>` items with `<a href="…">` links where applicable (omit whole section if none) |
| `{{OPEN_QUESTIONS_LIST}}` | `<li>` items (omit whole section if none) |
| `{{NEXT_STEPS_HTML}}` | `<p>` / `<ul>` / `<ol>` blocks |

### HTML rendering rules

- **Escape user content.** Treat any text drawn from commands, transcripts, file paths, or commit messages as untrusted: escape `&`, `<`, `>`, `"`, `'` before inserting into the template. Quoted code samples go inside `<pre><code>…</code></pre>` after escaping.
- **No external scripts or assets beyond what the template already declares** (Google Fonts CDN + inline SVG icon library). Keep the artifact openable without a server.
- **Use the bundled icon set only.** All icon ids start with `i-` and are defined in the template's `<defs>`. Do not invent new ids — pick the closest existing icon.
- **Do not modify the embedded CSS.** Token values track Aurora's source; if a token is missing, add a new value to the template body, not to the inlined output.
- **No emoji** in any rendered text, badge, or heading. Lucide icons only.
- **Sentence case** for section headers (the template already enforces this for the canonical 18 sections; do not retitle them).
