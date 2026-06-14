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

### Labby + Android Device Recovery

When `claude-in-mobile` is exposed through Labby, the MCP server can be healthy
while Android still reports `no device`. Do not stop there. First distinguish
tool availability from target availability.

```bash
# Confirm the upstream and helper namespace.
labby gateway list | rg -i 'claude-in-mobile|mobile'
labby gateway code exec --json --code \
  'async () => Object.keys(codemode.claude_in_mobile)'

# List targets through Labby.
labby gateway code exec --json --code \
  'async () => await codemode.claude_in_mobile.device({ action: "list" })'
```

If the Android system call fails with an ADB error such as:

```text
failed to connect to '172.19.0.1:5037': Connection refused
cannot start server on remote host
```

then the gateway can see the host, but host ADB is only listening on loopback.
Restart ADB with remote binding:

```bash
adb kill-server
adb -a start-server
ss -ltnp | rg ':5037'   # should show *:5037, not only 127.0.0.1:5037
```

If no device is connected, create or boot an emulator rather than reporting
that claude-in-mobile cannot test Android:

```bash
sdkmanager --list_installed | rg 'emulator|platform-tools|system-images;android'
avdmanager list avd

# Create one if needed, using an installed system image.
echo no | avdmanager create avd \
  -n mobile_debug \
  -k 'system-images;android-35-ext15;google_apis;x86_64' \
  -d pixel_6 --force

# Headless launch that works well under agent sessions.
/home/$USER/Android/Sdk/emulator/emulator -avd mobile_debug \
  -no-window -no-audio -no-boot-anim -no-snapshot \
  -gpu off -camera-back none -camera-front none -verbose
```

In another shell, wait for boot and re-check Labby:

```bash
for i in $(seq 1 120); do
  serial=$(adb devices | awk '/^emulator-[0-9]+[[:space:]]+device/{print $1; exit}')
  if [ -n "$serial" ]; then
    boot=$(adb -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')
    echo "serial=$serial boot=$boot"
    [ "$boot" = "1" ] && break
  fi
  sleep 2
done

labby gateway code exec --json --code \
  'async () => await codemode.claude_in_mobile.device({ action: "list" })'
```

Once Android appears, continue with install, launch, screenshots, UI tree, and
crash logs:

```bash
labby gateway code exec --json --code \
  'async () => await codemode.claude_in_mobile.app({ action: "install", path: "/tmp/app.apk" })'
labby gateway code exec --json --code \
  'async () => await codemode.claude_in_mobile.app({ action: "launch", package: "com.example.app" })'
adb -s emulator-5554 logcat -d -b crash
```

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
