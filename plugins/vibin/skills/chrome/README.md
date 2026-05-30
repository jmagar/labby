# chrome

Drive a Chrome browser running on a remote machine via the Chrome DevTools Protocol over SSH.

## What it does
- Lists tabs in the remote Chrome (must be launched with `--remote-debugging-port=9222`)
- Screenshots a tab by title/URL substring — works even if minimized
- Evaluates arbitrary JS in a tab via `Runtime.evaluate`
- Reads cookies, navigates, runs any CDP method via a generic `cdp-call.ps1` invoker
- Survives auto-enabled domains by looping past unsolicited CDP events until `id:1` arrives

## When to invoke
The user asks about "my chrome", "my tabs", a tab they have open, etc. — anything that implies *the user's real Chrome session* rather than a fresh headless browser (use `agent-browser` for that) or full-desktop pixels (use `screenshots`).

## Files
- `SKILL.md` — entry point, defaults, helpers
- `scripts/cdp-call.ps1` — generic CDP RPC invoker, supports `-ParamsStdin` to avoid shell-quoting hell
- `scripts/cdp-shot.ps1` — tab screenshot

## Setup
Remote Chrome needs `chrome.exe --remote-debugging-port=9222 --user-data-dir=C:\\chrome-debug`. On the default host (`steamy-wsl`) there's a "Chrome (debug)" desktop shortcut wired up for this.
