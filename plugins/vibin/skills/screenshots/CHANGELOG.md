# Changelog

All notable changes to the `screens` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.2] - 2026-05-17
- Documented `~/.claude/settings.json` `env` block as the correct persistence path for `SCREENS_*` overrides. Plain interactive-shell `export` doesn't reach Claude Code's Bash tool (it spawns its own shells), so the previous instructions were unreliable.
- Kept the inline `VAR=value <snippet>` form for one-shot overrides.

## [0.1.1] - 2026-05-17
- Renamed internal bash var `HOST` to `SSH_TARGET` to avoid colliding with zsh's built-in `$HOST` (current hostname).
- Deleted `config.sh`; inlined the defaults block at the top of SKILL.md, still env-var-overridable via `SCREENS_*`.
- Collapsed the dueling `SKILL_DIR` derivation + hardcoded override into one hardcoded line with a one-line comment.
- Tightened the description to scope to "the user's own desktop" (disarms false positives against browser-automation screenshot requests).
- Fixed description prose: said "override `SSH_TARGET`" but the actual user-facing knob is `SCREENS_HOST` (`SSH_TARGET` is the internal alias that reads it).

## [0.1.0] - Initial
- Initial skill version.
