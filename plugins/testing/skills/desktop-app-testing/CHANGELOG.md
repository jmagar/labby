# Changelog — desktop-app-testing

## 2026-06-06 — SSH-only fallback
- Added `references/ssh-fallback-capture.md` and a `## Fallback` note in SKILL.md: when
  `agent-os_windows-mcp` isn't a connected upstream, run the whole pass over plain `ssh agent-os`.
- Documents that SSH lands in session 0 (no window station) so a GUI `.exe` crashes there with
  `os error 1459` and `CopyFromScreen` is blank; the fix is a `schtasks /it` task that launches +
  captures in the interactive console session (session 1). Includes the `MSYS_NO_PATHCONV=1`
  git-bash gotcha, Win32 `FindWindow`/`SetForegroundWindow` + `SendKeys` driving, forward-slash scp,
  and seeding the app's config (`~/.axon/.env`, `%APPDATA%\<id>\settings.json`) to point it at a
  homelab backend over Tailscale.
- Added an in-process-vite + `playwright-core`→Edge browser dev-loop (token-injecting vite proxy) for
  iterating a Tauri/web frontend's identical bundle without a native rebuild.
- Live-validated 2026-06-06 building + driving the Axon Palette Tauri exe on agent-os.

## 2026-05-29 — initial release
- Added — initial release. Live end-to-end Windows `.exe` testing inside the agent-os VM via the
  Windows-MCP gateway.
- `references/windows-mcp-calls.md` — verified tool names/params + call patterns for
  `agent-os_windows-mcp`: observe (Screenshot/Snapshot), launch (PowerShell Start-Process), drive
  (Click/Type by label), detect (Process/Get-WinEvent/WaitFor), build transfer (HTTP-pull/SCP),
  evidence capture. Documents the destructive-action gate and the act-by-label loop.
- `references/report-format.md` — shared cross-platform report spec (duplicated across siblings).
- Live-validated 2026-05-29 (after lab destructive-gate fix `e87940c0`): `Process list`,
  `PowerShell Start-Process notepad` (PID 9176, exit 0), `Snapshot`/`Screenshot` all returned real
  data through the gateway; `Type` requires loc/label (no implicit focus).
- Baked-in gotchas: prefer PowerShell launch over `App {name}`; re-Snapshot after every UI change;
  Snapshot overflows the Code Mode envelope (filter in-sandbox); custom-rendered apps need
  screenshot+coords fallback; ~120s MCP call timeout; Unblock-File + firewall pre-rules.
