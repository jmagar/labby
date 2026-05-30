# Changelog

All notable changes to the `gh-pr` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Moved the entire `gh-webhook` install / systemd / Tailscale funnel / registration block from SKILL.md to `references/webhook-setup.md`; left a 5-line consumption note in SKILL.md.
- Introduced a `$SCRIPTS` shorthand (defined once at the top of Available CLI Tools) and replaced every `python3 skills/gh-pr/scripts/…` invocation with `python3 $SCRIPTS/…`. Cuts visual noise substantially.
- Fixed stale script name (`gh-fetch-comments` → `fetch_comments.py`) in `references/resolution-workflow.md`.
- Deleted stale `examples/basic-workflow.sh` (wrong install path, outdated invocation, no beads).
- Deleted unused `load-env.sh` (pointed at unrelated homelab path; nothing in the skill sourced it).
- Removed checked-in `scripts/__pycache__/`; added `.gitignore`.
- Rewrote README from scratch (prior version was stale: described TaskCreate not beads, listed 3 of 13 scripts).

## [0.1.0] - Initial
- Initial skill version.
