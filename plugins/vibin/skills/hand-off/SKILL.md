---
name: hand-off
description: Load the most recent save-to-md session log into a fresh session and brief the new agent on where the prior session left off. Use at the start of a new conversation when the user says "hand off", "pick up where we left off", "resume the last session", "continue from yesterday", "load the last session log", or otherwise signals they want the prior session's context restored. Pair with save-to-md — that skill writes the log; this one reads it back in.
allowed-tools: Read, Bash, Glob
argument-hint: [path-to-session-md]
---

## Context

- Date: !`TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'`
- Repo root: !`git rev-parse --show-toplevel 2>/dev/null || echo "not a git repo"`
- Current branch: !`git branch --show-current 2>/dev/null`
- Current HEAD: !`git rev-parse --short HEAD 2>/dev/null`
- Dirty files: !`git status --short 2>/dev/null`
- Recent commits: !`git log --oneline -5 2>/dev/null`
- Active PR: !`gh pr view --json number,title,url 2>/dev/null || echo "none"`
- Working directory: !`pwd`
- Argument: $ARGUMENTS
- Latest session files: !`ls -t "$(git rev-parse --show-toplevel 2>/dev/null)/docs/sessions"/*.md 2>/dev/null | head -5`

# Hand-Off: Resume Prior Session

Your job is to load the prior session's `save-to-md` log and brief the user on where things stand, so this fresh session can pick up cleanly.

## Step 1: Locate the session file

- If `$ARGUMENTS` is non-empty, treat it as the path (resolve relative paths from the repo root).
- Otherwise, use the most recent file from `docs/sessions/` (first entry in the "Latest session files" list above).
- If no session file is found, stop and tell the user — suggest running `save-to-md` next time they want a hand-off to work.

## Step 2: Read the session file

Read the full file. Pay attention to the YAML metadata block at the top and the **Next Steps**, **Open Questions**, **Files Modified**, and **Errors Encountered** sections in particular — those are what a fresh agent needs most.

## Step 3: Detect drift between then and now

Compare the session metadata to the current git state from the Context block above. Flag any of the following plainly:

- **Branch mismatch**: session branch vs current branch
- **HEAD moved**: session HEAD SHA vs current HEAD (note how many commits ahead/behind if easy to tell)
- **Worktree mismatch**: session worktree vs current `pwd`
- **PR state changed**: session PR number vs current PR (closed? merged? new?)
- **Uncommitted work**: dirty files now that weren't in the session, or vice versa

Drift is informational, not fatal. Report it; don't try to "fix" it without asking.

## Step 4: Produce the briefing

Output a single concise briefing using exactly this structure. No preamble.

```
## Hand-off from <session filename>

**Session metadata**
- When: <date from session>
- Branch: <session branch>  (current: <current branch>)  ← flag if different
- HEAD: <session HEAD>      (current: <current HEAD>)    ← flag if different
- PR: <session PR or "none"> (current: <current PR or "none">) ← flag if different

**What the prior session set out to do**
<1-2 sentences from User Request / Session Overview>

**What got done**
<3-5 bullets from Session Overview + Files Modified — concrete, with file:line where useful>

**Where things stand right now**
<2-4 bullets synthesizing current branch/PR/dirty-file state vs session end state. Call out drift explicitly.>

**Unfinished / next up** (from session Next Steps)
- [ ] <item>
- [ ] <item>

**Open questions** (from session, if any)
- <question>

**Suggested first move**
<One sentence: the single most natural next action given Next Steps + current state. Phrase as a suggestion, not a decision.>
```

## Step 5: Hand the wheel back

End the briefing with one short question, e.g.:

> Want me to start on **<suggested first move>**, or pick a different item?

Do not start work yet — wait for the user's direction. The whole point of the hand-off is to re-establish shared context before acting.

## Quality rules

- **Facts only.** Everything in the briefing must come from the session file or the current git/PR state. Don't infer status that isn't recorded.
- **Flag drift, don't paper over it.** If the session ended on branch X but we're on Y now, say so plainly — the user may have switched intentionally, or may have forgotten.
- **Keep it scannable.** A returning user wants to re-orient in 15 seconds, not read an essay. Trim aggressively; the full session file is one Read away if they want detail.
- **Don't re-read the transcript.** The `.jsonl` transcript referenced in the session metadata is huge and already summarized in the markdown. Trust the curated log.
- **Don't take destructive action.** Switching branches, stashing, checking out commits, closing PRs — all require explicit user approval, even if the session metadata suggests it.
