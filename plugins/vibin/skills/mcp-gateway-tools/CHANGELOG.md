# Changelog

All notable changes to the `mcp-gateway-tools` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Rewrote the `index_warming` error-table row to lead with `retry_after_ms` from the envelope rather than the hardcoded ~2s.
- Softened the hardcoded `~12 visible actions` and `16 KB` schema-size magic numbers so the skill ages gracefully against gateway changes.
- Added README.

## [0.1.0] - Initial
- Initial skill version.
