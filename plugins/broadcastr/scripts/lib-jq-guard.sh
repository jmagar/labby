# shellcheck shell=bash
# Sourced helper: require_jq <monitor-name>
#
# Exits the calling script if `jq` is missing, emitting a single
# alert-tier event into the bus so the failure is visible in the feed
# instead of silently disappearing.

require_jq() {
  local monitor="$1"
  command -v jq >/dev/null 2>&1 && return 0
  echo "${monitor}: jq not installed; exiting" >&2
  local plugin_root="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[1]}")/.." && pwd)}"
  "${plugin_root}/scripts/emit.sh" \
    --category agent-presence --tier alert --source claude-hook \
    --summary "${monitor}: jq missing; disabled this session" \
    --data "{\"monitor\":\"${monitor}\"}" 2>/dev/null || true
  exit 0
}
