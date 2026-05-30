# Changelog — android-app-testing

## 2026-05-29 — initial release
- Added — initial release. Live end-to-end APK testing on an emulator via direct local adb.
- `scripts/androidtest.sh` — adb driver: `boot` (headless + wait for boot_completed), `ready`,
  `install`, `launch`, `stop`, `shot` (screencap→host), `tree` (uiautomator dump→host), `taptext`
  (semantic tap by text from a live UI dump), `tapxy`, `text`, `key`, `current`, `logclear`,
  `crashes` (FATAL/ANR detection). Self-tested live against `axon_test` (Android 15, 1080×2400):
  launch Settings → shot 153KB → tree 23KB → taptext "Network" navigated to SubSettings.
- `references/report-format.md` — shared cross-platform report spec (duplicated across siblings).
- `references/claude-in-mobile-path.md` — documents the optional claude-in-mobile gateway path and
  its current blocker (labby container has no adb binary, host SDK unmounted, configured adb-server
  address unreachable) with the homelab fix steps.
- Decision: direct local adb is the PRIMARY path because the claude-in-mobile gateway path was
  found non-functional from the container this session; adb covers the full test loop standalone.
