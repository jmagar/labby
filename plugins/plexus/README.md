# Plexus

Plexus is a remote-device memory plugin. It gives each important host a
`REMOTE.md` file that behaves like a host-scoped `CLAUDE.md`, then pairs that
durable memory with live probes and recent `syslog-mcp` history before an agent
touches the machine.

## Current Scaffold

- `skills/operating-remote/SKILL.md` - generic trigger and workflow for hosts
  that have a persistent `remotes/<host>/REMOTE.md` profile.
- `skills/bootstrap-plexus/SKILL.md` - initializes persistent plugin data from
  bundled templates without overwriting user-authored profiles.
- `commands/remote-context.md` - slash-command style workflow for loading one
  host's context.
- `scripts/remote-context.py` - helper that reads host memory and gathers live
  SSH/Tailscale/syslog context.
- `templates/remotes/squirts/REMOTE.md` - bundled default host memory draft.
- `.claude-plugin/plugin.json` - Claude Code plugin manifest.
- `.codex-plugin/plugin.json` - Codex plugin manifest.

## Model

`REMOTE.md` is the durable source of truth for human-authored host memory:
roles, access paths, guardrails, important directories, and known quirks. The
script adds current state: uptime, OS, resource usage, containers, failed
services, listening ports, Tailscale identity, and recent syslog messages.

Bundled profiles under `templates/remotes/` are defaults only. The first plugin
session seeds missing profiles into persistent plugin data:

```text
${CLAUDE_PLUGIN_DATA}/remotes/<host>/REMOTE.md
```

For local development outside an installed plugin, Plexus uses:

```text
~/.plexus/remotes/<host>/REMOTE.md
```

Set `PLEXUS_DATA_DIR` or pass `--data-dir` to override the data directory.

## Quick Check

```bash
python3 plugins/plexus/scripts/remote-context.py squirts --no-probe
python3 plugins/plexus/scripts/remote-context.py squirts --format json --no-probe
python3 plugins/plexus/scripts/remote-context.py --init
```

Remove `--no-probe` when SSH and local tools are available.
