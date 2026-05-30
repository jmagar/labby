#!/usr/bin/env bash
# Lock the user's Windows workstation.
set -euo pipefail
HOST="${NIRCMD_HOST:-steamy-wsl}"
NIRCMD="${NIRCMD_PATH:-/mnt/c/tools/nircmd/nircmd.exe}"
ssh "$HOST" "$NIRCMD lockws"
echo "locked"
