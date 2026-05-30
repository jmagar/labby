#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  export TMP
  export CLAUDE_PROJECT_DIR="$TMP/repo"
  export BROADCASTR_HOME="$TMP/home"
  export BROADCASTR_GLOBAL_FEED=0
  export HOSTNAME=testbox
  export USER=tester
  unset CLAUDE_SESSION_ID BROADCASTR_DISABLED
  mkdir -p "$CLAUDE_PROJECT_DIR" "$BROADCASTR_HOME"
}

teardown() { rm -rf "$TMP"; }

@test "emit.sh fallback writes valid JSONL when binary absent" {
  # Force fallback by hiding the binary path
  local saved=""
  if [ -x "$PLUGIN_ROOT/bin/broadcastr-emit" ]; then
    saved="$PLUGIN_ROOT/bin/broadcastr-emit.hidden"
    mv "$PLUGIN_ROOT/bin/broadcastr-emit" "$saved"
  fi

  run "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "test from bash"
  [ "$status" -eq 0 ]

  if [ -n "$saved" ]; then
    mv "$saved" "$PLUGIN_ROOT/bin/broadcastr-emit"
  fi

  [ -f "$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl" ]
  line=$(cat "$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl")
  echo "$line" | grep -q '"category":"commit"'
  echo "$line" | grep -q '"summary":"test from bash"'
  echo "$line" | grep -qE '"id":"evt_[0-9A-HJKMNP-TV-Z]{26}"'
}

@test "emit.sh uses binary when present" {
  if [ ! -x "$PLUGIN_ROOT/bin/broadcastr-emit" ]; then
    skip "binary not built"
  fi
  run "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "binarypath"
  [ "$status" -eq 0 ]
  grep -q '"summary":"binarypath"' "$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
}

@test "emit.sh respects BROADCASTR_DISABLED" {
  export BROADCASTR_DISABLED=1
  run "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "disabled"
  [ "$status" -eq 0 ]
  [ ! -f "$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl" ]
}

@test "emit.sh round-trips summaries with quotes, backslashes, newlines" {
  # Pathological summary that would break naive JSON construction
  weird='He said "hi"; path C:\Users\test; line1
line2 with tab	end'
  run "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "$weird"
  [ "$status" -eq 0 ]
  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  [ -f "$bus" ]
  # Must be valid JSON
  jq -e . "$bus" >/dev/null
  # Summary must round-trip
  got=$(jq -r '.summary' "$bus")
  [ "$got" = "$weird" ] || { echo "got: $got"; echo "want: $weird"; false; }
}

@test "emit.sh preserves repo paths with spaces and quotes" {
  weird_dir="$TMP/path with \"quotes\" and spaces"
  mkdir -p "$weird_dir/.broadcastr"
  CLAUDE_PROJECT_DIR="$weird_dir" \
    run "$PLUGIN_ROOT/scripts/emit.sh" --category commit --tier info --summary "x"
  [ "$status" -eq 0 ]
  bus="$weird_dir/.broadcastr/events.jsonl"
  [ -f "$bus" ]
  jq -e . "$bus" >/dev/null
  got_repo=$(jq -r '.repo' "$bus")
  [ "$got_repo" = "$weird_dir" ]
}

@test "emit.sh wraps malformed --data with _parse_error marker" {
  run "$PLUGIN_ROOT/scripts/emit.sh" \
    --category commit --tier info --summary "x" --data 'not json at all'
  [ "$status" -eq 0 ]
  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  jq -e . "$bus" >/dev/null
  got_raw=$(jq -r '.data._raw' "$bus")
  [ "$got_raw" = "not json at all" ]
  got_err=$(jq -r '.data._parse_error' "$bus")
  [[ "$got_err" == *"invalid JSON"* ]]
}

@test "emit.sh preserves valid --data as object" {
  run "$PLUGIN_ROOT/scripts/emit.sh" \
    --category commit --tier info --summary "x" --data '{"sha":"abc","files":3}'
  [ "$status" -eq 0 ]
  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  jq -e . "$bus" >/dev/null
  [ "$(jq -r '.data.sha' "$bus")" = "abc" ]
  [ "$(jq -r '.data.files' "$bus")" = "3" ]
}
