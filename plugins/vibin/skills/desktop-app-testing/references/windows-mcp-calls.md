# Windows-MCP call patterns (agent-os VM via the Lab gateway)

Verified live 2026-05-29 after the destructive-gate fix. The desktop target is the **agent-os**
Windows 11 VM (container `agent-os-win11`, `dockur/windows` on host `dookie`), driven through the
`agent-os_windows-mcp` upstream on the Lab gateway.

## Invocation surface
In a Code Mode session, call upstream tools from `mcp__plugin_lab_lab__execute`:
```js
async () => {
  const r = await callTool("upstream::agent-os_windows-mcp::Screenshot", {});
  return (r?.content||[]).map(c => c.type==="text" ? c.text.slice(0,200) : {img:(c.data||"").length});
}
```
Outside Code Mode the same server may appear as direct `mcp__windows-mcp__*` tools, or via the
gateway `tool_execute`. **Canonical tool names + params are below** — adapt the wrapper to whatever
surface is live. Do NOT target `steamy-windows-mcp` (that's the user's personal desktop).

## ⚠️ Destructive-action gate (critical)
Write/drive tools (`PowerShell`, `App`, `Click`, `Type`, `Move`, `Scroll`, `Shortcut`, `Process`,
`MultiEdit`, `MultiSelect`, `Registry`, `FileSystem` writes) are flagged `destructive=true`. The
gateway blocks them unless **either**:
- the caller carries an execute-capable scope (`lab` / `lab:admin`) — allowlisted operators are
  auto-elevated to `lab:admin`, so an authenticated admin passes; **or**
- the surface explicitly allows destructive actions (CLI, or `confirm:true` on the native flow).

This was fixed in lab (commit `e87940c0`, "honor lab:admin/lab scope at the destructive-action
gate"). If destructive calls return `confirmation_required: "...destructive=true. Set
allow_destructive_actions=true..."`, the gateway is running a binary from **before** that fix —
rebuild + redeploy: `install -D target/release/labby bin/labby && docker compose -f
docker-compose.yml restart` (the dev container bind-mounts `./bin/labby`).

Read-only tools (`Screenshot`, `Snapshot`) are never gated.

## Preflight / readiness
1. VM up? `ssh dookie 'docker ps --format "{{.Names}}" | grep agent-os-win11'`. If absent:
   `ssh dookie 'cd /home/jmagar/compose/windows && docker compose up -d'` (storage is
   pre-provisioned → boots existing install, ~5 min cold). Windows-MCP starts via a **scheduled
   task** inside the guest — it comes up on its own after boot.
2. MCP reachable? **Do NOT** TCP-probe `agent-os.tootie.tv:8765` from dookie (false-negative — wrong
   interface). The real readiness check is a `Screenshot {}` call returning an image.

## Tool reference (canonical names + params)

### Observe (read-only, never gated)
- `Screenshot {}` → text summary (cursor pos, original size, open windows) + a PNG image block.
  Note "Screenshot Original Size" — if downscaled, multiply image coords by original/displayed.
- `Snapshot {use_vision?:bool, use_dom?:bool}` → accessibility/control tree: focused window, opened
  windows, **interactive elements with integer ids + coords**. This is what you act on by `label`.
  ⚠️ The tree is LARGE and the Code Mode envelope truncates (~24KB); slice/filter the text in the
  sandbox before returning (e.g. grep for the control you need) rather than returning the whole tree.

### Launch (destructive)
- **Prefer PowerShell** to launch a build binary — `App {name}` (Start-menu) silently no-op'd in
  testing:
  ```js
  callTool("upstream::agent-os_windows-mcp::PowerShell", {
    command: "Start-Process 'C:\\\\path\\\\app.exe'; Start-Sleep 2; (Get-Process app -ErrorAction SilentlyContinue|Select -First 1 -Expand Id)"
  })  // returns "Response: <pid>\nStatus Code: 0"
  ```
- `App {mode:"switch"|"resize", name}` — good for focusing/resizing an already-open window.

### Drive (destructive) — act by label from the LATEST Snapshot
- `Click {label:<int>}` OR `Click {loc:[x,y], button?, clicks?}` (clicks: 0 hover/1/2).
- `Type {label:<int>, text, clear?, press_enter?}` OR `Type {loc:[x,y], text, ...}`.
  ⚠️ **`Type` requires `loc` or `label`** — it does NOT type into the focused window implicitly
  (live-confirmed error: "Either loc or label must be provided"). Same for `Click`.
- `Shortcut {shortcut:"ctrl+s"}` (param is `shortcut`, not `keys`).
- `Scroll {label|loc, direction, wheel_times}`, `Move {label|loc, drag?}`,
  `MultiEdit {labels:[[id,text],…]}`.
- **Loop rule:** ids come from the most-recent `Snapshot`. After any UI change, re-`Snapshot` to get
  fresh ids; a stale id clicks the wrong element. A `Screenshot` between snapshots does NOT
  invalidate ids (only `Snapshot`/`use_ui_tree` refreshes the cache).

### Detect (destructive)
- `Process {mode:"list", name?}` → PIDs + memory; PID vanished = crash/exit.
- `PowerShell {command:"Get-WinEvent -LogName Application -MaxEvents 30 | ? {$_.LevelDisplayName -in 'Error','Critical'} | Select TimeCreated,ProviderName,Message | ConvertTo-Json"}` → crash/error events.
- `WaitFor {condition:"active_window"|"element_exists"|..., window_name|text, timeout<=120}` → poll
  for "app ready / dialog appeared" instead of fixed `Wait`. Use short timeouts (20–30s) in a retry
  loop; the outer MCP call can cut off near ~120s.

### Cleanup
- `Process {mode:"kill", name|pid}`.

## Getting a build .exe into the VM
The `\\host.lan\Data` SMB share is install-time only. Post-OOBE, transfer by either:
- **HTTP-pull via PowerShell** (verified reachable — guest→dookie `True`): serve the build on dookie
  (`python -m http.server`) and
  `PowerShell {command:"Invoke-WebRequest -Uri 'http://dookie:PORT/app.exe' -OutFile \"$env:USERPROFILE\\Desktop\\app.exe\"; Unblock-File \"$env:USERPROFILE\\Desktop\\app.exe\""}`.
- **SCP** to the guest: `scp -P 2222 app.exe docker@<dookie-ip>:` (port 2222 forwards to guest sshd).
Always `Unblock-File` copied binaries (MOTW/SmartScreen), and pre-create a firewall allow rule if
the app binds a port (first-bind raises a desktop prompt that stalls unattended runs).

## Evidence
- Screenshots come back as image blocks in the call result — save them to the host run dir.
- For host-side persistence of guest screenshots, a PowerShell `CopyFromScreen` to `C:\evidence\`
  then pull via SCP also works (fallback when the MCP Python env lacks cv2).
- Persist each `Snapshot` element list as a per-step `.txt`/`.json` in `evidence/`.
