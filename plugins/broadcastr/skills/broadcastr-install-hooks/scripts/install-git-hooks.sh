#!/usr/bin/env bash
set -euo pipefail

PLUGIN_ROOT="${BROADCASTR_PLUGIN_ROOT:-${CLAUDE_PLUGIN_ROOT:-}}"
if [ -z "$PLUGIN_ROOT" ]; then
  echo "broadcastr-install-hooks: BROADCASTR_PLUGIN_ROOT or CLAUDE_PLUGIN_ROOT must be set" >&2
  exit 1
fi

TARGET_REPO="${1:-$PWD}"
HOOK_DIR="$TARGET_REPO/.git/hooks"

if [ ! -d "$HOOK_DIR" ]; then
  echo "broadcastr-install-hooks: $HOOK_DIR not found; run inside a git repo or pass the repo path" >&2
  exit 1
fi

mkdir -p "$HOOK_DIR"

is_broadcastr_shim() {
  grep -q 'broadcastr-install-hooks SHIM' "$1" 2>/dev/null
}

for hook in post-commit pre-commit pre-push post-checkout post-merge; do
  src="$PLUGIN_ROOT/scripts/git-hooks/$hook"
  dst="$HOOK_DIR/$hook"
  prev="$dst.broadcastr-prev"

  # Shim exports BROADCASTR_HOOK_DIR so the plugin script can find the
  # repo's .git/hooks/ (where .broadcastr-prev legacy hooks live).
  # `dirname "$0"` inside the plugin script resolves to the plugin tree,
  # NOT the .git/hooks/ dir, so we have to pass it through.
  shim_contents="$(printf '%s\n' \
    '#!/usr/bin/env bash' \
    '# broadcastr-install-hooks SHIM v1' \
    "BROADCASTR_PLUGIN_ROOT='$PLUGIN_ROOT'" \
    'BROADCASTR_HOOK_DIR="$(cd "$(dirname "$0")" && pwd)"' \
    "export BROADCASTR_PLUGIN_ROOT BROADCASTR_HOOK_DIR" \
    "exec '$src' \"\$@\"")"

  # Write atomically via temp file + rename so a partially-written shim
  # never lands at $dst (would corrupt the chain on next invocation).
  write_atomic() {
    local target=$1 contents=$2
    local tmp="${target}.broadcastr.tmp.$$"
    printf '%s\n' "$contents" > "$tmp"
    chmod +x "$tmp"
    mv "$tmp" "$target"
  }

  if [ -e "$dst" ]; then
    if is_broadcastr_shim "$dst"; then
      write_atomic "$dst" "$shim_contents"
      continue
    fi
    if [ ! -e "$prev" ]; then
      mv "$dst" "$prev"
      chmod +x "$prev"
    fi
  fi

  write_atomic "$dst" "$shim_contents"
done

echo "broadcastr: installed shims into $HOOK_DIR"
