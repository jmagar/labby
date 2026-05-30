#!/usr/bin/env bash
# Generic supervisor for inotify watchers. Restarts the watcher on transient
# failure. On a clear "can't arm" error (watch limit, missing dir we can't
# create), emits one alert and exits so silence is visible.
set -uo pipefail

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
NAME="$1"; shift
# The first positional arg after NAME is the per-event handler command;
# remaining args are the directories to watch.
HANDLER="$1"; shift
TARGETS=("$@")

if ! command -v inotifywait >/dev/null 2>&1; then
  "$PLUGIN_ROOT/scripts/emit.sh" \
    --category agent-presence --tier alert --source claude-hook \
    --summary "broadcastr: inotifywait not installed; ${NAME} disabled" \
    --data "{\"monitor\":\"$NAME\"}"
  exit 0
fi

# Pre-create any target directories so inotifywait doesn't bail on missing paths.
# Mkdir failures are surfaced — if even the parent path can't be created,
# the corresponding watch will fail loudly below.
for d in "${TARGETS[@]}"; do
  if ! mkdir -p "$d" 2>/dev/null; then
    echo "broadcastr/${NAME}: mkdir -p $d failed" >&2
  fi
done

# Test-arm each target INDIVIDUALLY so partial failures (e.g. one path's watch
# limit exhausted, another path on a different filesystem) are caught instead
# of being masked by inotifywait's any-success-is-success semantics for multi-
# path invocations. Each probe is 1s; run them in parallel so startup latency
# stays ~1s regardless of how many targets the watcher has.
ARMED=()
FAILED=()
PIDS=()
for d in "${TARGETS[@]}"; do
  inotifywait -q -t 1 -e create "$d" >/dev/null 2>&1 &
  PIDS+=($!)
done
for i in "${!PIDS[@]}"; do
  wait "${PIDS[$i]}"
  rc=$?
  # 0 = event during the 1s window, 2 = timeout (arm worked but no event).
  if [ "$rc" -eq 0 ] || [ "$rc" -eq 2 ]; then
    ARMED+=("${TARGETS[$i]}")
  else
    FAILED+=("${TARGETS[$i]}")
  fi
done

if [ "${#FAILED[@]}" -gt 0 ]; then
  failed_list="$(printf '%s,' "${FAILED[@]}")"
  "$PLUGIN_ROOT/scripts/emit.sh" \
    --category agent-presence --tier alert --source claude-hook \
    --summary "broadcastr: ${NAME} failed to arm for: ${failed_list%,}" \
    --data "{\"monitor\":\"$NAME\",\"failed_targets\":$(printf '%s\n' "${FAILED[@]}" | jq -R . | jq -sc .)}"
fi

if [ "${#ARMED[@]}" -eq 0 ]; then
  exit 0
fi

trap 'pkill -P $$ 2>/dev/null; exit 0' SIGTERM SIGINT

# Track rapid-failure: if the producer dies repeatedly within a short window,
# emit one alert so silence is visible.
fast_failures=0
last_start=0
while true; do
  now=$(date +%s)
  if [ "$((now - last_start))" -lt 5 ]; then
    fast_failures=$((fast_failures + 1))
  else
    fast_failures=0
  fi
  last_start=$now
  if [ "$fast_failures" -ge 5 ]; then
    "$PLUGIN_ROOT/scripts/emit.sh" \
      --category agent-presence --tier alert --source claude-hook \
      --summary "broadcastr: ${NAME} watcher crash-looping; FS events stopped" \
      --data "{\"monitor\":\"$NAME\"}"
    exit 0
  fi

  while IFS= read -r line; do
    # Handler failures get one stderr line per failure so the monitor channel
    # surfaces them rather than silently dropping events.
    "$HANDLER" "$line" || echo "broadcastr/${NAME}: handler failed on: $line" >&2
  done < <(inotifywait -m -q -e close_write,create,moved_to --format '%w%f|%e' "${ARMED[@]}" 2>&1)
  sleep 1
done
