#!/usr/bin/env bash
set -euo pipefail

mode="${1:-dry-run}"
if [[ "$mode" != "dry-run" && "$mode" != "--dry-run" && "$mode" != "kill" && "$mode" != "--kill" ]]; then
  printf 'usage: %s [dry-run|kill]\n' "$0" >&2
  exit 2
fi

do_kill=0
if [[ "$mode" == "kill" || "$mode" == "--kill" ]]; then
  do_kill=1
fi

patterns=(
  'target/debug/labby serve mcp --stdio'
  'noxa mcp'
  'uv tool uvx github-chat-mcp'
  '/github-chat-mcp'
  'npm exec chrome-devtools-mcp@latest'
  'chrome-devtools-mcp/build/src/telemetry/watchdog/main.js'
)

printf 'mode: %s\n' "$([[ $do_kill -eq 1 ]] && printf kill || printf dry-run)"

for pattern in "${patterns[@]}"; do
  printf '\n[%s]\n' "$pattern"
  matches="$(pgrep -af "$pattern" || true)"
  if [[ -z "$matches" ]]; then
    printf 'no matches\n'
    continue
  fi

  printf '%s\n' "$matches"
  if [[ $do_kill -eq 1 ]]; then
    pkill -TERM -f "$pattern" || true
    sleep 1
    if pgrep -f "$pattern" >/dev/null 2>&1; then
      pkill -KILL -f "$pattern" || true
    fi
  fi
done

if [[ $do_kill -eq 1 ]]; then
  printf '\nafter kill:\n'
  free -h
fi
