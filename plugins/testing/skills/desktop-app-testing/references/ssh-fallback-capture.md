# SSH fallback: launch + capture a native GUI on agent-os without Windows-MCP

Use when the `agent-os_windows-mcp` upstream is **not reachable from the session** — e.g. the Lab
gateway is in code-mode and its `execute` interface isn't exposed, or windows-mcp simply isn't a
connected MCP server. Everything below drives agent-os over plain `ssh agent-os` and was
live-validated 2026-06 building + capturing the Axon Palette Tauri exe.

## Why plain SSH can't see a GUI (and the fix)

- `ssh agent-os` lands in **session 0** — a non-interactive service session with no window station.
  Launching a GUI `.exe` there panics with `This operation requires an interactive window station
  (os error 1459)`, and a PowerShell `CopyFromScreen` returns a **blank white** frame.
- The real desktop is **session 1** (the auto-logged-on `docker` console). Confirm:
  `ssh agent-os 'query user'` → `docker  console  1  Active`.
- A **scheduled task created with `/it`** runs in that interactive session, so the GUI gets a window
  station — and a capture script run the same way sees the real desktop.

> git-bash is agent-os's default SSH shell and mangles `schtasks /flags` into `C:/Program Files/Git/...`
> paths. Prefix **every** schtasks call with `MSYS_NO_PATHCONV=1`, and use **forward slashes** for any
> `-File` / scp path (backslash paths get doubled and fail).

## Launch the app in session 1

```bash
EXE='C:\Users\Docker\path\to\app.exe'
ssh agent-os "MSYS_NO_PATHCONV=1 schtasks /create /tn AppShow /tr '\"$EXE\"' /sc once /st 23:59 /it /f ; MSYS_NO_PATHCONV=1 schtasks /run /tn AppShow"
# survives == ran with a window station (no 1459 crash):
ssh agent-os 'powershell -NoProfile -Command "(Get-Process app -EA SilentlyContinue | Measure-Object).Count"'   # expect 1
```

## Drive + capture (one .ps1, also run via `/it`)

Put the logic in a `.ps1` and run it with `-File C:/Users/Docker/drive.ps1` (avoids inline-quoting
hell); have it write a sentinel file at the end and **poll for that** rather than guessing the run
duration. Inside the script:

- **Bring the window forward** with Win32 `FindWindow($null,"<title>")` + `SetForegroundWindow`; if a
  terminal covers it, minimize that terminal — Windows Terminal's class is
  `CASCADIA_HOSTING_WINDOW_CLASS` → `ShowWindow($hwnd, 6)`.
- **Type** with `(New-Object -ComObject WScript.Shell).SendKeys(...)` (SendKeys-escape `+ ^ % ~ ( ) { } [ ]`);
  `^l`/`{ENTER}` etc. work. A frameless window still has a title for `FindWindow` even with
  `decorations:false`.
- **Capture** the real desktop: `[System.Windows.Forms.Screen]::PrimaryScreen.Bounds` +
  `[System.Drawing.Graphics]::FromImage(...).CopyFromScreen(...)` → `bmp.Save("C:\Users\Docker\shot.png")`.
- Pull it back: `scp 'agent-os:C:/Users/Docker/shot.png' ./` (forward slashes).

Clean up afterward: `MSYS_NO_PATHCONV=1 schtasks /delete /tn <name> /f` and stop the process.

## Pointing a Tauri/desktop app at a homelab backend

The app reads its own config (for the Axon Palette: `%USERPROFILE%\.axon\.env` →
`AXON_SERVER_URL` / `AXON_MCP_HTTP_TOKEN`, plus `%APPDATA%\<bundle.id>\settings.json`). Seed those
files via SSH before launch to point the GUI at a real service. agent-os reaches dookie over
Tailscale (`http://100.88.16.79:8001`) and the static bearer token is accepted there.

## Faster loop: iterate the web frontend in a browser (no rebuild)

For a Tauri/Electron app you can exercise the **identical frontend bundle** the exe ships without a
native rebuild: run its vite dev server **in-process** and drive system Edge with `playwright-core`
(no browser download). Inject auth at a vite proxy so the browser never holds the token and CORS is
moot — the app's HTTP layer must use **relative** `/v1/...` paths in browser mode (not an absolute
baseUrl) so requests hit the proxy:

```js
// one foreground `node driver.mjs` ON agent-os (node child procs survive there; on dookie's
// sandbox a backgrounded/child node server gets SIGKILLed when the shell call ends).
import { chromium } from "playwright-core";
import { createServer } from "vite";
const server = await createServer({ root: APP, configFile: `${APP}/vite.config.ts`,
  server: { host: "127.0.0.1", port: 1420, strictPort: true, proxy: { "/v1": {
    target: process.env.BACKEND, changeOrigin: true,
    configure: p => p.on("proxyReq", r => r.setHeader("authorization", `Bearer ${process.env.TOKEN}`)) } } } });
await server.listen();
const b = await chromium.launch({ headless: true,
  executablePath: "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", args: ["--no-sandbox"] });
// navigate http://127.0.0.1:1420, drive, page.screenshot(), pull back
```

Final/native verification still uses the real exe via the `/it` capture above; this loop is for
fast visual iteration against the real backend.
