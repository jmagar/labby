# desktop-app-testing

Live end-to-end testing of a built **Windows `.exe`** inside the agent-os Windows 11 VM, producing a
works/doesn't-work + UI/UX report. One of three sibling testing skills (`web-app-testing`,
`android-app-testing`, `desktop-app-testing`) sharing a common report format.

## When to invoke
- "test my Windows app", "QA this .exe", "run my desktop build on agent-os and tell me what breaks",
  "click through the app", "review the desktop app's UX", "does my exe work".
- NOT for building/coding a desktop app, the user's personal Windows on steamy (`nircmd`), or
  general agent-os VM driving (`agent-os`).

## How it works
Drives the agent-os VM (`agent-os-win11`, dockur/windows on dookie) through the
`agent-os_windows-mcp` gateway: transfer the build in → launch (`PowerShell Start-Process`) →
enumerate controls from the UI Automation tree (`Snapshot`) → drive by element label
(`Click`/`Type`) → detect crashes/hangs/error dialogs (`Process`, `Get-WinEvent`, `WaitFor`) →
capture screenshots + tree dumps → structured report.

## Files
- `SKILL.md` — workflow, preflight, the destructive-action gate, failure taxonomy, gotchas.
- `references/windows-mcp-calls.md` — verified tool names/params, call patterns, build-transfer
  recipes, evidence capture. Live-validated 2026-05-29 after the gateway destructive-gate fix.
- `references/report-format.md` — shared cross-platform report spec, run-dir layout, verdict words.

## Prerequisites
- agent-os VM running on dookie (preflight starts it if absent; ~5 min cold boot).
- Lab gateway reachable with an execute-capable scope (`lab`/`lab:admin`) for drive actions.
- The `.exe`/installer on this host or a URL the guest can fetch.

## Important: the destructive-action gate
All drive actions are gated `destructive=true`; an authenticated admin (`lab:admin`) passes after
lab fix `e87940c0`. If drive calls return `confirmation_required`, the gateway predates the fix —
rebuild + redeploy `bin/labby` and restart the dev container. Read-only Screenshot/Snapshot always
work, so a UI/UX review is possible even before the gate is lifted.

## Companion skills
- `agent-os` — general agent-os VM driver (this skill builds on it for testing).
- `web-app-testing`, `android-app-testing` — same testing job, other targets, same report.
- `nircmd` — the user's personal Windows on steamy (different target; never used here).
