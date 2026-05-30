#!/usr/bin/env bash
# Dispatches to the compiled broadcastr-emit binary when present;
# falls back to the pure-bash emitter when the binary hasn't been built yet.
# Kept lean (no `set -euo pipefail`) because hooks and monitors invoke this
# on latency-sensitive paths.
PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
test -x "$PLUGIN_ROOT/bin/broadcastr-emit" && exec "$PLUGIN_ROOT/bin/broadcastr-emit" "$@"
exec "$PLUGIN_ROOT/scripts/emit-fallback.sh" "$@"
