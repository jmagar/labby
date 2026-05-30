---
name: broadcastr
description: Read or interact with the broadcastr activity feed across concurrent Claude sessions. Use when the user says "what are other agents doing", "show recent activity", "mute plan-exec notifications", "what's on the bus", "broadcastr status", "tail broadcastr", "emit a manual broadcastr event", or asks why a notification appeared. Also use to disable broadcastr in the current session (BROADCASTR_DISABLED=1) or check the global feed. Does NOT fire on generic "show me activity" or "what's happening" unless broadcastr is explicitly named.
---

# broadcastr

The broadcastr plugin captures activity from concurrent Claude Code sessions into a shared JSONL bus. This skill explains how to interact with it.

## Reading the feed

The plugin's `broadcastr-feed` monitor automatically prints each new event as a notification line:

```
[info] commit · commit a1b2c3d: <subject> · claude-code@dookie
[alert] push · push FAILED: feature/x · claude-code@steamy
```

Your own session's events are suppressed. Categories listed in `BROADCASTR_MUTE` (comma-separated) are filtered out.

## CLI

```bash
broadcastr emit <category> <tier> <summary> [--data <json>]   # manual emit
broadcastr tail                                                # one-shot tail (same filtering as monitor)
broadcastr recent --since=5m                                   # dump recent events as JSONL
broadcastr status                                              # bus paths, sizes, event counts
broadcastr mute <category>[,<category>...]                     # add to BROADCASTR_MUTE for this session
```

## Configuration

Set via plugin `userConfig` or env var:

- `BROADCASTR_DISABLED=1` — silence this session entirely (no emits, no notifications)
- `BROADCASTR_GLOBAL_FEED=0` — don't tail the cross-repo global bus
- `BROADCASTR_MUTE=plan-exec,session-doc` — drop these categories

## Manual emit example

```bash
broadcastr emit cli info "starting big migration of plugins/broadcastr"
```

## Where things live

- Per-repo bus: `<repo>/.broadcastr/events.jsonl` (gitignored)
- Global bus: `~/.claude/broadcastr/events.jsonl` (host-local, cross-repo)
- Rotated copies: `events.jsonl.1`, `.2`, `.3`

## Install in a new repo

Run the companion skill `broadcastr-install-hooks` to drop git-hook shims into the current repo. Without it, you'll get hook events from Claude (SessionStart, bash classifier) and inotify watchers, but no commit/push/branch events.
