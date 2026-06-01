# Changelog — desktop-app-testing

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
