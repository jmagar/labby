# Lab Plugins

The checked-in `plugins/labby` tree ships **no binary**. Hosts install `labby`
explicitly and the binary owns the setup flow from there:

```bash
curl -fsSL https://raw.githubusercontent.com/jmagar/lab/main/scripts/install.sh | sh
labby setup
```

`scripts/install.sh` downloads the latest GitHub release archive for the
platform (sha256-verified) into `~/.local/bin/labby`, falling back to
`cargo install --git` when no release asset exists. Its only job is bootstrap —
everything after first contact (config, credentials, connectivity, repair) is
owned by `labby setup`.

## Checked-in plugin (`plugins/labby`)

Skills and MCP configuration only. Its `.mcp.json` connects over HTTP to a
running `labby serve` (`${user_config.server_url}/mcp`), so machines that
install the plugin remotely never need a local binary at all. Hooks are
advisory: SessionStart runs `labby setup plugin-hook --no-repair` when `labby`
is on PATH and prints an install pointer when it is not; ConfigChange syncs
plugin settings via `labby setup plugin-hook` (again only when installed).
Nothing is auto-installed or auto-repaired at session start.

## Marketplace distribution

Lab no longer generates or publishes its own plugin marketplace. The marketplace
moved to a dedicated repo, [dendrite](https://github.com/jmagar/dendrite), so it
is decoupled from this Rust workspace. Dendrite catalogs `plugins/labby` (via a
`git-subdir` source pointing at this repo) alongside the other Lab/Labby plugins
and third-party entries.

Install `labby` with `scripts/install.sh` (above); browse and install
marketplace plugins through the `marketplace` dispatch service or the Labby web
UI.

Setup plugin lifecycle actions live in the `setup` dispatch service. The
canonical names follow the dotted `<resource>.<verb>` convention; the legacy
snake_case names remain as deprecated aliases:

| Canonical | Deprecated alias |
|-----------|------------------|
| `setup.plugins.installed` | `setup.installed_plugins` |
| `setup.plugin.install` | `setup.install_plugin` |
| `setup.plugin.uninstall` | `setup.uninstall_plugin` |
| `setup.services.status` | `setup.services_status` |

These four actions are restricted to loopback-only HTTP; both the canonical and
the alias forms are gated identically.

`plugin.install` and `plugin.uninstall` validate the registered service slug, derive `lab-<service>@lab`, check the org against `LAB_PLUGIN_ALLOWLIST`, and call the configured Claude Code CLI. Set `LAB_CLAUDE_BIN` when the binary is not named `claude`.

`labby help` and `lab://catalog` are env-aware by default: services with missing required env vars are hidden. Use `LAB_SHOW_ALL=1` or `labby help --all` to show the full compiled catalog.
