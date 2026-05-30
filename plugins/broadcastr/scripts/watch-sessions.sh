#!/usr/bin/env bash
set -euo pipefail
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
REPO="${CLAUDE_PROJECT_DIR:-$PWD}"

handle() {
  local line="$1"
  local path="${line%|*}"
  case "$path" in
    *.md)
      local base
      base="$(basename "$path")"
      local data='{}'
      if command -v jq >/dev/null 2>&1; then
        data="$(jq -nc --arg path "$path" '{path:$path}')"
      fi
      "$PLUGIN_ROOT/scripts/emit.sh" \
        --category session-doc --tier info --source inotify \
        --summary "session doc: $base" \
        --data "$data"
      ;;
  esac
}

if [ "${1:-}" ]; then
  handle "$1"
  exit 0
fi

exec "$PLUGIN_ROOT/scripts/supervisor.sh" "broadcastr-sessions" \
  "$PLUGIN_ROOT/scripts/watch-sessions.sh" \
  "$REPO/docs/sessions"
