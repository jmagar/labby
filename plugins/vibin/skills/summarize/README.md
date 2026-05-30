# Summarize

Use when the user wants to summarize an external URL, PDF, local file, YouTube video, podcast episode, or RSS feed using the `summarize` CLI. Trigger on "summarize this URL", "TL;DR a PDF", "summarize YouTube video", "summarize podcast", "get key takeaways from", "digest this article". Do NOT trigger when Claude already has the text in context — this skill is for fetching and summarizing external content that Claude cannot read directly.

## Usage

Invoke this skill when the user request matches the trigger conditions in `SKILL.md`. The skill body is the source of truth for workflow steps and operational constraints.

## Files

- `SKILL.md` - agent workflow and trigger guidance
- `agents/` - OpenAI runtime metadata
- `references/` - progressively loaded reference material
- `README.md` - packaging overview
- `CHANGELOG.md` - packaging change history
