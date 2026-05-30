# gh-pr

Systematic workflow for addressing GitHub PR review comments with mandatory resolution tracking. Threads become beads (via `bd`) so they show up alongside other ready work.

## What it does

1. Fetches all review comments + threads on the PR for the current branch.
2. (Optionally) triages them with an AI pass that scores actionability.
3. Creates one bead per unresolved thread; presents a grouped summary.
4. As you commit fixes, links commits to the right thread and closes the bead.
5. Blocks "done" until every actionable thread is resolved or explicitly dismissed.

## Invoke

Triggers: "address PR comments", "handle PR review", "work through review feedback", "resolve PR threads". Also invoked proactively whenever the user is acting on PR feedback.

## Prerequisites

- `gh` authenticated (`gh auth status`)
- `bd` installed (the bead tracker)
- Optional: `claude` on PATH for AI triage
- Optional: `gh-webhook` running locally for live notifications (see `references/webhook-setup.md`)

## Files

- `SKILL.md` — agent workflow
- `scripts/` — 13 Python helpers (fetch, triage, render, resolve, etc.); invoke from repo root with `SCRIPTS=skills/gh-pr/scripts`
- `references/` — workflow patterns, troubleshooting, webhook setup, quick reference
- `agents/openai.yaml` — OpenAI runtime metadata
- `assets/` — icons
