#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  export CLAUDE_PROJECT_DIR="$TMP/repo"
  export BROADCASTR_HOME="$TMP/home"
  export BROADCASTR_GLOBAL_FEED=0
  export HOSTNAME=testbox
  export USER=tester
  unset BROADCASTR_DISABLED BROADCASTR_MUTE
  mkdir -p "$CLAUDE_PROJECT_DIR/.broadcastr"
}

teardown() { rm -rf "$TMP"; }

@test "tail-bus drops own session events" {
  CLAUDE_SESSION_ID=mine "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "mine-event"
  CLAUDE_SESSION_ID=theirs "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "theirs-event"

  # tail starts AFTER both emits — startup gate should drop both since both pre-date tail start.
  # So we need to emit AFTER tail starts to validate self-suppression specifically.
  > "$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"

  ( CLAUDE_SESSION_ID=mine timeout 3 "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/out.txt" 2>&1 || true ) &
  TAILPID=$!
  sleep 1
  CLAUDE_SESSION_ID=mine "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "mine-after"
  CLAUDE_SESSION_ID=theirs "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "theirs-after"
  sleep 1
  kill $TAILPID 2>/dev/null || true
  wait 2>/dev/null || true

  ! grep -q "mine-after" "$TMP/out.txt"
  grep -q "theirs-after" "$TMP/out.txt"
}

@test "tail-bus drops muted categories" {
  export BROADCASTR_MUTE=plan-exec

  ( CLAUDE_SESSION_ID=mine timeout 3 "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/out.txt" 2>&1 || true ) &
  TAILPID=$!
  sleep 1
  CLAUDE_SESSION_ID=other "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "kept-event"
  CLAUDE_SESSION_ID=other "$PLUGIN_ROOT/scripts/emit.sh" --category plan-exec --tier info --summary "muted-event"
  sleep 1
  kill $TAILPID 2>/dev/null || true
  wait 2>/dev/null || true

  grep -q "kept-event" "$TMP/out.txt"
  ! grep -q "muted-event" "$TMP/out.txt"
}

@test "tail-bus dedups events that appear in both buses" {
  # Enable global bus + tail both. Single emit should produce a single
  # notification line, not two.
  unset BROADCASTR_GLOBAL_FEED
  export BROADCASTR_GLOBAL_FEED=1

  ( CLAUDE_SESSION_ID=mine timeout 3 "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/out.txt" 2>&1 || true ) &
  TAILPID=$!
  sleep 1
  CLAUDE_SESSION_ID=other "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "dedup-event"
  sleep 1.5
  kill $TAILPID 2>/dev/null || true
  wait 2>/dev/null || true

  count=$(grep -c "dedup-event" "$TMP/out.txt" || true)
  [ "$count" = "1" ] || { echo "expected 1, got $count"; cat "$TMP/out.txt"; false; }
}

@test "tail-bus formats feed lines compactly" {
  ( CLAUDE_SESSION_ID=mine timeout 3 "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/out.txt" 2>&1 || true ) &
  TAILPID=$!
  sleep 1
  CLAUDE_SESSION_ID=other "$PLUGIN_ROOT/scripts/emit.sh" \
    --category agent-presence --tier info --summary "joined: repo"
  sleep 1
  kill $TAILPID 2>/dev/null || true
  wait 2>/dev/null || true

  grep -q '^\[i\] presence joined: repo @testbox$' "$TMP/out.txt"
}

@test "tail-bus drops pre-startup events" {
  CLAUDE_SESSION_ID=other "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "before-start"
  sleep 1.1

  ( CLAUDE_SESSION_ID=mine timeout 3 "$PLUGIN_ROOT/scripts/tail-bus.sh" > "$TMP/out.txt" 2>&1 || true ) &
  TAILPID=$!
  sleep 1
  CLAUDE_SESSION_ID=other "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "after-start"
  sleep 1
  kill $TAILPID 2>/dev/null || true
  wait 2>/dev/null || true

  ! grep -q "before-start" "$TMP/out.txt"
  grep -q "after-start" "$TMP/out.txt"
}
