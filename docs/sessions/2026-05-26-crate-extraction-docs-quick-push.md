---
date: 2026-05-26 00:22:33 EDT
repo: git@github.com:jmagar/lab.git
branch: main
head: f5157c25
agent: Codex
session id: 0d60f661-02b9-4738-a7ed-5ae1e12f7ee5
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/0d60f661-02b9-4738-a7ed-5ae1e12f7ee5.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab f5157c25 [main]
---

# Crate Extraction Docs Quick Push

## User Request

Review all crate extraction docs for alignment, then quick-push the resulting
work straight to `main`.

## Session Overview

- Reviewed the crate extraction doc set for internal consistency.
- Fixed alignment issues across the spec, contract, dependency map, inventory,
  execution strategy, and open questions.
- Added the full crate extraction documentation set and related Gateway planning
  docs to git.
- Bumped the workspace and gateway-admin versions from `0.17.4` to `0.17.5`.
- Committed and pushed the work directly to `origin/main`.

## Sequence of Events

1. Read the crate extraction spec, contract, dependency map, open questions,
   execution strategy, and inventory.
2. Found mismatches around REST schema generation, frontend dependency direction,
   runtime surface optionality, OAuth boundary wording, and lane manifest
   ownership.
3. Patched the docs so the architecture and contract agree.
4. Ran a follow-up `rg` pass to confirm no stale blocking language remained
   outside the research note.
5. Per quick-push, bumped versions and updated the changelog.
6. Ran the all-features workspace check.
7. Committed and pushed `main`.

## Key Findings

- `docs/crate-extract/spec.md` still described `schemars -> utoipa` for REST
  client generation, while the contract and research called for
  `utoipa::ToSchema -> OpenAPI`.
- Frontend dependency diagrams could be read as
  `@jmagar/lab-api-client -> @jmagar/aurora`; docs now distinguish app-level
  dependencies from package-to-package dependencies.
- Product runtime examples required `router`, `registry`, and `catalog`, even
  though the contract allowed products to omit surfaces they do not expose.
- `lab-oauth` was already a target product boundary, so the open question is
  now when it gets a full runtime builder, not whether it exists.

## Technical Decisions

- REST/admin DTOs should use `utoipa::ToSchema`; `schemars::JsonSchema` is
  reserved for standalone JSON Schema consumers such as MCP/action schemas.
- `@jmagar/lab-api-client` must remain UI-framework-free and must not depend on
  Aurora or `@jmagar/lab-web`.
- Product runtime builders expose optional surface pieces so smaller products
  are not forced to provide HTTP, MCP, and catalog output.
- Product lanes may own package-local manifests, while the integration lane owns
  workspace/root manifests and global wiring.

## Files Modified

- `docs/crate-extract/*.md`: added and aligned the crate extraction doc set.
- `docs/superpowers/plans/2026-05-25-extract-gateway-server.md`: expanded the
  standalone Gateway extraction plan around MCP surfaces and transports.
- `docs/superpowers/plans/2026-05-25-gateway-fresh-clone-prune-list.md`: added
  Gateway-only fresh clone prune guidance.
- `docs/sessions/2026-05-25-code-mode-merge-cleanup.md`: added prior session
  note.
- `docs/sessions/2026-05-25-lab-rmcp-extraction-plans.md`: added prior session
  note.
- `CHANGELOG.md`: added `0.17.5` release notes.
- `Cargo.toml`, `Cargo.lock`, `apps/gateway-admin/package.json`: bumped
  versions to `0.17.5`.

## Commands Executed

- `rg ... docs/crate-extract`: checked schema-generation, dependency-direction,
  runtime-shape, OAuth, and manifest ownership language.
- `cargo check --workspace --all-features`: verified the Rust workspace after
  the version bump.
- `git commit -m "docs: add crate extraction architecture docs" ...`: created
  commit `f5157c25`.
- `git push origin main`: pushed `main` to GitHub.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `rg ... docs/crate-extract` | no stale blocking language outside historical research | only historical research references remained | pass |
| `cargo check --workspace --all-features` | workspace checks successfully | finished `dev` profile successfully | pass |
| `git push origin main` | push accepted | `734e1f4d..f5157c25 main -> main` | pass |

## Risks and Rollback

- Risk: the committed docs are architecture/planning artifacts, not
  implementation. The extraction still needs code work before the boundaries are
  enforceable.
- Rollback: revert `f5157c25` to remove the doc/version/changelog change set.

## Open Questions

- Exact external repo/package distribution remains undecided.
- `lab-oauth` runtime-builder timing remains open.
- Generated wrapper style for `@jmagar/lab-api-client` remains open.

## Next Steps

- Start implementation by creating shared platform crate boundaries in isolated
  worktrees.
- Add boundary enforcement checks once the new packages exist.
- Use the contract and execution strategy as the source of truth when assigning
  extraction lanes.
