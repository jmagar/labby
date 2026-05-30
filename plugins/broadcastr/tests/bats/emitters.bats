#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  export TMP
  export CLAUDE_PLUGIN_ROOT="$PLUGIN_ROOT"
  export BROADCASTR_PLUGIN_ROOT="$PLUGIN_ROOT"
  export CLAUDE_PROJECT_DIR="$TMP/repo"
  export BROADCASTR_HOME="$TMP/home"
  export BROADCASTR_GLOBAL_FEED=0
  export HOSTNAME=testbox
  export USER=tester
  unset CLAUDE_SESSION_ID BROADCASTR_DISABLED BROADCASTR_MUTE BROADCASTR_AGENT_NAME BROADCASTR_AGENT
  mkdir -p "$CLAUDE_PROJECT_DIR" "$BROADCASTR_HOME"
}

teardown() { rm -rf "$TMP"; }

bus() { printf '%s/.broadcastr/events.jsonl' "$CLAUDE_PROJECT_DIR"; }

init_git_repo() {
  cd "$CLAUDE_PROJECT_DIR"
  git init -q
  git config user.email test@example.com
  git config user.name "Test"
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"
}

@test "pre-commit emits start and pass events" {
  init_git_repo
  branch="$(git symbolic-ref --short HEAD)"
  echo "hello" > a.txt
  git add a.txt
  git commit -q -m "pre-commit pass"

  [ "$(jq -sr '.[0].summary' "$(bus)")" = "pre-commit start on $branch" ]
  [ "$(jq -sr '.[1].summary' "$(bus)")" = "pre-commit pass on $branch" ]
}

@test "pre-commit emits alert on previous hook failure" {
  init_git_repo
  printf '#!/usr/bin/env bash\nexit 7\n' > .git/hooks/pre-commit
  chmod +x .git/hooks/pre-commit
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"

  echo "hello" > a.txt
  git add a.txt
  run git commit -q -m "pre-commit fail"
  [ "$status" -ne 0 ]

  [ "$(jq -sr '.[-1].category' "$(bus)")" = "pre-commit" ]
  [ "$(jq -sr '.[-1].tier' "$(bus)")" = "alert" ]
  [[ "$(jq -sr '.[-1].summary' "$(bus)")" == "pre-commit FAIL on "*"(exit 7)" ]]
  [ "$(jq -sr '.[-1].data.exit' "$(bus)")" = "7" ]
}

@test "post-checkout emits branch event for branch checkouts" {
  init_git_repo
  echo main > a.txt
  git add a.txt
  git commit -q -m "main"
  git checkout -q -b feature/broadcast-test

  [ "$(jq -sr '.[-1].category' "$(bus)")" = "branch" ]
  [ "$(jq -sr '.[-1].summary' "$(bus)")" = "checkout: feature/broadcast-test" ]
  [ "$(jq -sr '.[-1].data.branch' "$(bus)")" = "feature/broadcast-test" ]
}

@test "push wrapper emits success and failure outcomes" {
  init_git_repo
  bare="$TMP/origin.git"
  git init -q --bare "$bare"
  git remote add origin "$bare"
  echo "hello" > a.txt
  git add a.txt
  git commit -q -m "push wrapper"
  branch="$(git rev-parse --abbrev-ref HEAD)"

  source "$PLUGIN_ROOT/scripts/push-wrapper.sh"
  run broadcastr-push origin HEAD:main
  [ "$status" -eq 0 ]
  grep -q "\"summary\":\"push succeeded: $branch\"" "$(bus)"
  grep -q '"subtype":"success"' "$(bus)"

  git remote add bad "$TMP/missing.git"
  run broadcastr-push bad HEAD:main
  [ "$status" -ne 0 ]
  grep -q "\"summary\":\"push FAILED: $branch (exit " "$(bus)"
  grep -q '"subtype":"fail"' "$(bus)"
}

@test "plan and session-doc watchers emit markdown file events" {
  mkdir -p "$CLAUDE_PROJECT_DIR/docs/plans" "$CLAUDE_PROJECT_DIR/docs/sessions"
  plan="$CLAUDE_PROJECT_DIR/docs/plans/ship-it.md"
  session="$CLAUDE_PROJECT_DIR/docs/sessions/2026-05-26-ship-it.md"
  touch "$plan" "$session"

  "$PLUGIN_ROOT/scripts/watch-plans.sh" "$plan|CLOSE_WRITE"
  "$PLUGIN_ROOT/scripts/watch-sessions.sh" "$session|CLOSE_WRITE"

  [ "$(jq -sr '.[0].category' "$(bus)")" = "plan" ]
  [ "$(jq -sr '.[0].summary' "$(bus)")" = "plan edit: ship-it.md" ]
  [ "$(jq -sr '.[0].data.path' "$(bus)")" = "$plan" ]
  [ "$(jq -sr '.[1].category' "$(bus)")" = "session-doc" ]
  [ "$(jq -sr '.[1].summary' "$(bus)")" = "session doc: 2026-05-26-ship-it.md" ]
  [ "$(jq -sr '.[1].data.path' "$(bus)")" = "$session" ]
}
