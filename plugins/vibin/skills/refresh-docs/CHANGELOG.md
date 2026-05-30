# Changelog

All notable changes to the `refresh-docs` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Fixed `scripts/archive-changes-report.sh` to resolve `ROOT_DIR` from `$PWD` (overridable via `$PROJECT_ROOT`) instead of walking four parents from the script's own location. The previous logic broke when the skill was installed outside the target repo (silently no-opped).
- Rewrote the description to add natural user-phrase triggers ("refresh docs", "refresh references", "update references") and to state the precondition (host project must provide `scripts/refresh-docs.sh` and `docs/references/`).
- Added README.

## [0.1.0] - Initial
- Initial skill version.
