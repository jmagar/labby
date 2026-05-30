---
name: clipboard
description: Use when the user wants to push text to or read text from their Windows clipboard over SSH. The killer use case is "copy this to my clipboard" — the agent can't paste images into chat over SSH, but it can shove URLs, commands, code snippets, multi-line text, Unicode, or anything else onto the user's Windows clipboard for Ctrl+V wherever they want it. Triggers include "copy this to my clipboard", "put X on my clipboard", "push this to clipboard", "set my clipboard to", "what's on my clipboard", "read my clipboard", "clipboard contents". Always targets `ssh steamy-wsl` (Jacob's primary Win11 desktop, where he works 99% of the time) — invoke this skill regardless of what host this Claude session is running on. Auto-routes between NirCmd (fast ASCII) and PowerShell `Set-Clipboard` via a UTF-8 temp file (lossless Unicode); degrades gracefully when NirCmd is absent.
---

# clipboard

Push text to / pull text from a remote Windows clipboard over SSH. Two transport paths, selected automatically:

- **ASCII + no newlines + NirCmd available** → `nircmd clipboard set` (one-shot, fastest)
- **Anything Unicode, multi-line, or no NirCmd** → PowerShell `Set-Clipboard` via a temp file (lossless, no extra binary needed)

Reads always go through PowerShell `Get-Clipboard` (NirCmd's reader is ANSI-only).

## Defaults (override via env vars)

```bash
CLIPBOARD_HOST="${CLIPBOARD_HOST:-${NIRCMD_HOST:-steamy-wsl}}"
CLIPBOARD_NIRCMD="${CLIPBOARD_NIRCMD:-${NIRCMD_PATH:-/mnt/c/tools/nircmd/nircmd.exe}}"
CLIPBOARD_POWERSHELL="${CLIPBOARD_POWERSHELL:-/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe}"
CLIPBOARD_TMP_DIR="${CLIPBOARD_TMP_DIR:-/mnt/c/Users/Docker/AppData/Local/Temp}"
SKILL_DIR=/home/jmagar/.agents/src/skills/clipboard
```

The defaults fall through to `nircmd`'s env vars so a user who set `NIRCMD_HOST` for that skill doesn't have to set anything twice.


## Push — push text to clipboard

Triggers: "copy this to my clipboard", "put X on my clipboard", "send this to my clipboard", "I want to paste X".

```bash
"$SKILL_DIR/scripts/clip.sh" "the text to push"
```

Multiline / Unicode / piped:

```bash
cat some-file.md | "$SKILL_DIR/scripts/clip.sh" -
"$SKILL_DIR/scripts/clip.sh" "$(jq -r .body some.json)"
"$SKILL_DIR/scripts/clip.sh" "résumé with 🎉 émojis"
```

The wrapper auto-detects content type, picks the right transport, and reports which one it used. Always tell the user what landed on their clipboard so they know what they're about to paste.

## Pull — read clipboard contents

```bash
ssh "$CLIPBOARD_HOST" "$CLIPBOARD_POWERSHELL -NoProfile -Command 'Get-Clipboard -Raw'" 2>/dev/null
```

If you only want the first line / a single value, append `| head -1` locally. `Get-Clipboard -Raw` preserves newlines; without `-Raw` PS may collapse them.

## Save a clipboard image to PNG

Useful when the user copies a screenshot and you want to look at it.

```bash
WIN_OUT='C:\Users\Docker\AppData\Local\Temp\clip.png'
POSIX_OUT='/mnt/c/Users/Docker/AppData/Local/Temp/clip.png'
ssh "$CLIPBOARD_HOST" "$CLIPBOARD_POWERSHELL -NoProfile -Command \"Add-Type -AssemblyName System.Windows.Forms; \\\$i = [Windows.Forms.Clipboard]::GetImage(); if (\\\$i) { \\\$i.Save('$WIN_OUT'); 'ok' } else { 'no_image' }\"" 2>/dev/null | tr -d '\r\n'
ssh "$CLIPBOARD_HOST" "cat '$POSIX_OUT'" > "${CLAUDE_JOB_DIR:-/tmp}/clip.png"
```

The classic NirCmd `clipboard saveimage` works too if NirCmd is installed and the image is in standard formats:
```bash
ssh "$CLIPBOARD_HOST" "$CLIPBOARD_NIRCMD clipboard saveimage 'C:\\Users\\Docker\\AppData\\Local\\Temp\\clip.png'"
```

## Clear / replace

```bash
ssh "$CLIPBOARD_HOST" "$CLIPBOARD_POWERSHELL -NoProfile -Command 'Set-Clipboard -Value \$null'"
# or via NirCmd:
ssh "$CLIPBOARD_HOST" "$CLIPBOARD_NIRCMD clipboard clear"
```

## Why two transports

NirCmd's `clipboard set` writes via the ancient `CF_TEXT` clipboard format (ANSI / CP-1252) — emoji become `?`, CJK becomes mojibake, em-dashes survive only by luck. It's the fastest single-shot path for short ASCII.

PowerShell's `Set-Clipboard` uses `CF_UNICODETEXT` and is lossless, but pushing the text through ssh requires quoting, and inline quoting breaks on any non-trivial content. The wrapper sidesteps that by writing to a temp file first and having PS `Get-Content -Raw -Encoding UTF8` read it back.

## Adapting to another machine

`~/.claude/settings.json` is the durable place:

```json
{
  "env": {
    "CLIPBOARD_HOST": "workbox",
    "CLIPBOARD_TMP_DIR": "/mnt/c/Users/yourname/AppData/Local/Temp"
  }
}
```

One-shot:
```bash
CLIPBOARD_HOST=workbox "$SKILL_DIR/scripts/clip.sh" "hi from another machine"
```

For macOS, swap the whole pipeline for `pbcopy`:
```bash
printf '%s' "$text" | ssh "$CLIPBOARD_HOST" pbcopy
```
Not wired into `clip.sh` by default — add a branch if it comes up.

## Notes

- Pushing an empty string (`clip.sh ""`) clears the clipboard — `Set-Clipboard -Value $null` is the effective call.

- If `clip.sh` reports "pushed N bytes (unicode via powershell)" but the user pastes garbage, suspect the wrong active user / temp dir — the file path is in `CLIPBOARD_TMP_DIR` and must be writable by the SSH user.
- Companion skill `nircmd` covers everything else NirCmd does (audio, TTS, windows, keystrokes) — use it when the action isn't clipboard.
- Saving an image *from* the clipboard (Mode "Save image") needs the user to have copied an image first; you can't paste an image into the clipboard from here.
