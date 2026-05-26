---
date: 2026-05-26 03:18:39 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 6b54fd47b09dc004a87320587edca8cd4c9d6547
session id: 0d60f661-02b9-4738-a7ed-5ae1e12f7ee5
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/0d60f661-02b9-4738-a7ed-5ae1e12f7ee5.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
---

# Crate extraction ADR records

## User Request

Create proper ADR records for the crate extraction docs in `docs/crate-extract/`,
then run `save-to-md` and `quick-push` straight to `main`.

## Session Overview

Added a dedicated ADR directory for Lab architecture decisions and created nine
accepted decision records from the crate/package extraction planning set.
Linked the ADR index from the main docs entrypoint and crate-extract README,
then prepared a docs-only patch release bump to `0.17.6`.

## Sequence of Events

1. Read the crate extraction spec, contract, research, dependency map, API
   surface, package manifest, inventory, roadmap, open questions, testing
   strategy, and execution strategy.
2. Checked for an existing ADR convention and found decision sections but no
   dedicated ADR directory.
3. Added `docs/adr/README.md` plus ADRs `0001` through `0009`.
4. Linked ADR discovery from `docs/README.md` and `docs/crate-extract/README.md`.
5. Followed quick-push release hygiene by bumping Rust workspace and
   gateway-admin package versions from `0.17.5` to `0.17.6`.
6. Ran the all-features workspace check and prepared this session artifact.

## Key Findings

- The crate extraction docs already contained clear decision material, but no
  dedicated ADR home existed under `docs/`.
- The repository already had `main` checked out and tracking `origin/main`.
- `docs/crate-extract/execution-strategy.md` was dirty before the ADR work and
  already contained the OAuth-lane ownership update.
- `docs/sessions/` is normally ignored in Lab workflows, so this session note
  must be force-added.

## Technical Decisions

- Created a focused ADR set instead of copying every planning document into a
  single large record.
- Kept ADRs under `docs/adr/` to make accepted architecture decisions distinct
  from draft plans, specs, roadmaps, and research notes.
- Used `Status: Accepted` for the records because they capture decisions already
  expressed as contract/spec rules in `docs/crate-extract/`.
- Treated the push as docs-only and applied a patch version bump.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| created | `docs/adr/README.md` | - | ADR index and directory purpose | `find docs/adr -type f` |
| created | `docs/adr/0001-extract-lab-as-reusable-packages.md` | - | Package extraction decision | `rg "ADR 0001" docs/adr` |
| created | `docs/adr/0002-shared-platform-and-product-runtime-crates.md` | - | Shared/product crate boundary decision | `rg "ADR 0002" docs/adr` |
| created | `docs/adr/0003-product-runtime-builders.md` | - | Runtime builder decision | `rg "ADR 0003" docs/adr` |
| created | `docs/adr/0004-rest-admin-and-mcp-action-surfaces.md` | - | REST/MCP surface decision | `rg "ADR 0004" docs/adr` |
| created | `docs/adr/0005-typescript-client-generation-from-openapi.md` | - | OpenAPI client-generation decision | `rg "ADR 0005" docs/adr` |
| created | `docs/adr/0006-lab-web-frontend-package-boundary.md` | - | Lab web package boundary decision | `rg "ADR 0006" docs/adr` |
| created | `docs/adr/0007-versioning-and-distribution.md` | - | Versioning and distribution decision | `rg "ADR 0007" docs/adr` |
| created | `docs/adr/0008-extraction-execution-lanes.md` | - | Extraction lane decision | `rg "ADR 0008" docs/adr` |
| created | `docs/adr/0009-extraction-verification-gates.md` | - | Verification gate decision | `rg "ADR 0009" docs/adr` |
| modified | `docs/README.md` | - | Link crate-extract and ADR docs from main docs entrypoint | `git diff -- docs/README.md` |
| modified | `docs/crate-extract/README.md` | - | Link ADR index from crate-extract reading order | `git diff -- docs/crate-extract/README.md` |
| modified | `docs/crate-extract/execution-strategy.md` | - | OAuth lane ownership and merge ordering | pre-existing dirty file plus `git diff` |
| modified | `Cargo.toml` | - | Patch version bump to `0.17.6` | `rg "0.17.6" Cargo.toml` |
| modified | `Cargo.lock` | - | Cargo lockfile version sync | `cargo check --workspace --all-features` |
| modified | `apps/gateway-admin/package.json` | - | Gateway admin package version bump | `rg "0.17.6" apps/gateway-admin/package.json` |
| modified | `CHANGELOG.md` | - | Release section for `0.17.6` | `sed -n '1,40p' CHANGELOG.md` |
| created | `docs/sessions/2026-05-26-crate-extraction-adr-records.md` | - | Session artifact | this file |

## Beads Activity

No bead activity observed for this session. `bd list --all --sort updated
--reverse --limit 20 --json` returned existing closed historical issues; no
crate-extraction ADR bead was created, edited, or closed.

## Repository Maintenance

### Plans

No plan files were moved. Quick-push mode limited maintenance to read-only
checks and documentation of observed state.

### Beads

Read-only bead inspection was performed. No directly relevant open bead was
identified from the recent issue output.

### Worktrees and branches

`git worktree list --porcelain` showed only `/home/jmagar/workspace/lab` on
`refs/heads/main`. `git branch -vv` showed only `main` tracking `origin/main`.
No worktree or branch cleanup was needed.

### Stale docs

The docs touched by the session were updated directly: `docs/README.md` and
`docs/crate-extract/README.md` now point to the ADR index.

### Transparency

No destructive cleanup was performed. The existing dirty edit in
`docs/crate-extract/execution-strategy.md` was preserved and included in the
push because the user requested a straight quick-push to `main`.

## Tools and Skills Used

- **Skills.** Used `save-to-md` for this artifact and `quick-push` for release,
  commit, and push workflow.
- **Shell commands.** Used `git`, `rg`, `sed`, `find`, `date`, `cargo`, `gh`,
  and `bd` for repo inspection, doc verification, version sync, and metadata.
- **File editing.** Used `apply_patch` to add ADR files, update docs, bump
  versions, update the changelog, and write this session note.
- **Memory.** Used Lab quick-push/session memory to confirm `docs/sessions/`
  force-add behavior and Lab verification preferences.

## Commands Executed

| command | result |
|---|---|
| `rg --files -g '*ADR*' -g 'adr*' -g 'docs/**'` | Found no dedicated ADR directory in Lab docs. |
| `sed -n '1,260p' docs/crate-extract/spec.md` | Read target architecture and package model. |
| `sed -n '1,260p' docs/crate-extract/contract.md` | Read enforceable extraction contract. |
| `git status --short --branch` | Confirmed `main...origin/main` with dirty docs and new ADR files. |
| `git fetch origin main` | Refreshed remote `main`. |
| `rg -n '^version\s*=|"version"\s*:' ...` | Found `0.17.5` in `Cargo.toml` and gateway-admin `package.json`. |
| `cargo check --workspace --all-features` | Passed in 42.75s and synced crate versions in `Cargo.lock`. |
| `git worktree list --porcelain` | Confirmed only the main worktree. |
| `git branch -vv` | Confirmed only `main` tracking `origin/main`. |

## Errors Encountered

- `rg` returned exit code 2 when optional version-bearing files were absent from
  the path list; the relevant matches were still observed in existing files.
- A zsh glob check for a non-existent `docs/sessions/*-v*.md` path reported
  `no matches found`; this only affected collision probing and did not block
  the session note path.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| ADR discovery | No dedicated ADR index under `docs/` | `docs/adr/README.md` lists accepted decision records |
| Crate extraction decisions | Decisions lived across spec, contract, research, and roadmap docs | Nine focused ADR records capture accepted decisions |
| Docs entrypoint | Main docs and crate-extract README did not point at ADRs | Both now link the ADR index |
| Release metadata | Workspace and gateway-admin versions were `0.17.5` | Versions are `0.17.6` |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo check --workspace --all-features` | workspace check passes | finished successfully in 42.75s | pass |
| `find docs/adr -type f -maxdepth 1 \| sort` | README plus 9 ADR files | listed all 10 ADR files | pass |
| `rg -n "Status: Accepted" docs/adr` | every ADR has accepted status | found accepted status in ADRs 0001-0009 | pass |

## Risks and Rollback

Risk is limited to documentation and version metadata. Rollback is a normal git
revert of the quick-push commit, which would remove the ADR records, docs links,
session note, changelog entry, and `0.17.6` version bump.

## Decisions Not Taken

- Did not create a single mega-ADR; that would duplicate the crate-extract spec
  and contract instead of recording discrete decisions.
- Did not split ADRs by every package name; the current records cover stable
  architectural decisions while package-specific ownership remains in the
  package manifest.
- Did not move or delete branches/worktrees during quick-push.

## References

- `docs/crate-extract/spec.md`
- `docs/crate-extract/contract.md`
- `docs/crate-extract/research.md`
- `docs/crate-extract/dependency-map.md`
- `docs/crate-extract/api-surface.md`
- `docs/crate-extract/package-manifest.md`
- `docs/crate-extract/inventory.md`
- `docs/crate-extract/migration-roadmap.md`
- `docs/crate-extract/open-questions.md`
- `docs/crate-extract/testing-strategy.md`
- `docs/crate-extract/execution-strategy.md`

## Open Questions

- Whether future ADRs should remain in `docs/adr/` only, or whether the repo
  should also add an ADR template and contribution rule.
- Whether crate-extraction implementation beads should be created before the
  first extraction wave starts.

## Next Steps

- Commit and push this work directly to `main`.
- After the push, verify `origin/main` contains the new commit and that the
  committed file set includes this force-added session artifact.
