#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  export CLAUDE_PLUGIN_ROOT="$PLUGIN_ROOT"
  export CLAUDE_PROJECT_DIR="$TMP/workspace/lab/.worktrees/some-worktree"
  export BROADCASTR_HOME="$TMP/home"
  export BROADCASTR_GLOBAL_FEED=0
  export HOSTNAME=testbox
  export USER=tester
  unset CLAUDE_SESSION_ID BROADCASTR_DISABLED BROADCASTR_MUTE
  mkdir -p "$CLAUDE_PROJECT_DIR" "$BROADCASTR_HOME"
}

teardown() { rm -rf "$TMP"; }

@test "session lifecycle summaries stay token-compact" {
  "$PLUGIN_ROOT/scripts/hook-on-session-start.sh"
  "$PLUGIN_ROOT/scripts/hook-on-stop.sh"

  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  [ "$(jq -sr '.[0].summary' "$bus")" = 'Claude joined: `lab/.worktrees/some-worktree`' ]
  [ "$(jq -sr '.[1].summary' "$bus")" = 'Claude left: `lab/.worktrees/some-worktree`' ]
  [ "$(jq -sr '.[0].data.cwd' "$bus")" = "$CLAUDE_PROJECT_DIR" ]
  [ "$(jq -sr '.[0].data.agent' "$bus")" = "Claude" ]
}

@test "session lifecycle summary honors explicit agent name" {
  export BROADCASTR_AGENT_NAME=Codex
  "$PLUGIN_ROOT/scripts/hook-on-session-start.sh"
  "$PLUGIN_ROOT/scripts/hook-on-stop.sh"

  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  [ "$(jq -sr '.[0].summary' "$bus")" = 'Codex joined: `lab/.worktrees/some-worktree`' ]
  [ "$(jq -sr '.[1].summary' "$bus")" = 'Codex left: `lab/.worktrees/some-worktree`' ]
}
