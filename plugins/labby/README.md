# labby — Claude Code plugin

Skills and MCP configuration for the Lab homelab control plane.

This plugin does **not** bundle the `labby` binary and does not auto-install
or auto-repair anything. It ships:

- the `using-labby` skill,
- the `creating-snippets` skill for Labby Code Mode snippet authoring,
- an HTTP MCP server entry pointing at a running `labby serve`
  (`${user_config.server_url}/mcp` — remote machines never need a local binary),
- advisory hooks: SessionStart reports setup status via
  `labby setup plugin-hook --no-repair` when `labby` is on `PATH` (and prints
  an install pointer when it is not); ConfigChange syncs plugin settings via
  `labby setup plugin-hook`.

## Installing labby (server host only)

```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/labby/main/scripts/install.sh | sh
labby setup
```

The script downloads the latest GitHub release for this platform
(sha256-verified) into `~/.local/bin/labby`, falling back to
`cargo install --git https://github.com/jmagar/labby --bin labby --all-features`
when no release asset exists. Everything after install — config, credentials,
connectivity checks, repair — is owned by `labby setup`.
The web app also serves the same script at `https://labby.tootie.tv/install.sh`
for convenience, but GitHub is the canonical installer source.

## Configuration

Plugin settings (server URL, auth mode, token, …) are declared in
`.claude-plugin/plugin.json` `userConfig` and synced into `~/.labby/.env` as
`LAB_*` variables by `labby setup plugin-hook` when settings change.
