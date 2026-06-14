# Marketplace Stash Integration Contract

Status: Draft
Date: 2026-06-13

This contract defines the stable boundary between `marketplace`, `stash`, HTTP,
MCP, CLI, and gateway-admin for marketplace-origin Stash components.

## Service Ownership Contract

### Required Direction

```text
marketplace -> marketplace/stash_bridge -> stash helpers/store
```

Allowed:

- Marketplace bridge calls Stash service helpers.
- Marketplace update code reads marketplace-origin Stash components.
- Marketplace bridge may call Stash helpers directly only when it also preserves
  equivalent dispatch logging, lock discipline, and `spawn_blocking` behavior for
  blocking filesystem/store work.
- Gateway-admin calls Marketplace artifact actions for fork/update workflows.
- Gateway-admin calls Stash actions for generic stash browsing later.

Forbidden:

- Stash dispatch importing Marketplace modules.
- Stash resolving Marketplace sources.
- Stash shelling out to plugin/runtime CLIs.
- Frontend writing directly to filesystem paths.
- Marketplace update code creating a separate durable fork store outside Stash.

## Type Contract

### `StashComponent`

Serialized JSON includes all existing fields plus optional `origin_meta`.

```json
{
  "id": "01aryz6s41tpz5x11k39dv3r2g",
  "kind": "skill",
  "name": "demo-skill",
  "label": "Demo Skill",
  "head_revision_id": "01b7x6s41tpz5x11k39dv3r2g",
  "origin": "marketplace://demo@labby?artifact=skills/demo/SKILL.md",
  "origin_meta": {
    "kind": "marketplace",
    "plugin_id": "demo@labby",
    "artifact_path": "skills/demo/SKILL.md",
    "source_version": "1.0.0",
    "source_fingerprint": "tree-fingerprint-abc123"
  },
  "workspace_root": "/home/user/.lab/stash/workspaces/01aryz6s41tpz5x11k39dv3r2g/SKILL.md",
  "workspace_shape": "file",
  "unix_mode": null,
  "created_at": "2026-06-13T12:00:00Z",
  "updated_at": "2026-06-13T12:00:00Z"
}
```

Compatibility rules:

- `origin_meta` is optional and defaults to `null`.
- Old component records without `origin_meta` must deserialize.
- `origin` remains an optional display/backward-compatibility string.
- New code must use `origin_meta` for behavior.
- API responses may include absolute workspace paths for admin/local operator
  workflows. Gateway-admin list views should prefer component ids and relative
  artifact paths; absolute paths must not be logged and should be redacted from
  broad read-only summaries.

### `StashOrigin`

Marketplace origin:

```json
{
  "kind": "marketplace",
  "plugin_id": "demo@labby",
  "artifact_path": "skills/demo/SKILL.md",
  "source_version": "1.0.0",
  "source_fingerprint": "tree-fingerprint-abc123"
}
```

Whole-plugin fork:

```json
{
  "kind": "marketplace",
  "plugin_id": "demo@labby",
  "source_version": "1.0.0",
  "source_fingerprint": "tree-fingerprint-abc123"
}
```

Local path origin:

```json
{
  "kind": "local_path",
  "source_path": "/home/user/workspace/demo/SKILL.md"
}
```

## Action Contract

All actions use the existing action dispatch envelope:

```json
{
  "action": "<action-name>",
  "params": {}
}
```

### Stash `component.adopt`

Creates a Stash component from a validated source path, attaches origin metadata,
and saves the initial revision. Direct HTTP use is privileged and requires
`lab:admin`. Marketplace must not pass caller-supplied absolute paths through
this action; the bridge resolves paths from `plugin_id` and relative
`artifact_path` inside a known Marketplace source root.

Request:

```json
{
  "action": "component.adopt",
  "params": {
    "kind": "skill",
    "name": "demo-skill",
    "label": "Demo Skill",
    "source_path": "/home/user/.lab/plugins/marketplaces/labby/demo/skills/demo",
    "origin": {
      "kind": "marketplace",
      "plugin_id": "demo@labby",
      "artifact_path": "skills/demo",
      "source_version": "1.0.0",
      "source_fingerprint": "tree-fingerprint-abc123"
    },
    "save_label": "Fork from demo@labby"
  }
}
```

Response:

```json
{
  "component": {
    "id": "01aryz6s41tpz5x11k39dv3r2g",
    "kind": "skill",
    "name": "demo-skill",
    "label": "Demo Skill",
    "head_revision_id": "01b7x6s41tpz5x11k39dv3r2g",
    "origin": "marketplace://demo@labby?artifact=skills/demo",
    "origin_meta": {
      "kind": "marketplace",
      "plugin_id": "demo@labby",
      "artifact_path": "skills/demo",
      "source_version": "1.0.0",
      "source_fingerprint": "tree-fingerprint-abc123"
    },
    "workspace_root": "/home/user/.lab/stash/workspaces/01aryz6s41tpz5x11k39dv3r2g",
    "workspace_shape": "directory",
    "unix_mode": null,
    "created_at": "2026-06-13T12:00:00Z",
    "updated_at": "2026-06-13T12:00:00Z"
  },
  "revision": {
    "id": "01b7x6s41tpz5x11k39dv3r2g",
    "component_id": "01aryz6s41tpz5x11k39dv3r2g",
    "label": "Fork from demo@labby",
    "content_digest": "sha256hex",
    "created_at": "2026-06-13T12:00:00Z",
    "file_count": 1,
    "unix_mode": null
  }
}
```

Errors:

| Kind | Meaning |
|------|---------|
| `missing_param` | Required field absent |
| `invalid_param` | Invalid kind, origin, or path |
| `not_found` | Source path does not exist |
| `symlink_rejected` | Source or nested source is a symlink |
| `path_traversal` | Source path is unsafe |
| `forbidden` | HTTP caller lacks `lab:admin` for direct Stash adoption |
| `file_too_large` | Single file exceeds Stash limits |
| `workspace_too_large` | Directory import exceeds Stash limits |
| `internal_error` | Store, lock, or filesystem failure |

### Marketplace `artifact.fork`

Forks one or more Marketplace artifacts into Stash. V1 returns a wrapper object
so warnings, duplicate-skips, and partial failures can be represented without
changing the wire shape.

Single artifact request:

```json
{
  "action": "artifact.fork",
  "params": {
    "plugin_id": "demo@labby",
    "artifacts": ["skills/demo/SKILL.md"]
  }
}
```

Whole plugin request:

```json
{
  "action": "artifact.fork",
  "params": {
    "plugin_id": "demo@labby"
  }
}
```

Response:

```json
{
  "forks": [
    {
      "plugin_id": "demo@labby",
      "component_id": "01aryz6s41tpz5x11k39dv3r2g",
      "revision_id": "01b7x6s41tpz5x11k39dv3r2g",
      "stash_workspace": "/home/user/.lab/stash/workspaces/01aryz6s41tpz5x11k39dv3r2g",
      "forked_artifacts": ["skills/demo/SKILL.md"]
    }
  ],
  "warnings": []
}
```

Contract rules:

- `artifact.fork` is a write action because it creates Stash component records,
  workspaces, sidecar metadata, and revisions. It must be protected by the same
  admin/write scope policy as other state-changing artifact actions.
- Each selected artifact creates one Stash component.
- A whole-plugin fork creates exactly one directory-shaped
  `StashComponentKind::Plugin` component with `artifact_path = null`.
- Existing matching forks are returned idempotently instead of duplicated unless
  a future `force` parameter is added.

Errors:

| Kind | Meaning |
|------|---------|
| `missing_param` | `plugin_id` absent |
| `invalid_param` | Plugin id or artifact path invalid |
| `not_found` | Plugin source or artifact path missing |
| `symlink_rejected` | Source contains symlink |
| `forbidden` | HTTP caller lacks admin scope for write actions |
| `internal_error` | Stash or Marketplace filesystem failure |

### Marketplace `artifact.list`

Lists Stash components with Marketplace origin metadata.

Request:

```json
{
  "action": "artifact.list",
  "params": {
    "plugin_id": "demo@labby"
  }
}
```

Response:

```json
[
  {
    "plugin_id": "demo@labby",
    "component_id": "01aryz6s41tpz5x11k39dv3r2g",
    "stash_workspace": "/home/user/.lab/stash/workspaces/01aryz6s41tpz5x11k39dv3r2g",
    "forked_artifacts": ["skills/demo/SKILL.md"],
    "status": "unknown"
  }
]
```

### Marketplace `artifact.unfork`

Removes fork tracking by deleting the matching Stash component.

Request:

```json
{
  "action": "artifact.unfork",
  "params": {
    "plugin_id": "demo@labby",
    "artifacts": ["skills/demo/SKILL.md"],
    "confirm": true
  }
}
```

Response:

```json
{
  "plugin_id": "demo@labby",
  "removed_component_ids": ["01aryz6s41tpz5x11k39dv3r2g"]
}
```

Contract rules:

- Confirmation is enforced by the shared surface gate. HTTP/MCP requests include
  `confirm: true`; API dispatch strips it before marketplace params parsing.
- If `artifacts` is omitted, all Stash components for `plugin_id` are removed.
- Removing a component must remove its workspace and provider/revision indexes
  according to Stash store deletion semantics.

### Marketplace `artifact.reset`

Restores forked artifact workspace content from the base snapshot.

Request:

```json
{
  "action": "artifact.reset",
  "params": {
    "plugin_id": "demo@labby",
    "artifacts": ["skills/demo/SKILL.md"],
    "confirm": true
  }
}
```

Response:

```json
{
  "plugin_id": "demo@labby",
  "reset_artifacts": ["skills/demo/SKILL.md"]
}
```

Contract rules:

- Confirmation is enforced by the shared surface gate. HTTP/MCP requests include
  `confirm: true`; API dispatch strips it before marketplace params parsing.
- Reset writes to the Stash workspace, not the Marketplace source tree.
- Reset creates a new Stash revision labeled `Reset to marketplace base` unless
  the implementation returns `status = "workspace_dirty"` to require explicit
  caller save. V1 should prefer creating the revision so deploy/export use the
  reset content immediately.

### Marketplace `artifact.update.check`

Checks upstream versions for marketplace-origin Stash components.

Request:

```json
{
  "action": "artifact.update.check",
  "params": {
    "plugin_id": "demo@labby"
  }
}
```

Response:

```json
[
  {
    "plugin_id": "demo@labby",
    "update_available": true,
    "has_update": true,
    "current_version": "1.0.0",
    "available_version": "1.1.0",
    "new_version": "1.1.0"
  }
]
```

### Marketplace `artifact.update.preview`

Builds a three-way preview using:

- base snapshot from fork time
- user's Stash workspace content
- current Marketplace upstream content

Request:

```json
{
  "action": "artifact.update.preview",
  "params": {
    "plugin_id": "demo@labby"
  }
}
```

Response:

```json
{
  "plugin_id": "demo@labby",
  "has_update": true,
  "current_version": "1.0.0",
  "upstream_version": "1.1.0",
  "new_version": "1.1.0",
  "upstream_fingerprint": "tree-fingerprint-def456",
  "local_fingerprint": "base-and-workspace-fingerprint-abc123",
  "unchanged": [],
  "upstream_only": [],
  "user_only": [],
  "clean_merges": [
    {
      "path": "skills/demo/SKILL.md",
      "merged_content": "# merged\n",
      "yours_diff": "--- old\n+++ new\n",
      "theirs_diff": "--- old\n+++ new\n"
    }
  ],
  "conflicts": []
}
```

Preview size rules:

- `artifact.update.preview` must cap total preview files, per-file bytes, and
  total diff bytes.
- Truncated entries include `truncated: true`, `original_size`, and a `preview`
  string.
- Binary or non-UTF-8 files are outside the text-merge contract and are not
  embedded in previews; callers should treat omitted artifact paths as
  non-previewable content and reset/apply them through explicit artifact
  operations instead of expecting a merge-conflict payload.
- `local_fingerprint` binds the preview to the base snapshot plus user's Stash
  workspace content so apply can reject local edits made after preview.

### Marketplace `artifact.update.apply`

Applies a pending preview into the Stash workspace.

Request:

```json
{
  "action": "artifact.update.apply",
  "params": {
    "plugin_id": "demo@labby",
    "strategy": "keep_mine",
    "confirm": true
  }
}
```

Response:

```json
{
  "plugin_id": "demo@labby",
  "new_version": "1.1.0",
  "applied_clean": ["skills/demo/SKILL.md"],
  "applied_strategy": [],
  "needs_resolution": [],
  "status": "complete"
}
```

Contract rules:

- Confirmation is enforced by the shared surface gate. HTTP/MCP requests include
  `confirm: true`; API dispatch strips it before marketplace params parsing.
- Apply writes to the Stash workspace.
- Apply saves a new Stash revision after successful writes.
- Apply updates Marketplace origin metadata with the new upstream version and
  source fingerprint.
- If the pending preview is stale because upstream or the local fork changed,
  return `stale_preview`.
- If conflicts exist and strategy is `always_ask`, return
  `status = "partial_conflicts"` and do not write conflicted files.

## Path Contract

### Plugin Id

Plugin ids must be in:

```text
name@marketplace
```

Both parts must be non-empty. `/`, `\`, `:`, and path traversal components are
invalid.

### Artifact Path

Artifact paths must:

- be relative
- use forward slashes
- contain only normal path components
- not contain empty segments
- not contain `.`
- not contain `..`
- not contain null bytes
- not contain backslashes

Valid:

```text
skills/demo/SKILL.md
agents/reviewer.md
settings.json
```

Invalid:

```text
../secrets
/etc/passwd
skills/../settings.json
C:\Users\demo
```

### Absolute Paths

Absolute paths are internal operator data. They may appear in direct admin
responses that need workspace diagnostics, but they must not be logged and
should not be required by Gateway-admin to perform Marketplace fork/update
workflows.

## Storage Contract

### Stash Root

Stash root resolution remains:

```text
[workspace].root/stash
```

Fallback:

```text
~/.lab/stash
```

### Marketplace Fork Helper Files

Marketplace writes these helper files under a Stash-owned sidecar directory
outside the tracked Stash workspace:

```text
stash/marketplace/<component_id>/
├── base/
├── pending-update.json
└── drift-cache.json
```

Rules:

- Helper files are implementation metadata.
- Helper files are not Marketplace source files.
- Helper files must never be included in Stash revisions, provider sync, export,
  deploy, or Marketplace plugin deploy payloads.
- If future migrations place helper files under workspaces, revision/export/deploy
  exclusion tests are required in the same change.

## API Contract

HTTP routes remain existing service endpoints:

```text
POST /v1/marketplace
POST /v1/stash
POST /v1/marketplace/artifact/fork
POST /v1/marketplace/artifact/list
POST /v1/marketplace/artifact/unfork
POST /v1/marketplace/artifact/reset
POST /v1/marketplace/artifact/update/check
POST /v1/marketplace/artifact/update/preview
POST /v1/marketplace/artifact/update/apply
```

No new top-level HTTP route is required. Existing artifact convenience routes
remain compatibility routes and must preserve auth metadata and confirmation
behavior.

Gateway-admin client helpers must call `/v1/marketplace` for Marketplace
artifact workflows and `/v1/stash` only for generic Stash workflows.

## MCP Contract

No new MCP tool is added.

Existing tools:

```text
marketplace({ "action": "artifact.fork", "params": { ... } })
stash({ "action": "component.adopt", "params": { ... } })
```

The action catalogs are the source of truth for destructive metadata.

## Error Envelope Contract

Errors use the existing `ToolError` envelope shape with stable `kind`.

New or reused kinds:

| Kind | Surface | Meaning |
|------|---------|---------|
| `missing_param` | both | Required input missing |
| `invalid_param` | both | Invalid plugin id, artifact path, origin, kind, or request shape |
| `not_found` | both | Plugin source, artifact, component, or base snapshot missing |
| `symlink_rejected` | stash | Source or nested entry is a symlink |
| `path_traversal` | both | Unsafe path rejected |
| `file_too_large` | stash | Single file exceeds limit |
| `workspace_too_large` | stash | Import exceeds workspace limits |
| `marketplace_auth_required` | marketplace | Git fetch requires credentials |
| `stale_preview` | marketplace | Upstream or local fork changed after preview |
| `preview_truncated` | marketplace | Preview exceeded configured size limits |
| `forbidden` | both | Caller lacks required scope |
| `content_contains_secrets` | marketplace | Merge suggestion input appears secret-bearing |
| `ai_backend_not_configured` | marketplace | AI merge requested without backend |
| `internal_error` | both | Unexpected filesystem, lock, parse, or runtime error |

## Frontend Contract

Gateway-admin helpers:

```ts
forkMarketplaceArtifact(input: {
  pluginId: string
  artifacts?: string[]
}, signal?: AbortSignal): Promise<unknown>

listMarketplaceForks(
  pluginId?: string,
  signal?: AbortSignal,
): Promise<MarketplaceForkStatus[]>

resetMarketplaceArtifact(input: {
  pluginId: string
  artifacts?: string[]
}, signal?: AbortSignal): Promise<unknown>

unforkMarketplaceArtifact(input: {
  pluginId: string
  artifacts?: string[]
}, signal?: AbortSignal): Promise<unknown>
```

Write/destructive HTTP helpers, including `forkMarketplaceArtifact`, must include
`confirm: true`; marketplace dispatch parsers must not require it after the
shared API helper strips confirmed params.

## Verification Contract

Minimum required checks:

```bash
cargo test -p lab-apis marketplace_origin_round_trips component_origin_meta_is_optional_for_existing_records
cargo test -p labby --all-features dispatch_adopt_imports_and_saves_marketplace_component
cargo test -p labby --all-features stash_bridge
cargo test -p labby --all-features artifact_update
cargo nextest run --workspace --all-features
pnpm --dir apps/gateway-admin exec vitest run lib/api/marketplace-artifacts.test.ts components/marketplace/plugin-files-panel.test.tsx
pnpm --dir apps/gateway-admin exec tsc --noEmit
just docs-generate
```

Generated docs must include these service/action pairs:

- service `marketplace`, action `artifact.fork`
- service `marketplace`, action `artifact.list`
- service `marketplace`, action `artifact.unfork`
- service `marketplace`, action `artifact.reset`
- service `stash`, action `component.adopt`
