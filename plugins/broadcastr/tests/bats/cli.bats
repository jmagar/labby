#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  export CLAUDE_PLUGIN_ROOT="$PLUGIN_ROOT"
  export CLAUDE_PROJECT_DIR="$TMP/repo"
  export BROADCASTR_HOME="$TMP/home"
  export BROADCASTR_GLOBAL_FEED=1
  export HOSTNAME=testbox
  export USER=tester
  unset CLAUDE_SESSION_ID BROADCASTR_DISABLED BROADCASTR_MUTE
  mkdir -p "$CLAUDE_PROJECT_DIR" "$BROADCASTR_HOME"
}

teardown() { rm -rf "$TMP"; }

@test "recent dedups events written to repo and global buses" {
  "$PLUGIN_ROOT/bin/broadcastr" emit cli info "recent-dedup-event"

  run "$PLUGIN_ROOT/bin/broadcastr" recent --since=10m
  [ "$status" -eq 0 ]

  count=$(printf '%s\n' "$output" | grep -c "recent-dedup-event" || true)
  [ "$count" = "1" ]
}
