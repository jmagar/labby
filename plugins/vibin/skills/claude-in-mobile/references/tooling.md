# claude-in-mobile Tooling Notes

## Current Upstream

Last verified: 2026-05-23 against npm `claude-in-mobile@3.8.1`.

- npm package: `claude-in-mobile`
- repository: `https://github.com/AlexGladkov/claude-in-mobile`
- verify latest version with `npm view claude-in-mobile version`

## Setup Commands

```bash
# MCP for Codex
codex mcp add mobile -- npx -y claude-in-mobile

# MCP for Claude Code
claude mcp add --scope user --transport stdio mobile -- npx claude-in-mobile@latest

# Native CLI on macOS
brew tap AlexGladkov/claude-in-mobile https://github.com/AlexGladkov/claude-in-mobile
brew install claude-in-mobile

# Install generated skill files for Codex from the native CLI
claude-in-mobile setup codex --global
```

## Platform Setup

- Android: set `ADB_PATH` if auto-discovery misses ADB. Discovery checks
  `ADB_PATH`, `ANDROID_HOME`, `ANDROID_SDK_ROOT`, common OS SDK locations, then
  `adb` from `PATH`.
- iOS: use a booted Simulator. Install Appium and the xcuitest driver for WDA:
  `npm install -g appium && appium driver install xcuitest`.
- Desktop: macOS only in current upstream docs. Grant Accessibility permission
  to the automation host before driving windows.
- Browser: Chrome/Chromium must be installed or `CHROME_PATH` must point to it.
- Aurora OS: install `audb-client`, enable SSH on the device, and install Python
  on device for tap/swipe support.

## Coordinate Rule

Raw `input` coordinates come from the most recent screenshot pixel space and can
be auto-scaled to the device. Coordinates from `ui(action:'find')` and
`ui(action:'tree')` are already device coordinates. Prefer `index`, `text`,
`resourceId`, or `label` when acting on UI-tree results.

## Useful Native CLI Checks

```bash
claude-in-mobile doctor
claude-in-mobile --version
claude-in-mobile screenshot android -o screenshot.png
claude-in-mobile ui-dump android
claude-in-mobile launch android com.example.app
claude-in-mobile input android "test@example.com"
```
