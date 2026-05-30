---
name: android-app-testing
description: 'Use when the user wants to live-test an Android APK end-to-end on an emulator/device and get a works/doesn''t-work + UI/UX report — driving the real app, not writing test code. Triggers: "test my Android app", "QA this APK", "run the app on the emulator and tell me what breaks", "click through every screen", "review the app''s UX", "does my APK work", "test the built APK". Installs/launches the APK, enumerates screens from the accessibility tree, exercises each feature via adb (tap/type/swipe/launch), watches logcat for crashes/ANRs, captures screenshots + UI dumps, and emits a structured report. Primary path is direct local adb; optional richer path via claude-in-mobile. Sibling of web-app-testing and desktop-app-testing (shared report format). Does NOT fire for: building/coding an Android app (use claude-android-ninja / jetpack-compose-expert), iOS, or unit tests.'
---

# android-app-testing

Live, end-to-end testing of a built **Android APK** on an emulator (or device): install, launch,
drive every screen/feature, watch for crashes & ANRs, review UI/UX, and emit a structured
works/doesn't-work report. Companion to `web-app-testing` and `desktop-app-testing` — all three
share one report format (`references/report-format.md`).

## Two paths (use the primary)
- **PRIMARY — direct local adb** (`scripts/androidtest.sh`). Fully validated, no gateway needed.
  Everything below uses this.
- **OPTIONAL — claude-in-mobile** via the Lab gateway (richer semantic locators, autopilot crawler).
  Currently blocked by a container adb gap — see `references/claude-in-mobile-path.md`. Don't reach
  for it unless that's been fixed; the adb path covers the full loop.

## Prerequisites
- **Android SDK** with `adb` + `emulator` (default `~/Android/Sdk/...`; override `ADB`/`EMULATOR`).
- **An AVD** (default `axon_test`; override `AVD`). The emulator does NOT auto-boot — step 1 boots it.
- The APK to test (a path on this host).

## The driver
`scripts/androidtest.sh` wraps the verified adb drive loop. Commands:
```
androidtest.sh boot   [<avd>]            # launch headless, wait for sys.boot_completed
androidtest.sh ready                     # serial + boot/model/android; exit 0 if ready
androidtest.sh install <apk>             # adb install -r -g
androidtest.sh launch <pkg>[/<activity>] # am start (or monkey LAUNCHER if no activity)
androidtest.sh stop   <pkg>              # am force-stop
androidtest.sh shot   <run_dir> <name>   # screencap -> evidence/<name>.png (pulled to host)
androidtest.sh tree   <run_dir> <name>   # uiautomator dump -> evidence/<name>.xml (host)
androidtest.sh taptext <run_dir> <text>  # find text in a fresh UI dump, tap its center
androidtest.sh tapxy  <x> <y>            # input tap
androidtest.sh text   "<string>"         # input text into focused field
androidtest.sh key    <BACK|HOME|ENTER|…># input keyevent
androidtest.sh current                   # current focused activity/package
androidtest.sh logclear                  # logcat -c  (call before a feature)
androidtest.sh crashes                   # grep buffer for FATAL/ANR/SIG/died
```
All primitives are live-validated (2026-05-29) against `axon_test` (Android 15, 1080×2400).

## Workflow

1. **Boot & confirm.** `androidtest.sh boot` (waits for `sys.boot_completed`), then `ready`. Create
   the run dir `~/.agents/docs/sessions/<app>-android-test/run_<id>/`.
2. **Install.** `androidtest.sh install <apk>`. Note the package name
   (`aapt dump badging <apk> | grep package`, or `adb shell pm list packages -3` after install).
3. **Launch & map.** `launch <pkg>`; `shot`/`tree` the first screen. Parse the `uiautomator` XML to
   enumerate clickable elements, text fields, tabs, nav targets. Build the feature checklist (merge
   with any user-supplied spec) — one row per feature in the report. Write `plan.md` in the run dir.
4. **Exercise each feature.** For each: `logclear` → act (`taptext` for semantic taps, `tapxy` for
   coords, `text`/`key` for input, `swipe` via the gateway or `input swipe` coords) → `shot` +
   `tree` for evidence → check `current` (did navigation happen?) and `crashes` (FATAL/ANR?).
   Classify PASS / PARTIAL / FAIL / BLOCKED.
   - Prefer **`taptext`** over raw coordinates — it survives layout changes (coords don't).
5. **Detect failures** after each action:
   - **Crash** — `crashes` shows `FATAL EXCEPTION` / `signal …(SIG…)` / `has died`. → FAIL.
   - **ANR/hang** — `crashes` shows `ANR in`, or `current` stops changing / UI frozen. → FAIL.
   - **Wrong result / no feedback** — screen didn't change when it should, or wrong screen. → PARTIAL/FAIL.
   - **Can't reach** — needs login/data/permissions the run lacks. → BLOCKED.
6. **Reset between independent features** — `stop <pkg>` then `launch` to avoid state leaking.
7. **UX/a11y pass** — score the rubric in the report format from the UI dumps + screenshots. Nodes
   in the `uiautomator` XML with empty `text` AND empty `content-desc` on interactive elements =
   accessibility findings.
8. **Write the report** → `report.md` + `result.json` in the run dir, per
   `references/report-format.md`.

## Evidence
Run-dir layout per `references/report-format.md`: `evidence/*.png` (screencaps), `evidence/*.xml`
(uiautomator dumps), logcat captures, `result.json`. The driver pulls screenshots/dumps from the
device to the host run dir automatically.

## Gotchas (live-validated)
- The emulator drops its adb registration if `adb kill-server` is run — avoid it; if a device
  disappears, the qemu process is usually still alive and re-registers, or just `boot` a fresh one.
- `am start -n pkg/.Activity` needs the real activity; if unknown, use `launch <pkg>` (monkey
  LAUNCHER) which resolves the launcher activity.
- `input text` can't type spaces directly — the driver encodes them (`%s`); for complex strings
  prefer per-field taps.
- Headless GPU: `-gpu swiftshader_indirect` is reliable for CI-style headless; custom-rendered
  (game/Canvas/Flutter-impeller) UIs may expose little in `uiautomator` → fall back to screenshot +
  coordinate taps and flag reduced confidence.

## References
- `references/report-format.md` — shared cross-platform report spec, run-dir layout, verdicts.
- `references/claude-in-mobile-path.md` — the optional gateway path + its current blocker + fix.
