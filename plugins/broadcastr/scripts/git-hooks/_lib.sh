# shellcheck shell=bash
# Sourced by every broadcastr git-hook script. Centralizes path resolution,
# branch lookup, and the chain-to-previous-hook tail dance.
#
# Usage:
#   set -uo pipefail
#   . "$(dirname "$0")/_lib.sh"
#   bcr_hook_init "<hook-name>"   # sets PLUGIN_ROOT HOOK_DIR PREV BRANCH
#   ...your hook body...
#   bcr_chain_prev "$@"           # execs .broadcastr-prev if executable
#   exit 0

# Sentinel file written by post-merge, read by post-commit for dedup
BCR_MERGE_SENTINEL_BASENAME=".broadcastr.last-merge-sha"

bcr_hook_init() {
  local hook_name="$1"
  PLUGIN_ROOT="${BROADCASTR_PLUGIN_ROOT:-$HOME/.claude/plugins/broadcastr}"
  HOOK_DIR="${BROADCASTR_HOOK_DIR:-$(dirname "$0")}"
  PREV="$HOOK_DIR/${hook_name}.broadcastr-prev"
  BRANCH="$(git symbolic-ref --short HEAD 2>/dev/null || true)"
  if [ -z "$BRANCH" ]; then
    BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)"
  fi
  if [ -z "$BRANCH" ] || [ "$BRANCH" = "HEAD" ]; then
    BRANCH="?"
  fi
}

bcr_chain_prev() {
  if [ -x "$PREV" ]; then
    exec "$PREV" "$@"
  fi
}
