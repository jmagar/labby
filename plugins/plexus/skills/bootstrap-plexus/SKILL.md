---
name: bootstrap-plexus
description: "This skill should be used when the user wants to install, set up, repair, or initialize the Plexus remote-device memory plugin; when they ask where Plexus stores persistent REMOTE.md host profiles; or when host profiles are missing and need to be seeded from bundled defaults."
argument-hint: "[--json] [--data-dir PATH]"
---

# Bootstrap Plexus

Use this skill before first use of Plexus, after plugin install/upgrade, or when
the user asks to repair missing host profiles.

## Dynamic Bootstrap

This initializes missing persistent profiles from bundled templates. It never
overwrites existing `REMOTE.md` files.

!`python3 "${CLAUDE_PLUGIN_ROOT:-plugins/plexus}/scripts/remote-context.py" --init $ARGUMENTS`

## Persistent Data Contract

Plexus must not store mutable host memory in the plugin source tree. Bundled
files under `${CLAUDE_PLUGIN_ROOT:-plugins/plexus}/templates/remotes/` are
defaults only.

Persistent host profiles live at:

```text
${CLAUDE_PLUGIN_DATA}/remotes/<host>/REMOTE.md
```

During local development outside an installed plugin, the fallback is:

```text
~/.plexus/remotes/<host>/REMOTE.md
```

The user can override this with `PLEXUS_DATA_DIR` or `--data-dir`.

## After Bootstrap

Report the data directory and any profiles seeded. If a profile already exists,
leave it untouched and tell the user it was preserved.

To inspect a seeded profile:

```bash
python3 "${CLAUDE_PLUGIN_ROOT:-plugins/plexus}/scripts/remote-context.py" <host> --no-probe
```
