#!/usr/bin/env bash
# SessionStart / ConfigChange hook for the lab plugin.
# Delegates all setup logic to the lab binary so shell script complexity stays at zero.
set -euo pipefail

# Ensure the lab binary is reachable. The bundled bin/ directory is checked as
# a fallback so the script works before `labby` is on PATH system-wide.
: "${CLAUDE_PLUGIN_ROOT:=$(cd "$(dirname "$0")/.." && pwd)}"

if ! command -v lab >/dev/null 2>&1; then
  bundled="${CLAUDE_PLUGIN_ROOT}/bin/lab"
  if [[ -x "${bundled}" ]]; then
    export PATH="${CLAUDE_PLUGIN_ROOT}/bin:${PATH}"
  else
    printf 'lab plugin setup: lab binary not found on PATH or at %s\n' "${bundled}" >&2
    exit 1
  fi
fi

exec lab setup plugin-hook "$@"
