# Changelog

## [0.1.4] - 2026-05-27
- Removed `Stop` hook from `hooks.json` to eliminate the infinite "Claude left" event loop. The `Stop` hook fires after every Claude AI response turn (not just at session end), causing a feedback cycle: Stop hook emits "Claude left" → monitor picks it up → AI responds → Stop hook fires again. Presence on the bus now comes from `SessionStart` only; session-end events are deferred until a proper lifecycle event is available.

## [0.1.3] - 2026-05-27
- Fixed monitor self-suppression bug: `tail-bus.sh` now shows events when `CLAUDE_SESSION_ID` is not set in the environment (null session_id was incorrectly treated as equal to the empty monitor sid, suppressing every event).

## [0.1.2] - 2026-05-26
- Made monitor scripts fall back to their current working directory when Claude does not provide `CLAUDE_PROJECT_DIR`.

## [0.1.1] - 2026-05-26
- Disabled the Claude `PostToolUse` Bash classifier hook. Bead and stash activity can be observed through syslog instead, avoiding a broadcastr hook invocation after every Bash tool call.

## [0.1.0] - 2026-05-25
- Initial release. Per-repo + host-global JSONL bus, Claude hooks (SessionStart/Stop/PostToolUse-Bash), git hooks (post-commit, pre-commit, pre-push, post-checkout, post-merge), inotify watchers (plans, sessions), feed monitor with self-suppression, apprise alert gateway. Claude Code only — Codex deferred.
