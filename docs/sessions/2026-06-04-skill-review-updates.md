---
date: 2026-06-04 03:20:47 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: c7db07a6
session id: baf2ab39-cb4e-4df8-9ea4-9fb603127e11
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/baf2ab39-cb4e-4df8-9ea4-9fb603127e11.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  c7db07a6 [main]
---

# Skill quality sweep — lab plugins + rmcp workspace

## User Request

Dispatch `plugin-dev:skill-reviewer` agents to thoroughly review all plugins in `plugins/` and all the Rust rmcp MCP server skills in `~/workspace`, then update the skills as necessary.

## Session Overview

Dispatched 8 parallel `plugin-dev:skill-reviewer` agents covering all ~70 SKILL.md files across the lab plugin tree and 10 rmcp workspace repositories. All agents returned text-only reports (no write access in agent context), so every edit was applied directly by the main agent. In total, ~50 individual file edits were made spanning description rewrites, stale path fixes, structural cleanup, body additions, and two full skill rewrites.

## Sequence of Events

1. **Scoped the work.** Enumerated all SKILL.md files across `plugins/` (39 files, 26 plugins) and workspace rmcp repos (20 files, 10 repos).
2. **Dispatched 8 parallel agents.** Each covered a logical group: utilities (19 single-skill), arrs (10), testing (6), vibin (28), misc lab (broadcastr/plexus/acp), rmcp single-skill (7), cortex (9), apprise+template (3).
3. **Applied testing fixes.** Fixed stale `axon_rust` → `axon` path in two mcpjam files; removed dangling empty header; added missing References section to mcporter.
4. **Applied cortex fixes.** Rewrote all 7 sub-skill descriptions from imperative/second-person to third-person trigger form; added 3 new workflow sections to main cortex skill; added missing action calls to cortex-report; added body guards and edge-case handling.
5. **Applied vibin fixes.** Fixed rmcp version `1.4` → `1.6` in using-rmcp Cargo.toml examples; removed coercive MANDATORY SKILL INVOCATION block from paperless-ngx.
6. **Applied utility plugin fixes.** memos: removed 3 redundant Detailed Flow blocks, collapsed Common Errors, fixed References. linkding: added References section. bytestash: removed emoji, rewrote Agent Tool Usage. radicale: removed duplicate Bundled Resources section.
7. **Applied arrs fixes.** Fixed 19 wrong plex script paths and 34 wrong tautulli script paths via global replace; fixed prose references; removed inaccurate tautulli Multiple Servers section; fixed stale `.env` comments in sonarr and overseerr; removed empty stub bash block in qbittorrent; strengthened jellyfin description.
8. **Applied rmcp single-skill fixes.** Fixed binary path (`unraid` → `runraid`) in unraid; replaced one-liner descriptions with proper trigger-phrase-rich third-person descriptions for synapse2, tailscale, unifi, gotify; changed "Use this skill whenever" → "This skill should be used when" patterns.
9. **Full rewrites.** rarcane: complete rewrite from sparse 400-word skill to 1,100-word skill with per-domain tables, workflows, gotchas. rustarr: complete rewrite from 300-word skill to 900-word skill with per-service API path examples, common workflows, gotchas.
10. **Applied apprise and template fixes.** Fixed apprise `.claude/` mirror (binary name `apprise` → `rapprise`, extended trigger phrases); added post-approval implementation reference section to scaffold-project.
11. **Rewrote acp/rust skill.** Major revision: added Lab ACP Runtime section covering `Client.builder()`/`ByteStreams`/`attach_session`/`session_config_options()`/`SetSessionConfigOptionRequest`; fixed async-trait contradiction (body claimed native async fn works, examples use `#[async_trait(?Send)]`); updated Cargo.toml to show pinned lab version; added Extending Lab Runtime checklist.

## Key Findings

- **Cortex descriptions were all wrong format.** All 7 sub-skills used imperative openers ("Troubleshoot...", "Deploy...", "Run a comprehensive...") rather than third-person trigger form — this directly degrades automatic skill routing since the description field is the primary trigger signal.
- **Stale `axon_rust` path.** `plugins/testing/skills/mcpjam-ui-testing/SKILL.md:72` and its reference file had `axon_rust` (deprecated non-git copy per CLAUDE.md) instead of the live `axon` repo.
- **plex and tautulli script paths broken.** All invocations used `./skills/<service>/scripts/<script>.sh` (absolute from plugin root) instead of `./scripts/<script>.sh` (correct relative form matching all other arrs skills). 18 plex occurrences, 33 tautulli occurrences.
- **rmcp version mismatch.** `vibin/using-rmcp/SKILL.md` showed `rmcp = { version = "1.4" }` in Cargo.toml examples but the lab workspace uses `1.6`.
- **acp/rust async-trait contradiction.** Body text said "do NOT add async-trait — use native async fn in trait" but `examples/agent-impl.rs:65` uses `#[async_trait::async_trait(?Send)]`. The `?Send` bound in the 0.13.x SDK requires async-trait.
- **rarcane and rustarr were effectively empty.** rarcane had no workflow examples, no gotchas, and no `envId` guidance. rustarr had no per-service API paths — an agent reading it would not know how to check what's downloading or call Sonarr.
- **paperless-ngx had a coercive MANDATORY block.** Lines 6–21 used caps-lock, warning emoji, and "Failure to invoke this skill violates your operational requirements" — an anti-pattern that's ineffective and degrades prompt quality.
- **tautulli Multiple Servers section was inaccurate.** Instructed users to manually edit `~/.config/lab-arrs/config.env`, which is auto-generated by the hook and overwritten on every SessionStart.

## Technical Decisions

- **Applied all edits from the main agent, not the subagents.** Subagents only had Read/Grep/Glob/advisor tools — no Write access. This was an expected subagent environment constraint.
- **Global sed for script path fixes.** Used `sed -i 's|old|new|g'` for plex (18 occurrences) and tautulli (33 occurrences) rather than individual edits — faster and guaranteed complete.
- **Full rewrites for rarcane and rustarr.** Both were so sparse that targeted edits would have required more complexity than a clean rewrite. Agent provided complete replacement content.
- **Did not edit upstream-vendored skills.** `claude-android-ninja` (DrJacky/Apache-2.0) and `jetpack-compose-expert` (aldefy/compose-skill) were flagged by the vibin agent as read-only upstream vendor skills — left untouched.
- **Did not align plugin.json files.** apprise-mcp had a `.claude/` plugin.json divergence from the primary; noted but left for a separate task since it was out of scope for skill review.

## Files Changed

| Status | Path | Purpose |
|---|---|---|
| modified | `plugins/acp/skills/rust/SKILL.md` | Full rewrite: Lab ACP Runtime section, async-trait fix, pinned Cargo.toml |
| modified | `plugins/arrs/skills/jellyfin/SKILL.md` | Strengthened description with concrete trigger phrases |
| modified | `plugins/arrs/skills/overseerr/SKILL.md` | Fixed stale `.env` comment → `plugin settings` |
| modified | `plugins/arrs/skills/plex/SKILL.md` | Fixed 19 wrong script paths + prose reference |
| modified | `plugins/arrs/skills/qbittorrent/SKILL.md` | Removed empty stub bash block |
| modified | `plugins/arrs/skills/sonarr/SKILL.md` | Fixed stale `.env` → `plugin settings (config.env)` |
| modified | `plugins/arrs/skills/tautulli/SKILL.md` | Fixed 34 wrong script paths, prose reference, removed inaccurate Multiple Servers section |
| modified | `plugins/bytestash/skills/bytestash/SKILL.md` | Removed emoji, rewrote Agent Tool Usage section |
| modified | `plugins/linkding/skills/linkding/SKILL.md` | Added References section for 3 bundled reference files |
| modified | `plugins/memos/skills/memos/SKILL.md` | Removed 3 redundant Detailed Flow blocks, collapsed Common Errors, fixed References |
| modified | `plugins/plexus/skills/operating-remote/SKILL.md` | Description opener: "Use when" → "This skill should be used when" |
| modified | `plugins/radicale/skills/radicale/SKILL.md` | Removed duplicate Bundled Resources section, merged into single Reference section |
| modified | `plugins/testing/skills/mcpjam-ui-testing/SKILL.md` | Fixed stale `axon_rust` → `axon` path |
| modified | `plugins/testing/skills/mcpjam-ui-testing/references/commands.md` | Fixed stale `axon_rust` → `axon` path |
| modified | `plugins/testing/skills/mcporter/SKILL.md` | Removed dangling empty header; added References section |
| modified | `plugins/vibin/skills/paperless-ngx/SKILL.md` | Removed coercive MANDATORY SKILL INVOCATION block |
| modified | `plugins/vibin/skills/using-rmcp/SKILL.md` | Fixed rmcp version `1.4` → `1.6` in Cargo.toml examples |

**Workspace repos (outside lab, separate git repos):**

| Status | Path | Purpose |
|---|---|---|
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex/SKILL.md` | Expanded description + 3 new workflow sections |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-logs/SKILL.md` | Description rewrite + cortex-troubleshoot next-step mention |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-troubleshoot/SKILL.md` | Description rewrite to third-person trigger form |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-report/SKILL.md` | Added anomalies/silent_hosts/unaddressed_errors calls to step 3 |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-deploy-dropins/SKILL.md` | Description rewrite |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-dr/SKILL.md` | Description rewrite |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-frustration-assessment/SKILL.md` | Description rewrite, upstream trigger phrases added |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-redeploy/SKILL.md` | Description rewrite + missing-script guard |
| modified | `~/workspace/cortex/plugins/cortex/skills/cortex-version-check/SKILL.md` | Description rewrite + container-not-running edge case |
| modified | `~/workspace/rustcane/plugins/rarcane/skills/rarcane/SKILL.md` | Full rewrite: per-domain tables, workflows, gotchas |
| modified | `~/workspace/rustarr/plugins/rustarr/skills/rustarr/SKILL.md` | Full rewrite: per-service API paths, workflows, gotchas |
| modified | `~/workspace/rustifi/plugins/unifi/skills/unifi/SKILL.md` | Description opener fix |
| modified | `~/workspace/rustify/plugins/gotify/skills/gotify/SKILL.md` | Description rewrite to trigger-first format |
| modified | `~/workspace/rustscale/plugins/tailscale/skills/tailscale/SKILL.md` | Description opener reframed |
| modified | `~/workspace/unrust/plugins/unraid/skills/unraid/SKILL.md` | Binary path `unraid` → `runraid` |
| modified | `~/workspace/synapse2/plugins/synapse2/skills/synapse2/SKILL.md` | Replaced one-liner with third-person + 15 trigger phrases |
| modified | `~/workspace/apprise-mcp/.claude/plugins/apprise-mcp/skills/apprise/SKILL.md` | Binary `apprise` → `rapprise`, extended trigger phrases |
| modified | `~/workspace/rmcp-template/plugins/rtemplate/skills/scaffold-project/SKILL.md` | Added post-approval implementation reference section |

## Beads Activity

No bead activity observed — this session was a skill maintenance sweep with no tracked feature work.

## Repository Maintenance

### Plans

- `docs/plans/fleet-ws-plan-lab-n07n.md` — Active brainstorm (bead `lab-n07n` open). Not moved.
- `docs/plans/mcp-streamable-http-oauth-proxy.md` — Multi-phase plan (Phase 0–3 listed, no "complete" marker). Not moved. `docs/plans/complete/` not created — no completed plans to move.

### Worktrees and branches

- `p1-fixes` branch: `git merge-base --is-ancestor p1-fixes main` confirmed merged into main.
- Worktree at `/home/jmagar/workspace/lab-p1-fixes` is **dirty** (5 modified files: `crates/lab-apis/src/acp/persistence.rs`, `crates/lab/src/acp.rs`, `crates/lab/src/acp/registry.rs`, `crates/lab/src/cli/serve.rs`, `crates/lab/src/dispatch/acp/params.rs`). Left untouched — dirty worktrees are never removed without explicit approval.

### Stale docs

- No session-related stale docs identified. The skills themselves were the documentation being updated.

### Transparency

- Workspace repo changes (cortex, rustarr, etc.) are not reflected in `git status` of the lab repo — they live in separate git repos and must be committed/pushed separately.
- The lab `plugins/` changes above are unstaged and uncommitted as of session end.

## Tools and Skills Used

- **Agent tool (parallel subagents).** 8 `plugin-dev:skill-reviewer` agents dispatched in parallel. All ran successfully and returned complete text reports. None had file-write access — all edits applied by main agent.
- **Read tool.** Used extensively to read current SKILL.md content before editing, and to verify section boundaries.
- **Edit tool.** Primary editing tool for targeted in-place fixes.
- **Write tool.** Used for two full rewrites (rarcane, rustarr SKILL.md) and the acp/rust SKILL.md rewrite.
- **Bash tool.** Used for `grep -c`, `sed -i` global replacements, `wc -l`, `tail`, `find`, and maintenance checks (`git worktree list`, `git merge-base`).
- **`vibin:save-to-md` skill.** This session documentation.

## Commands Executed

| Command | Result |
|---|---|
| `find ~/workspace/lab/plugins -name "SKILL.md" \| sort` | Enumerated 39 SKILL.md files across 26 plugins |
| `find ~/workspace/r*/... -name "SKILL.md"` | Enumerated 20 SKILL.md files across 10 rmcp workspace repos |
| `sed -i 's\|./skills/plex/scripts/plex-api.sh\|./scripts/plex-api.sh\|g' plex/SKILL.md` | Replaced 18 wrong plex script paths |
| `sed -i 's\|./skills/tautulli/scripts/tautulli-api.sh\|./scripts/tautulli-api.sh\|g' tautulli/SKILL.md` | Replaced 33 wrong tautulli script paths |
| `git merge-base --is-ancestor p1-fixes main` | Confirmed p1-fixes merged into main |
| `git -C lab-p1-fixes status --short` | Confirmed worktree dirty (5 files) — not removed |

## Errors Encountered

- **Subagents lacked write access.** All 8 `plugin-dev:skill-reviewer` agents reported they had only Read/Grep/Glob/advisor tools — no Write or Edit. Workaround: agents delivered precise diff-style recommendations, main agent applied all edits directly.
- **`Edit` rejected unread file** for `unrust/plugins/unraid/skills/unraid/SKILL.md` on first attempt (tool requires a prior Read). Resolved by reading the relevant lines first, then editing successfully.
- **`Edit` rejected unread `tautulli/SKILL.md`** after `sed -i` modified it externally. Resolved by reading the target line first before editing.

## Behavior Changes (Before/After)

| Area | Before | After |
|---|---|---|
| Cortex sub-skill routing | All 7 descriptions started with imperatives ("Troubleshoot...", "Deploy...") — degrades auto-routing | All 7 rewritten to "This skill should be used when..." with specific trigger phrases |
| rarcane skill | ~400 words, no workflows, no gotchas, no `envId` guidance | ~1,100 words: per-domain subaction tables, 4 workflows, 6 gotchas |
| rustarr skill | ~300 words, no per-service API paths | ~900 words: per-service examples for all 9 services, 3 workflows, 6 gotchas |
| acp/rust skill | Missing lab client-side patterns; async-trait contradiction | Added Lab ACP Runtime section; fixed async-trait guidance to match examples |
| plex/tautulli arrs skills | Wrong script paths (`./skills/<svc>/scripts/`) | Correct relative paths (`./scripts/`) — scripts now callable |
| cortex main skill | Missing trigger phrases for anomalies, silent hosts, AI transcript correlation | Added all missing triggers + 3 new workflow subsections |
| paperless-ngx | Coercive MANDATORY block with warning emoji and all-caps threats | Clean `# Paperless-ngx` heading with content starting at `## Purpose` |

## Risks and Rollback

- All changes are to documentation/skill files only — no Rust source, no tests, no config. Risk is low.
- Workspace repo changes (cortex, rustarr, etc.) are in separate git repos and have not been committed yet. If any change is incorrect, `git checkout -- <file>` in the respective repo is sufficient.
- The lab `plugins/` changes are unstaged — `git checkout -- plugins/` would revert all in-repo skill changes.

## Next Steps

- **Commit lab `plugins/` changes.** Stage and commit the 17 modified files in `plugins/` with a descriptive message like `docs(skills): skill quality sweep — fix paths, descriptions, and content gaps`.
- **Commit workspace repo changes.** Each rmcp repo (cortex, rustarr, rustcane, rustifi, rustify, rustscale, unrust, synapse2, apprise-mcp, rmcp-template) has modified SKILL.md files that need their own commits and pushes.
- **Investigate p1-fixes worktree.** The worktree at `~/workspace/lab-p1-fixes` is dirty with ACP-related files. Determine if this in-progress work should be committed, stashed, or merged.
- **Address apprise plugin.json divergence.** The `.claude/` mirror plugin.json has a different `name` and no `userConfig` vs the primary — noted as out-of-scope but should be aligned separately.
- **Run `/reload-plugins`** after pushing lab plugins changes to pick up the updated skill files in the active Claude Code session.
