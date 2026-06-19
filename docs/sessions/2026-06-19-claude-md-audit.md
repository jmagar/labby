---
date: 2026-06-19 16:24:39 EST
repo: git@github.com:jmagar/lab.git
branch: claude/dreamy-mclean-661c82
head: 665c7dbf
working directory: /home/jmagar/workspace/lab/.claude/worktrees/dreamy-mclean-661c82
worktree: /home/jmagar/workspace/lab/.claude/worktrees/dreamy-mclean-661c82
beads: none
---

# CLAUDE.md audit — purge post-pivot stale references

## User Request

"claude md updater" — invoke the CLAUDE.md improver to audit and update the
repository's CLAUDE.md files. Follow-ups: apply the fixes (full 🔴 + 🟡 pass),
and update the contradicted auto-memory.

## Session Overview

Audited all 15 CLAUDE.md files in the repo. The suite was already high quality;
the one systemic issue was staleness introduced by commit `d2e2d768` ("pivot lab
to gateway-focused — rip per-service homelab integrations"), which deleted the
`radarr`/`sonarr`/`servarr`/`openai`/`overseerr` client family. Seven CLAUDE.md
files still taught from those deleted services (including a phantom
`mount_if_enabled!` macro that exists nowhere in the codebase, and a Tier‑1 CLI
reference to a nonexistent `radarr.rs`). Applied 8 targeted edits across 7 files,
then corrected the now-wrong `project_scaffold_audit` auto-memory.

## Sequence of Events

1. Invoked the `claude-md-management:claude-md-improver` skill.
2. Discovered 15 CLAUDE.md files; inventoried actual repo structure (crates,
   `lab-apis/src` modules, dispatch dirs, Justfile targets).
3. Cross-checked claims against source: found `lab-apis` slimmed to
   `core/marketplace/device_runtime/acp/doctor/setup/stash` (+ feature-gated
   `deploy/mcpregistry/acp_registry`); confirmed `radarr`/`openai`/`overseerr`
   modules are gone via `git log` (`d2e2d768`).
4. Produced a quality report; asked user for scope → "🔴 + 🟡 full pass" +
   "delete Batch section".
5. Applied 🔴 fixes (lab-apis, lab, cli), then 🟡 example re-pointing (api, mcp,
   dispatch, gateway). Discovered `mount_if_enabled!` was a phantom macro and
   replaced it with the real `#[cfg(feature)]` + `nest()` pattern.
6. Verified no stale refs remained; the two surviving `Servarr` mentions are
   legitimate (live `Category::Servarr` enum variant + `X-Api-Key` convention origin).
7. Resolved a leftover contradiction: removed the `audit`/`scaffold`
   "infrastructure commands" parenthetical (no such CLI subcommands exist).
8. Rewrote the `project_scaffold_audit` memory and its `MEMORY.md` index line to
   record the removal.

## Key Findings

- `lab-apis/src/lib.rs` + `Cargo.toml` features confirm the arr/AI client family
  is fully removed; only `acp_registry`, `deploy`, `mcpregistry` are opt-in features.
- `mount_if_enabled!` appeared only in `crates/lab/src/api/CLAUDE.md:76` — it is
  not defined or used anywhere in source. Real mounting is `#[cfg(feature)]`-gated
  `v1.nest(...)` in `crates/lab/src/api/router.rs:1067-1090`.
- No `Scaffold`/`Audit` variant in the top-level CLI enum (`crates/lab/src/cli.rs`);
  the `// [lab-scaffold: ...]` lines are codegen anchors. "Audit configured
  services" is the doc comment on the `Doctor` subcommand (`cli.rs:75-76`).
- `crates/lab/src/scaffold/` and `crates/lab/src/audit/` directories and the
  `lab-service-onboarding` skill no longer exist; `docs/dev/SCAFFOLD_AND_AUDIT.md`
  still exists but describes removed tooling.
- `Category::Servarr` is still a real enum variant (`crates/lab-apis/src/core/plugin.rs:52`),
  so root CLAUDE.md's 10-variant list and the `X-Api-Key (Servarr convention)`
  note are accurate — left untouched.

## Technical Decisions

- Re-pointed illustrative examples to live services (`marketplace`, `gateway`,
  `deploy`, `nodes`) rather than deleting them, preserving the teaching value
  while removing dead names a reader would grep for and not find.
- Deleted the cli `Batch commands` section outright (per user choice) because no
  `_many` implementation survives the pivot — keeping it would document a
  capability with zero backing code.
- Updated rather than deleted the stale memory, keeping the historical "why"
  (commit `d2e2d768`) and a pointer to current manual onboarding.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| modified | `crates/lab-apis/CLAUDE.md` | — | removed dead `servarr` transitive-feature bullet + `openai`/`overseerr` examples | `git diff --stat`: 6 lines |
| modified | `crates/lab/CLAUDE.md` | — | Tier‑1 ref → `marketplace.rs`/`nodes.rs`; dropped stale `audit`/`scaffold` parenthetical | `git diff --stat`: 2 lines |
| modified | `crates/lab/src/api/CLAUDE.md` | — | fixed transport example; replaced phantom `mount_if_enabled!` with real `nest()` pattern | `git diff --stat`: 14 lines |
| modified | `crates/lab/src/cli/CLAUDE.md` | — | deleted dead "Batch commands" section | `git diff --stat`: 11 deletions |
| modified | `crates/lab/src/dispatch/CLAUDE.md` | — | domain-module examples → `plugins.rs`/`sources.rs`/`forks.rs`/`artifacts.rs` | `git diff --stat`: 2 lines |
| modified | `crates/lab/src/dispatch/gateway/CLAUDE.md` | — | `(radarr, unraid, …)` → `(marketplace, mcpregistry, …)` | `git diff --stat`: 2 lines |
| modified | `crates/lab/src/mcp/CLAUDE.md` | — | tool-name + register example → `deploy` | `git diff --stat`: 6 lines |
| modified | `~/.claude/.../memory/project_scaffold_audit.md` | — | rewrote to record scaffold/audit removal (outside repo) | Write tool |
| modified | `~/.claude/.../memory/MEMORY.md` | — | updated index line to flag REMOVED (outside repo) | Edit tool |

## Beads Activity

No bead activity observed. This was a documentation-correctness pass; no tracker
state was read or changed during the session.

## Repository Maintenance

- **Plans**: Checked `docs/plans/*.md` — one active plan remains
  (`docs/plans/fleet-ws-plan-lab-n07n.md`). It is unrelated to this session and
  not completed; left in place. No completed plans to move.
- **Beads**: Not touched — no session work mapped to a bead. The injected
  `Beads recent issues` set is all pre-existing/closed history.
- **Worktrees/branches**: Inspected the injected `git worktree list` and branch
  set. Four worktrees and the protected `marketplace-no-mcp` branch exist; none
  were cleaned — this docs-only session created no merged/obsolete refs, and
  cleanup of unrelated worktrees is out of scope.
- **Stale docs**: This session WAS the stale-docs pass for CLAUDE.md files. One
  related doc, `docs/dev/SCAFFOLD_AND_AUDIT.md`, is now historical (describes
  removed tooling) — flagged in Open Questions rather than edited, since
  rewriting it was outside the requested scope.
- **Transparency**: All edits verified via post-edit grep sweep (no remaining
  `radarr|sonarr|servarr|overseerr|prowlarr|tautulli|mount_if_enabled|add_many`).

## Tools and Skills Used

- **Skill**: `claude-md-management:claude-md-improver` — drove the audit workflow.
- **Shell (Bash)**: repo inventory (`ls`, `grep`, `git log`, `git diff --stat`,
  `find`), claim verification, symlink integrity checks. No failures.
- **File tools**: `Read` (CLAUDE.md files + memory), `Edit`/`Write` (8 doc edits
  + 2 memory files). Two `Edit` calls failed once with "File has not been read
  yet" (api and gateway docs) and succeeded after a targeted `Read` — expected
  harness behavior, no data impact.
- **AskUserQuestion**: collected scope decision (full pass + delete Batch section).
- No MCP servers, subagents, browser tools, or external CLIs were used.

## Commands Executed

| command | result |
|---|---|
| `find . -name CLAUDE.md` | 15 files |
| `ls crates/lab-apis/src/` | confirmed arr/AI modules absent |
| `git log ... -- 'crates/lab-apis/src/radarr*'` | `d2e2d768 refactor: pivot lab to gateway-focused — rip per-service homelab integrations` |
| `grep -rn 'mount_if_enabled' crates/lab/src` | only hit: the CLAUDE.md doc itself |
| `grep -A40 '^\[features\]' crates/lab-apis/Cargo.toml` | features: acp_registry, deploy, mcpregistry |
| `grep -rni '<stale terms>' $(find . -name CLAUDE.md)` (post-edit) | only legitimate `Servarr` refs remain |
| `git diff --stat` | 7 files, 17 insertions, 26 deletions |

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| CLAUDE.md examples | referenced deleted services (radarr/sonarr/openai/overseerr) | reference live services (marketplace/gateway/deploy/nodes) |
| api CLAUDE.md mounting | described phantom `mount_if_enabled!` macro | describes real `#[cfg(feature)]` + `nest()` pattern |
| cli CLAUDE.md | documented removed `add_many` batch capability | section removed |
| `project_scaffold_audit` memory | claimed scaffold/audit commands are mandatory | records they were removed; points to manual onboarding |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| post-edit stale-term grep over all CLAUDE.md | no arr/AI/macro refs | only `Servarr` (enum/convention) | pass |
| `grep -wE 'openai' $(find . -name CLAUDE.md)` | none | NONE | pass |
| `grep 'pub enum Category'` source | `Servarr` still present | present | pass |
| symlink check (AGENTS.md/GEMINI.md) | symlinks → CLAUDE.md | all symlinks | pass |

## Risks and Rollback

Low risk — documentation-only edits, no code or build impact. Rollback:
`git checkout -- <CLAUDE.md paths>` restores prior text; the two memory files are
outside the repo and can be reverted independently if desired.

## Decisions Not Taken

- Did not edit `docs/dev/SCAFFOLD_AND_AUDIT.md` (now historical) — out of the
  requested CLAUDE.md scope; flagged for follow-up instead.
- Did not delete the stale memory outright — preserved the "why" with a removal note.
- Did not clean unrelated worktrees/branches — not produced by this session.

## References

- Commit `d2e2d768` — "pivot lab to gateway-focused — rip per-service homelab integrations"
- `docs/dev/SERVICE_ONBOARDING.md`, `crates/lab/src/dispatch/CLAUDE.md` — current onboarding authority

## Open Questions

- `docs/dev/SCAFFOLD_AND_AUDIT.md` documents removed scaffold/audit tooling. Should
  it be deleted, or rewritten to point at the manual onboarding flow? Worth a bead.

## Next Steps

- The 7 CLAUDE.md edits are uncommitted in the working tree — commit them when ready
  (separate from this session-log commit, which is path-limited to this file).
- Consider a follow-up to reconcile or remove `docs/dev/SCAFFOLD_AND_AUDIT.md`.
- Optional: grep the broader docs/ tree for other `mount_if_enabled!` or arr-service
  references that may have survived the pivot outside CLAUDE.md files.
