---
description: Load durable REMOTE.md memory and live operating context for a named host.
argument-hint: <host> [--json] [--no-probe]
---

# Remote Context

Load host-specific operating context before touching a remote device.

```bash
python3 "${CLAUDE_PLUGIN_ROOT:-plugins/plexus}/scripts/remote-context.py" $ARGUMENTS
```

Use the output as the working host context for this turn. Durable notes come
from the persistent plugin data profile at
`${CLAUDE_PLUGIN_DATA}/remotes/<host>/REMOTE.md` when installed, or
`~/.plexus/remotes/<host>/REMOTE.md` during local development. Live state comes
from SSH, Tailscale, Docker/systemd probes, and `syslog-mcp` when available.
