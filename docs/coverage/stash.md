# Stash Coverage

**Last updated:** 2026-04-26
**Source:** Internal — stash is a product-local service with no upstream HTTP API
**SDK surface:** N/A — stash is implemented entirely in `crates/lab/src/dispatch/stash/`
**Shared dispatch:** `crates/lab/src/dispatch/stash/` (catalog.rs, client.rs, params.rs, dispatch.rs)
**MCP registration:** `crates/lab/src/registry.rs` (registered behind the `stash` cargo feature, member of `all`)
**CLI surface:** `crates/lab/src/cli/stash.rs`
**API handler:** `crates/lab/src/api/services/stash.rs`

## Legend

| Symbol | Meaning |
|--------|---------|
| ✅ | Implemented |
| ⬜ | Not implemented |
| — | Not applicable |

## Config

No env vars required. The stash root is configured via `config.toml`:

```toml
[workspace]
root = "~/.labby/stash"   # default if not set
```

## SDK Surface

Stash is a product-local service — its types live in `crates/lab-apis/src/stash/types.rs`
but there is no `StashClient` or network transport. All operations go through the
`dispatch/stash/` layer directly.

## Action Catalog

All 16 actions are dispatched via the shared dispatch layer in `crates/lab/src/dispatch/stash/`.

| Action | Implemented | Destructive | Dispatch Path | Error Kinds |
|--------|-------------|-------------|---------------|-------------|
| `help` | ✅ | No | `dispatch::dispatch` (built-in) | — |
| `schema` | ✅ | No | `dispatch::dispatch` (built-in) | `unknown_action` |
| `components.list` | ✅ | No | `service::components_list` | `internal_error` |
| `component.get` | ✅ | No | `service::component_get` | `not_found` |
| `component.create` | ✅ | No | `service::component_create` | `invalid_param`, `internal_error` |
| `component.import` | ✅ | Yes | `service::component_import` → `import::import_component` | `not_found`, `invalid_param`, `symlink_rejected`, `workspace_too_large`, `file_too_large`, `ambiguous_kind`, `internal_error` |
| `component.workspace` | ✅ | No | `service::component_workspace` | `not_found` |
| `component.save` | ✅ | No | `service::component_save` → `revision::save_revision` | `not_found`, `symlink_rejected`, `internal_error` |
| `component.revisions` | ✅ | No | `service::component_revisions` → `revision::list_revisions` | `internal_error` |
| `component.export` | ✅ | Yes | `service::component_export` → `export::export_component` | `not_found`, `export_target_not_empty`, `symlink_rejected`, `path_traversal`, `internal_error` |
| `component.deploy` | ✅ | Yes | `service::component_deploy` | `not_found`, `unknown_target`, `deploy_failed`, `unsupported_provider`, `internal_error` |
| `providers.list` | ✅ | No | `service::providers_list` | `internal_error` |
| `provider.link` | ✅ | No | `service::provider_link` | `not_found`, `unsupported_provider`, `invalid_param`, `internal_error` |
| `provider.push` | ✅ | Yes | `service::provider_push` | `not_found`, `sync_failed` |
| `provider.pull` | ✅ | Yes | `service::provider_pull` | `not_found`, `sync_failed` |
| `targets.list` | ✅ | No | `service::targets_list` | `internal_error` |
| `target.add` | ✅ | No | `service::target_add` | `invalid_param`, `internal_error` |
| `target.remove` | ✅ | Yes | `service::target_remove` | `not_found`, `internal_error` |

## Provider Drivers

| Driver | Implemented | Config Keys |
|--------|-------------|-------------|
| `filesystem` | ✅ | `root` (string, absolute path) |
| `google_drive` | ⬜ | Deferred to v2 |
| `s3` | ⬜ | Deferred to v2 |

## Store Layout

```
<stash_root>/
├── components/    — StashComponent JSON records
├── revisions/     — Immutable revision snapshots (meta.json + files/)
├── workspaces/    — Live working copies
├── providers/     — StashProviderRecord JSON records
└── targets/       — StashDeployTarget JSON records
```

## Marketplace-Origin Components

Marketplace artifact forks are stored as normal stash components with
`origin_meta.kind = "marketplace"`. Marketplace owns source discovery, upstream
version checks, and merge/diff presentation. Stash owns the copied workspace,
saved revisions, provider sync, export, and deploy handoff.

Primary entry points:

| Surface | Action | Purpose |
|---------|--------|---------|
| marketplace | `artifact.fork` | Copy one plugin artifact or a whole plugin into stash |
| marketplace | `artifact.list` | List stash components whose origin is marketplace |
| marketplace | `artifact.update.*` | Compare stash edits against marketplace upstream |
| stash | `component.adopt` | Generic create/import/save action used by marketplace |

## Surfaces

| Surface | Status | Notes |
|---------|--------|-------|
| MCP tool | ✅ | `stash({ "action": "...", "params": {...} })` |
| CLI subcommand | ✅ | `lab stash <action> [key=value ...]` |
| HTTP API | ✅ | `POST /v1/stash` |
| TUI plugin entry | — | TUI surface is currently deferred |

## Observability

- Dispatch events: ✅ `tracing::info/warn/error` at the `dispatch()` boundary
- Standard fields: `surface`, `service`, `action`, `elapsed_ms`, `kind` (errors)
- `surface` is hardcoded to `"mcp"` at the shared dispatch layer (known gap)

## Security

- Symlink rejection: ✅ enforced in `revision::collect_files` and `import`
- Path traversal guard: ✅ absolute-path requirements enforced in param parsers
- System path rejection: ✅ enforced in `service::component_deploy`
- `lab:admin` scope: ✅ required for all write operations over HTTP
