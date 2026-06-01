# Optional path: claude-in-mobile via the Lab gateway

The **primary** android-app-testing path is direct local adb (`scripts/androidtest.sh`) — it needs
nothing but the host SDK + a running emulator and is fully validated. This file documents the
**optional** richer path through the `claude-in-mobile` MCP server, and its current blocker.

## What it adds
`claude-in-mobile` (TypeScript MCP, by AlexGladkov) wraps adb with higher-level, agent-friendly
actions and **semantic locators** — tap by `text`/`id`/`index` against a parsed accessibility tree,
screenshot compression/diffing, an `autopilot` BFS/DFS crawler, visual-regression and a11y-audit
tools. Nice for exhaustive mapping; not required for a solid test pass.

## Tool surface (upstream names + params)
Reached as upstream `claude-in-mobile` on the Lab gateway. Action-routed meta-tools:
- `device` — `list` / `set_target {device}` / `get_target` / `enable_module`
- `app` — `launch {package}` / `stop` / `install` / `list`
- `input` — `tap`/`double_tap`/`long_press` (coords or `text`/`id`/`label`/`index`), `swipe {direction}`, `text`
- `screen` — `capture` (compression/diff), `annotate`
- `ui` — `tree` (a11y tree), `find`, `find_tap`
- `system` — `shell`, `logs`, clipboard, permissions, files
- `flow` — `batch` / `run` / `parallel` (multi-step automation)

## How it's invoked here
Through the Lab gateway. In a Code Mode session:
```js
// mcp__plugin_lab_lab__execute
async () => callTool("upstream::claude-in-mobile::device", { action: "list", platform: "android" })
```
(Outside Code Mode the same upstream may appear as direct `mcp__claude-in-mobile__*` tools.)

## ⚠️ Current blocker (verified 2026-05-29) — container adb gap
The gateway runs in the `labby` Docker container, and claude-in-mobile inside it **cannot reach an
adb binary or the emulator**:
- `device {action:"list"}` → `Unknown platform: undefined`; with `platform:"android"` →
  `ADB_NOT_INSTALLED`.
- Probed inside the container: no `adb` binary; host SDK `/home/jmagar/Android/Sdk` NOT mounted; the
  configured `ANDROID_ADB_SERVER_ADDRESS=172.19.0.1:5037` (from `~/.lab/config.toml`) is UNREACHABLE
  from the container network.

**To enable this path (homelab fix, not part of the skill):**
1. Make adb reachable from the labby container — either mount the host SDK
   (`-v $HOME/Android/Sdk:$HOME/Android/Sdk:ro`) and set `ADB_PATH`, or install
   `android-platform-tools` in the image.
2. Point `ANDROID_ADB_SERVER_ADDRESS` at an adb server the container can actually reach (verify with
   `docker exec labby sh -c 'timeout 3 sh -c "echo > /dev/tcp/<ip>/5037"'`), or run adb in
   `-a` listen-all mode on the host and use the host's bridge IP.
3. Re-test `device {action:"list", platform:"android"}` → should list `emulator-5554`.

Until then, **use the direct adb path** (`scripts/androidtest.sh`). It covers the full test loop.
