# Changelog

All notable changes to the `mcporter` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Fixed `scripts/smoke.sh` empty-arrays footgun: now exits 2 with a clear message when both TOOLS and RESOURCES arrays are empty (was silently reporting "0 passed, 0 failed").
- Fixed `scripts/smoke.sh` `set -e` + `((fail++))` trap: replaced with `fail=$((fail+1))` form so a failing case doesn't abort the loop on the first failure (the previous form returned non-zero when `fail` was 0, which trips `set -e`).
- Added trailing newline to `smoke.sh`.
- Trimmed the duplicate inline test-harness from SKILL.md (was teaching the weaker, buggy pattern). Replaced with a short conceptual sketch + pointer to `scripts/smoke.sh`.
- Added README.

## [0.1.0] - Initial
- Initial skill version.
