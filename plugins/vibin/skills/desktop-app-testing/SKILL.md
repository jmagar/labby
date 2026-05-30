---
name: desktop-app-testing
description: 'Use when the user wants to live-test a built Windows desktop application (.exe) end-to-end inside the agent-os Windows 11 VM and get a works/doesn''t-work + UI/UX report — launching the real binary and driving it, not writing test code. Triggers: "test my Windows app", "QA this .exe", "run my desktop build on agent-os and tell me what breaks", "click through the app", "review the desktop app''s UX", "does my exe work", "test the built binary in the Windows VM". Gets the .exe into the VM, launches it, enumerates controls from the UI Automation tree, drives every feature (click/type by element), watches for crashes/hangs/error dialogs, captures screenshots + control-tree dumps, and emits a structured report. Drives the agent-os VM via the Windows-MCP gateway. Sibling of web-app-testing and android-app-testing (shared report format). Does NOT fire for: building/coding a desktop app, the user''s personal Windows on steamy (use nircmd), or general agent-os VM driving (use agent-os).'
---

# desktop-app-testing

Live, end-to-end testing of a built **Windows `.exe`** inside the agent-os Windows 11 VM: transfer
the build in, launch it, drive every feature, watch for crashes/hangs/error dialogs, review UI/UX,
and emit a structured works/doesn't-work report. Companion to `web-app-testing` and
`android-app-testing` — all three share one report format (`references/report-format.md`).

Drives the **agent-os** VM (container `agent-os-win11`, `dockur/windows` on `dookie`) through the
`agent-os_windows-mcp` upstream on the Lab gateway. Builds on the `agent-os` skill (which is the
general VM driver) but adds the testing harness: build transfer, feature enumeration, failure
taxonomy, evidence pipeline, and the report.

## When to use vs. neighbors
- **This skill** — a *test pass + report* over a desktop app's features and UX.
- `agent-os` — general driving of the Windows VM (install software, run PowerShell, one-off tasks).
- `nircmd` — the user's PERSONAL Windows on steamy, not the sandbox. Never target steamy here.

## ⚠️ Destructive-action gate — read first
Every drive action (`PowerShell`, `Click`, `Type`, `App`, `Process`, …) is `destructive=true` and
gated by the gateway. An authenticated admin (`lab`/`lab:admin` scope) passes — this was fixed in
lab commit `e87940c0`. If drive calls return `confirmation_required: "...destructive=true..."`, the
gateway predates the fix; rebuild + redeploy (see `references/windows-mcp-calls.md`). Read-only
`Screenshot`/`Snapshot` are never gated, so a UI/UX *review* works even if the gate isn't lifted.

## Prerequisites
- The **agent-os VM running** on dookie (the skill's preflight starts it if absent).
- The Lab gateway reachable with an execute-capable scope (for drive actions).
- The built `.exe`/installer on this host (or a URL the guest can fetch).

## Workflow

1. **Preflight.**
   - VM up? `ssh dookie 'docker ps --format "{{.Names}}" | grep agent-os-win11'`. If absent:
     `ssh dookie 'cd /home/jmagar/compose/windows && docker compose up -d'` (boots existing install,
     ~5 min cold; Windows-MCP auto-starts via an in-guest scheduled task).
   - MCP ready? Call `Screenshot {}` — an image back means ready. (Do NOT TCP-probe :8765, false
     negative.)
   - Drive gate? Do one cheap destructive call (`Process {mode:"list"}`) — if it returns data, the
     gate is open; if `confirmation_required`, fix the gateway first.
   - Create run dir `~/.agents/docs/sessions/<app>-desktop-test/run_<id>/`.
2. **Transfer the build** into the VM (see `references/windows-mcp-calls.md`): HTTP-pull via
   PowerShell `Invoke-WebRequest` (verified reachable) or SCP to `dookie:2222`. `Unblock-File` the
   copied binary; pre-create a firewall allow rule if it binds a port.
3. **Launch.** Prefer `PowerShell {command:"Start-Process 'C:\\...\\app.exe'; ...return PID"}`
   (Start-menu `App {name}` is unreliable for arbitrary binaries). Confirm a PID came back.
4. **Wait for ready, map controls.** `WaitFor {condition:"active_window", window_name:"<title>"}`
   (short timeout in a retry loop), then `Snapshot {}` — enumerate menus, buttons, tabs, fields from
   the UI Automation tree. Build the feature checklist (merge with any user spec). Write `plan.md`.
5. **Exercise each feature** — the act-by-label loop:
   `Snapshot` → pick the target element's integer `label` → `Click {label}` / `Type {label, text}`
   → `Snapshot` again (UI changed, ids are stale) → repeat. `Screenshot` between steps for evidence
   (doesn't invalidate ids). Use `Shortcut` for keyboard ops. **`Type`/`Click` REQUIRE `loc` or
   `label`** — there is no implicit-focus typing.
6. **Detect failures** after each action:
   - **Crash/exit** — `Process {mode:"list", name}` shows the PID gone; check
     `Get-WinEvent -LogName Application` for Error/Critical. → FAIL.
   - **Hang** — `WaitFor` times out / `Snapshot` shows "(Not Responding)". → FAIL.
   - **Error dialog** — `Snapshot` surfaces dialog text; screenshot it. → FAIL/PARTIAL.
   - **Wrong output / no feedback** — expected UI change didn't happen. → PARTIAL/FAIL.
   - **Can't reach** — needs creds/data/license the run lacks. → BLOCKED.
7. **Reset between independent features** — `Process {mode:"kill", name}` then relaunch, so input/
   mode state doesn't leak across tests.
8. **UX/a11y pass** — score the report-format rubric from snapshots + screenshots. Interactive
   elements with no accessible name in the UIA tree = accessibility findings.
9. **Write the report** → `report.md` + `result.json` in the run dir, per
   `references/report-format.md`. Save screenshots to `evidence/` and index them.

## Gotchas (live-validated)
- **Prefer `PowerShell Start-Process` over `App {name}` to launch** an arbitrary `.exe`.
- **Re-`Snapshot` after every UI change** — `label` ids are valid only against the latest Snapshot.
- **`Snapshot` output overflows the Code Mode envelope (~24KB)** — filter/slice the tree text in the
  sandbox before returning; don't dump the whole tree.
- **Custom-rendered apps** (GPUI, some Electron/canvas) expose little to UI Automation → `Snapshot`
  is sparse; fall back to `Screenshot` + coordinate clicks + `SendKeys`, and flag reduced confidence.
- **MCP calls can time out ~120s** — chunk long operations; use short `WaitFor` in a retry loop.
- **Security prompts stall unattended runs** — `Unblock-File`, `SEE_MASK_NOZONECHECKS=1`, pre-create
  firewall rules before expecting hands-off captures.

## References
- `references/windows-mcp-calls.md` — verified tool names, params, call patterns, the destructive
  gate, build-transfer recipes, evidence capture.
- `references/report-format.md` — shared cross-platform report spec, run-dir layout, verdicts.
