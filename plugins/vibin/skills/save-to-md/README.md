# save-to-md

Save the current Claude session as a markdown file under `docs/sessions/`, pre-injected with full context: date, repo, branch, HEAD, recent commits, dirty files, transcript path, active PR, worktree, recent Beads state, registered worktrees, branches, and plan files. After creating the artifact, `save-to-md` stages, commits, and pushes only that generated session file.

## What it does

1. Resolves a target path (`$ARGUMENTS` or auto: `docs/sessions/YYYY-MM-DD-description.md`).
2. Refuses to overwrite — appends `-v2`, `-v3` suffixes on conflict.
3. If the injected transcript path is set, reads the raw `.jsonl` to recover the full session (the live context window may be truncated).
4. Performs a repository maintenance pass: moves completed plans to `docs/plans/complete/`, updates relevant beads, safely cleans stale worktrees/branches, and checks for stale docs.
5. Writes a metadata block + numbered sections: what was done, files changed, bead activity, repository maintenance, tools used, errors encountered, next steps, open questions, verification evidence.
6. Stages only the generated artifact with `git add -f -- <session-file>`.
7. Commits only that path with `git commit -m "docs: save session log" --only -- <session-file>` so unrelated staged or dirty files are excluded.
8. Pushes the current branch and verifies the session-file commit contains no other paths.
9. Facts-only rule — no speculation; ambiguity goes into Open Questions.

The generated session note must list every bead created, closed, edited, claimed, assigned, commented on, or otherwise worked during the session. If no bead activity occurred, it says so explicitly.

The generated session note must also document every repo maintenance action, no-op, skipped item, blocked item, and cleanup decision with evidence. Cleanup is only complete when completed plans, beads, worktrees/branches, and stale docs were checked or explicitly marked out of scope with a reason.

Pairs with `hand-off` — this writes the log; `hand-off` reads it back into a fresh session.

## HTML output

Pass `--html` as the first argument (or a path ending in `.html`) to render an Aurora-styled HTML artifact instead of markdown. The artifact uses the Aurora design tokens (dark navy with cyan/violet accents, Manrope + Inter + JetBrains Mono), a sticky table-of-contents sidebar with Lucide icons, an at-a-glance stat row, Tier 2 panels with section icons, collapsible command transcript, semantic status badges, and a print stylesheet. It's a single self-contained file (Google Fonts CDN only) that opens directly in a browser.

Markdown remains the default; the HTML mode is opt-in.

## Invoke

Triggers: "save session", "save to md", "document this session", "write up what we did", "save session notes". Add `--html` or a `.html` path for the rich artifact.

## Files

- `SKILL.md` — agent instructions
- `references/html-template.html` — Aurora-styled HTML template, used when output is `.html`
