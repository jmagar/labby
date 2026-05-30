# clipboard

Push text to / pull text from Jacob's Windows clipboard over SSH.

## What it does
- `scripts/clip.sh "<text>"` or `scripts/clip.sh -` (stdin) — pushes to clipboard
- Auto-routes ASCII single-line through NirCmd (fast), everything else through PowerShell `Set-Clipboard` via UTF-8 temp file (lossless Unicode)
- Falls back to PowerShell when NirCmd isn't installed
- Hardened against shell injection in the NirCmd branch (any `$`, backtick, quote, or backslash forces the temp-file path)

## When to invoke
"copy this to my clipboard", "push X to my clipboard", "what's on my clipboard", "read my clipboard". Always targets `steamy-wsl` regardless of which host this Claude session is running on.

## Why it matters
You can't paste images *into* this Claude session over SSH, but you can shove any text out onto the user's clipboard for them to Ctrl+V wherever.

## Files
- `SKILL.md` — entry point, defaults, transport routing
- `scripts/clip.sh` — the push wrapper

## Companion skills
- `nircmd` — everything else NirCmd does (audio, TTS, windows, NirSoft tools)
- `screenshots` — desktop screenshots
