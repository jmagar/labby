---
name: agent-os
description: "This skill should be used when the user wants to drive, control, screenshot, run PowerShell on, install software on, or interact with the agent-os Windows 11 sandbox VM. Triggers include: \"run X on agent-os\", \"screenshot agent-os\", \"PowerShell on agent-os\", \"spin up agent-os\", \"drive the windows VM\", \"the windows sandbox\", or \"winbox\". Also applies for web-dev browser verification against the agent-os Chrome endpoint. Does not apply for the user's personal Windows on steamy (use the nircmd skill). The sandbox is Claude's alone — install software, change settings, run shells freely without asking for confirmation."
---

# agent-os (Windows sandbox VM)

A real Windows 11 desktop reserved for Claude, running on host `tootie` as the **`agent-os`** VM (container name `agent-os-win11`, image `dockur/windows`). "Winbox" is only the historical nickname; the skill name and official name are **agent-os**. Both `agent-os` and `winbox` remain trigger phrases for compatibility.

Drive it through **Windows-MCP** ([CursorTouch/Windows-MCP](https://github.com/CursorTouch/Windows-MCP)) — an MCP server installed inside the VM that exposes native click/type/shell/clipboard/filesystem/registry tools as `mcp__windows-mcp__*`.

The sandbox is Claude's. Install software, write to the registry, kill processes — don't ask first. Only think twice about actions that escape into host `tootie` (Docker daemon, mounted volumes, host network).

## Why Windows-MCP, not noVNC

The previous version of this skill drove the VM through `agent-browser` against `http://tootie:8006`'s noVNC canvas. That path still works as a visual fallback, but Windows-MCP is the new primary surface because:

- **Real keyboard.** `Type` reliably handles full strings including shifted symbols (`!@#$%^&*()`, `:`, `"`, etc.). The noVNC `Shift+<digit>` bug is gone.
- **Real accessibility tree.** `Snapshot` returns interactive elements with ids — Claude can target controls by name, not by reasoning about pixels.
- **Native shell.** `PowerShell` runs commands directly; no `Win+R`, no canvas focus juggling.
- **Faster.** No browser, no canvas event dispatch, no per-character `press` loop.

Reach for noVNC at `http://tootie:8006` only when you need to *see* the desktop visually for debugging (e.g. confirming a screenshot that Windows-MCP returned), or if Windows-MCP is unreachable.

## Browser/web-dev priority

When the task is web development, browser verification, screenshots, or page interaction, use this order unless the user explicitly asks for a specific machine or browser session:

1. **CDP running on agent-os** - first choice for sandbox Chrome inspection and page automation.
2. **agent-browser** - first fallback for fresh browser automation when the task does not need the agent-os desktop/session.
3. **claude-in-chrome on agent-os** - use when the workflow specifically needs Claude-in-Chrome inside the sandbox VM.
4. **agent-os Windows-MCP** - use for OS-level control, desktop apps, PowerShell, installer flows, or browser tasks that require the actual desktop.
5. **claude-in-chrome on steamy** - last choice for the user's personal desktop/session.

Do not use Windows-MCP just because it is available when `agent-browser` can do a generic web check more directly. Do use Windows-MCP when the task depends on installed Windows software, the sandbox desktop, the filesystem, registry, native dialogs, or a browser profile inside agent-os.

## Connection

Windows-MCP is exposed inside agent-os over HTTP + Bearer token and registered in `~/.claude.json` as the `windows-mcp` server (Tailscale address; the bearer token lives in that config, not in this skill). Claude Code reaches it automatically — there is nothing to start.

If the server is unreachable: SSH into `tootie`, confirm the VM container is up (`docker ps --format '{{.Names}}' | grep agent-os`; the container name is `agent-os-win11`), and that the MCP service inside is listening. The container persists `/storage` across restarts so installed software survives.

## Tool surface

All tools are namespaced `mcp__windows-mcp__<Name>`. Use them directly — they're loaded when the windows-mcp server is up.

### Look at the screen first

- **`Screenshot`** — fast capture: returns cursor position, active/open windows, and a PNG. Use this before deciding what to do; it's the cheapest way to orient.
- **`Snapshot`** — full desktop state with interactive element ids (accessibility tree). Slower than Screenshot, but you get *handles* you can click by name instead of pixel coordinates. Use this whenever you'd otherwise be reasoning about coordinates.

### Move and click

- **`Click`** — click at `(x, y)` screen coordinates. Origin is top-left of the primary display.
- **`Move`** — move pointer to `(x, y)`, or set `drag=True` for a drag-to.
- **`Scroll`** — vertical/horizontal scroll on the whole window or a region.
- **`MultiSelect`** — select multiple files/folders/checkboxes, optionally with Ctrl-held.

### Type and shortcut

- **`Type`** — type text into a field. Handles full strings; takes an optional flag to clear existing text first. **Use this instead of the old `winbox_type` per-char loop.**
- **`Shortcut`** — keyboard shortcuts like `Ctrl+c`, `Alt+Tab`, `Win+R`. Use modifier names verbatim.
- **`MultiEdit`** — fill several input fields at once (each entry is `(x, y, text)`).

### App and process

- **`App`** — launch an app from the Start menu, then optionally resize/move its window or switch between open apps. The fastest way to "open Edge" / "open Notepad".
- **`Process`** — list running processes or terminate by PID/name. Use to clean up runaways or check whether a service is alive.

### PowerShell, clipboard, filesystem

- **`PowerShell`** — execute PowerShell. The single biggest speedup over the old GUI flow. Use this whenever the task can be expressed as a command: file ops, package installs (`winget`/`choco`), service queries, registry tweaks, network introspection.
- **`Clipboard`** — read or set Windows clipboard. Cleaner than typing for long/sensitive strings, and round-trips paste targets that don't accept `Type`.
- **`FileSystem`** — read/write/list files on the guest filesystem directly. Skip `PowerShell` for plain CRUD on files.

### Registry and notifications

- **`Registry`** — read, write, delete, list registry values and keys. Use when a setting needs to persist or unlock a feature flag.
- **`Notification`** — send a Windows toast (title + message). Handy as a "I finished, look at me" cue when running long tasks.

### Misc

- **`Wait`** — pause for N seconds. Use sparingly: prefer polling `Screenshot`/`Snapshot` over fixed sleeps. Useful right after `App` launch when the window hasn't painted yet.
- **`Scrape`** — pull text content from the current webpage (when a browser is foregrounded). Convenience for reading what's on screen in Edge/Chrome without OCR.

## Recipes

### Open an app and do something

```
App {"name": "Notepad"}
# wait until Notepad's title bar paints
Wait {"seconds": 1}
Type {"text": "hello from claude"}
Shortcut {"keys": "Ctrl+s"}
```

### Run PowerShell directly (preferred for headless work)

```
PowerShell {"command": "Get-Process | Where-Object {$_.CPU -gt 10} | Select-Object Name,CPU,Id -First 10 | ConvertTo-Json"}
```

You get stdout back as text. JSON-out makes the result trivial to parse. Use `PowerShell` for anything that's expressible as a command — it sidesteps every GUI hazard.

### Script and capture a desktop GUI app

When a GUI app must be launched, driven, and screenshotted, run the automation through Windows-MCP's desktop-attached PowerShell, not through a plain SSH session. SSH can start the process, but it may not be able to foreground the app or send reliable synthetic input into the interactive desktop.

Use a PowerShell harness with `WScript.Shell` for launch/focus/input:

```powershell
$ws = New-Object -ComObject WScript.Shell
$null = $ws.Run('"C:\path\to\app.exe"', 1, $false)
Start-Sleep -Seconds 2
$null = $ws.AppActivate('Window Title')
$ws.SendKeys('status{ENTER}')
```

Observed gotchas from the Axon Palette session:

- `Start-Process` can launch the app but may leave synthetic keyboard input unreliable for GPUI windows. `WScript.Shell.Run` plus `AppActivate` worked better.
- A non-interactive SSH-launched PowerShell session failed to foreground the GPUI window; the same script worked through Windows-MCP PowerShell because it is attached to the desktop.
- Downloaded executables may raise publisher/security prompts. Run `Unblock-File` on copied `.exe`/`.ps1` files and set process env `SEE_MASK_NOZONECHECKS=1` before launch when shelling into child executables.
- The first child process that binds/listens may still raise a Windows Firewall prompt. Pre-create firewall rules or accept the prompt once from the desktop before expecting unattended captures.
- If `Snapshot`/`Screenshot` fail because the Windows-MCP Python environment is missing `cv2`, capture screenshots inside PowerShell with `System.Drawing.Graphics.CopyFromScreen` and the target window rect from `user32!GetWindowRect`.
- Clipboard paste is not universally accepted by GPUI text inputs; literal `SendKeys('<command>{ENTER}')` was the reliable path for Axon Palette.
- Kill/relaunch the app between operation captures when testing command output. It avoids mode/input state leaking from one operation into the next.
- The MCP call layer can still time out around 120 seconds even if a higher timeout is requested. Split long screenshot batches into chunks and write a manifest beside the PNGs.

### Click by element instead of pixel

```
Snapshot {}                      # returns elements with ids and labels
Click {"x": 412, "y": 287}       # use coordinates returned for the element you want
```

Snapshot beats Screenshot whenever you need to *interact* — element coordinates come from the accessibility tree, not vision.

### Install software via winget

```
PowerShell {"command": "winget install --id Microsoft.PowerToys --silent --accept-package-agreements --accept-source-agreements"}
```

### Push and paste a long string (bypass typing)

```
Clipboard {"action": "set", "text": "<your long or symbol-heavy string>"}
Click {"x": ..., "y": ...}        # focus the field
Shortcut {"keys": "Ctrl+v"}
```

### Send a desktop notification when a long task ends

```
Notification {"title": "agent-os", "message": "winget install finished"}
```

### Persist a Windows setting

```
Registry {"action": "write", "path": "HKCU\\Software\\YourApp", "name": "Setting", "type": "REG_SZ", "value": "x"}
```

## Visual debugging via the noVNC fallback

When something looks wrong and you need eyeballs, the old path still works:

- URL: `http://tootie:8006/vnc.html?autoconnect=1&resize=remote`
- Drive with `agent-browser` (see git history of this file for the canvas/dispatch helpers).

Treat this strictly as a visual debugger. Once you've identified the problem, fix it through Windows-MCP — don't fall back into the per-char `press` loop just because noVNC is open.

## Alternative side-channels (still valid)

These predate Windows-MCP but remain useful where the MCP path is awkward:

- **`/oem` install-time drop folder.** Host path `/home/jmagar/compose/windows/oem` is mounted as `\\host.lan\Data` *only during initial Windows install/OOBE*. Once setup completes, the SMB share is gone. Use only for first-boot provisioning.
- **RDP on `tootie:33890`.** Exposed by the `agent-os-win11` container in addition to noVNC. No agent-side RDP client installed today; install `freerdp` if a real interactive session is ever needed (now that Windows-MCP exists, the case for this is weaker).
- **SSH to the guest on `tootie:2222`.** The container forwards host port `2222` → guest port `22`. Confirmed exposed on the running container; whether sshd is actually running and configured inside the guest depends on first-boot provisioning. If it answers, this is the cleanest scripted side-channel — no `PowerShell` round-trip through the MCP server.

## Operating notes

- The `agent-os-win11` container persists `/storage` across restarts. Installed software, registry edits, and most filesystem state survive reboots.
- If a task is GUI-bound and slow, kick it off through `App`/`PowerShell`, then `ScheduleWakeup` or move on. Come back with a `Screenshot` to check progress.
- For *anything* expressible as PowerShell, prefer `PowerShell` over clicking. It's faster, more reliable, and leaves a paper trail in the command rather than in pixel coordinates.
- Don't paste credentials via `Type` — round-trip through `Clipboard` so they don't end up in screenshots or logs of the typing stream.
