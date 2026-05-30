---
name: nircmd
description: Drive a Windows machine over SSH via the NirCmd CLI and its NirSoft companion utilities — capture specific windows or regions, control audio (volume, mute, mediaplay), TTS via `speak`, lock workstation, list/activate windows, simulate keystrokes, plus scriptable NirSoft tools for network state (CurrPorts, WifiInfoView), system forensics (LastActivityView, BrowsingHistoryView, TurnedOnTimesView), open handles (OpenedFilesView), and file search (SearchMyFiles). Use whenever the user wants to control audio/volume, make Windows speak something, lock the workstation, list open ports / Wi-Fi APs / recent activity / open file handles on Windows, or do any other Windows-side action via NirCmd or a NirSoft companion CLI. For clipboard operations use the `clipboard` skill; for desktop screenshots prefer the `screenshots` skill. Defaults to `ssh steamy-wsl`, NirCmd at `C:\tools\nircmd\nircmd.exe`, NirSoft companions at `C:\tools\nirsoft\`; override via `NIRCMD_HOST`, `NIRCMD_PATH`, `NIRSOFT_DIR` env vars.
---

# nircmd

Bridge for driving a Win11 desktop remotely from this SSH session via the NirCmd CLI. Use it for audio control, TTS, window management, keystroke simulation, and any NirSoft companion utility. **Clipboard work lives in the `clipboard` skill; desktop screenshots live in `screenshots`** — this skill is what's left over: the audio/window/system-info surface.

## Defaults (override via env vars)

```bash
NIRCMD_HOST="${NIRCMD_HOST:-steamy-wsl}"                          # ssh alias
NIRCMD_PATH="${NIRCMD_PATH:-/mnt/c/tools/nircmd/nircmd.exe}"      # POSIX path from WSL
NIRSOFT_DIR="${NIRSOFT_DIR:-/mnt/c/tools/nirsoft}"                # NirSoft companion tools dir
SKILL_DIR=/home/jmagar/.agents/src/skills/nircmd                      # hardcoded; SKILL.md isn't sourced as a real script
```

For persistence across sessions, set in `~/.claude/settings.json` under `env` (see `screenshots` skill for the pattern).

## Universal invocation pattern

```bash
ssh "$NIRCMD_HOST" "$NIRCMD_PATH <command> [args...]"
```

NirCmd exits 0 on success, non-zero on bad args. It does **not** print errors helpfully — bad args often produce no output and exit 0 anyway, so verify side effects (e.g., check the target file exists, query window list) when in doubt.



## Most-used commands inline



### Window control

| Command | Effect |
|---------|--------|
| `win activate ititle "Visual Studio Code"` | Bring window matching title (substring match) to front |
| `win min/max/normal/close ititle "..."` | Minimize / maximize / restore / close |
| `win settopmost ititle "..." 1` | Pin window always-on-top (0 to unpin) |
| `win move ititle "..." x y w h` | Move/resize a window |

Match modes: `title` (exact), `ititle` (case-insensitive substring), `class`, `process`. `ititle` is usually what you want.

### Audio

| Command | Effect |
|---------|--------|
| `setsysvolume 32768` | Set master volume to ~50% (0-65535) |
| `changesysvolume 5000` | Bump master volume up |
| `mutesysvolume 1/0/2` | Mute / unmute / toggle |
| `setappvolume "chrome.exe" 0.3` | Per-app volume (0.0-1.0) |
| `mediaplay 0` | Play/pause toggle (also: `1`=play, `2`=pause, `3`=stop) |
| `speak text "hello"` | TTS — Windows says it out loud |
| `stdbeep` | Short system beep |

### Session

| Command | Effect | Safety |
|---------|--------|--------|
| `lockws` | Lock the workstation | auto |
| `monitor off` | Turn the monitor(s) off | auto |
| `monitor on` | Turn them back on (or wiggle mouse) | auto |
| `screensaver` | Start the screensaver | auto |

### Dialogs (interactive — prompts the USER)

| Command | Effect |
|---------|--------|
| `infobox "Title" "Body"` | Show an info popup on their desktop |
| `qbox "Question" "Title" "command-if-yes"` | Yes/no prompt; runs command on Yes |
| `trayballoon "Title" "Body" "icon" 5000` | Tray notification balloon |

Useful for "ping me when this is done" patterns.

## NirSoft companion tools

NirCmd's sibling utilities — same author, same install pattern, same SSH wrapping. Use these when the question is *"what is the state of the Windows box right now"* rather than *"do this action."* Output goes to CSV/XML files on the Windows side, which you fetch back over SSH for processing locally.

Universal invocation:
```bash
ssh "$NIRCMD_HOST" "$NIRSOFT_DIR/<tool>.exe <flags>"
```

Most-useful set, with the one flag combo that matters:

| Question | Tool + invocation |
|---|---|
| What's holding port N? | `cports.exe /scomma C:\Temp\ports.csv` → filter on the local port column |
| Which Wi-Fi APs are around? | `WirelessNetView.exe /scomma C:\Temp\aps.csv` |
| What ran on this box today? | `LastActivityView.exe /scomma C:\Temp\activity.csv` |
| When was it last on / rebooted? | `TurnedOnTimesView.exe /scomma C:\Temp\uptime.csv` |
| Which process is holding this file? | `OpenedFilesView.exe /filefilter "C:\stuck" /scomma C:\Temp\holders.csv` |
| Find files matching a saved query | `SearchMyFiles.exe /cfg C:\saved\q.cfg /scomma C:\Temp\hits.csv` |
| What's the browser history? | `BrowsingHistoryView.exe /HistorySource 1 /scomma C:\Temp\hist.csv` |
| Live DNS queries from this box | `DNSQuerySniffer.exe /scomma C:\Temp\dns.csv` |

Standard fetch-back pattern (write to a Windows-side path, then read over SSH):
```bash
WIN_OUT='C:\Users\jmaga\AppData\Local\Temp\ports.csv'
POSIX_OUT='/mnt/c/Users/jmaga/AppData/Local/Temp/ports.csv'
ssh "$NIRCMD_HOST" "$NIRSOFT_DIR/cports.exe /scomma '$WIN_OUT'"
ssh "$NIRCMD_HOST" "cat '$POSIX_OUT'"   # pipe to awk/grep/jq as needed
```

Full catalog (with all documented flags, install via NirLauncher for the 200+ bundle) lives in `references/nirsoft-tools.md`.

**Credential-dumping tools** (WebBrowserPassView, Mail PassView, WirelessKeyView, etc.) are intentionally *not* surfaced here. They're legitimate forensics tools but the blast radius is unique — treat any explicit request the same as `runas` or registry writes: ask first, no auto-fetch of results.

## Safety boundaries

NirCmd can do destructive things. The skill enforces three tiers — see `references/safety-boundaries.md` for the full list:

- **Auto-allowed** (use freely): screen capture, audio, lock, speak, window listing/activation, dialogs, beep.
- **Ask the user first** (do not invoke without explicit confirmation): killprocess / closeprocess, runas / elevate / runinteractive, registry writes (regsetval, regdelkey, regdelval, regsvr), service control (start/stop/restart), file delete (filldelete, emptybin), setprocesspriority/affinity.
- **Refuse without an extremely explicit user instruction**: exitwin (shutdown/logoff/poweroff/reboot), initshutdown, standby, hibernate. Even with explicit ask, confirm the exact target and timing.

## Bundled scripts

- `scripts/win-shot.sh <title-substring> <out-path>` — activate a window by title, then capture it
- `scripts/lock.sh` — lock the workstation

Clipboard helpers live in the `clipboard` skill. Use that skill for push/pull clipboard work instead of adding clipboard wrappers here.

## References

- `references/command-reference.md` — categorized list of all 115 NirCmd commands with canonical syntax
- `references/window-control.md` — window matching and manipulation patterns
- `references/safety-boundaries.md` — full auto/ask/refuse classification with rationale
- `references/nirsoft-tools.md` — NirSoft companion CLIs with all documented flags + round-trip patterns
