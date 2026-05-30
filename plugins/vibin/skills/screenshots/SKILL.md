---
name: screenshots
description: Bridge for viewing the user's own desktop when they're SSH'd in and can't paste images. Three modes — (1) fetch an existing screenshot from the user's screenshots dir, (2) take a fresh full-screen / region / active-window capture via ShareX CLI on a Windows target, (3) fall back to a PowerShell-only capture if ShareX isn't installed. Use when the user says "check my screenshot", "look at my screen", "latest screen", "take a screenshot of my desktop", "capture my screen", "show me my desktop", "screenshot this window", "grab a region of my screen", or similar references to seeing their own machine. Defaults target the user's Win11 box via `ssh steamy-wsl`; override `SCREENS_HOST` (and friends) if pointing at a different host. **For Chrome tab screenshots (any window state, including minimized), use the `chrome` skill instead — this skill is for desktop pixels.**
---

# screenshots

The user develops over SSH; their desktop is on another machine. This skill turns whatever's on that machine into a PNG and pipes it back so it can be `Read` inline. Primary tool is **ShareX** via its CLI; PowerShell fallback covers boxes without ShareX.

## Defaults (override via env vars)

```bash
SSH_TARGET="${SCREENS_HOST:-steamy-wsl}"                                            # ssh alias of the desktop machine
REMOTE_DIR="${SCREENS_REMOTE_DIR:-/mnt/c/screens}"                                  # screenshots dir, POSIX path on the SSH host
NATIVE_DIR="${SCREENS_NATIVE_DIR:-C:\\screens}"                                     # same dir in native OS form (Windows only)
POWERSHELL="${SCREENS_POWERSHELL:-/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe}"
SHAREX="${SCREENS_SHAREX:-/mnt/c/Program Files/ShareX/ShareX.exe}"                  # ShareX CLI path
SKILL_DIR=/home/jmagar/.agents/src/skills/screenshots
```

Variable names match the prior `ss` skill so existing `~/.claude/settings.json` entries keep working. Paste this block at the top of any snippet. If the user mentions a different machine ("check my mac", "look at the work laptop"), set the matching `SCREENS_*` env vars for the session.

## Universal patterns

- **Fetching any file**: `ssh "$SSH_TARGET" "cat \"$path\"" > "$dest"` — never use scp. Filenames have spaces; cat-over-ssh sidesteps the quoting hell.
- **Local temp path**: `dest="${CLAUDE_JOB_DIR:-/tmp}/<name>"`. Auto-cleaned.
- **PowerShell output from bash**: PS emits `\r\n` line endings. Always `2>/dev/null` and `tr -d '\r\n'` when capturing into a bash var.
- **Path translation**: `$REMOTE_DIR/foo.png` (POSIX) ↔ `$NATIVE_DIR\foo.png` (Windows). Scripts that run on Windows want native; bash fetches want POSIX.

## Mode 1 — fetch the latest screenshot

Triggers: "check my screen", "look at the screenshot", "latest screen".

```bash
dest="${CLAUDE_JOB_DIR:-/tmp}/screen-$$.png"
latest=$(ssh "$SSH_TARGET" "ls -t $REMOTE_DIR/*.png 2>/dev/null | head -1")
[ -z "$latest" ] && { echo "no screenshots found"; exit 1; }
ssh "$SSH_TARGET" "cat \"$latest\"" > "$dest"
echo "$dest"
```

List the 10 most recent and let the user pick: `ssh "$SSH_TARGET" "ls -lt $REMOTE_DIR/*.png | head -10"`.

## Mode 2 — fresh capture via ShareX (preferred)

ShareX has a clean CLI for unattended captures and saves directly to a folder you control. Triggers: "take a screenshot", "capture my screen now", "snapshot the desktop", "grab a region", "screenshot this window".

ShareX CLI flags relevant here:
- `-FullScreen` — capture all monitors stitched together
- `-Screen` — capture the primary monitor only
- `-ActiveWindow` — capture only the foreground window (no terminal in the shot — focus the thing first)
- `-RectangleRegion` — interactive region picker (needs user input; rarely useful unattended)
- `-AutoCapture` — start the auto-capture flow (don't use unattended)
- `-CustomUploader` — bypass uploaders, just save locally
- `-silent` — no toasts/UI

Capture pattern — ShareX writes to its configured Screenshots folder; ask it to print the path:

```bash
shot() {
  local mode="${1:-Screen}"   # Screen | ActiveWindow | FullScreen
  ssh "$SSH_TARGET" "'$SHAREX' -$mode -silent" 2>/dev/null
  # ShareX writes to %USERPROFILE%\Documents\ShareX\Screenshots\<yyyy-MM>\... by default;
  # picking the newest PNG anywhere under that tree is the most robust fetch.
  local latest_win=$(ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -Command \"
    \\\$d = Join-Path \\\$env:USERPROFILE 'Documents\\ShareX\\Screenshots';
    Get-ChildItem -Path \\\$d -Filter *.png -Recurse |
      Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
  \"" 2>/dev/null | tr -d '\r\n')
  [ -z "$latest_win" ] && { echo 'no shot found'; return 1; }
  local latest_posix=$(echo "$latest_win" | sed 's|\\|/|g; s|^C:|/mnt/c|')
  local dest="${CLAUDE_JOB_DIR:-/tmp}/$(basename "$latest_posix")"
  ssh "$SSH_TARGET" "cat \"$latest_posix\"" > "$dest"
  echo "$dest"
}

shot Screen           # primary monitor
shot ActiveWindow     # just the foreground window (warn user to focus what they want)
shot FullScreen       # all monitors
```

**Configuring ShareX for unattended use** (run once on a fresh install):
1. Open ShareX once via noVNC / desktop (`agent-os` skill helps for the sandbox host).
2. Settings → Image → After capture tasks: disable "Copy image to clipboard" (optional but cleaner).
3. Settings → Paths → Custom Screenshots folder path: set to whatever's in `$REMOTE_DIR` if you want everything in one place — otherwise the default `Documents\ShareX\Screenshots\<yyyy-MM>` is fine.
4. Settings → Advanced → DisableHotkeys: true (don't fight the user's keybinds).

ShareX is silent on success and noisy on failure; check exit code if a shot doesn't appear.

## Mode 3 — PowerShell fallback (no ShareX needed)

For hosts where installing ShareX is more trouble than it's worth.

```bash
name=$(ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -Command \"
  Add-Type -AssemblyName System.Windows.Forms,System.Drawing;
  \\\$b = New-Object Drawing.Bitmap([Windows.Forms.Screen]::PrimaryScreen.Bounds.Width, [Windows.Forms.Screen]::PrimaryScreen.Bounds.Height);
  [Drawing.Graphics]::FromImage(\\\$b).CopyFromScreen(0,0,0,0,\\\$b.Size);
  \\\$n = 'shot-' + (Get-Date -f yyyyMMdd-HHmmss) + '.png';
  \\\$b.Save('$NATIVE_DIR\\' + \\\$n);
  [Console]::Out.Write(\\\$n)
\"" 2>/dev/null | tr -d '\r\n')
dest="${CLAUDE_JOB_DIR:-/tmp}/$name"
ssh "$SSH_TARGET" "cat \"$REMOTE_DIR/$name\"" > "$dest"
echo "$dest"
```

Captures **primary monitor only**, **current contents** — including the terminal if it's on top. Warn the user to focus what they want first. For all-monitors: swap `PrimaryScreen.Bounds` for the union of `[Windows.Forms.Screen]::AllScreens.Bounds`.

## Chrome tabs

This skill does **not** screenshot Chrome tabs anymore — that moved to the `chrome` skill which uses CDP and works even when the tab is minimized or behind other windows. Use:

```bash
# (see chrome skill for setup) — one-liner equivalent of "grab the github tab"
# Stage cdp-shot.ps1 once, then capture by tab title/URL substring — see chrome skill for full setup.
/home/jmagar/.agents/src/skills/chrome/scripts/cdp-shot.ps1 -Pattern 'github.com' ...
```

## Adapting to another machine

**Best path — persist via `~/.claude/settings.json`.** Claude Code injects the `env` block into every session it spawns, so these reach the Bash tool reliably:

```json
{
  "env": {
    "SCREENS_HOST": "workbox",
    "SCREENS_REMOTE_DIR": "~/Pictures/Screenshots",
    "SCREENS_NATIVE_DIR": "",
    "SCREENS_POWERSHELL": "",
    "SCREENS_SHAREX": ""
  }
}
```

Restart Claude Code (or start a new session) for the change to take effect. Per-project overrides go in `<project>/.claude/settings.json` (or `.claude/settings.local.json` for personal/uncommitted ones).

**One-shot override** — set inline before the snippet:

```bash
SCREENS_HOST=workbox SCREENS_REMOTE_DIR=~/Pictures/Screenshots <paste the snippet>
```

For a **macOS** target, Mode 1 works as-is. For fresh captures use `screencapture` instead of ShareX/PS — a future mode if it comes up.

## Notes

- If ssh itself fails, sanity-check with `ssh "$SSH_TARGET" true`.
- ShareX has region/window/scrolling/timed/text-OCR modes that aren't wired into the shot() helper. Add a branch in the helper if a session keeps asking for one specific mode.
- ShareX vs PS Mode 3 vs NirCmd's `savescreenshot`: ShareX is the most flexible (modes, post-processing, OCR), PS works without any third-party install, NirCmd is the lightest but can only do full-screen. The `nircmd` skill covers NirCmd if needed.
- For multi-monitor with PS Mode 3: `[Windows.Forms.Screen]::AllScreens | ForEach-Object { ... }` — see PS docs.
