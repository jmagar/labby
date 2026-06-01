---
name: claude-in-mobile
description: Use when the user wants to use claude-in-mobile, its MCP server, or its native CLI to automate Android devices/emulators, iOS Simulators, Aurora OS devices, macOS desktop apps, Chrome/Chromium CDP sessions, screenshots/logs, UI inspection, accessibility or visual checks, multi-device flows, or app-store release operations.
---

# Claude in Mobile

Use this skill for `claude-in-mobile` MCP and native CLI workflows.

## What It Is

`claude-in-mobile` is an MCP server plus a native CLI for automating Android,
iOS Simulator, macOS desktop apps, Aurora OS devices, Chrome/Chromium sessions,
quality checks, and app-store release operations. It exposes token-efficient
meta-tools instead of many single-purpose tools.

## MCP Configuration

For Codex:

```bash
codex mcp add mobile -- npx -y claude-in-mobile
```

For Claude Code:

```bash
claude mcp add --scope user --transport stdio mobile -- npx claude-in-mobile@latest
```

Equivalent JSON config:

```json
{
  "mcpServers": {
    "mobile": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "claude-in-mobile@latest"]
    }
  }
}
```

Use MCP tools for interactive agent automation. Use the native CLI for scripts,
CI smoke tests, local setup, or quick manual checks. Check the current package
with `npm view claude-in-mobile version` before pinning a version.

## Requirements

- Android: `adb` in `PATH`, plus a connected USB-debuggable device or emulator.
- iOS: macOS, Xcode, a booted iOS Simulator, and WebDriverAgent/Appium
  xcuitest for full UI tree inspection.
- Desktop: macOS and Accessibility permission for the automation host.
- Aurora OS: `audb`/`audb-client`, SSH enabled on the device, and Python
  installed on the device for tap and swipe support.
- Browser: Chrome or Chromium reachable through CDP when using browser tools.
- Store publishing: platform credentials and explicit package/artifact/track
  confirmation before upload, rollout, promote, halt, or submit operations.

Run `claude-in-mobile doctor` first when setup is uncertain. It checks common
dependencies such as ADB, Android SDK paths, Xcode/simctl, Appium/WDA, JDK,
audb-client, and Chrome.

## Core Tool Families

- `device`: list devices, set/get active target, and enable/disable modules.
- `input`: tap, long press, swipe, text, and key events.
- `screen`: capture and annotate screenshots.
- `ui`: inspect trees, find elements, tap text, wait, and assert UI state.
- `app`: launch, stop, install, and list apps.
- `system`: shell, logs, info, URLs, clipboard, permissions, files, and metrics.
- `flow_batch`: execute multiple sequential operations in one round trip.
- `flow_run`: run conditional or repeated automation flows.
- `flow_parallel`: fan out the same action across multiple devices.

Quality tools:

- `accessibility`: audit for labels, touch targets, focus order, and duplicates.
- `visual`: save baselines and compare screenshots for visual regressions.
- `recorder`: record and replay taps, swipes, and text input.
- `sync`: coordinate multi-device test barriers.
- `autopilot`: explore apps with BFS/DFS and self-healing locators.
- `performance`: collect CPU, memory, FPS, and snapshot metrics.

Optional modules:

- `browser`: Chrome/Chromium navigation, clicks, form fill, screenshots, and JS.
- `desktop`: app launch, windows, focus, resize, clipboard, performance, and
  monitor operations.
- `store`: Google Play, Huawei AppGallery, and RuStore upload/release workflows.

## Common Workflows

Device discovery: list connected devices, set the active target, then capture a
screenshot before taking action.

Visual inspection: capture an annotated screenshot, inspect the UI tree, then
tap by text, accessibility label, resource id, or screenshot index. Prefer
semantic locators over raw coordinates.

Cross-platform app smoke test: launch the app on Android and iOS, wait for the
first screen, assert expected text, capture screenshots, and collect logs on
failure.

QA pass: run accessibility audit, capture a visual baseline or compare against
one, record a short repro if useful, and take performance snapshots around the
interaction under test.

Desktop app test: enable the desktop module, launch or attach to the macOS app,
focus the window, resize to a stable viewport, inspect windows, then drive
clicks and keyboard input.

Browser test: enable or call the browser module, open/navigate to the URL,
snapshot DOM refs, click/fill by ref or selector, wait for selectors, and
capture visual evidence.

Store release: confirm package name, artifact path, store, track, and rollout
intent before uploading or submitting. Treat store actions as destructive.

See [references/tooling.md](references/tooling.md) for setup commands, platform
details, and coordinate handling.

## Native CLI Examples

These examples assume the native CLI binary is installed via Homebrew or a
release artifact. The npm package is primarily used for MCP stdio startup.

```bash
claude-in-mobile doctor
claude-in-mobile screenshot android
claude-in-mobile tap android 540 960 --from-size 540x960
claude-in-mobile input android "hello world"
claude-in-mobile ui-dump android | grep "Login"
claude-in-mobile store upload --package com.example.app --file app.aab
```

## Guardrails

- Verify the active target before any destructive action.
- Prefer UI text or accessibility identifiers over raw coordinates when
  possible.
- Raw tap/swipe coordinates are interpreted in the most recent screenshot's
  pixel space and may be auto-scaled. UI-tree coordinates are device
  coordinates. Do not mix them blindly.
- Collect screenshots and logs before changing app/device state during bug
  investigation.
- Treat `system shell`, file operations, permission changes, and store actions
  as privileged operations.
- Do not run store publishing, rollout, or destructive device commands without
  explicit package, artifact, and target confirmation.
