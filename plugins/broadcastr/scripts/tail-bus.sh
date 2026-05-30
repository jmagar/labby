#!/usr/bin/env bash
# Tails the per-repo and (optionally) global bus, applies self-suppression
# by $CLAUDE_SESSION_ID, drops events older than monitor startup, drops
# muted categories, and formats one display line per event to stdout.
set -uo pipefail
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
REPO="${CLAUDE_PROJECT_DIR:-$PWD}"
SESSION_ID="${CLAUDE_SESSION_ID:-}"

PER_REPO_BUS="$REPO/.broadcastr/events.jsonl"
GLOBAL_HOME="${BROADCASTR_HOME:-$HOME/.claude/broadcastr}"
GLOBAL_BUS="$GLOBAL_HOME/events.jsonl"
WANT_GLOBAL="${BROADCASTR_GLOBAL_FEED:-1}"

. "$PLUGIN_ROOT/scripts/lib-jq-guard.sh"
require_jq broadcastr-feed

mkdir -p "$(dirname "$PER_REPO_BUS")"
touch "$PER_REPO_BUS"
if [ "$WANT_GLOBAL" != "0" ]; then
  mkdir -p "$GLOBAL_HOME"
  touch "$GLOBAL_BUS"
fi

MUTE_LIST="${BROADCASTR_MUTE:-}"
MUTE_JQ='[]'
if [ -n "$MUTE_LIST" ]; then
  MUTE_JQ="$(printf '%s' "$MUTE_LIST" | tr ',' '\n' | jq -R . | jq -s .)"
fi

STARTUP="$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)"

format_line() {
  # Inline single-quoted jq strings can't contain apostrophes (they break
  # bash's quoting), so the filter lives in format-line.jq and is loaded
  # with -f. The three runtime variables are injected via --arg/--argjson.
  jq --unbuffered -rc \
    --arg     sid     "$SESSION_ID" \
    --arg     startup "$STARTUP" \
    --argjson mute    "$MUTE_JQ" \
    -f "$PLUGIN_ROOT/scripts/format-line.jq"
}

# Dedup by event id: every emit writes to BOTH per-repo and global buses
# when BROADCASTR_GLOBAL_FEED=1, so `tail -F file1 file2` sees the same
# event twice. Strip duplicates by ULID before formatting. The seen-set
# is bulk-purged every 10k entries to keep memory bounded for long
# sessions; the dup window in practice is sub-second so a periodic flush
# loses nothing real.
dedup_events() {
  awk '
    {
      if (match($0, /"id":"evt_[^"]+"/)) {
        id = substr($0, RSTART, RLENGTH)
        if (!(id in seen)) {
          seen[id] = 1
          print; fflush()
        }
        if (length(seen) > 10000) delete seen
      } else {
        print; fflush()
      }
    }
  '
}

cleanup() { pkill -P $$ 2>/dev/null || true; }
trap cleanup SIGTERM SIGINT EXIT

if [ "$WANT_GLOBAL" != "0" ]; then
  tail -n0 -F "$PER_REPO_BUS" "$GLOBAL_BUS" 2>/dev/null \
    | grep --line-buffered -v "^==>" \
    | dedup_events \
    | format_line &
else
  tail -n0 -F "$PER_REPO_BUS" 2>/dev/null | format_line &
fi
wait
