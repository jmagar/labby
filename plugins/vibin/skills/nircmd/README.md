# nircmd

Drive a Windows machine over SSH via the NirCmd CLI and its NirSoft companion utilities. **Killer use case:** push text/files to your Windows clipboard from any Claude session so you can `Ctrl+V` long commands, URLs, snippets, etc. anywhere. Plus granular screen capture, audio control, window management, lock, TTS, system dialogs — and a curated set of scriptable NirSoft companions (CurrPorts, LastActivityView, OpenedFilesView, SearchMyFiles, …) for inspecting Windows state from the shell.

Built on [NirSoft NirCmd](https://www.nirsoft.net/utils/nircmd.html) (~120KB binary, 115 commands) plus selected [NirSoft utilities](https://www.nirsoft.net/utils/) (200+ tools, available as a bundle via [NirLauncher](https://launcher.nirsoft.net/)).

## What it does

| Capability | One-line example |
|------------|------------------|
| Push text to your Win clipboard | `clip.sh "the curl command I want you to run"` |
| Push a file to your clipboard | `clip.sh < generated.sh` |
| Pull current clipboard back as a file | `clip-grab.sh` (auto-detects text vs image) |
| Screenshot a specific window | `win-shot.sh "Visual Studio Code"` |
| Lock the workstation | `lock.sh` |
| Audio (volume/mute/per-app) | `nircmd setsysvolume 32768` |
| TTS — Windows speaks | `nircmd speak text "deploy finished"` |
| Window control (focus/min/max/close/pin) | `nircmd win activate ititle "Chrome"` |
| Region capture by coords | `nircmd savescreenshot out.png 0 0 1920 1080` |
| Multi-monitor stitched capture | `nircmd savescreenshotfull out.png` |
| Tray notification | `nircmd trayballoon "Title" "Body" "icon" 5000` |
| What's holding a port | `cports.exe /scomma C:\Temp\ports.csv` |
| Recent activity on the box | `LastActivityView.exe /scomma C:\Temp\activity.csv` |
| Which process holds this file | `OpenedFilesView.exe /filefilter "C:\path" /scomma out.csv` |
| Nearby Wi-Fi APs / signal | `WirelessNetView.exe /scomma C:\Temp\aps.csv` |
| Saved file search | `SearchMyFiles.exe /cfg saved.cfg /scomma out.csv` |

## How it works

```
[remote Linux + Claude]                       [Win11 desktop]
       │
       │  ssh steamy-wsl  ─────────────────►  WSL Ubuntu
       │                                           │
       │                                           │  shells out to
       │                                           ▼
       │                              /mnt/c/tools/nircmd/nircmd.exe
       │                                           │
       │                                           ▼
       │                              Windows clipboard / windows /
       │                                  audio / screen / etc.
       ▼
   `clip.sh` / `win-shot.sh` / etc. wrappers
```

## Prerequisites

- Passwordless SSH from the Claude host to your Windows-side WSL (`ssh steamy-wsl`).
- NirCmd installed on Windows at `C:\tools\nircmd\nircmd.exe`. To install:
  ```powershell
  Invoke-WebRequest 'https://www.nirsoft.net/utils/nircmd-x64.zip' -OutFile $env:TEMP\nircmd.zip
  Expand-Archive $env:TEMP\nircmd.zip -DestinationPath C:\tools\nircmd -Force
  ```
- **Optional but recommended** — the NirSoft companion bundle at `C:\tools\nirsoft\` (or via NirLauncher). One-shot install of all 200+ tools:
  ```powershell
  Invoke-WebRequest 'https://launcher.nirsoft.net/downloads/nirlauncher.zip' -OutFile $env:TEMP\nl.zip
  Expand-Archive $env:TEMP\nl.zip -DestinationPath C:\tools\NirLauncher -Force
  # then set NIRSOFT_DIR to /mnt/c/tools/NirLauncher/NirSoft
  ```
  Or grab individual tools from `https://www.nirsoft.net/utils/<tool>.html` into `C:\tools\nirsoft\`.

## Pointing at a different machine

```json
{
  "env": {
    "NIRCMD_HOST": "workbox",
    "NIRCMD_PATH": "/mnt/c/tools/nircmd/nircmd.exe",
    "NIRSOFT_DIR":  "/mnt/c/tools/nirsoft"
  }
}
```

in `~/.claude/settings.json` (same env-injection pattern as the `screens` skill — Claude's Bash tool doesn't pick up interactive-shell `export`s).

## Charset gotcha

NirCmd args go through Windows' ANSI codepage. Em-dashes, smart quotes, emoji, non-Latin scripts get mangled (`—` → `-`, `'` → `?`). The `clip.sh` wrapper auto-detects non-ASCII and falls back to a UTF-8 temp file + `clipboard readfile`. For ad-hoc NirCmd calls with non-ASCII args, do the same — see `references/clipboard.md`.

## Safety

NirCmd can do destructive things. The skill enforces three tiers (see `references/safety-boundaries.md` for the full classification):

- **Auto-allowed**: clipboard, screen capture, audio, lock, speak, window listing/activation, dialogs, beep.
- **Ask first**: killprocess / closeprocess, runas / elevate, registry writes, service control, file delete.
- **Refuse without extremely explicit instruction**: shutdown / reboot / logoff / standby / hibernate.

## Files

```
nircmd/
├── SKILL.md                                Agent instructions
├── README.md                               This file
├── CHANGELOG.md
├── scripts/
│   ├── clip.sh                             Push text to clipboard (UTF-8 safe)
│   ├── clip-grab.sh                        Pull clipboard back as file (text or image)
│   ├── win-shot.sh                         Activate window by title, then capture
│   └── lock.sh                             Lock the workstation
└── references/
    ├── command-reference.md                All 115 NirCmd commands, categorized
    ├── clipboard.md                        Clipboard patterns in detail
    ├── window-control.md                   Window matching and manipulation
    ├── safety-boundaries.md                Auto / ask / refuse classification + rationale
    └── nirsoft-tools.md                    NirSoft companion CLIs (CurrPorts, LastActivityView, etc.)
```
