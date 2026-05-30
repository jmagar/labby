---
name: broadcastr-install-hooks
description: Install broadcastr's git-hook shims into the current repo. Use when the user says "install broadcastr hooks", "wire up broadcastr in this repo", "set up the broadcastr git hooks", or after first installing the broadcastr plugin in a new repo. Idempotent — safe to run repeatedly. Preserves any pre-existing hook as `<hook>.broadcastr-prev` and chains to it.
---

# broadcastr-install-hooks

Drop broadcastr's shim hooks into the current repo's `.git/hooks/` so commits, pushes, and branch operations emit events to the bus.

## When to use

- After first installing the broadcastr plugin
- After cloning a fresh repo where you want broadcastr active
- Any time commit/push notifications go silent in a repo (re-installs are safe)

## How

```bash
"$BROADCASTR_PLUGIN_ROOT/skills/broadcastr-install-hooks/scripts/install-git-hooks.sh" [repo_path]
```

`repo_path` defaults to `$PWD`. The script installs five hooks: `post-commit`, `pre-commit`, `pre-push`, `post-checkout`, `post-merge`. Pre-existing hooks are preserved as `<name>.broadcastr-prev` and chained.

## Behavior

- **Idempotent.** Running twice is a no-op.
- **Non-destructive.** Existing hooks become `.broadcastr-prev` and continue to run.
- **Identifies our shims** by a marker comment (`# broadcastr-install-hooks SHIM v1`), so reinstalls don't stack.
