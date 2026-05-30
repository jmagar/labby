# Changelog

All notable changes to the `save-to-md` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.2.1] - 2026-05-25
- Added the post-write contract: force-stage, path-limited commit, and push only the generated session artifact.
- Documented verification that the session-file commit contains no paths other than the generated artifact.
- Removed caller-specific framing from the save-to-md README and skill description.

## [0.2.0] - 2026-05-25
- Added `--html` flag and `.html`-extension detection to render a rich Aurora-styled HTML artifact instead of markdown.
- Added `references/html-template.html` — self-contained dark-mode template with embedded Aurora token subset, Google Fonts CDN, sticky ToC sidebar with Lucide icons, at-a-glance stat row, Tier 2 section panels with section icons, collapsible command transcript, semantic status badges, scroll-spy, and print stylesheet.
- Documented HTML rendering rules: template tokens, status→icon mapping, empty-section drop behavior, escape requirements, sentence-case discipline, no-emoji rule.
- Markdown remains the default.

## [0.1.2] - 2026-05-23
- Added Beads recent issue and interaction context injection.
- Added a mandatory **Beads Activity** section covering beads created, closed, edited, claimed, assigned, commented on, or otherwise worked during the session.
- Updated README to document bead activity capture.

## [0.1.1] - 2026-05-17
- Added `Repo root` to the injected Context block so the "paths resolve from repo root" rule is mechanically executable.
- Added an instruction to read the injected transcript `.jsonl` path so the "document the entire session" rule survives a truncated live context window.
- Dropped the `agent:` field from the metadata block (no injection source — invited fabrication).
- Marked `session id` / `transcript` fields as omit-if-empty so missing injections don't produce fake values.
- Added README.

## [0.1.0] - Initial
- Initial skill version.
