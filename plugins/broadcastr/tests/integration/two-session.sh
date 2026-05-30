#!/usr/bin/env bash
# Simulates two concurrent Claude sessions on the same repo by spawning two
# tail-bus.sh processes and one emitter. Asserts that A's emit shows up in B
# but not in A.
set -euo pipefail

PLUGIN_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TMP=$(mktemp -d)
trap 'pkill -P $$ 2>/dev/null || true; rm -rf "$TMP"' EXIT

REPO="$TMP/repo"
mkdir -p "$REPO/.broadcastr"
cd "$REPO"
git init -q

export CLAUDE_PROJECT_DIR="$REPO"
export BROADCASTR_HOME="$TMP/home"
export BROADCASTR_GLOBAL_FEED=0
export HOSTNAME=testbox USER=tester
unset BROADCASTR_DISABLED BROADCASTR_MUTE

# Session A's monitor
( CLAUDE_SESSION_ID=A "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/a.out" 2>&1 ) &
A_PID=$!
# Session B's monitor
( CLAUDE_SESSION_ID=B "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/b.out" 2>&1 ) &
B_PID=$!

sleep 1.5

# Session A emits
CLAUDE_SESSION_ID=A "$PLUGIN_ROOT/scripts/emit.sh" \
  --category commit --tier info --summary "from-A-cross-session"

sleep 2

kill "$A_PID" "$B_PID" 2>/dev/null || true
wait 2>/dev/null || true

if grep -q "from-A-cross-session" "$TMP/a.out"; then
  echo "FAIL: A saw its own event"
  exit 1
fi
if ! grep -q "from-A-cross-session" "$TMP/b.out"; then
  echo "FAIL: B did not see A's event"
  echo "--- B output ---"
  cat "$TMP/b.out"
  exit 1
fi
echo "PASS: cross-session visibility working"
