# screens

A Claude Code skill that lets the agent see what's on your desktop when you're working over SSH and can't paste images into the CLI.

## What it does

Three ways to put a PNG of your desktop in front of Claude:

| Mode | Trigger phrases | What you get |
|------|-----------------|--------------|
| **Fetch existing** | "check my screen", "look at the screenshot", "latest screen" | The newest PNG from your screenshots folder. Pairs with Win+Shift+S → drag → autosave. |
| **Fresh capture** | "take a screenshot", "capture my screen" | Live grab of your primary monitor via PowerShell + .NET. Captures whatever's on top right now. |
| **Chrome tab** | "grab my chrome tab", "screenshot the X page", "show me chrome" | Any tab in your debug Chrome — works even if the window is minimized or behind others. Uses Chrome DevTools Protocol. |

## How it works

You're SSH'd from Windows into a remote Linux box running Claude. Claude can't see your Windows screen. This skill solves that by `ssh`ing back to your Windows machine (via WSL), grabbing the image you want, and streaming it to a local temp file that Claude can `Read`.

```
[Win11 desktop]                  [Remote Linux + Claude]
  Chrome / windows                       │
  C:\screens\*.png                       │
       ↑                                 │
       │                                 │
  steamy-wsl (WSL Ubuntu)  ─── ssh ────  │
       │                                 │
       └── powershell.exe ───────────────┘
                                  Claude `Read`s the PNG
```

## Prerequisites

- Passwordless SSH from the Claude host to your Windows-side WSL (`ssh steamy-wsl` should just work).
- A screenshots folder on the Windows side — defaults to `C:\screens`. Win+Shift+S autosave or any tool that lands PNGs there will work for Mode 1.
- For Mode 3 (Chrome tab capture): a Chrome instance launched with `--remote-debugging-port=9222`. There's a "Chrome (debug)" shortcut on the desktop that handles this — double-click it to start.

## Setup notes

**SSH alias** — `~/.ssh/config` on the Linux side has an entry like:
```
Host steamy-wsl
    HostName <win11-ip-or-tailscale-name>
    User <wsl-username>
    IdentityFile ~/.ssh/...
```

**Chrome debug shortcut** — already on the Win11 desktop. Target:
```
"C:\Program Files\Google\Chrome\Application\chrome.exe" --remote-debugging-port=9222 --user-data-dir=C:\chrome-debug
```
It runs alongside your normal Chrome (separate profile) so no quit-everything dance.

**`.zshenv` on the WSL host** — has `[ -f ... ] && . ...` guards on `~/.cargo/env` and `~/.rover/env` so SSH sessions don't spam errors. Original backed up to `~/.zshenv.bak`.

## Pointing at a different machine

Set the `SCREENS_*` env vars. The reliable place is `~/.claude/settings.json` (Claude Code injects this into every session it spawns — plain interactive-shell `export` doesn't reach Claude's Bash tool):

```json
{
  "env": {
    "SCREENS_HOST": "workbox",
    "SCREENS_REMOTE_DIR": "~/Pictures/Screenshots",
    "SCREENS_NATIVE_DIR": "",
    "SCREENS_POWERSHELL": "",
    "SCREENS_CHROME_PORT": "9223"
  }
}
```

Restart Claude (or start a new session) for the change to take effect. For per-project overrides use `<project>/.claude/settings.json`; for personal/uncommitted use `.claude/settings.local.json`.

If the target isn't Windows, only Mode 1 (fetch existing) works out of the box.

## Files

```
screens/
├── SKILL.md              Agent-facing instructions for all three modes
├── README.md             This file
└── scripts/
    └── cdp-shot.ps1      PowerShell: connects to Chrome WebSocket, captures a tab
```

## Limitations

- Mode 2 captures the **primary monitor only** and **whatever's on top** — focus the thing you want first.
- Mode 3 needs Chrome started with the debug flag; opening the page in your *normal* Chrome won't work.
- No region/window grab mode. Install NirCmd or ShareX and add a mode if you want one.
- No video. ShareX records video but Claude can't watch MP4 directly — you'd need an ffmpeg-to-frames step.

## Troubleshooting

| Symptom | Check |
|---------|-------|
| "no screenshots found" | `ssh steamy-wsl ls /mnt/c/screens` — is the dir populated? Is `$SCREENS_REMOTE_DIR` pointing at the right place? |
| Mode 3 fails with connection error | Debug Chrome isn't running. Double-click "Chrome (debug)" on the Win11 desktop. |
| Mode 3 captures the wrong tab | Pass a more specific `PATTERN` (title or URL substring). |
| `ssh steamy-wsl` itself errors | Test with `ssh steamy-wsl true`. Likely an SSH config / network issue, not the skill. |
| Captured PNG is all black | Hardware-accelerated app — known PowerShell `CopyFromScreen` limitation. Use Mode 3 if it's Chrome; otherwise needs NirCmd or `PrintWindow` + `PW_RENDERFULLCONTENT`. |
