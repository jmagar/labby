#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  cd "$TMP"
  git init -q
  git config user.email test@example.com
  git config user.name "Test"
  export BROADCASTR_PLUGIN_ROOT="$PLUGIN_ROOT"
  export CLAUDE_PROJECT_DIR="$TMP"
  export BROADCASTR_HOME="$TMP/home"
  export BROADCASTR_GLOBAL_FEED=0
  export HOSTNAME=testbox
  export USER=tester
  unset BROADCASTR_DISABLED BROADCASTR_MUTE CLAUDE_SESSION_ID

  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"
}

teardown() { rm -rf "$TMP"; }

@test "post-commit emits commit event with sha + subtype" {
  echo "hello" > a.txt
  git add a.txt
  git commit -q -m "first commit"

  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  [ -f "$bus" ]
  grep -q '"category":"commit"' "$bus"
  grep -q '"subtype":"commit"' "$bus"
  expected_sha=$(git rev-parse HEAD)
  grep -q "\"sha\":\"$expected_sha\"" "$bus"
}

@test "post-commit chains to .broadcastr-prev legacy hook" {
  printf '#!/usr/bin/env bash\necho LEGACY_HOOK_RAN > "%s/legacy.marker"\n' "$TMP" \
    > .git/hooks/post-commit
  chmod +x .git/hooks/post-commit
  # Re-install so the legacy hook moves to .broadcastr-prev
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"

  echo a > b.txt
  git add b.txt
  git commit -q -m "with legacy"

  [ -f "$TMP/legacy.marker" ]
  grep -q LEGACY_HOOK_RAN "$TMP/legacy.marker"
  grep -q '"category":"commit"' "$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
}

@test "post-merge + post-commit: merge commit dedup" {
  # Set up a branch and merge
  echo a > a.txt && git add a.txt && git commit -q -m "main 1"
  git checkout -q -b feature
  echo b > b.txt && git add b.txt && git commit -q -m "feature 1"
  git checkout -q main 2>/dev/null || git checkout -q master
  git merge -q --no-ff -m "merge feature" feature

  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  merge_sha=$(git rev-parse HEAD)

  # Should have exactly one event for the merge sha (subtype=merge wins)
  merge_events=$(grep -c "\"sha\":\"$merge_sha\"" "$bus" || true)
  [ "$merge_events" = "1" ]
  grep -q "\"sha\":\"$merge_sha\".*\"subtype\":\"merge\"\|\"subtype\":\"merge\".*\"sha\":\"$merge_sha\"" "$bus" \
    || { echo "expected one merge-subtype event for $merge_sha"; cat "$bus"; false; }
}

@test "pre-push emits attempt with remote info" {
  # Push to a local bare repo
  bare="$TMP/origin.git"
  git init -q --bare "$bare"
  git remote add origin "$bare"
  echo a > a.txt && git add a.txt && git commit -q -m "x"
  git push -q origin HEAD:main 2>/dev/null || git push -q origin HEAD:master 2>/dev/null || true

  bus="$CLAUDE_PROJECT_DIR/.broadcastr/events.jsonl"
  grep -q '"category":"push"' "$bus"
  grep -q '"subtype":"attempt"' "$bus"
  grep -q '"remote":"origin"' "$bus"
}
