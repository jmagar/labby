#!/usr/bin/env bash
# PostToolUse(Bash) classifier. Reads tool input from stdin (JSON), extracts
# the command, and emits an event if it matches a known pattern.
set -euo pipefail
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

INPUT="$(cat || true)"

# Cheap pre-filter against the raw JSON: 95%+ of Bash tool calls don't match
# any classifier pattern, and jq cold-start is ~5-15ms per call. Substring
# match against the unparsed input first; only invoke jq if a candidate
# token appears anywhere in the payload. False positives are fine because
# the precise case-match runs after jq extracts the actual command.
case "$INPUT" in
  *"bd "*|*"git stash"*) ;;
  *) exit 0 ;;
esac

command -v jq >/dev/null 2>&1 || exit 0
CMD="$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty' 2>/dev/null || true)"
[ -z "$CMD" ] && exit 0

emit_event() {
  local category=$1 summary=$2 data=$3
  "$PLUGIN_ROOT/scripts/emit.sh" \
    --category "$category" --tier info --source claude-hook \
    --summary "$summary" \
    --data "$data"
}

CMD_JSON="$(printf '%s' "$CMD" | jq -Rs .)"

case "$CMD" in
  *"bd create"*|*"bd update"*|*"bd close"*|*"bd reopen"*)
    emit_event bead "bd: ${CMD:0:200}" "{\"cmd\":$CMD_JSON}"
    ;;
  *"git stash"*)
    emit_event stash "git stash: ${CMD:0:200}" "{\"cmd\":$CMD_JSON}"
    ;;
esac
exit 0
