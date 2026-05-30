# Changelog

All notable changes to the `dozzle` skill are recorded here.

## 2026-05-23

### Changed

- Replaced stale Lab MCP/CLI instructions with the direct Dozzle API workflow and optional `DOZZLE_SESSION_COOKIE` auth fallback.
- Added safer cookie handling, session refresh guidance, and container/log discovery steps.
- Added official Dozzle auth provider, forward-proxy/Authelia, MCP, and security-boundary guidance from indexed Dozzle docs/repo.

### Added

- Added packaging metadata files so the skill has an OpenAI UI descriptor, README, and changelog alongside `SKILL.md`.
