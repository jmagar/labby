# Plugin Capability Notes

These notes capture the Axon research pass plus the current official docs used
while scaffolding Plexus.

## Claude Code Plugins

Claude Code plugins are self-contained directories. The manifest lives at
`.claude-plugin/plugin.json` and is optional, but useful for metadata and custom
component paths. If present, `name` is the only required field.

Supported Claude Code component locations:

- `skills/` - skill directories with `SKILL.md`.
- `commands/` - flat Markdown skills; use `skills/` for new plugins.
- `agents/` - subagent Markdown files.
- `output-styles/` - output style definitions.
- `themes/` - color themes, currently experimental when declared by manifest.
- `hooks/hooks.json` - lifecycle hook configuration.
- `.mcp.json` - MCP server definitions.
- `.lsp.json` - LSP server configurations.
- `monitors/monitors.json` - background monitors, currently experimental.
- `bin/` - executables added to the Bash tool PATH.
- `settings.json` - default settings; currently only `agent` and
  `subagentStatusLine` are supported.

Recognized Claude Code manifest fields:

- `$schema`
- `name`
- `displayName`
- `version`
- `description`
- `author`
- `homepage`
- `repository`
- `license`
- `keywords`
- `skills`
- `commands`
- `agents`
- `hooks`
- `mcpServers`
- `outputStyles`
- `lspServers`
- `experimental.themes`
- `experimental.monitors`
- `userConfig`
- `channels`
- `dependencies`

## Codex Plugins

Codex plugins require `.codex-plugin/plugin.json` for this repo's scaffold
workflow. The manifest identifies the plugin, points at bundled components, and
defines install-surface metadata.

Supported Codex component locations in the current docs/scaffold:

- `skills/` - skill directories with `SKILL.md`.
- `hooks/hooks.json` - lifecycle hooks.
- `.mcp.json` - bundled MCP server definitions.
- `.app.json` - app or connector mappings.
- `assets/` - icons, logos, screenshots, and other visual assets.

Recognized Codex manifest fields from the scaffold/docs:

- `name`
- `version`
- `description`
- `author.name`
- `author.email`
- `author.url`
- `homepage`
- `repository`
- `license`
- `keywords`
- `skills`
- `mcpServers`
- `apps`
- `hooks`
- `interface.displayName`
- `interface.shortDescription`
- `interface.longDescription`
- `interface.developerName`
- `interface.category`
- `interface.capabilities`
- `interface.websiteURL`
- `interface.privacyPolicyURL`
- `interface.termsOfServiceURL`
- `interface.defaultPrompt`
- `interface.brandColor`
- `interface.composerIcon`
- `interface.logo`
- `interface.screenshots`

## Source Notes

- Axon was asked separately for Claude Code plugin creation guidance and Codex
  plugin creation guidance.
- Axon was also asked where mutable plugin data belongs. The answer confirmed
  that plugin payload paths such as `${CLAUDE_PLUGIN_ROOT}` should be treated as
  read-only package assets, while `${CLAUDE_PLUGIN_DATA}` is the persistent
  location for user-authored data that survives plugin upgrades.
- Official Claude Code plugin reference confirmed components, manifest fields,
  path behavior, and plugin CLI commands.
- Official OpenAI Codex plugin docs confirmed the `.codex-plugin/plugin.json`
  scaffold, marketplace registration flow, manifest fields, path rules, MCP
  config, apps, hooks, and assets.
