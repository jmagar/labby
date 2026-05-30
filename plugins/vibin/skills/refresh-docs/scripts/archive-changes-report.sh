#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="${PROJECT_ROOT:-$PWD}"
REPORT="$ROOT_DIR/docs/references/CHANGES-REPORT.md"
ARCHIVE_DIR="$ROOT_DIR/docs/references/archive/changes-reports"

usage() {
  cat <<'EOF'
Usage: .agents/src/skills/refresh-docs/scripts/archive-changes-report.sh

Archive docs/references/CHANGES-REPORT.md to docs/references/archive/changes-reports.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ ! -f "$REPORT" ]]; then
  printf 'No existing CHANGES-REPORT.md to archive.\n'
  exit 0
fi

mkdir -p "$ARCHIVE_DIR"
timestamp="$(date -u +%Y%m%dT%H%M%S.%NZ)"
target="$ARCHIVE_DIR/CHANGES-REPORT-$timestamp-$$.md"

mv -- "$REPORT" "$target"
printf 'Archived existing report to %s\n' "$target"
