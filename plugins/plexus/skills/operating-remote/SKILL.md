---
name: operating-remote
description: Use when the user mentions operating on a named remote host that has a Plexus `remotes/<host>/REMOTE.md` profile, such as squirts, dookie, steamy, or another homelab device. Before taking action, load durable host memory and live context with `remote-context.py <host>`. This is the host-scoped equivalent of CLAUDE.md/AGENTS.md for remote machines.
argument-hint: <host> [--json] [--no-probe]
---

# Operating Remote Hosts With Plexus

Use this skill when the task is about a named remote machine and Plexus has a
matching `remotes/<host>/REMOTE.md` profile.

## Dynamic Context

If invoked with a host argument, use the injected context below as the current
operating context before making changes:

!`python3 "${CLAUDE_PLUGIN_ROOT:-plugins/plexus}/scripts/remote-context.py" $ARGUMENTS`

## Required First Step

If the dynamic context block is empty, failed, or the skill was auto-triggered
without `$ARGUMENTS`, identify the host from the user's request and run:

```bash
python3 "${CLAUDE_PLUGIN_ROOT:-plugins/plexus}/scripts/remote-context.py" <host>
```

Use `--no-probe` only when the user asks for an offline plan or when SSH/live
tools are unavailable.

## How To Use The Context

Treat `REMOTE.md` as durable host memory: roles, important paths, access
patterns, guardrails, and known quirks. Treat live probe output as the current
state. If the two disagree, call out the discrepancy and prefer observed live
state for operational decisions.

## Operating Rules

- Use non-interactive SSH: `ssh -o BatchMode=yes <host> <command>`.
- Inspect before changing state: uptime, disk, memory, failed units, Docker
  containers, and recent syslog entries.
- Prefer host-specific guardrails in `REMOTE.md` over generic assumptions.
- For service work, capture enough before/after evidence to prove the change.
- If a task touches a reverse proxy, certificate, firewall, storage pool, or
  auth boundary, validate first and use a rollback path.

## Related Context

Plexus pairs well with `cortex:cortex-logs`: recent logs and AI/session history explain
what changed before the current request. It also composes with service-specific
skills like `tailscale`, `unraid`, `create-swag-config`, and `homelab-map`.
