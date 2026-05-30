# broadcastr

Real-time activity feed across concurrent Claude Code sessions. Every session
emits events to a shared JSONL bus; every other session sees a notification line
as they happen.

## Install

Ships via the `~/.agents` marketplace. Run `broadcastr-install-hooks` in each
repo to add git-hook emitters.

## What you see

```
📡 👤[lab] Claude joined
📡 🌿[aurora-design-system] Claude made commit a1b2c3d: fix token handling
🚨 🌿[syslog-mcp] Claude's push FAILED · feature/x
📡 📝[axon_rust] Claude saved: 2026-05-27-android-bugfix.md
📡 🎯[lab] Claude closed beads-042
📡 🌿[lab] Claude switched to · feature/new-thing
📡 👤[lab] Claude left
```

Your own session's events are suppressed. Alert-tier events (`🚨`) are also
forwarded to your phone via apprise.

## Events

| Glyph | Categories | Source |
|-------|-----------|--------|
| 👤 | `agent-presence` — joined / left | Claude SessionStart / Stop hooks |
| 📝 | `session-doc` — session doc saved | inotify on `docs/sessions/` |
| 📝 | `plan` / `plan-exec` — plan file edited | inotify on `docs/plans/` |
| 🌿 | `commit` — commit or merge | git post-commit / post-merge hooks |
| 🌿 | `push` — attempt / success / **FAILED** | git pre-push hook + push-wrapper |
| 🌿 | `pre-commit` — start / pass / **FAILED** | git pre-commit hook |
| 🌿 | `branch` — branch switch | git post-checkout hook |
| 🌿 | `stash` — git stash | Claude bash classifier |
| 🎯 | `bead` — create / update / close / reopen | Claude bash classifier |

## Architecture

One monitor process (`broadcastr monitor`) covers everything:
- **Main thread** — polls per-repo + global bus, deduplicates, formats, prints to stdout
- **Thread** — inotify on `docs/sessions/`, emits `session-doc` events
- **Thread** — inotify on `docs/plans/`, emits `plan` events
- **Thread** — tails global bus for alert-tier events, dispatches to apprise

Events live in two JSONL files:
- Per-repo: `<repo>/.broadcastr/events.jsonl` (gitignored)
- Global: `~/.claude/broadcastr/events.jsonl` (cross-repo, host-local)

## Configuration

Override per-session with env vars (or set in `userConfig`):

| Var | Default | Effect |
|-----|---------|--------|
| `BROADCASTR_DISABLED` | `0` | `1` = silence this session entirely |
| `BROADCASTR_GLOBAL_FEED` | `1` | `0` = per-repo bus only |
| `BROADCASTR_MUTE` | _(empty)_ | Comma-separated categories to suppress |

## CLI

```bash
broadcastr emit <category> <tier> <summary> [--data <json>]
broadcastr tail
broadcastr recent [--since=5m]
broadcastr status
```

## Skills

- `broadcastr` — read the feed, mute categories, emit manually
- `broadcastr-install-hooks` — idempotent per-repo git-hook installer
