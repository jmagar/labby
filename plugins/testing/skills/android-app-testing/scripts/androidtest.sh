#!/usr/bin/env bash
# android-app-testing driver — direct local adb primitives.
#
# Wraps the verified-working adb drive loop so the agent doesn't re-derive it
# each run. PRIMARY path is direct local adb (no gateway dependency) — this is
# what was live-validated 2026-05-29. The claude-in-mobile gateway path (richer
# semantic locators) is optional and currently blocked by a container adb gap;
# see references/claude-in-mobile-path.md.
#
# Usage:
#   androidtest.sh boot   [<avd>]            # launch emulator headless, wait for boot_completed
#   androidtest.sh ready                     # print serial + boot state, exit 0 if a device is ready
#   androidtest.sh shot   <run_dir> <name>   # screencap -> evidence/<name>.png on host
#   androidtest.sh tree   <run_dir> <name>   # uiautomator dump -> evidence/<name>.xml on host
#   androidtest.sh launch <pkg>[/<activity>] # am start (activity optional via monkey)
#   androidtest.sh stop   <pkg>              # am force-stop
#   androidtest.sh install <apk>             # adb install -r -g
#   androidtest.sh tapxy  <x> <y>            # input tap
#   androidtest.sh taptext <run_dir> <text>  # find <text> in a fresh uiautomator dump, tap its center
#   androidtest.sh text   "<string>"         # input text (focused field)
#   androidtest.sh key    <KEYCODE|name>     # input keyevent (e.g. BACK, HOME, ENTER)
#   androidtest.sh logclear                  # logcat -c
#   androidtest.sh crashes                   # grep logcat buffer for FATAL/ANR/crash since logclear
#   androidtest.sh current                   # current focused activity/package
#
# Env: ADB (path to adb, default ~/Android/Sdk/platform-tools/adb),
#      EMULATOR (path, default ~/Android/Sdk/emulator/emulator),
#      AVD (default axon_test), ANDROID_SERIAL (default first device).
set -euo pipefail

ADB="${ADB:-$HOME/Android/Sdk/platform-tools/adb}"
EMULATOR="${EMULATOR:-$HOME/Android/Sdk/emulator/emulator}"
AVD_DEFAULT="${AVD:-axon_test}"

adb() { "$ADB" ${ANDROID_SERIAL:+-s "$ANDROID_SERIAL"} "$@"; }

cmd="${1:-}"; shift || true

case "$cmd" in
boot)
  avd="${1:-$AVD_DEFAULT}"
  nohup "$EMULATOR" -avd "$avd" -no-window -no-audio -no-boot-anim \
    -gpu swiftshader_indirect -no-snapshot >"/tmp/avd_${avd}.log" 2>&1 &
  echo "emulator launching (pid $!), log /tmp/avd_${avd}.log"
  # wait for boot_completed
  for _ in $(seq 1 60); do
    if [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; then
      echo "BOOT_COMPLETED $(adb get-serialno 2>/dev/null)"; exit 0
    fi
    sleep 5
  done
  echo "TIMEOUT waiting for boot_completed" >&2; exit 1
  ;;
ready)
  adb devices
  echo "boot_completed=$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')"
  echo "model=$(adb shell getprop ro.product.model 2>/dev/null | tr -d '\r') android=$(adb shell getprop ro.build.version.release 2>/dev/null | tr -d '\r')"
  [ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]
  ;;
shot)
  run="$1"; name="$2"; mkdir -p "$run/evidence"
  adb shell screencap -p /sdcard/_shot.png
  adb pull /sdcard/_shot.png "$run/evidence/${name}.png" >/dev/null
  adb shell rm -f /sdcard/_shot.png
  echo "$run/evidence/${name}.png ($(stat -c%s "$run/evidence/${name}.png") bytes)"
  ;;
tree)
  run="$1"; name="$2"; mkdir -p "$run/evidence"
  adb shell uiautomator dump /sdcard/_ui.xml >/dev/null
  adb pull /sdcard/_ui.xml "$run/evidence/${name}.xml" >/dev/null
  adb shell rm -f /sdcard/_ui.xml
  echo "$run/evidence/${name}.xml ($(wc -c <"$run/evidence/${name}.xml") bytes)"
  ;;
launch)
  target="$1"
  if [[ "$target" == */* ]]; then adb shell am start -n "$target"
  else adb shell monkey -p "$target" -c android.intent.category.LAUNCHER 1; fi
  ;;
stop)    adb shell am force-stop "$1"; echo "force-stopped $1" ;;
install) adb install -r -g "$1" ;;
tapxy)   adb shell input tap "$1" "$2" ;;
text)    adb shell input text "${1// /%s}" ;;
key)     adb shell input keyevent "$1" ;;
logclear) adb logcat -c; echo "logcat cleared" ;;
crashes)
  # Surface crash/ANR signals from the buffer (use after logclear + actions).
  adb logcat -d -v brief 2>/dev/null | grep -iE "FATAL EXCEPTION|ANR in|signal [0-9]+ \(SIG|beginning of crash|force.?finishing|has died" || echo "no crash/ANR signals"
  ;;
current)
  adb shell dumpsys activity activities 2>/dev/null | grep -m1 -E "mResumedActivity|ResumedActivity|topResumedActivity" | sed 's/^[[:space:]]*//' || \
  adb shell dumpsys window 2>/dev/null | grep -m1 mCurrentFocus
  ;;
taptext)
  # Find <text> in a fresh uiautomator dump and tap the center of its bounds.
  run="$1"; want="$2"
  adb shell uiautomator dump /sdcard/_ui.xml >/dev/null
  adb pull /sdcard/_ui.xml "/tmp/_ui_tap.xml" >/dev/null
  adb shell rm -f /sdcard/_ui.xml
  # Extract bounds of the first node whose text/content-desc contains $want.
  coords=$(python3 - "$want" <<'PY'
import sys, re
want = sys.argv[1].lower()
xml = open("/tmp/_ui_tap.xml", encoding="utf-8", errors="ignore").read()
for m in re.finditer(r'<node[^>]*>', xml):
    tag = m.group(0)
    t = (re.search(r'text="([^"]*)"', tag) or [None, ""])[1]
    d = (re.search(r'content-desc="([^"]*)"', tag) or [None, ""])[1]
    if want in t.lower() or want in d.lower():
        b = re.search(r'bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"', tag)
        if b:
            x1,y1,x2,y2 = map(int, b.groups())
            print((x1+x2)//2, (y1+y2)//2); break
PY
)
  if [ -z "$coords" ]; then echo "text not found: $want" >&2; exit 1; fi
  adb shell input tap $coords
  echo "tapped \"$want\" at $coords"
  ;;
*)
  echo "unknown command: $cmd" >&2
  sed -n '2,30p' "$0" >&2
  exit 2
  ;;
esac
