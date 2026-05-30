# Changelog

All notable changes to the `work-it` skill are recorded here. Format roughly follows [Keep a Changelog](https://keepachangelog.com/).

## [0.1.2] - 2026-05-26
- Changed the implementation phase so the coordinator dispatches a dedicated implementation agent inside the worktree.
- Required that implementation agent to invoke `superpowers:executing-plans` and return a verification handoff.
- Made missing implementation-agent dispatch a blocked workflow instead of falling back to direct coordinator implementation.

## [0.1.1] - 2026-05-17
- Resolved the ordering ambiguity between step 9 and Non-Negotiable 7: `vibin:save-to-md` now consistently described as running "after step 8 (PR comment resolution) and before step 10's final `git add .`".
- Added README.

## [0.1.0] - Initial
- Initial skill version.
