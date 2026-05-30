---
name: chrome
description: 'Use when the user wants to inspect or control a real Chrome session over SSH via Chrome DevTools Protocol. For web-dev/browser verification, prefer this ladder: CDP on agent-os, agent-browser, claude-in-chrome on agent-os, agent-os Windows-MCP, then claude-in-chrome on steamy. Triggers imply a real Chrome session: "grab my chrome tab", "show me my tabs", "screenshot my <site> tab", "what''s open in my chrome", "eval this in my browser", "cookies from my chrome", "navigate my chrome to", "what''s the page console showing", "check my chrome network requests". For the user''s personal Chrome on steamy, require Chrome launched with `--remote-debugging-port=9222`; for generic automation, fall back instead of stopping.'
---

# chrome

Talk to a real, running Chrome instance on a remote machine via CDP (Chrome DevTools Protocol). The remote Chrome must be launched with `--remote-debugging-port=<PORT>`. Everything in this skill is one SSH-hop away and stays on the user's machine — no data leaves their box except the response payload you fetch back.

## Preferred web-dev tool priority

For web development, browser verification, screenshots, and interactive page checks, use this order unless the user explicitly asks for a specific machine or browser session:

1. **CDP running on agent-os** - best first choice when the agent-os Chrome debug endpoint is available.
2. **agent-browser** - best fallback for fresh automation, screenshots, form flows, and ref-based browser testing.
3. **claude-in-chrome on agent-os** - use when the workflow specifically needs the Claude-in-Chrome path on the sandbox VM.
4. **agent-os Windows-MCP** - use for OS-level control, desktop apps, PowerShell, or browser tasks that cannot be handled cleanly through CDP/agent-browser.
5. **claude-in-chrome on steamy** - last choice for the user's personal desktop/session.

If a higher-priority surface is unavailable, record the observed failure briefly and move to the next option. Only stop for user action when the task specifically requires the user's personal Chrome session and steamy CDP is down.

## Defaults (override via env vars)

```bash
SSH_TARGET="${CHROME_HOST:-steamy-wsl}"
CHROME_PORT="${CHROME_PORT:-9222}"
POWERSHELL="${CHROME_POWERSHELL:-/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe}"
REMOTE_DIR="${CHROME_REMOTE_DIR:-/mnt/c/screens}"
NATIVE_DIR="${CHROME_NATIVE_DIR:-C:\\screens}"
SKILL_DIR=/home/jmagar/.agents/src/skills/chrome
```

The host needs a Chrome started like:
```
chrome.exe --remote-debugging-port=9222 --user-data-dir=C:\chrome-debug
```
On the default host there's a "Chrome (debug)" desktop shortcut wired to this. If `curl -s http://127.0.0.1:9222/json` from the remote returns nothing, ask the user to launch the debug Chrome and open the target page in *that* window — a normal Chrome session won't expose CDP.

## Sanity check (do this first)

```bash
ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -Command \"try { Invoke-RestMethod -Uri http://127.0.0.1:$CHROME_PORT/json/version -TimeoutSec 3 | Select-Object Browser,'User-Agent' } catch { 'CDP_DOWN' }\""
```

If you see `CDP_DOWN`, follow the web-dev priority ladder:

- For generic web-dev verification, screenshots, and automation, fall back to `agent-browser`.
- For sandbox-specific browser or desktop work, use `agent-os` through CDP if possible, then `claude-in-chrome` on agent-os, then Windows-MCP.
- For explicit "my Chrome", "my tabs", cookies, or steamy personal-session tasks, ask the user to start the debug Chrome and open the target page in that window.

Everything below assumes the selected CDP endpoint is live.

## List tabs

```bash
ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -Command \"(Invoke-RestMethod -Uri http://127.0.0.1:$CHROME_PORT/json -TimeoutSec 3) | Where-Object { \\\$_.type -eq 'page' } | ForEach-Object { '{0}  ::  {1}' -f \\\$_.title, \\\$_.url }\""
```

Pick a tab by title or URL substring — every helper below takes a `-Pattern` that does a case-insensitive substring match against both fields.

## Workhorse — generic CDP call

`scripts/cdp-call.ps1` opens a WebSocket to a tab (or to the browser endpoint), sends one JSON-RPC call, and prints the raw response. Stage it once per session:

```bash
scp -q "$SKILL_DIR/scripts/cdp-call.ps1" "$SSH_TARGET:$REMOTE_DIR/cdp-call.ps1"
scp -q "$SKILL_DIR/scripts/cdp-shot.ps1" "$SSH_TARGET:$REMOTE_DIR/cdp-shot.ps1"

cdp() {
  local pat="$1" method="$2" params="${3:-{\}}"
  ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File '$NATIVE_DIR\\cdp-call.ps1' -Pattern '$pat' -Port $CHROME_PORT -Method '$method' -Params '$params'" 2>/dev/null
}
```

Now any CDP method works:

```bash
cdp github 'Page.navigate' '{"url":"https://example.com"}' | jq
cdp 'github.com' 'Runtime.evaluate' '{"expression":"document.title","returnByValue":true}' | jq .result.result.value
cdp '' 'Network.getCookies' '{}' | jq '.result.cookies | length'    # all cookies for active tab
```

`-Browser` switches to the browser-wide endpoint (for things like `Target.getTargets`, `Browser.getVersion`).

## Tab screenshot — works even if minimized

CDP renders off-screen; window state doesn't matter.

```bash
chrome_shot() {
  local pat="$1"
  local name=$(ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File '$NATIVE_DIR\\cdp-shot.ps1' -Pattern '$pat' -Port $CHROME_PORT -OutDir '$NATIVE_DIR'" 2>/dev/null | tr -d '\r\n')
  [ -z "$name" ] && { echo "no tab matched '$pat'"; return 1; }
  local dest="${CLAUDE_JOB_DIR:-/tmp}/$name"
  ssh "$SSH_TARGET" "cat \"$REMOTE_DIR/$name\"" > "$dest"
  echo "$dest"
}

chrome_shot 'github.com'        # screenshot the github tab → Read the result
```

For full-page (beyond-viewport) screenshots, call `cdp` with `Page.captureScreenshot` and `{"captureBeyondViewport":true}`.

## Evaluate JS in a tab

```bash
# Pipe expression over SSH stdin so apostrophes/quotes in JS don't fight the shell.
chrome_eval() {
  local pat="$1" expr="$2"
  local params=$(printf '%s' "$expr" | python3 -c 'import sys,json;print(json.dumps({"expression":sys.stdin.read(),"returnByValue":True,"awaitPromise":True}))')
  printf '%s' "$params" | ssh "$SSH_TARGET" "$POWERSHELL -NoProfile -ExecutionPolicy Bypass -File '$NATIVE_DIR\\cdp-call.ps1' -Pattern '$pat' -Port $CHROME_PORT -Method Runtime.evaluate -ParamsStdin" \
    | jq '.result.result.value // .result.exceptionDetails'
}

chrome_eval github 'document.querySelectorAll("a[href*=\'foo\']").length'   # apostrophes safe
chrome_eval github 'fetch("/api/foo").then(r=>r.json())'                       # awaitPromise unwraps it
```

`returnByValue:true` is important — without it CDP returns an `objectId` reference, not the actual value. The `-ParamsStdin` switch on `cdp-call.ps1` keeps the JS expression out of every shell quoting hazard between bash → ssh → PowerShell.

## Console messages

CDP streams console events; to collect them, enable Runtime, then keep the socket open. For a one-shot "show me what's already in the console", scrape via JS instead (Chrome doesn't replay past console events to a new attached client):

```bash
chrome_eval github 'console.history?.slice(-50)'   # only works if a userscript captured them
```

For live capture, use the more complex `cdp-listen.ps1` pattern (not bundled — add it if a session keeps wanting it). Or open DevTools manually and ask the user to copy what's there.

## Network requests

Same caveat: CDP only sees requests *after* `Network.enable` is sent. To inspect prior traffic, ask the user to open DevTools → Network → export HAR, then pull the HAR file via ssh.

For ongoing traffic capture (e.g. "show me what this page is fetching when I click X"), enable Network and stream events:

```bash
cdp github 'Network.enable' '{}'
# then the page actions happen
cdp github 'Network.getResponseBody' '{"requestId":"..."}'   # need the requestId from streamed events
```

## Cookies

```bash
cdp '' 'Network.getCookies' '{}' | jq '.result.cookies[] | {name, domain, value: (.value[0:20])}'
# Storage.getCookies is browser-context scoped; pass a browserContextId to target incognito.
cdp '' 'Storage.getCookies' '{}' | jq '.result.cookies | length'
```

## Navigate / reload / close

```bash
cdp github 'Page.navigate' '{"url":"https://example.com"}'
cdp github 'Page.reload' '{}'
# closing requires the target id from Target.getTargets:
cdp '' 'Target.closeTarget' '{"targetId":"<id from getTargets>"}'
```

## DOM snapshot

For "what's on this page" without screenshotting:

```bash
chrome_eval github 'document.body.innerText.slice(0,2000)'   # quick text dump
# or use CDP for a structured tree:
cdp github 'DOMSnapshot.captureSnapshot' '{"computedStyles":[]}' | jq '.result | keys'
```

## Adapting to another machine

Persist via `~/.claude/settings.json`'s `env` block (reloaded per session):

```json
{
  "env": {
    "CHROME_HOST": "workbox",
    "CHROME_PORT": "9223",
    "CHROME_REMOTE_DIR": "~/Downloads",
    "CHROME_NATIVE_DIR": "",
    "CHROME_POWERSHELL": ""
  }
}
```

One-shot override inline:
```bash
CHROME_HOST=workbox CHROME_PORT=9223 <paste any snippet>
```

If the target is non-Windows (macOS/Linux), drop `$POWERSHELL` and use `curl`/`websocat` directly:
```bash
ssh "$SSH_TARGET" "curl -s http://127.0.0.1:$CHROME_PORT/json"
# websocat would handle CDP WebSocket calls — install on the remote if not present
```


## How `cdp-call.ps1` works

The script opens one WebSocket, sends `{"id":1, method, params}`, then **loops past any unsolicited events** (frames without an `id` field) until it receives the response with `id:1`. That means it's safe to call against tabs where another tool has already done `Runtime.enable` / `Page.enable` — the events get discarded silently rather than confusing the reply.

## Notes

- **CDP target scopes**: tab (page) vs browser. `cdp-call.ps1 -Browser` switches. Some methods (Browser.*, Target.*) only work on the browser endpoint.
- **One call per ws connection** in `cdp-call.ps1`. That's fine for ad-hoc work; for streams (Network/Page events), you need a long-lived connection — extend the script if needed.
- **Headless Chrome on the same port**: doesn't conflict, but you'll get *both* sets of tabs in `/json`. Filter by `-Pattern`.
- **agent-browser fallback**: agent-browser spawns its own Chromium locally for automation. Prefer it when the task does not need an existing real Chrome profile/session, especially after CDP is unavailable.
- **Sister skill `screenshots`** handles full-desktop captures (which CDP can't see). Use `chrome_shot` when you want a specific tab; use `screenshots` Mode 2 when you want the whole monitor.
