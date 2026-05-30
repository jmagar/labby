#!/usr/bin/env bash
set -euo pipefail
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
CWD="${CLAUDE_PROJECT_DIR:-}"

agent_label() {
  local agent="${BROADCASTR_AGENT_NAME:-${BROADCASTR_AGENT:-}}"
  case "${agent,,}" in
    codex) printf 'Codex' ;;
    gemini) printf 'Gemini' ;;
    claude|claude-code|"") printf 'Claude' ;;
    *) printf '%s' "$agent" ;;
  esac
}

project_label() {
  if [ -z "$1" ]; then
    printf '?'
    return
  fi
  local normalized before rest project label
  normalized="$(printf '%s' "$1" | sed 's#\\#/#g; s#/*$##')"

  case "$normalized" in
    */.worktrees/*)
      before="${normalized%%/.worktrees/*}"
      rest="${normalized#*/.worktrees/}"
      project="${before##*/}"
      printf '%s/.worktrees/%s' "${project:-?}" "$rest"
      return
      ;;
  esac

  label="${normalized##*/}"
  printf '%s' "${label:-$normalized}"
}

AGENT="$(agent_label)"
SUMMARY="${AGENT} left: \`$(project_label "$CWD")\`"
"$PLUGIN_ROOT/scripts/emit.sh" \
  --category agent-presence --tier info --source claude-hook \
  --summary "$SUMMARY" \
  --data "$(jq -nc --arg cwd "$CWD" --arg agent "$AGENT" '{action:"left", cwd:$cwd, agent:$agent}' 2>/dev/null || echo '{"action":"left"}')"
