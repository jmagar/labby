#!/usr/bin/env bash
# Activate a window by title substring, then capture it. Returns the local PNG path.
#
# Usage:
#   win-shot.sh "Visual Studio Code"
#   win-shot.sh "Chrome" /path/to/out.png

set -euo pipefail

HOST="${NIRCMD_HOST:-steamy-wsl}"
NIRCMD="${NIRCMD_PATH:-/mnt/c/tools/nircmd/nircmd.exe}"

if [[ $# -eq 0 ]]; then
  echo "usage: win-shot.sh <title-substring> [out-path]" >&2
  exit 2
fi

title="$1"
dest="${2:-${CLAUDE_JOB_DIR:-/tmp}/winshot-$(date +%s).png}"

remote_win="C:\\Users\\jmaga\\AppData\\Local\\Temp\\winshot-$$.png"
remote_posix="/mnt/c/Users/jmaga/AppData/Local/Temp/winshot-$$.png"

# activate, give Windows a beat to redraw, then capture the now-active window
ssh "$HOST" "$NIRCMD win activate ititle \"$title\" && $NIRCMD wait 200 && $NIRCMD savescreenshotwin '$remote_win'"
ssh "$HOST" "cat '$remote_posix'" > "$dest"
ssh "$HOST" "rm -f '$remote_posix'"

echo "$dest"
