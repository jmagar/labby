---
date: 2026-05-23 01:49:47 EDT
repo: git@github.com:jmagar/lab.git
branch: bd-work/scout-security-fixes
head: 466586d0
agent: Codex
working directory: /home/jmagar/workspace/lab
---

# Quick Push: Scout Security and Paperless Cleanup

## User Request

Use mcporter to test optimized tool search, remove stale Paperless remnants, quick-push the work, merge it back into main, and clean up stale branches/worktrees.

## Session Overview

- Verified Lab MCP `scout` behavior with `mcporter` against both stdio and HTTP surfaces.
- Confirmed Paperless is no longer active in the live gateway catalog.
- Removed active Paperless plugin/docs/UI/env/health-check remnants from the repo.
- Added scout access hardening and schema-visibility tests already present in the branch.
- Bumped the release surfaces to `0.17.1`, updated changelog, committed, and pushed `bd-work/scout-security-fixes`.

## Sequence of Events

1. Used `mcporter list` and `mcporter call` to inspect the Lab MCP surfaces.
2. Identified `labby-http` as the correct live gateway-backed target for `scout` quality checks.
3. Verified `paperless` was absent from `gateway.list` and returned `not_found` from `gateway.server.get`.
4. Removed remaining active Paperless references and deleted `plugins/paperless-ngx/` plus the vendored Paperless reference dump.
5. Ran verification, bumped versions, updated `CHANGELOG.md`, committed, and pushed.

## Key Findings

- `~/.labby/config.toml` had `virtual_servers = []`; Paperless only existed under quarantined virtual servers.
- `gateway.server.get` for `paperless` returned `not_found`, confirming it was no longer active.
- The stdio MCP path did not exercise the full live gateway catalog; `labby-http` did.
- `scout` returned useful live HTTP results for Docker, notifications, UniFi, and gateway-route queries.

## Technical Decisions

- Kept historical `docs/sessions`, `docs/superpowers`, and `docs/reports` references intact as audit trail.
- Removed active Paperless references from docs, UI catalogs, plugin metadata, env examples, and health scripts.
- Bumped patch version only: this was a cleanup and access-hardening release, not a new feature line.

## Files Modified

- `crates/lab/src/mcp/server.rs`: scout scope and schema visibility hardening.
- `plugins/paperless-ngx/**`: removed stale Paperless plugin package.
- `docs/references/paperless-ngx/repo.md`: removed vendored Paperless reference dump.
- `apps/gateway-admin/lib/**`: removed Paperless from service slugs and brand metadata.
- `scripts/health-check` and `plugins/homelab-health/**`: removed Paperless probes.
- `README.md`, `CLAUDE.md`, `docs/**`, `plugins/lab/**`: removed active Paperless documentation entries.
- `Cargo.toml`, `Cargo.lock`, `apps/gateway-admin/package.json`, `CHANGELOG.md`: release bump to `0.17.1`.

## Commands Executed

- `mcporter list lab --schema --all-parameters --json`: verified stdio MCP server shape.
- `mcporter list labby-http --schema --all-parameters --json`: verified HTTP MCP server shape.
- `mcporter call labby-http.scout ...`: checked tool-search quality.
- `mcporter call labby-http.invoke ... gateway.list`: verified live gateway catalog.
- `git grep -n -i "paperless" -- . ':!docs/sessions/**' ':!docs/superpowers/**' ':!docs/reports/**'`: verified no active Paperless references remained except changelog release note context after version entry.
- `cargo check --workspace --all-features`: passed.
- `pnpm --dir apps/gateway-admin test`: passed.
- `git push -u origin bd-work/scout-security-fixes`: pushed branch.

## Errors Encountered

- Initial sandboxed `mcporter` stdio launch failed because Labby could not create its log file under a read-only filesystem. Retried after the environment allowed normal filesystem/network access.
- Stdio `scout` produced weak fallback results because it did not use the live HTTP gateway catalog; HTTP MCP was used for quality validation.

## Behavior Changes

| Before | After |
| --- | --- |
| Paperless appeared in plugin/docs/UI/env/health surfaces despite no active implementation. | Active Paperless remnants are removed. |
| `scout` access/schema visibility needed scope hardening. | `lab:read` can discover names/descriptions; full schemas remain limited to stronger Lab scopes. |
| Version was `0.17.0`. | Version is `0.17.1`. |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `cargo check --workspace --all-features` | Build check passes | Finished dev profile successfully | Pass |
| `pnpm --dir apps/gateway-admin test` | Unit tests pass | 362 passed, 0 failed | Pass |
| `git grep -n -i "paperless" -- . ':!docs/sessions/**' ':!docs/superpowers/**' ':!docs/reports/**'` | No active stale references | Only changelog context after release note was intentionally added | Pass |
| `mcporter call labby-http.scout ...` | Relevant gateway tools are discoverable | Docker found Arcane, notifications found Apprise/Gotify, UniFi found UniFi | Pass |

## Risks and Rollback

- Deleting the vendored Paperless reference dump is a large deletion; rollback is `git revert 466586d0` if that reference is still needed.
- Historical docs still mention Paperless; this is intentional audit trail, not active product surface.

## Next Steps

- Merge `bd-work/scout-security-fixes` into `main`.
- Clean up stale merged branches and worktrees after the merge is verified.
