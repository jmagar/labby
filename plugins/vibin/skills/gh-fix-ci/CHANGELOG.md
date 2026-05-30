# Changelog

All notable changes to the `gh-fix-ci` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.1] - 2026-05-17
- Rewrote SKILL.md from scratch. Removed all references to the bundled `scripts/inspect_pr_checks.py` (the script never existed; the references made the skill misleading).
- Renamed H1 title from "Gh Pr Checks Plan Fix" to "gh-fix-ci — Fix Failing GitHub PR Checks".
- Description now leads with natural trigger phrases ("CI is red", "PR checks failing"); Buildkite/external-provider scope clause moved into the body.
- Added a one-liner showing how to derive `<job_id>` from a run via `gh run view --json jobs` for the direct job-logs API call.
- Aligned step 7 with the Overview's `create-plan`-optional hedge.
- Added README.

## [0.1.0] - Initial
- Initial skill version.
