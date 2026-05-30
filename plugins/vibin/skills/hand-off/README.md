# hand-off

Load the most recent `save-to-md` session log into a fresh Claude session and brief the new agent on where the prior session left off.

## What it does

1. Finds the newest `docs/sessions/*.md` in the repo (or one passed as `$ARGUMENTS`).
2. Reads the full file — Next Steps, Open Questions, Files Modified, Errors Encountered.
3. Compares the session's git/PR state to the current state and flags drift (branch mismatch, HEAD moved, PR closed, etc.).
4. Produces a short briefing so the new agent can pick up cleanly.

Pairs with `save-to-md` — that skill writes the log; this one reads it back.

## Invoke

Triggers: "hand off", "pick up where we left off", "resume the last session", "continue from yesterday", "load the last session log".

## Files

- `SKILL.md` — agent instructions and output template
