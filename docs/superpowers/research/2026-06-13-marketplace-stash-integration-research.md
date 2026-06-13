# Marketplace Stash Integration Research

Date: 2026-06-13

Research corpus:

- `docs/superpowers/plans/2026-06-13-marketplace-stash-integration.md`
- `docs/superpowers/specs/2026-06-13-marketplace-stash-integration-spec.md`
- `docs/contracts/marketplace-stash-integration.md`

This file is a research artifact for the marketplace-to-stash wiring plan. It
does not revise the plan, spec, or contract. Because the Lavra research skill is
epic-bead oriented and this work is document-based, findings are logged here
instead of as bead comments.

## Domain Profile

Languages and runtimes:

- Rust 2024, Tokio, Axum, rmcp, serde JSON.
- TypeScript, React, Next.js gateway-admin UI.

Primary code areas:

- `crates/lab-apis/src/stash/types.rs`
- `crates/lab/src/dispatch/stash/*`
- `crates/lab/src/dispatch/marketplace/*`
- `crates/lab/src/api/services/stash.rs`
- `crates/lab/src/api/services/marketplace.rs`
- `apps/gateway-admin/lib/api/marketplace-client.ts`
- `apps/gateway-admin/components/marketplace/plugin-files-panel.tsx`

Concerns:

- Cross-service ownership between marketplace and stash.
- Durable metadata and revision integrity.
- Filesystem path safety and local secret exposure.
- Destructive action confirmation and HTTP scope gates.
- Git fetch hardening and update preview staleness.
- Preview payload size and whole-plugin filesystem costs.
- Frontend helper and UI compatibility with current marketplace APIs.

## Agent Roster

- `architecture-strategist`: service boundaries, feature gates, dispatch ownership.
- `security-sentinel`: path safety, admin scopes, git hardening, metadata leakage.
- `data-integrity-guardian`: stash revisions, metadata atomicity, update/apply consistency.
- `performance-oracle`: file walks, preview payload size, fetch batching.
- `pattern-recognition-specialist`: existing catalog, confirmation, route, and docs conventions.
- `repo-research-analyst`: live code path verification and stale plan references.

Local evidence gathering also used Lumen semantic code search first, followed by
exact `rg`/`nl` lookups for known symbols.

## Executive Findings

The broad architecture is right: Marketplace should own source discovery and
upstream update UX, while Stash owns durable components, revisions, providers,
export, and deploy. The one-way dependency direction, `marketplace ->
stash_bridge -> stash`, is compatible with Marketplace being feature-gated and
Stash being always-on, as long as shared origin types stay in `lab-apis` and
Stash never imports Marketplace.

The current draft needs a design revision before implementation. The largest
issues are helper-file placement inside Stash workspaces, overly broad
`component.adopt` source paths, confirmation handling that conflicts with the
shared gates, and missing explicit revision saves on update apply/reset paths.

## High Priority Findings

### Helper Files Inside Stash Workspaces

Severity: high

The spec says `.base/`, `.pending-update.json`, and `.drift-cache.json` are
Marketplace helper state and must not count as user artifact content. Current
Stash revision saving walks every file under `store.workspace_dir(component_id)`
and snapshots all of it:

- `crates/lab/src/dispatch/stash/revision.rs:21`
- `crates/lab/src/dispatch/stash/revision.rs:175`
- `crates/lab/src/dispatch/stash/revision.rs:193`

If helper files live inside `stash/workspaces/<component_id>/`, they will be
captured in revisions and can later leak through export/deploy/provider sync.

Research recommendation:

- Prefer storing Marketplace helper state outside the tracked workspace, for
  example under a sidecar metadata directory owned by `StashStore`.
- If V1 keeps helper files under workspaces, the plan must add an explicit
  revision/export/deploy exclusion mechanism and tests for it.
- The spec open decision around `.base/` placement should be resolved before
  implementation.

### `component.adopt` Source Path Safety

Severity: high

The plan/contract currently make `component.adopt` accept an absolute
`source_path`. Current Stash import rejects symlinks and broad sensitive roots,
but it is still intentionally a local path import:

- `crates/lab/src/dispatch/stash/import.rs:270`
- `crates/lab/src/dispatch/path_safety.rs:34`

If exposed as a general MCP/HTTP Stash action, `component.adopt` becomes a
high-powered local-file import primitive for any authorized caller.

Research recommendation:

- Make Marketplace bridge adoption resolve sources from validated marketplace
  roots, not caller-provided absolute paths.
- Either keep direct `stash component.adopt` admin-only and clearly privileged,
  or make the public action take a marketplace-origin reference instead of an
  arbitrary path.
- Reuse the stricter marketplace source containment behavior from
  `marketplace/update.rs::source_paths_for_plugin`, not the looser installed
  plugin lookup path.

### HTTP Admin Scope Gates

Severity: high

Adding a destructive Stash action to the catalog is not enough for HTTP. Stash
has a separate hard-coded write allowlist:

- `crates/lab/src/api/services/stash.rs:20`

If `component.adopt` is not added there, API scope behavior will drift from the
action semantics. Security review also found the marketplace artifact shortcut
routes currently call `handle_marketplace_action(..., None, ...)`, dropping
`AuthContext` before shared handling:

- `crates/lab/src/api/services/marketplace.rs:173`

Research recommendation:

- Add `component.adopt` to `STASH_WRITE_ACTIONS`.
- Review marketplace artifact convenience routes so auth metadata/scope context
  is not discarded.
- Include explicit HTTP auth tests for `component.adopt`, `artifact.fork`,
  `artifact.unfork`, `artifact.reset`, and `artifact.update.apply`.

### Confirmation Must Stay At Surface Gates

Severity: high

The plan’s Task 6 tells implementers to require `confirm: true` inside
marketplace parsers. That conflicts with current architecture. API confirms
destructive actions in `handle_action_with_meta`, then strips `confirm` before
dispatch:

- `crates/lab/src/api/services/helpers.rs:166`
- `crates/lab/src/api/services/helpers.rs:190`

CLI confirmation is driven by `ActionSpec.destructive` and `-y`, without adding
`params.confirm`:

- `crates/lab/src/cli/helpers.rs:123`

MCP confirmation is also catalog-driven:

- `crates/lab/src/mcp/call_tool.rs:350`

Research recommendation:

- Remove parser-level `confirm` requirements from the implementation plan.
- Keep `ActionSpec.destructive` as the single source of truth.
- Clarify in the contract that `confirm` is a surface/request gate, not a
  domain parser field after dispatch.

### `artifact.update.apply` Must Save Real Stash Revisions

Severity: high

The spec and contract require `artifact.update.apply` to update the Stash
workspace, save a new Stash revision, and update origin metadata. Current
Marketplace update code only writes pseudo-stash files and private `.stash.json`
metadata:

- `crates/lab/src/dispatch/marketplace/update.rs:376`
- `crates/lab/src/dispatch/marketplace/update.rs:490`
- `crates/lab/src/dispatch/marketplace/update.rs:608`

The plan says to rewire update logic, but does not make the new revision save a
specific acceptance step.

Research recommendation:

- Add an explicit task step: after a successful apply, call the Stash revision
  path or a new Stash helper that saves a revision and updates
  `head_revision_id`.
- Update `origin_meta` under a component lock after apply.
- Test that apply returns a revision id and that `component.revisions` shows it.

### Adopt Atomicity And Locking

Severity: high

The proposed `adopt_component_from_path` imports, saves a revision, then writes
an updated component record. Current `revision::save_revision` already updates
`head_revision_id` under the component lock, and `StashStore::write_component`
expects callers needing exclusive access to use `with_component_lock`.

Relevant code:

- `crates/lab/src/dispatch/stash/import.rs:384`
- `crates/lab/src/dispatch/stash/revision.rs:213`
- `crates/lab/src/dispatch/stash/store.rs:255`

Research recommendation:

- Avoid writing a stale cloned component after `save_revision`.
- Re-read the component after save, or add a single Stash helper that owns the
  whole adopt transaction and component locking.
- Define crash behavior between import and initial revision save.

### Single-File Artifact Path Semantics

Severity: high

The contract examples assume a forked single artifact can live at
`workspaces/<id>/<artifact_path>`, for example `skills/demo/SKILL.md`. Current
Stash file-shaped import stores only `workspaces/<id>/<basename>`:

- `crates/lab/src/dispatch/stash/import.rs:395`
- `crates/lab/src/dispatch/stash/import.rs:431`

Marketplace update/reset code assumes `stash.join(artifact_path)`, so a
single-file fork will not line up with existing Stash workspace semantics.

Research recommendation:

- Either represent artifact forks as directory-shaped components rooted at the
  artifact parent, or add an explicit origin/workspace path mapping.
- Do not assume `workspace_root` preserves the original marketplace-relative
  path for file-shaped components.

### Preview Payload Limits

Severity: high

`artifact.update.preview` can return full merged content and full diffs for many
files. Current update code reads and returns full text/diffs, while existing
marketplace artifact listing has response caps:

- `crates/lab/src/dispatch/marketplace/update.rs:341`
- `crates/lab/src/dispatch/marketplace/update.rs:819`
- `crates/lab/src/dispatch/marketplace/update.rs:926`
- `crates/lab/src/dispatch/marketplace/dispatch.rs:299`

Research recommendation:

- Add max file count, max bytes per file, max diff bytes, and truncation fields
  to the preview contract.
- Add tests for oversized previews and stable truncation markers.

## Medium Priority Findings

### `artifact.fork` Destructive Semantics

Severity: medium

The current catalog marks `artifact.fork` as non-destructive, and the draft spec
does not list it among destructive actions. Planned behavior creates Stash
components, workspaces, and revisions, which is state-changing.

Research recommendation:

- Decide whether `artifact.fork` should be `destructive: true`.
- If product semantics keep it non-destructive because it is additive, document
  that explicitly and ensure HTTP admin scope still protects the write.

### `artifact.fork` Return Shape

Severity: medium

The plan returns `Vec<ForkResult>` for multi-artifact forks, but the current
catalog returns string is `ForkResult`:

- `crates/lab/src/dispatch/marketplace/catalog.rs:161`

Research recommendation:

- Make the catalog and contract agree: either `ForkResult[]` or a wrapper such
  as `{ forks: ForkResult[] }`.
- Prefer a wrapper if the response may need warnings, skipped duplicates, or
  partial results later.

### Multi-Artifact Identity

Severity: medium

The contract lets `artifact.fork` accept multiple `artifacts`, while
`MarketplaceOrigin.artifact_path: Option<String>` can represent only one path.
The spec also says V1 is one component per fork request.

Research recommendation:

- Choose one V1 model:
  - One component per selected artifact.
  - One component for a whole plugin.
  - One component for an artifact set with `artifact_paths: Vec<String>`.
- Update origin metadata and list/reset/update semantics accordingly.

### Whole-Plugin Fork Kind

Severity: medium

`StashComponentKind` currently has specific artifact kinds but no `Plugin` kind:

- `crates/lab-apis/src/stash/types.rs:33`

Whole-plugin forks therefore need a clear representation.

Research recommendation:

- Either add a `Plugin` or `Bundle` kind, or define whole-plugin fork as a group
  of component forks rather than one Stash component.

### Git Fetch Hardening

Severity: medium

Current update code already hardens git fetch with marketplace-root
containment, timeout, disabled prompting, disabled global config, protocol
blocks, and hooks disabled:

- `crates/lab/src/dispatch/marketplace/update.rs:1017`
- `crates/lab/src/dispatch/marketplace/update.rs:1070`
- `crates/lab/src/dispatch/marketplace/update.rs:1099`

Research recommendation:

- Require reuse of the existing hardened fetch helper or an explicitly
  equivalent helper.
- Add this as a contract/security invariant so the bridge rewrite does not
  accidentally introduce raw `git fetch`.

### Base Snapshot Symlink Handling

Severity: medium

The plan’s base-copy helper silently skips symlinks. The stash import/revision
path rejects symlinks.

Research recommendation:

- Base snapshot creation should return `symlink_rejected`, not silently skip.
- Tests should cover symlinks in marketplace source trees.

### Source Fingerprint Naming And Cost

Severity: medium

The docs use `source_commit` for a value that current logic computes as a tree
fingerprint. Current `compute_tree_fingerprint` is O(files + bytes), and the
plan calls it inside per-artifact loops.

Research recommendation:

- Rename the field to `source_fingerprint`, or split `source_ref` and
  `source_fingerprint`.
- Compute the source fingerprint once per plugin/source tree per request.

### Full Component Scans

Severity: medium

The plan’s fork lookup paths scan all Stash components with
`store.list_components()` and filter by origin. Current max component count is
bounded, but this remains O(N) for list/update/reset lookups:

- `crates/lab-apis/src/stash/types.rs:125`
- `crates/lab/src/dispatch/stash/store.rs:270`

Research recommendation:

- Accept O(N) for V1 only if documented.
- Consider a marketplace-origin index if UI calls become frequent or
  component count approaches the configured limit.

### Drift Status Contract

Severity: medium

The current marketplace catalog says `artifact.list` includes drift status, but
the plan hardcodes `dirty: false`.

Research recommendation:

- Either remove drift claims from V1 or implement meaningful drift detection.
- Do not return a misleading `dirty: false` field if it has not been computed.

### Observability Boundary

Severity: medium

The spec wants Stash observability for `component.adopt`, but direct calls from
`marketplace/stash_bridge.rs` into Stash service/store helpers will bypass
`dispatch_for_surface` logging.

Research recommendation:

- Either call a dispatch entry point with explicit surface context, or move the
  required observability into the bridge/helper and document that ownership.

### Existing HTTP Compatibility Routes

Severity: medium

The contract lists only action-style endpoints, but Marketplace already has
compatibility routes like `/v1/marketplace/artifact/fork`:

- `crates/lab/src/api/services/marketplace.rs:25`

Research recommendation:

- Mention that existing artifact convenience routes remain supported.
- Ensure those routes preserve auth metadata and destructive confirmation.

### Sync Work In Async Handlers

Severity: medium

The bridge snippets call synchronous Stash store/filesystem helpers from async
Marketplace handlers. Current Stash dispatch uses `spawn_blocking` for sync
store work.

Research recommendation:

- Wrap bridge filesystem/store work in `spawn_blocking`, or expose async Stash
  helper entry points that own their blocking behavior.

## Low Priority Findings

### Absolute Path Leakage

Severity: low

The contract exposes absolute `workspace_root`, `stash_workspace`, and
`LocalPath.source_path`. This can leak usernames, cache paths, and local layout.

Research recommendation:

- Define which surfaces may return absolute paths.
- Redact or omit paths in web/list responses unless they are explicitly needed.

### Generated Docs Verification Names

Severity: low

The contract says generated docs should include dotted names such as
`marketplace.artifact.fork`. Current generated catalog format stores service and
action separately.

Research recommendation:

- Verify generated docs by service/action pairs, not by assuming a single
  concatenated dotted name.

### Frontend Whole-Plugin Fork UI

Severity: low

The plan says the UI should support selected artifact or whole plugin, but the
front-end task only wires a selected-file fork button.

Research recommendation:

- Either add whole-plugin fork UI acceptance criteria, or scope V1 UI to
  selected artifact only and keep whole-plugin fork CLI/API-only.

### LLM Merge Safety Tests

Severity: low

The spec mentions untrusted file content and secret-looking merge regions. The
current code has a secret heuristic before AI merge requests, but the plan does
not include regression tests for the Stash-backed merge path.

Research recommendation:

- Add tests that secret-looking conflict regions fail before AI calls.
- Add tests that prompt text treats artifact content as untrusted.

## Recommended Next Design Changes

For the next `/lavra-design` or plan-revision pass:

1. Resolve helper-state placement before implementation.
2. Redesign `component.adopt` so Marketplace does not pass arbitrary absolute
   paths from callers.
3. Remove parser-level confirmation requirements and document shared gate
   behavior.
4. Add `component.adopt` to HTTP admin-scope tests and write-action gate.
5. Make `artifact.update.apply` save a Stash revision and update origin metadata
   under lock.
6. Define single-file artifact workspace mapping.
7. Add preview payload caps and truncation contract.
8. Decide `artifact.fork` destructive/admin semantics.
9. Make multi-artifact and whole-plugin fork identity explicit.
10. Require reuse of hardened git fetch behavior.

## Verification Notes

Commands run during this research pass:

```bash
sed -n '1,520p' /home/jmagar/.codex/plugins/cache/jmagar-lab/lavra/0.7.7/skills/lavra-research/SKILL.md
wc -l docs/superpowers/plans/2026-06-13-marketplace-stash-integration.md
wc -l docs/superpowers/specs/2026-06-13-marketplace-stash-integration-spec.md
wc -l docs/contracts/marketplace-stash-integration.md
rg -n "artifact\\.fork|artifact\\.list|artifact\\.reset|artifact\\.update|component\\.adopt|component\\.import|StashOrigin|Origin|StashMeta|pending-update|FETCH_GUARDS|destructive|WRITE_ACTIONS|delete_component|walk_files_sorted|Component::Normal" crates docs/superpowers/plans/2026-06-13-marketplace-stash-integration.md docs/superpowers/specs/2026-06-13-marketplace-stash-integration-spec.md docs/contracts/marketplace-stash-integration.md
find apps/gateway-admin -path '*/node_modules' -prune -o -path '*/.next' -prune -o \( -name '*.tsx' -o -name '*.ts' \) -print
```

No code was changed and no tests were run. The research artifact itself is the
only file created in this pass.
