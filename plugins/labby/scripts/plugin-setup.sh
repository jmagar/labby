#!/usr/bin/env bash
# SessionStart / ConfigChange hook for the labby plugin.
# Delegates all setup logic to the labby binary; the binary self-installs into
# ~/.local/bin (via `labby setup plugin-hook`) so it is on your PATH too.
set -euo pipefail

: "${CLAUDE_PLUGIN_ROOT:=$(cd "$(dirname "$0")/.." && pwd)}"

BUNDLED="${CLAUDE_PLUGIN_ROOT}/bin/labby"
if [[ -x "${BUNDLED}" ]]; then
  exec "${BUNDLED}" setup plugin-hook "$@"
elif command -v labby >/dev/null 2>&1; then
  exec labby setup plugin-hook "$@"
else
  printf 'labby plugin setup: labby binary not found at %s or on PATH\n' "${BUNDLED}" >&2
  exit 1
fi
