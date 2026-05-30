#!/usr/bin/env bats

setup() {
  PLUGIN_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  TMP="$(mktemp -d)"
  cd "$TMP"
  git init -q
  git config user.email test@example.com
  git config user.name "Test"
  mkdir -p .git/hooks
  export BROADCASTR_PLUGIN_ROOT="$PLUGIN_ROOT"
}

teardown() { rm -rf "$TMP"; }

@test "install creates all five shim hooks" {
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"
  for h in post-commit pre-commit pre-push post-checkout post-merge; do
    [ -x ".git/hooks/$h" ]
    grep -q "broadcastr" ".git/hooks/$h"
  done
}

@test "install preserves existing hook as .broadcastr-prev" {
  printf '#!/bin/sh\necho legacy\n' > .git/hooks/pre-commit
  chmod +x .git/hooks/pre-commit
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"
  [ -x ".git/hooks/pre-commit.broadcastr-prev" ]
  grep -q legacy ".git/hooks/pre-commit.broadcastr-prev"
  grep -q broadcastr ".git/hooks/pre-commit"
}

@test "second install does not stack shims" {
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"
  SHA1=$(sha256sum .git/hooks/post-commit | cut -d' ' -f1)
  "$PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh"
  SHA2=$(sha256sum .git/hooks/post-commit | cut -d' ' -f1)
  [ "$SHA1" = "$SHA2" ]
  [ ! -f .git/hooks/post-commit.broadcastr-prev ]
}
