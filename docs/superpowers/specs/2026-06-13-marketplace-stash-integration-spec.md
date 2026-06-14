# Marketplace Stash Integration Spec

Date: 2026-06-13
Status: Draft for implementation

## Summary

Marketplace artifact forks should become first-class Stash components.

Marketplace remains the source discovery and upstream workflow surface. Stash
becomes the durable library for forked content: component identity, editable
workspace, immutable revisions, provider sync, export, and deploy handoff.

This removes the current split-brain model where `marketplace` has its own
`.stash.json` and `.base/` conventions while the real `stash` service has no
knowledge of marketplace-origin artifacts.

## Problem

Plugin marketplace content often needs local customization. Users need a way to
change one skill, agent, command, hook, or config file from a plugin without
losing their work on the next plugin update.

Today the codebase has three separate lanes:

- `marketplace.plugin.workspace`, `plugin.save`, and `plugin.deploy` edit and
  deploy a plugin-oriented workspace under `[workspace].root/plugins`.
- `marketplace.artifact.*` actions are partially designed around fork/update
  metadata, but key lifecycle actions are stubs.
- `stash` owns proper component workspaces, revisions, providers, export, and
  deploy, but marketplace-origin content is not wired into it.

The result is conceptually close but not properly connected.

## Goals

- Make marketplace artifact forks durable Stash components.
- Preserve Marketplace as the user-facing upstream/plugin workflow.
- Preserve Stash as the canonical authored artifact library.
- Support forking either a whole plugin or selected artifact paths.
- V1 represents each selected artifact as its own Stash component.
- V1 represents a whole-plugin fork as one directory-shaped
  `StashComponentKind::Plugin` component with `artifact_path = null`.
- Record structured origin metadata linking a stash component to:
  - plugin id
  - exactly one optional artifact path
  - upstream version
  - upstream source fingerprint
- Save an initial immutable Stash revision at fork/adopt time.
- Let Marketplace update/merge actions compare Stash workspace edits against
  upstream Marketplace content.
- Let existing Stash provider/export/deploy flows work for marketplace-origin
  components.
- Keep CLI, MCP, HTTP, and gateway-admin behavior aligned through the existing
  action dispatch surfaces.

## Non-Goals

- Publishing stash components back to public marketplaces.
- Replacing plugin install/uninstall.
- Making Stash discover marketplace sources.
- Background update polling.
- Multi-user collaboration or ACLs.
- A general git replacement for plugin repositories.
- Supporting live references into Marketplace source trees. V1 materializes
  copies into Stash-owned workspaces.

## Ownership Model

### Marketplace Owns

- Marketplace source discovery.
- Runtime-specific marketplace backends.
- Plugin id parsing and source resolution.
- Plugin install state.
- Plugin component inspection.
- Upstream update lookup.
- Diff, merge preview, merge strategy, and conflict UX.
- Fleet/device cherry-pick workflows.

### Stash Owns

- Component ids and component records.
- Stash-owned editable workspaces.
- Immutable revision snapshots.
- Provider linkage and provider sync.
- Export and deploy handoff.
- Stash target records.
- Cross-machine artifact portability.

### Bridge Owns

A small Marketplace-owned bridge maps Marketplace plugin artifacts into Stash
components. The bridge is allowed to depend on both Marketplace and Stash
dispatch internals. No other Marketplace code should directly manipulate Stash
store layout.

The intended module is:

```text
crates/lab/src/dispatch/marketplace/stash_bridge.rs
```

## Product Flows

### Fork A Single Artifact

1. User opens a Marketplace plugin detail page.
2. User selects `skills/foo/SKILL.md` and chooses `Fork to Stash`.
3. Gateway-admin calls:

```json
{
  "action": "artifact.fork",
  "params": {
    "plugin_id": "plugin@marketplace",
    "artifacts": ["skills/foo/SKILL.md"]
  }
}
```

4. Marketplace resolves the local plugin source.
5. Bridge creates a Stash component with marketplace origin metadata.
6. Stash imports the selected artifact into a Stash-owned workspace.
7. Stash saves the initial revision.
8. Marketplace returns the Stash component id, revision id, and workspace path.

### Fork A Whole Plugin

1. User chooses `Fork plugin to Stash`.
2. Gateway-admin calls `artifact.fork` without `artifacts`.
3. Bridge imports the plugin source directory into one
   `StashComponentKind::Plugin` component.
4. The component origin has `artifact_path = null`.

### Edit A Fork

The edit surface may continue to be Marketplace plugin files initially, but the
durable edit target for forked artifacts is the Stash workspace. Once a fork
exists, Marketplace should prefer the Stash workspace for artifact update and
merge flows.

### Preview Upstream Update

1. User runs `artifact.update.preview`.
2. Marketplace loads the Stash component whose origin matches the plugin id.
3. Marketplace resolves upstream plugin source.
4. Marketplace compares:
   - base snapshot from the fork moment
   - user's Stash workspace content
   - current upstream plugin content
5. Marketplace returns clean merges and conflicts.

### Apply Upstream Update

1. User runs `artifact.update.apply`; HTTP/MCP/CLI confirmation is enforced by
   the shared `ActionSpec.destructive` gates before dispatch.
2. Marketplace validates the pending preview is fresh.
3. Marketplace applies clean merges and selected strategy results to the Stash
   workspace.
4. Stash saves a new revision after successful apply.
5. Marketplace updates origin metadata to the new upstream version/fingerprint.

### Deploy Or Sync Forked Content

After a marketplace artifact is a Stash component, deploy and sync use Stash:

```json
{ "action": "component.deploy", "params": { "id": "<component_id>", "target_id": "local" } }
```

```json
{ "action": "provider.push", "params": { "id": "<component_id>", "provider_id": "<provider_id>" } }
```

## Data Model

### Stash Origin

Add structured origin metadata while keeping the existing `origin` string for
compatibility.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StashOrigin {
    Marketplace(MarketplaceOrigin),
    LocalPath { source_path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarketplaceOrigin {
    pub plugin_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
}
```

`StashComponent` adds:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub origin_meta: Option<StashOrigin>,
```

### Marketplace Fork State

The source of truth is the Stash component record. Marketplace keeps merge/update
helper files in a Stash-owned sidecar directory outside the tracked component
workspace:

```text
stash/marketplace/<component_id>/
├── base/
│   └── <artifact base snapshots>
├── pending-update.json
└── drift-cache.json
```

These helper files are Marketplace-owned metadata. They must not be counted as
user artifact content in Stash revisions unless explicitly intended by a future
schema migration.

## Actions

### Stash

Add:

```text
component.adopt
```

Purpose:

- Create a Stash component.
- Import a validated source path into its workspace. Marketplace bridge callers
  must resolve that path from a known Marketplace source root; generic HTTP/MCP
  callers must have `lab:admin` and still pass Stash read-path safety checks.
- Attach structured origin metadata.
- Save the initial revision.

### Marketplace

Make these live:

```text
artifact.fork
artifact.list
artifact.unfork
artifact.reset
artifact.update.check
artifact.update.preview
artifact.update.apply
artifact.merge.suggest
artifact.config.set
```

`artifact.update.*` should operate on Stash components whose origin metadata is
`kind = "marketplace"`.

## Security Requirements

- Marketplace artifact paths must be relative, normal path segments only.
- Absolute paths, `..`, `.`, null bytes, and backslashes are rejected.
- Marketplace source paths must remain under known Marketplace roots.
- Marketplace bridge adoption must not accept caller-supplied absolute paths.
  It resolves sources from plugin id + artifact path inside a validated
  Marketplace root.
- Direct Stash adoption/import is privileged and requires `lab:admin` on HTTP.
- Stash import must reject symlinks and sensitive system paths.
- Base snapshots must reject symlinks.
- Destructive action confirmation is owned by the shared surface gates driven by
  `ActionSpec.destructive`. HTTP/MCP callers pass `confirm: true`; CLI callers
  use `-y`/interactive confirmation. Dispatch params parsers must not re-require
  `confirm` after the API helper strips it.
- Destructive or write actions include:
  - `component.adopt`
  - `artifact.fork`
  - `artifact.unfork`
  - `artifact.reset`
  - `artifact.update.apply`
  - Stash export/deploy/provider push and pull actions as already cataloged
- File contents used for AI merge suggestions are untrusted data and must never
  be treated as instructions.
- Secret-looking merge regions must continue to fail before AI merge requests.
- Absolute local paths are operator diagnostics. They must not be logged and
  should be omitted or redacted from broad read-only UI summaries.

## Observability Requirements

Marketplace dispatch events use:

```text
surface=<surface>
service=marketplace
action=<artifact action>
elapsed_ms=<duration>
kind=<error kind on failure>
```

Stash dispatch events use this shape when `component.adopt` is called directly:

```text
surface=<surface>
service=stash
action=component.adopt
elapsed_ms=<duration>
kind=<error kind on failure>
```

Marketplace bridge operations that call Stash helpers directly must emit
equivalent structured fields because they do not automatically pass through
`stash::dispatch_for_surface`:

- `plugin_id`
- `artifact_path`
- `component_id`
- `revision_id`

Do not log file contents, auth headers, tokens, cookies, or secret env values.

## Compatibility

- Existing Stash component JSON without `origin_meta` remains valid.
- Existing `origin: Option<String>` remains present.
- Existing Marketplace read actions remain backward-compatible.
- `artifact.fork` now returns real data instead of `not_implemented`.
- Existing generated action docs should continue to expose one `marketplace`
  MCP tool and one `stash` MCP tool.

## Open Decisions

1. Whether Marketplace plugin file editing should automatically fork before
   saving. V1 makes the fork action explicit.

## Acceptance Criteria

- `marketplace artifact.fork` creates a Stash component with marketplace origin
  metadata and an initial revision.
- `marketplace artifact.list` lists marketplace-origin Stash components.
- `marketplace artifact.update.preview` reads Stash workspace content for the
  user's side of the merge.
- `marketplace artifact.update.apply` updates the Stash workspace and saves a
  new Stash revision.
- `stash components.list` shows marketplace-origin components.
- `stash component.revisions` shows the fork revision and any later update
  revisions.
- Gateway-admin can fork the selected Marketplace file to Stash.
- Destructive Marketplace actions continue to be gated by
  `ActionSpec.destructive` through HTTP, MCP, and CLI confirmation helpers.
- `component.adopt` is included in the Stash HTTP admin write-action gate.
- Marketplace helper files are not included in Stash revisions, export, deploy,
  or provider sync.
- Update previews enforce file count and byte limits and return explicit
  truncation metadata when the response is capped.
- `cargo nextest run --workspace --all-features` passes.
- Focused gateway-admin tests for the new API helpers pass.
