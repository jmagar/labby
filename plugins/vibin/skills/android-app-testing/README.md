# android-app-testing

Live end-to-end testing of a built **Android APK** on an emulator/device, producing a
works/doesn't-work + UI/UX report. One of three sibling testing skills (`web-app-testing`,
`android-app-testing`, `desktop-app-testing`) sharing a common report format.

## When to invoke
- "test my Android app", "QA this APK", "run it on the emulator and tell me what breaks", "click
  through every screen", "review the app's UX", "does my APK work".
- NOT for building/coding an Android app (`claude-android-ninja`, `jetpack-compose-expert`), iOS, or
  unit tests.

## How it works
Primary path is **direct local adb** (`scripts/androidtest.sh`): boot emulator → install → launch →
enumerate screens from `uiautomator` dumps → drive features (`taptext`/`tapxy`/`text`/`key`) →
watch `logcat` for crashes/ANRs → capture screenshots + UI dumps → structured report. An optional
richer path via the `claude-in-mobile` MCP server is documented but currently blocked by a container
adb gap (see references).

## Files
- `SKILL.md` — workflow, driver commands, failure taxonomy, gotchas.
- `scripts/androidtest.sh` — the adb driver (boot/install/launch/shot/tree/taptext/crashes/…).
  Self-tested live against `axon_test` (Android 15).
- `references/report-format.md` — shared cross-platform report spec, run-dir layout, verdict words.
- `references/claude-in-mobile-path.md` — the optional gateway path + its current blocker + fix.

## Prerequisites
- Android SDK with `adb` + `emulator` (override via `ADB`/`EMULATOR`).
- An AVD (default `axon_test`, override `AVD`). The driver boots it; it does not auto-start.
- The APK file on this host.

## Companion skills
- `web-app-testing`, `desktop-app-testing` — same testing job, other targets, same report.
- `claude-in-mobile` — the underlying mobile MCP/CLI (optional richer path).
- `claude-android-ninja`, `jetpack-compose-expert` — for *building* Android apps, not testing them.
