# Changelog

All notable changes to the `validate-skill` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Removed `Agent` from the `allowed-tools` whitelist (not a real tool name — valid is `Task`). Removed `NotebookRead` (no longer a current Claude Code tool).
- Added a `skills-ref` presence check at the start of step 2; emits one WARN and skips schema validation if not installed instead of failing the bash call.
- Added a check that frontmatter `name:` matches the skill directory basename (common real-world bug).
- Added a description-length check (40–1024 chars).
- Added README.

## [0.1.0] - Initial
- Initial skill version.
