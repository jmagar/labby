# TUI

Last updated: 2026-04-09

The TUI is a plugin marketplace browser and manager for Claude Code, Codex, and Gemini CLI.

## Scope

The TUI exists to help users:

- browse and configure `lab`'s own compiled-in services (enable/disable, set env vars, wire into `.mcp.json`)
- add, browse, and refresh plugin marketplaces for Claude Code and Codex
- browse and install Gemini CLI extensions directly from GitHub repos
- inspect available plugins/extensions with metadata (description, version, author, category)
- install and remove plugins/extensions into Claude Code, Codex, and/or Gemini CLI
- update the `lab` binary itself

It is not intended to replicate every CLI or MCP operation.

## Implementation

- `ratatui`
- `crossterm`
- always compiled

## Theme Boundary

The TUI should align with the CLI design system at the semantic-token level, not by reusing the CLI renderer directly.

Rules:

- reuse Aurora terminal palette values where they make sense
- reuse semantic token names and status meaning
- do not reuse the CLI string-rendering helpers or plain-text fallback API from `crates/lab/src/output/`
- TUI rendering owns its own `ratatui` style mapping, layout, and interaction treatment
- future TUI theme work should treat the CLI contract as a palette and semantics reference, not as a component library

## Supported Ecosystems

The TUI supports three plugin ecosystems with different conventions.

### Claude Code

| Concept | Path / Command |
|---------|----------------|
| Manifest | `plugins/.claude-plugin/plugin.json` |
| Marketplace file | `.claude-plugin/marketplace.json` |
| Plugin cache | `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/` |
| Marketplace state | `~/.claude/plugins/known_marketplaces.json` |
| Install state | `~/.claude/plugins/installed_plugins.json` |
| CLI | `claude plugin marketplace add/list/remove/update`, `claude plugin list/install/uninstall` |

Marketplace sources: `github` (`owner/repo`), `url` (any git URL), `git-subdir` (sparse monorepo), `npm`, or relative path.

### Codex

| Concept | Path |
|---------|------|
| Manifest | `.codex-plugin/plugin.json` |
| Marketplace file | `.agents/plugins/marketplace.json` |
| Repo marketplace | `$REPO_ROOT/.agents/plugins/marketplace.json` |
| Personal marketplace | `~/.agents/plugins/marketplace.json` |
| Plugin cache | `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/` |
| Plugin state | `~/.codex/config.toml` |
| CLI | none — file-based only |

Codex marketplace source type is `local` with a `path` field. The TUI manages Codex plugins by reading/writing the marketplace JSON files and copying plugin directories directly, since there is no CLI equivalent.

### Gemini CLI

| Concept | Path / Command |
|---------|----------------|
| Manifest | `gemini-extension.json` (at repo root, no wrapper directory) |
| Install location | `~/.gemini/extensions/<name>/` |
| CLI | `gemini extensions install/update/uninstall/enable/disable/list` |

Gemini has no marketplace concept. Extensions are GitHub repos installed directly by URL or `github.com/user/repo` shorthand. The TUI treats each extension repo as a single-item catalog. Changes take effect only after restarting the Gemini CLI session.

## How Marketplaces Work

A marketplace is a JSON catalog of plugins. The TUI reads catalogs from local files (for already-added marketplaces) and fetches them remotely for preview (for repos not yet added). Gemini extensions have no catalog — each repo is a standalone extension.

### Detecting ecosystem

When the user provides a repo, the TUI probes for all manifest and marketplace files:

| Files found | Behaviour |
|-------------|-----------|
| `.claude-plugin/marketplace.json` only | Claude Code marketplace preview |
| `.agents/plugins/marketplace.json` only | Codex marketplace preview |
| `gemini-extension.json` only | Gemini CLI extension preview |
| Multiple ecosystem files present | Prompt user to choose ecosystem before preview |
| `plugins/.claude-plugin/plugin.json` (no marketplace) | Single Claude Code plugin — synthesize one-entry marketplace |
| `.codex-plugin/plugin.json` (no marketplace) | Single Codex plugin — synthesize one-entry marketplace |

Synthesizing a single-entry marketplace means the preview screen and confirm flow are identical regardless of whether the repo is a full catalog or a single plugin. No special-casing downstream. Gemini extensions always follow the single-extension path since there is no catalog format.

### Data sources for known marketplaces

**Claude Code:**
- `~/.claude/plugins/known_marketplaces.json` → marketplace list + local clone paths
- `<installLocation>/.claude-plugin/marketplace.json` → plugin catalog for each marketplace
- `~/.claude/plugins/installed_plugins.json` → install state

**Codex:**
- `~/.agents/plugins/marketplace.json` (personal) and `$REPO_ROOT/.agents/plugins/marketplace.json` (repo) → catalogs
- `~/.codex/config.toml` → enabled/disabled state per plugin

**Gemini CLI:**
- `~/.gemini/extensions/` directory listing → installed extensions
- Each `~/.gemini/extensions/<name>/gemini-extension.json` → name, version, description

Cross-referencing catalog + install state produces: installed+enabled, installed+disabled, available-not-installed.

## Primary Screen

The TUI has three top-level tabs, switchable with `1` / `2` / `3` or `Tab`:

| Tab | Key | Purpose |
|-----|-----|---------|
| **Services** (default) | `1` | Browse and configure `lab`'s compiled-in services; toggle `.mcp.json` wiring |
| **Plugins** | `2` | Browse marketplaces and install plugins/extensions for Claude Code, Codex, Gemini |
| **Update** | `3` | Check for and install a new `lab` binary |

### Services tab

The service list grouped by category. Each row shows service name, short description, category, health dot, and enabled state. Selecting a service opens the env var detail pane on the right.

### Plugins tab

A plugin list grouped by category with install state and concise metadata. Each row reflects:

- plugin/extension name
- short description
- version
- marketplace source label (or repo for Gemini)
- ecosystem badge (Claude Code / Codex / Gemini / combinations)
- install state (`installed` / `available` / `update available`)

## Interaction Model

Expected interaction is simple:

**Services tab:**
- navigate the service list by category
- select a service to view env vars and health detail
- toggle a service enabled/disabled (writes `.mcp.json`)
- press `e` to open `~/.lab/.env` in `$EDITOR`
- press `r` to reveal a masked secret (shows banner)
- press `F5` to refresh health dots

**Plugins tab:**
- navigate marketplaces and their plugin lists
- add / remove a marketplace (Claude Code, Codex)
- browse and install extensions directly from repos (Gemini)
- install / remove a plugin or extension
- update all or a specific marketplace

**Global:**
- `1` / `2` / `3` or `Tab` — switch tabs
- `j` / `k` or `↑` / `↓` — navigate list
- `Enter` — select / confirm
- `Esc` — back / cancel
- `q` / `Ctrl-C` — quit

Complex modal workflows must be the exception, not the baseline.

## Adding a New Marketplace or Extension (Preview Flow)

Users provide a GitHub `owner/repo` shorthand or any git URL. The TUI previews the content before registering it — no state is written until the user confirms.

### Preview fetch

**GitHub repos** — fetch manifest/marketplace file via raw content URL, no clone required:

```
# Claude Code marketplace
https://raw.githubusercontent.com/<owner>/<repo>/<branch>/.claude-plugin/marketplace.json

# Claude Code single plugin
https://raw.githubusercontent.com/<owner>/<repo>/<branch>/plugins/.claude-plugin/plugin.json

# Codex marketplace
https://raw.githubusercontent.com/<owner>/<repo>/<branch>/.agents/plugins/marketplace.json

# Codex single plugin
https://raw.githubusercontent.com/<owner>/<repo>/<branch>/.codex-plugin/plugin.json

# Gemini CLI extension
https://raw.githubusercontent.com/<owner>/<repo>/<branch>/gemini-extension.json
```

**Arbitrary git URLs** — sparse-clone into a temp directory, read the files, then discard:

```
git clone --depth 1 --sparse --filter=blob:none <url> /tmp/lab-preview-xxx
git -C /tmp/lab-preview-xxx sparse-checkout set .claude-plugin .agents/plugins .codex-plugin gemini-extension.json
```

The temp clone is deleted after the preview is rendered regardless of what the user does next.

### Preview screen

Displays:

- name, owner, description
- detected ecosystem(s)
- full plugin list or single extension details with name, description, category, version

The user can browse and optionally mark items for installation before confirming.

### Confirm flow

**Claude Code:**
1. `claude plugin marketplace add <source>` — marketplace is now registered
2. For each selected plugin → `claude plugin install <name>@<marketplace>`

**Codex:**
1. Write or update the appropriate `marketplace.json` (`~/.agents/plugins/` or `$REPO_ROOT/.agents/plugins/`)
2. For each selected plugin → copy plugin directory into `~/.codex/plugins/cache/` and update `~/.codex/config.toml`

**Gemini CLI:**
1. `gemini extensions install github.com/<owner>/<repo>` (optionally with `--ref <branch>`)
2. Inform user that a Gemini CLI restart is required for the extension to take effect

If the user cancels, nothing is written. The temp clone is cleaned up and all state is unchanged.

## Plugin Install / Remove

**Claude Code** — delegates entirely to the CLI:

- Install: `claude plugin install <name>@<marketplace>`
- Remove: `claude plugin uninstall <name>@<marketplace>`

**Codex** — file-based (no CLI):

- Install: copy plugin directory to `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/`, update `~/.codex/config.toml`
- Remove: remove cache directory, remove entry from `~/.codex/config.toml`

**Gemini CLI** — delegates entirely to the CLI:

- Install: `gemini extensions install github.com/<owner>/<repo>`
- Update: `gemini extensions update <name>` or `--all`
- Remove: `gemini extensions uninstall <name>`
- Enable/disable: `gemini extensions enable/disable <name>`

The TUI must not invent its own install logic for Claude Code or Gemini CLI. For Codex, file manipulation is the only available mechanism.

## Lab Service Manager

The primary tab of the TUI is a browser for `lab`'s own compiled-in services. This is the main reason to run `lab plugins` — it lets you see what's available and wire services into your MCP config without hand-editing files.

### Service list

Services are enumerated from `PluginMeta` constants at compile time via `metadata.rs`. The list is grouped by `Category`:

| Category | Services |
|----------|----------|
| Media | plex, tautulli |
| Servarr | radarr, sonarr, prowlarr |
| Download | sabnzbd, qbittorrent |
| Notes | memos |
| Documents | linkding, bytestash |
| Network | tailscale, unifi, unraid |
| Notifications | apprise, gotify |
| Ai | openai, notebooklm, qdrant, tei |
| Bootstrap | extract |

Each row shows: service name, short description, category, health dot (from `labby doctor`), and enabled state.

### Enable / disable

Enabling a service adds it to the `--services` array in the `lab` entry of `.mcp.json`. Disabling removes it. The same binary runs with different service sets depending on which are listed.

### Env var panel

Selecting a service opens a detail pane showing `required_env` and `optional_env` from its `PluginMeta`. Values present in `~/.lab/.env` are shown inline. Values with `secret: true` are masked by default — reveal with `r`, which shows a banner.

Missing required env vars are highlighted. The user can press `e` to open `~/.lab/.env` in `$EDITOR`.

### `.mcp.json` patching rules

The TUI is the only part of `lab` that writes `.mcp.json`. Every write must:

1. **Backup first.** Copy `.mcp.json` → `.mcp.json.bak.<timestamp>`.
2. **Atomic write.** Write to a temp file, then rename.
3. **Preserve unrelated keys.** Parse the full file, mutate only the `lab` entry's `--services` array, serialize back.
4. **Dedupe** the services array on write.
5. **Refuse to write** if the file contains invalid JSON — do not silently overwrite user edits.

### `.mcp.json` location

`.mcp.json` lives at the **repo root** (the directory containing the `lab` workspace `Cargo.toml`). The TUI locates it by walking up from `std::env::current_dir()` until it finds a `Cargo.toml` with `[workspace]`. This is the same root Claude Code uses as its project directory.

### `.mcp.json` shape

The `lab` entry follows the standard MCP server entry format. The `--services` arg lists enabled services:

```json
{
  "mcpServers": {
    "lab": {
      "command": "/path/to/labby",
      "args": ["mcp", "--services", "radarr", "sonarr", "plex"]
    }
  }
}
```

The TUI appends/removes service names from the `args` array immediately after `--services`. All other keys in `mcpServers` and the file are left untouched.

### First-run (no `.mcp.json`)

If `.mcp.json` does not exist, the TUI creates it with an empty `--services` list and the path to the current `lab` binary resolved via `std::env::current_exe()`:

```json
{
  "mcpServers": {
    "lab": {
      "command": "/path/to/labby",
      "args": ["mcp", "--services"]
    }
  }
}
```

No backup is made on first-run creation since there is nothing to back up.

### Service availability rule

A service is only functional when **both** conditions are met:

1. Listed in the `--services` array in `.mcp.json`
2. Required env vars present in `~/.lab/.env`

Either condition alone is not enough. A service in `--services` with missing env vars is non-functional. A service with env vars set but not listed in `--services` is not exposed by the MCP server.

### Health dots

Each service row shows a live health indicator sourced from `labby doctor`:

| Dot | Meaning |
|-----|---------|
| ● green | in `--services`, env vars present, reachable, auth ok |
| ● yellow | in `--services`, env vars present, reachable, auth failed |
| ● red | in `--services`, env vars present, not reachable |
| ○ grey | env vars missing — not functional regardless of `--services` |
| ○ dim | not in `--services` — not exposed regardless of env vars |

Health is fetched once on TUI open and can be refreshed with `F5`.

## Binary Updates

The TUI exposes a `lab` binary update flow:

- check latest release from the configured update source
- display current vs available version
- prompt user to confirm before downloading
- download and replace binary atomically
- verify the new binary reports the expected version

## State Policy

TUI state is ephemeral.

Rules:

- no persisted window state
- no persisted selection state
- no config file for TUI preferences

This is a hard anti-creep rule.

## Relationship to CLI

The TUI is a convenience layer for marketplace browsing and plugin lifecycle. For Claude Code and Gemini CLI, the respective CLIs are the underlying operator surface. For Codex, the TUI is the operator surface since no equivalent CLI exists.
