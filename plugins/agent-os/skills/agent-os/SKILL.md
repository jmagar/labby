---
name: agent-os
description: Use when the user asks to drive or test Claude's reserved agent-os Windows 11 sandbox VM via Windows-MCP, PowerShell, screenshots, desktop apps, installers, registry, filesystem, or noVNC. Triggers include agent-os, the agent-os VM or desktop, windows sandbox, winbox, run on agent-os, screenshot agent-os, or PowerShell on agent-os. Prefer webwright for generic web verification; use agent-os only when the task depends on the Windows sandbox or desktop. Do not use for the user's personal Windows machines such as steamy or steamy-wsl.
---

# agent-os (Windows sandbox VM)

A real Windows 11 desktop reserved for Claude, running on host `tootie` as the **`agent-os`** VM (container name `agent-os-win11`, image `dockur/windows`). "Winbox" is only the historical nickname; the skill name and official name are **agent-os**. Both `agent-os` and `winbox` remain trigger phrases for compatibility.

Drive it through **Windows-MCP** ([CursorTouch/Windows-MCP](https://github.com/CursorTouch/Windows-MCP)) — an MCP server installed inside the VM that exposes native click/type/shell/clipboard/filesystem/registry tools as `mcp__windows-mcp__*`.

The sandbox is Claude's. Install software, write to the registry, kill processes — don't ask first. Only think twice about actions that escape into the Docker host (Docker daemon, mounted volumes, host network).

## Configuration

This skill ships in the `agent-os` plugin. All connection and host details come from the plugin's **userConfig** (set in plugin settings). The same values reach two surfaces with two *different* syntaxes — this is a Claude Code rule, not a typo:

- **Config-file substitution** (`.mcp.json`, hook commands, the `/agent-os` command): `${user_config.<key>}` — the literal lowercase userConfig key. This is how the plugin registers `windows-mcp` from your config, so no `~/.claude.json` edit is required.
- **Inside subprocess scripts** (e.g. the SessionStart `setup.sh`): values arrive as `$CLAUDE_PLUGIN_OPTION_<KEY>` env vars, where `<KEY>` is the key **uppercased**.

| Setting | userConfig key | `${user_config.…}` (configs) | `$CLAUDE_PLUGIN_OPTION_…` (scripts) |
|---|---|---|---|
| Windows-MCP URL | `agent_os_mcp_url` | `${user_config.agent_os_mcp_url}` | `…_AGENT_OS_MCP_URL` |
| Bearer token (sensitive) | `agent_os_mcp_token` | `${user_config.agent_os_mcp_token}` | `…_AGENT_OS_MCP_TOKEN` |
| VM Tailscale IP | `agent_os_vm_tailscale_ip` | `${user_config.agent_os_vm_tailscale_ip}` | `…_AGENT_OS_VM_TAILSCALE_IP` |
| Docker host | `agent_os_vm_host` | `${user_config.agent_os_vm_host}` | `…_AGENT_OS_VM_HOST` |
| Host-forward SSH / port | `agent_os_host_forward_ssh` / `…_port` | `${user_config.agent_os_host_forward_ssh}` | `…_AGENT_OS_HOST_FORWARD_SSH` / `…_PORT` |
| Compose file | `agent_os_compose_file` | `${user_config.agent_os_compose_file}` | `…_AGENT_OS_COMPOSE_FILE` |
| Container name | `agent_os_container_name` | `${user_config.agent_os_container_name}` | `…_AGENT_OS_CONTAINER_NAME` |
| noVNC URL | `agent_os_novnc_url` | `${user_config.agent_os_novnc_url}` | `…_AGENT_OS_NOVNC_URL` |
| Auto-start VM if down | `agent_os_autostart` | `${user_config.agent_os_autostart}` | `…_AGENT_OS_AUTOSTART` |

Sensitive values (the token) substitute in configs and reach subprocesses as env vars, but are **not** expanded into skill/agent prose. Commands below read the `$CLAUDE_PLUGIN_OPTION_*` env vars. Any concrete hostnames/IPs/paths shown elsewhere are just the **defaults** — your configured values take precedence. **Never echo** the token.

> **Tool namespace.** Because windows-mcp is provided by this plugin, its tools surface under the plugin's MCP namespace (typically `mcp__plugin_agent-os_windows-mcp__<Tool>`). This skill refers to them by base name (`Screenshot`, `Click`, `PowerShell`, …) and the historical `mcp__windows-mcp__*` form — match whichever your client shows in `/mcp`.

## Why Windows-MCP, not noVNC

The previous version of this skill drove the VM through `agent-browser` against `http://tootie:8006`'s noVNC canvas. That path still works as a visual fallback, but Windows-MCP is the new primary surface because:

- **Real keyboard.** `Type` reliably handles full strings including shifted symbols (`!@#$%^&*()`, `:`, `"`, etc.). The noVNC `Shift+<digit>` bug is gone.
- **Real accessibility tree.** `Snapshot` returns interactive elements with ids — Claude can target controls by name, not by reasoning about pixels.
- **Native shell.** `PowerShell` runs commands directly; no `Win+R`, no canvas focus juggling.
- **Faster.** No browser, no canvas event dispatch, no per-character `press` loop.

Reach for noVNC at `http://tootie:8006` only when you need to *see* the desktop visually for debugging (e.g. confirming a screenshot that Windows-MCP returned), or if Windows-MCP is unreachable.

## Browser/web-dev priority

When the task is web development, browser verification, screenshots, or page interaction, use this order unless the user explicitly asks for a specific machine or browser session:

1. **webwright** - default and most reliable for web tasks/verification. Code-as-action Playwright workflow with screenshot evidence; prefer it over everything below unless the task specifically needs the agent-os desktop/session.
2. **CDP running on agent-os** - for sandbox Chrome inspection and page automation when you need agent-os's actual browser/session.
3. **agent-browser** - fresh browser automation when the task does not need the agent-os desktop/session.
4. **claude-in-chrome on agent-os** - when the workflow specifically needs Claude-in-Chrome inside the sandbox VM.
5. **agent-os Windows-MCP** - OS-level control, desktop apps, PowerShell, installer flows, or browser tasks that require the actual desktop.
6. **claude-in-chrome on steamy** - last choice; the user's personal desktop/session.

Do not reach for Windows-MCP (or the agent-os desktop browser) just because it's available when `webwright` can do the web task more directly — webwright is the default for generic web work. Do use Windows-MCP / the agent-os browser when the task depends on installed Windows software, the sandbox desktop, the filesystem, registry, native dialogs, or a browser profile inside agent-os.

## Connection

Already wired — Claude Code reaches it automatically, nothing to start per session. The link has four layers, inner → outer; understand them so you can repair whichever one breaks:

1. **Server (inside the VM).** Windows-MCP (CursorTouch/Windows-MCP, a Python HTTP MCP server) runs in the guest on `localhost:8000`, bearer-token protected. It's installed and starts with the VM.
2. **Exposure (inside the VM).** `tailscale serve` publishes it over the tailnet with HTTPS and a stable MagicDNS name — `https://agent-os.manatee-triceratops.ts.net/ → http://localhost:8000`. This is what keeps the URL stable as the VM moves hosts. Recreate with `tailscale serve --bg http://localhost:8000` if the mapping is ever lost (`tailscale serve status` to check).
3. **Client (this plugin).** The `agent-os` plugin's `.mcp.json` registers the `windows-mcp` server from your userConfig via `${user_config.*}` substitution. Set the URL + token in plugin settings; there's nothing to edit in `~/.claude.json`. The registration:
   ```jsonc
   "windows-mcp": {
     "type": "http",
     "url": "${user_config.agent_os_mcp_url}",
     "headers": { "Authorization": "Bearer ${user_config.agent_os_mcp_token}" }
   }
   ```
4. **Gateway (optional).** Labby can also front it: a `~/.lab/config.toml` `[[upstream]]` entry (`url` + `bearer_token_env = "LAB_GW_WINDOWS_MCP_AUTH_HEADER"`, the value in `~/.lab/.env`), surfaced to gateway clients as `agent-os_windows-mcp`. Apply changes with `lab gateway reload`; health-check with `lab gateway list | grep agent-os` → expect `✓ … 🔧 18`.

The bearer token lives in plugin userConfig (secure OS storage) — and, if you use the gateway, `~/.lab/.env`. Never in this skill.

**Bring the VM back up** (container down): `ssh "$CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_HOST" 'docker compose -f "$CLAUDE_PLUGIN_OPTION_AGENT_OS_COMPOSE_FILE" up -d'` (container `$CLAUDE_PLUGIN_OPTION_AGENT_OS_CONTAINER_NAME`; the VM disk persists across restarts).

**If the MCP is unreachable**, repair outward through the four layers: container up on the Docker host (`ssh "$CLAUDE_PLUGIN_OPTION_AGENT_OS_VM_HOST" 'docker ps | grep "$CLAUDE_PLUGIN_OPTION_AGENT_OS_CONTAINER_NAME"'`) → guest Tailscale up (see **Tailscale maintenance**) → `tailscale serve` mapping present → server listening on `:8000` → `lab gateway reload`. Full symptom/fix grid in **Troubleshooting**. The plugin's SessionStart hook reports this status automatically; `/agent-os status` runs it on demand. If `agent_os_autostart` is `"true"`, the hook also tries to `docker compose up -d` the VM when it's down (it only *starts* an already-provisioned VM — it does not install Windows-MCP).

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

- **`PowerShell`** — execute PowerShell commands. The single biggest speedup over the old GUI flow. Use this whenever the task can be expressed as a command: file ops, package installs (`winget`/`choco`), service queries, registry tweaks, network introspection. (The tool is named `PowerShell`, not `Shell`.)
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

### Driving native / GPUI desktop apps

Custom desktop apps — especially ones built on **GPUI** (Zed's Rust GUI framework) and other non-standard toolkits — often don't expose a usable accessibility tree, so `Snapshot`/`Click`-by-coordinate and plain `Type` can be unreliable. Launch, focus, and input must run through Windows-MCP's **desktop-attached** PowerShell, not a plain SSH session: SSH can start the process but typically can't foreground it or deliver synthetic input to the interactive desktop.

Use a `WScript.Shell` harness for launch/focus/input:

```powershell
$ws = New-Object -ComObject WScript.Shell
$null = $ws.Run('"C:\path\to\app.exe"', 1, $false)
Start-Sleep -Seconds 2
$null = $ws.AppActivate('Window Title')
$ws.SendKeys('status{ENTER}')
```

Lessons for these apps:

- Prefer `WScript.Shell` `Run` + `AppActivate` over `Start-Process` — the latter launches but often leaves synthetic keyboard input unreliable for GPUI windows.
- Run it through Windows-MCP `PowerShell` (desktop-attached), not non-interactive SSH — the SSH path can't foreground the window.
- GPUI text inputs frequently ignore clipboard paste; literal `SendKeys('<command>{ENTER}')` is the reliable path.
- Kill and relaunch the app between captures when testing command output, so input/mode state doesn't leak from one operation into the next.

For launch-blocking and capture failures on these apps (SmartScreen/`Unblock-File`, firewall prompts, missing `cv2` screenshot fallback, the ~120s MCP-call timeout), see **Troubleshooting**.

### Click by accessibility coordinates instead of vision-guessing

```
Snapshot {}                      # returns interactive elements with labels + their coordinates
Click {"x": 412, "y": 287}       # pass the coordinates Snapshot reported for the element you want
```

`Click` still takes `(x, y)` — you don't click by element name. The win is that `Snapshot` gives you each element's coordinates straight from the accessibility tree, so you copy those in instead of guessing pixels from a Screenshot. Use Snapshot whenever you need to *interact*; use Screenshot when you just need to *look*.

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

- **`/oem` install-time drop folder.** Host path `/mnt/cache/compose/windows/oem` **on tootie** is mounted as `\\host.lan\Data` *only during initial Windows install/OOBE*. Once setup completes, the SMB share is gone. Use only for first-boot provisioning. (Note: the VM moved hosts on 2026-05-31 — the old `/home/jmagar/compose/windows/oem` path on dookie no longer exists.)
- **RDP on `tootie:33890`.** Exposed by the `agent-os-win11` container in addition to noVNC. No agent-side RDP client installed today; install `freerdp` if a real interactive session is ever needed (now that Windows-MCP exists, the case for this is weaker).
- **SSH to the guest on `tootie:2222`.** The container forwards host port `2222` → guest port `22`. Confirmed exposed on the running container; whether sshd is actually running and configured inside the guest depends on first-boot provisioning. If it answers, this is the cleanest scripted side-channel — no `PowerShell` round-trip through the MCP server.

## Operating notes

- The `agent-os-win11` container persists `/storage` across restarts. Installed software, registry edits, and most filesystem state survive reboots.
- If a task is GUI-bound and slow, kick it off through `App`/`PowerShell`, then `ScheduleWakeup` or move on. Come back with a `Screenshot` to check progress.
- For *anything* expressible as PowerShell, prefer `PowerShell` over clicking. It's faster, more reliable, and leaves a paper trail in the command rather than in pixel coordinates.
- Don't paste credentials via `Type` — round-trip through `Clipboard` so they don't end up in screenshots or logs of the typing stream.

## Tailscale maintenance (read before bouncing the guest's Tailscale)

The guest runs its own `tailscaled`, and `ssh agent-os` connects **over that same Tailscale IP** (`100.109.125.128`). That creates a footgun:

- **Never run `tailscale down` (or restart the Tailscale service) over `ssh agent-os`.** The moment Tailscale drops, your SSH session is severed mid-command — so the follow-up `tailscale up` never executes and the node is left **stopped** (no Tailscale, MCP/gateway unreachable).
- **Do Tailscale maintenance via the host port-forward instead:** `ssh -p 2222 docker@100.120.242.29` (tootie → Docker → guest `:22`). This path goes through Docker, not the guest's Tailscale, so it survives `tailscale down/up`.
- **Windows `tailscale up` won't run bare** when non-default prefs are set (this VM uses `--exit-node-allow-lan-access --unattended`). It errors and tells you to either re-list every non-default flag or use `tailscale up --reset`. Easiest reliable bounce: `Restart-Service Tailscale -Force` then `tailscale up --reset` (or re-list the flags).
- **"offline but reachable" is expected here.** The control plane can show the node `offline / not in map poll` while it's still reachable via a direct LAN route (`10.1.0.2`) — fine on-LAN (SSH/MCP work), but unreliable from a remote network. A clean `tailscale up` after a service restart re-establishes the map poll and clears it.
- If the gateway shows `agent-os_windows-mcp` as `⚠ upstream discovery timed out`, first check the guest's Tailscale is actually up (`ssh -p 2222 docker@100.120.242.29 tailscale status`), then `lab gateway reload`.

## Troubleshooting

Work top to bottom — most failures are "the VM/MCP isn't reachable," and the checks are layered from outermost (container) to innermost (the tool call).

| Symptom | Likely cause | Fix |
|---|---|---|
| `mcp__windows-mcp__*` tools missing / "server not found" | windows-mcp server or VM container down | On tootie: `docker ps \| grep agent-os-win11`. If absent: `docker compose -f /mnt/cache/compose/windows/docker-compose.yml up -d`. Then `lab gateway reload`. |
| Gateway shows `agent-os_windows-mcp ⚠ upstream discovery timed out` | guest Tailscale down/flapping, or MCP slow to list tools | `ssh -p 2222 docker@100.120.242.29 tailscale status` (host-forward path). If Tailscale is down, bring it up (see **Tailscale maintenance**). Then `lab gateway reload`. |
| `ssh agent-os` times out but `ssh -p 2222 docker@100.120.242.29` works | guest Tailscale is stopped/offline | Bring Tailscale up via the host-forward path — see **Tailscale maintenance**. Never `tailscale down` over `ssh agent-os`. |
| Node shows `offline` in `tailscale status` but pings/SSH work on-LAN | "not in map poll" — control-plane session not held (NAT churn) | Expected on-LAN; for remote access do a clean `Restart-Service Tailscale -Force` + `tailscale up --reset` via host-forward. |
| `Snapshot`/`Screenshot` fail (Python `cv2` missing) | Windows-MCP image missing OpenCV | Capture in PowerShell instead: `System.Drawing.Graphics.CopyFromScreen` with the window rect from `user32!GetWindowRect`. |
| MCP call dies around ~120s on a long op | MCP call layer timeout (not your `timeout` arg) | Split long work into chunks; write a manifest beside outputs. Kick off via `PowerShell`, poll with `Screenshot`. |
| GUI app launches but synthetic input/focus unreliable (esp. GPUI windows) | `Start-Process` / non-interactive SSH not desktop-attached | Drive through Windows-MCP `PowerShell` with `WScript.Shell` `Run` + `AppActivate` + `SendKeys` (see **Driving native / GPUI desktop apps**). |
| Downloaded `.exe`/`.ps1` blocked, SmartScreen/publisher prompt | Mark-of-the-Web on copied files | `Unblock-File` the file; set `SEE_MASK_NOZONECHECKS=1` before launching child exes. |
| First run of an app raises a Windows Firewall prompt | new listener needs an allow rule | Pre-create a firewall rule, or accept once from the desktop (via noVNC) before expecting unattended runs. |
| Need to *see* the desktop to debug | — | noVNC at `http://tootie:8006/vnc.html?autoconnect=1&resize=remote` (visual only — fix through Windows-MCP). |
| Installed software/files vanished after reboot | something written outside `/storage` | Only `/storage` (the VM disk) persists; the container is reachable again after `docker compose up -d`. Re-install if it landed on an ephemeral layer. |
