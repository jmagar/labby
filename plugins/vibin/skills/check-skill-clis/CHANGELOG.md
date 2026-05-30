# Changelog

All notable changes to the `check-skill-clis` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Scoped `skill_name()` regex to the YAML frontmatter block so a stray `name:` line in prose can't shadow the real value.
- Reconciled Expected Output Shape table header with the script's actual columns (`cli refs | missing`).
- Documented useful flags (`--json`, `--include-common`, `--only-root`, `--disabled-skill[s-file]`) and the exit-code 1 behavior.
- Removed checked-in `scripts/__pycache__/`; added `.gitignore`.
- Added README.

## [0.1.0] - Initial
- Initial skill version.
