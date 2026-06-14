# Lab Plugins

Lab plugins (the checked-in `plugins/labby` tree and the generated marketplace
tree alike) ship **no binary**. Hosts install `labby` explicitly and the
binary owns the setup flow from there:

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

## Generated marketplace tree

`labby marketplace generate --out <dir>` builds a Claude Code plugin
marketplace tree from the compiled service registry and each service
`PluginMeta`:

- `lab-core/`: setup commands and an `install-labby` skill (no binary).
- `lab-<service>/`: one config-only service plugin per service with required env vars.
- `.claude-plugin/marketplace.json` and/or `.agents/plugins/marketplace.json`: indexes for generated plugin marketplaces.

Service plugins invoke `labby` from `PATH`. Their `.mcp.json` points at:

```json
{ "command": "labby", "args": ["mcp", "--services", "<service>"] }
```

The core plugin provides:

- `/setup-core`: opens `labby setup --mode plugin`.
- `/setup-core-advanced`: opens `labby setup --mode full`.

Plugin manifests intentionally omit `version`; marketplace release identity is Git-SHA based unless an individual plugin explicitly documents a different manifest-level version contract.

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
