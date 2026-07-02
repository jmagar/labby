# Stash

Stash is an always-on local versioning service built into `lab`. It tracks versioned snapshots of homelab components — skills, agents, commands, hooks, scripts, configs, and more — stored under `~/.labby/stash/` on the local machine. No external service or database is required.

## Overview

Stash solves two related problems:

1. **Versioning local artefacts** — lab manages many small configuration files and directory bundles (skills, agents, MCP configs) that change over time. Stash gives each one an immutable revision history without needing git.

2. **Syncing to remote storage** — via the provider model, stash can push or pull revisions to another directory on disk (filesystem provider) or, in future versions, to cloud storage.

Stash is always compiled in — it is not feature-gated.

## Storage Model

All data lives under a configurable stash root, defaulting to `~/.labby/stash/`.

```
~/.labby/stash/
├── components/        # Component records (one .json per component)
│   ├── <id>.json      # StashComponent record
│   ├── <id>.lock      # Advisory lock file (created during write ops)
│   └── <id>.deploy.lock
├── revisions/         # Immutable revision snapshots
│   └── <rev_id>/
│       ├── meta.json  # StashRevision metadata
│       └── files/     # Copied workspace content
├── workspaces/        # Live working copies
│   └── <id>/          # Component workspace (dir or file)
├── providers/         # Provider link records (<id>.json)
└── targets/           # Deploy target records (<id>.json)
```

Key properties:
- **No object store** — revision files are plain copies, not a content-addressed store.
- **SHA-256 digests** — each revision stores a `content_digest` (SHA-256 of all file contents concatenated in sorted-by-relative-path order).
- **Atomic writes** — all JSON records are written via a temp file + rename.
- **Advisory locking** — component operations acquire a `.lock` file to prevent concurrent writes; deploy operations use a `.deploy.lock`. Timeouts yield `conflict` errors.

### Configuring the stash root

The stash root is read from `[workspace].root` in `config.toml`, falling back to `~/.labby/stash`. Set it in `~/.labby/config.toml`:

```toml
[workspace]
root = "/data/homelab/stash"
```

## Component Kinds

Stash tracks 13 component kinds. Each kind has a fixed workspace shape.

| Kind | Shape | Description |
|------|-------|-------------|
| `skill` | Directory | Claude Code skill definition |
| `agent` | Directory | AI agent bundle |
| `command` | Directory | Slash command definition |
| `channel` | Directory | Notification channel definition |
| `monitor` | Directory | Process / log monitor definition |
| `hook` | Directory | Lifecycle hook |
| `output_style` | Directory | Output style / renderer |
| `theme` | Directory | Visual theme |
| `settings` | File | Settings file (JSON, TOML, YAML) |
| `mcp_config` | File | MCP configuration file |
| `lsp_config` | File | LSP configuration file |
| `script` | File | Shell or script file |
| `bin_file` | File | Compiled binary artefact |

**Directory-shaped components** occupy a workspace directory under `workspaces/<id>/`.

**File-shaped components** occupy a single file under `workspaces/<id>/file` (or the original filename when imported).

**Detection heuristics** (used by `component.import` when `kind` is not specified):
- `.json`, `.toml`, `.yaml` files → `settings` or `mcp_config` or `lsp_config` depending on filename patterns
- `.sh`, `.bash`, `.zsh`, `.fish` files → `script`
- Executable files without extension → `bin_file`
- Directories containing `skill.md` or `index.md` → `skill`
- Directories containing `agent.md` → `agent`
- Directories containing `command.md` → `command`
- All other directories → `agent` (fallback; override with `kind` param)

If kind cannot be auto-detected and no override is given, `component.import` returns `ambiguous_kind`.

## Marketplace-origin components

Stash is also the durable home for **marketplace forks**. When the marketplace
service forks a plugin or an individual artifact, it does not keep a private
copy — it adopts the content as a first-class Stash component. These components
are ordinary Stash components in every respect (revisions, workspaces, providers,
deploy) and additionally carry origin metadata that records where they came from.

### `StashOrigin` and `origin_meta`

Each component record may carry an optional `origin_meta` field describing its
provenance via the `StashOrigin` shape. The marketplace variant is
`StashOrigin::Marketplace`:

```json
{
  "kind": "marketplace",
  "plugin_id": "demo@labby",
  "artifact_path": "skills/demo/SKILL.md",
  "source_version": "1.0.0",
  "source_fingerprint": "tree-fingerprint-abc123"
}
```

Compatibility rules:

- `origin_meta` is optional and defaults to `null`.
- Component records written before this field existed must still deserialize.
- New code uses `origin_meta` for behavior; the older free-form `origin` string
  is retained only for display/back-compat.

A component with `StashOrigin::Marketplace` is a normal, first-class Stash
component — there is no separate "marketplace store". This is the canonical
storage for marketplace forks.

### Fork sidecar and legacy path

Marketplace-specific helper metadata for a fork (base snapshot, pending update,
drift cache) lives in a Stash-owned sidecar directory at
`<stash_root>/marketplace/<component_id>/`, **outside** the tracked workspace, so
it never appears in revisions, provider sync, export, or deploy payloads.

An older representation wrote a `.stash.json` file under
`<workspace.root>/plugins/<plugin_id>/`. That path is **retired and read-only**:
it is recognized only as a legacy discovery branch and migrated on read into the
modern component-record + sidecar model above. It is not the authoritative fork
store.

The full marketplace ↔ stash boundary is specified in
[marketplace-stash-integration.md](../contracts/marketplace-stash-integration.md).

## Actions

All 18 actions are dispatched via `stash({ "action": "...", "params": {...} })`.

### Component lifecycle (9 actions)

| Action | Destructive | Description |
|--------|-------------|-------------|
| `components.list` | No | List all tracked components |
| `component.get` | No | Get details for a single component |
| `component.create` | No | Create an empty component record |
| `component.import` | Yes | Import a local path into the stash |
| `component.workspace` | No | Get the live workspace path |
| `component.save` | No | Snapshot the current workspace |
| `component.revisions` | No | List saved revisions |
| `component.export` | Yes | Export a component to a local path |
| `component.deploy` | Yes | Deploy a component to a registered target |

### Provider sync (4 actions)

| Action | Destructive | Description |
|--------|-------------|-------------|
| `providers.list` | No | List registered providers (optionally filtered by `component_id`) |
| `provider.link` | No | Register a storage provider for a component |
| `provider.push` | Yes | Push the head revision to a provider |
| `provider.pull` | Yes | Pull the latest revision from a provider |

### Deploy targets (3 actions)

| Action | Destructive | Description |
|--------|-------------|-------------|
| `targets.list` | No | List registered deploy targets |
| `target.add` | No | Register a new deploy target |
| `target.remove` | Yes | Remove a registered deploy target |

### Built-in meta-actions (2 actions)

| Action | Description |
|--------|-------------|
| `help` | Show the action catalog |
| `schema` | Return parameter schema for a named action |

## Provider Model

Providers allow a component's revisions to be synced to remote storage.

### v1: Filesystem provider

The only implemented provider in v1. Syncs revisions to another directory on the local host (e.g. a NAS mount or a shared path).

**Config:**
```json
{ "root": "/mnt/nas/stash-backup" }
```

Revision content is stored at `<root>/<component_id>/<rev_id>/`.

**Register a filesystem provider:**
```json
{
  "action": "provider.link",
  "params": {
    "id": "<component_id>",
    "kind": "filesystem",
    "label": "NAS backup",
    "config": { "root": "/mnt/nas/stash" }
  }
}
```

**Push:**
```json
{ "action": "provider.push", "params": { "id": "<cid>", "provider_id": "<pid>" } }
```

**Pull:**
```json
{ "action": "provider.pull", "params": { "id": "<cid>", "provider_id": "<pid>" } }
```

### Deferred providers

- **Google Drive** — deferred to v2
- **S3-compatible storage** — deferred to v2

## Security Model

### Three-layer path safety

All paths undergo three checks:
1. **Absolute-path requirement** — `source_path`, `output_path`, and `path` params must be absolute.
2. **Symlink rejection** — symlinks encountered during workspace walks return `symlink_rejected` (HTTP 422). This prevents symlink-following attacks.
3. **System path rejection** — deploy targets may not point to `/etc`, `/usr`, `/bin`, `/sbin`, `/boot`, `/sys`, or `/proc`.

### `lab:admin` scope

Write operations (`component.import`, `component.save`, `component.export`, `component.deploy`, `provider.link`, `provider.push`, `provider.pull`, `target.add`, `target.remove`) require the `lab:admin` scope when invoked over HTTP. Read operations are accessible to any authenticated caller.

### Credential guard

Provider config objects must not contain secret values — use them for non-secret configuration only (e.g. `root` path for filesystem providers, bucket name for S3). Credentials are stored separately in `~/.labby/.env` and accessed by the provider at runtime.

Provider config is stored as JSON in `~/.labby/stash/providers/<id>.json`. These files are visible to any process with filesystem access to the stash root.

## Token and Config Storage

Provider link records are stored under `~/.labby/stash/providers/<id>.json`. No secrets are stored in provider config — only non-sensitive configuration like paths, bucket names, or provider labels.

Deploy target records are stored under `~/.labby/stash/targets/<id>.json`.

Both are plain JSON files written atomically via temp+rename.

## CLI Examples

```bash
# List components
lab stash components.list

# Create a new skill component
lab stash component.create kind=skill name=my-skill label="My Skill"

# Import a local skill directory into an existing component
lab stash component.import --yes id=<id> source_path=/home/user/.claude/skills/my-skill

# Save the current workspace state
lab stash component.save id=<id> label="v1.0.0"

# List revisions
lab stash component.revisions id=<id>

# Export to a path
lab stash component.export --yes id=<id> output_path=/tmp/my-skill-export

# Deploy to a registered target
lab stash component.deploy --yes id=<id> target_id=<tid>

# Register a filesystem provider
lab stash provider.link id=<id> kind=filesystem label="NAS" config='{"root":"/mnt/nas/stash"}'

# Push head revision to provider
lab stash provider.push --yes id=<id> provider_id=<pid>

# Pull latest from provider
lab stash provider.pull --yes id=<id> provider_id=<pid>
```

## MCP Examples

```jsonc
// List all components
stash({ "action": "components.list" })

// Create a component
stash({ "action": "component.create", "params": { "kind": "skill", "name": "my-skill", "label": "My Skill" } })

// Import a directory
stash({ "action": "component.import", "params": { "id": "01abc...", "source_path": "/home/user/.claude/skills/my-skill" } })

// Save a revision
stash({ "action": "component.save", "params": { "id": "01abc...", "label": "initial" } })

// Export a component
stash({ "action": "component.export", "params": { "id": "01abc...", "output_path": "/tmp/export" } })

// Link a filesystem provider
stash({ "action": "provider.link", "params": { "id": "01abc...", "kind": "filesystem", "label": "backup", "config": { "root": "/mnt/nas/stash" } } })
```

## HTTP Examples

```bash
# List components
curl -H "Authorization: Bearer $TOKEN" http://localhost:5150/v1/stash \
  -d '{"action": "components.list"}'

# Create a component
curl -H "Authorization: Bearer $TOKEN" http://localhost:5150/v1/stash \
  -d '{"action": "component.create", "params": {"kind": "skill", "name": "my-skill"}}'

# Export (destructive — requires confirm)
curl -H "Authorization: Bearer $TOKEN" http://localhost:5150/v1/stash \
  -d '{"action": "component.export", "params": {"id": "...", "output_path": "/tmp/out", "confirm": true}}'
```

## Error Reference

See [docs/ERRORS.md](./ERRORS.md) for the full error taxonomy and HTTP status mappings.

Stash-specific error kinds:
- `conflict` — advisory lock timed out (HTTP 409)
- `unsupported_provider` — provider kind not implemented (HTTP 422)
- `unsupported_component_kind` — operation not valid for this component kind (HTTP 422)
- `sync_failed` — provider push/pull I/O failure (HTTP 502)
- `workspace_too_large` — workspace exceeds 200 MiB (HTTP 413)
- `file_too_large` — single file exceeds 50 MiB (HTTP 413)
- `path_traversal` — path escapes target root (HTTP 422)
- `symlink_rejected` — symlink encountered during workspace walk (HTTP 422)
- `export_target_not_empty` — output directory non-empty and `force` not set (HTTP 409)
- `ambiguous_kind` — component kind could not be auto-detected (HTTP 422)
- `secrets_export_not_allowed` — `component.export` of a `settings`/`mcp_config` component is refused (may carry secrets); suggested 403, currently 500
- `too_many_files` — import workspace exceeds `MAX_FILE_COUNT`; suggested 413, currently 500
- `unknown_target` — `component.deploy` named an unregistered deploy target; suggested 404, currently 500
- `deploy_failed` — `component.deploy` failed during execution; HTTP 500

> Note: several of the kinds above (`secrets_export_not_allowed`, `too_many_files`, `unknown_target`) are not yet given an explicit HTTP status in `api/error.rs` and currently fall through to HTTP 500. See [ERRORS.md](../dev/ERRORS.md#stash-specific-kinds) for the canonical status mapping and suggested target codes.
