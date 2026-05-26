---
date: 2026-05-25 14:09:00 EDT
repo: git@github.com:jmagar/lab.git
branch: main
head: df097f26
session id: 9f45d408-3929-41c9-b7ee-95c456de0a33
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/9f45d408-3929-41c9-b7ee-95c456de0a33.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
beads: lab-hjhnu, lab-hjhnu.1, lab-hjhnu.2, lab-hjhnu.3
---

# Lab rmcp-template extraction planning session

## User Request

Explore `../lab` and create Lavra-backed plans to spin Gateway, the unified ACP/MCP Registry plus Claude/Codex Marketplace, and ACP Chat into three separate fresh clones of `rmcp-template`, each with a web UI.

## Session Overview

Created the extraction epic and child beads, documented research and engineering-review findings, wrote three implementation plans, corrected the plans to be copy-only from Lab, and expanded the Gateway plan with an explicit MCP surface extraction task.

## Sequence of Events

1. Used the Lavra and writing-plans workflows to inspect Lab's existing Gateway, Marketplace/Registry, and ACP Chat surfaces.
2. Created the Beads epic `lab-hjhnu` and child beads `lab-hjhnu.1`, `lab-hjhnu.2`, and `lab-hjhnu.3`.
3. Wrote extraction research and engineering-review notes under `.lavra/research/`.
4. Created three implementation plans under `docs/superpowers/plans/`.
5. Corrected the plans and Beads comments after the user clarified that Lab should remain intact and the work should copy only needed code into fresh clones.
6. Expanded the Gateway extraction plan after the user asked what the plan covered for MCP.
7. Ran a save-to-md maintenance pass and wrote this session note.

## Key Findings

- Gateway is already a distinct vertical with dispatch, CLI, API, MCP, docs, and web UI code. The Gateway plan now includes explicit MCP source inventory and an MCP-specific task in `docs/superpowers/plans/2026-05-25-extract-gateway-server.md`.
- Marketplace and registry code spans Lab dispatch plus `lab-apis` modules for marketplace, MCP Registry, and ACP Registry.
- ACP Chat has the deepest UI/runtime footprint and depends on Lab's vendored `agent-client-protocol` patch.
- The extraction direction is copy-only: Lab remains unchanged and is not rewired to call the new services.

## Technical Decisions

- Use three fresh `rmcp-template` clones: `/home/jmagar/workspace/lab-gateway`, `/home/jmagar/workspace/lab-marketplace-registry`, and `/home/jmagar/workspace/lab-acp-chat`.
- Treat all three as platform servers with CLI, MCP, HTTP API, and embedded web UI surfaces.
- Give each clone its own env prefix, port, scopes, and appdata root.
- Keep Marketplace Registry as the provider/catalog owner; ACP Chat consumes provider data over HTTP/API or a static snapshot instead of compile-time coupling.
- Preserve Gateway `scout` and `invoke` as the primary MCP vocabulary, with legacy aliases only where Lab already has them.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| created | `.lavra/research/2026-05-25-rmcp-extraction-research.md` | - | Captured source inventory and extraction conclusions. | File exists under ignored `.lavra/`; Beads comments reference it. |
| created | `.lavra/research/2026-05-25-rmcp-extraction-eng-review.md` | - | Captured engineering-review constraints and corrections. | File exists under ignored `.lavra/`; Beads comments reference it. |
| created | `docs/superpowers/plans/2026-05-25-extract-gateway-server.md` | - | Gateway fresh-clone extraction plan. | Present in `docs/superpowers/plans/`. |
| created | `docs/superpowers/plans/2026-05-25-extract-marketplace-registry-server.md` | - | Marketplace Registry fresh-clone extraction plan. | Present in `docs/superpowers/plans/`. |
| created | `docs/superpowers/plans/2026-05-25-extract-acp-chat-server.md` | - | ACP Chat fresh-clone extraction plan. | Present in `docs/superpowers/plans/`. |
| modified | `docs/superpowers/plans/2026-05-25-extract-gateway-server.md` | - | Added explicit MCP Gateway source inventory, task, and smoke checks. | `git diff` shows MCP task insertion. |
| created | `docs/sessions/2026-05-25-lab-rmcp-extraction-plans.md` | - | This session handoff. | Written by save-to-md. |

## Beads Activity

| bead | title | actions | final status | why it mattered |
|---|---|---|---|---|
| `lab-hjhnu` | Extract Gateway, Marketplace Registry, and ACP Chat into rmcp-template servers | Created epic; commented with research/review artifacts; commented with copy-only correction. | open | Tracks the cross-repo extraction wave. |
| `lab-hjhnu.1` | Extract Gateway into standalone rmcp-template server with web UI | Created child; commented with Gateway constraints; commented with copy-only correction. | open | Tracks Gateway clone planning. |
| `lab-hjhnu.2` | Extract unified Marketplace Registry into standalone rmcp-template server with web UI | Created child; commented with Marketplace Registry constraints; commented with copy-only correction. | open | Tracks unified registry/marketplace clone planning. |
| `lab-hjhnu.3` | Extract ACP Chat into standalone rmcp-template server with web UI | Created child; commented with ACP Chat constraints; commented with copy-only correction. | open | Tracks ACP Chat clone planning. |

## Repository Maintenance

### Plans

Checked `docs/plans/` and `docs/superpowers/plans/`. No plan was moved to `docs/plans/complete/`: `docs/plans/fleet-ws-plan-lab-n07n.md` and `docs/plans/mcp-streamable-http-oauth-proxy.md` were not part of this session, and the three extraction plans are active.

### Beads

Read `bd show lab-hjhnu --json` and comments for the epic and child beads. Added correction comments that this wave is copy-only and should not include Lab source removal, forwarding adapters, or runtime rewiring.

### Worktrees and branches

Checked `git worktree list --porcelain` and `git branch -vv`. Only the current worktree `/home/jmagar/workspace/lab` on `main` was registered in the observed output. No branch or worktree cleanup was performed.

### Stale docs

Updated the Gateway extraction plan after the user flagged MCP coverage as incomplete. No broader stale-doc sweep was attempted because the request was to save the session and the extraction plans remain active.

### Skipped items

Did not move or delete any existing untracked session files. Did not run full test suites because the work was planning/documentation only.

## Tools and Skills Used

- **Skills.** Used `lavra-plan`, `lavra-research`, `lavra-eng-review`, `writing-plans`, and `save-to-md`.
- **Shell commands.** Used `rg`, `find`, `sed`, `git`, `bd`, `date`, and `ls` for repo exploration, tracker updates, verification, and session metadata.
- **File tools.** Used `apply_patch` to create and modify markdown/research files.
- **Beads CLI.** Used `bd create`, `bd comment`, `bd show`, `bd comments`, and `bd dolt status`.
- **Subagents.** None spawned; the work was done locally.
- **Browser tools.** None used.

## Commands Executed

| command | result |
|---|---|
| `rg --files crates/lab/src crates/lab-apis/src apps/gateway-admin/app apps/gateway-admin/components apps/gateway-admin/lib docs \| rg '(gateway|marketplace|registry|mcpregistry|acp|chat)'` | Produced the first-party source inventory for the three extraction plans. |
| `bd create ...` | Created epic `lab-hjhnu` and child beads `lab-hjhnu.1`, `.2`, `.3`. |
| `bd comment ...` | Added research/review and copy-only correction comments to the epic and child beads. |
| `rg -n "compatibility adapter|forwarding adapter|source removal|..." ...` | Verified only explicit negative guardrails remained after copy-only correction. |
| `rg -n "MCP|mcp|tool|schema|resource|stdio|Streamable|transport" docs/superpowers/plans/2026-05-25-extract-gateway-server.md` | Confirmed the initial Gateway plan needed stronger MCP detail. |
| `git diff -- docs/superpowers/plans/2026-05-25-extract-gateway-server.md` | Confirmed the final MCP-focused plan edit. |
| `git status --short` | Showed `docs/superpowers/plans/2026-05-25-extract-gateway-server.md` modified at save time. |

## Errors Encountered

- A broad search over Lab docs also swept generated reference docs and produced noisy output. The search was narrowed to first-party crates and gateway-admin files.
- The first plan pass included Lab compatibility adapters and future source-removal framing. The user corrected the direction, and the plans plus Beads comments were updated to copy-only extraction.
- The Gateway plan initially mentioned MCP only as parity; the user asked about MCP, and a dedicated MCP Gateway task was added.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Extraction tracking | No dedicated epic for the three fresh-clone extractions. | `lab-hjhnu` and three child beads track the extraction wave. |
| Extraction docs | No standalone plans for the three rmcp-template clones. | Three active implementation plans exist under `docs/superpowers/plans/`. |
| Copy-only policy | Initial notes included Lab compatibility adapters and source-removal language. | Plans and Beads comments state Lab remains unchanged and the clones copy only needed code. |
| Gateway MCP coverage | Gateway plan only mentioned MCP parity at a high level. | Gateway plan has an explicit MCP surface task with transports, tools, resources, proxying, auth/scope rules, and tests. |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `test -s <created files>` | All research and plan files exist. | Printed `created files present`. | pass |
| `rg -n "TODO|TBD|placeholder" ...` | No placeholder markers in created docs. | No matches. | pass |
| `rg -n "compatibility adapter|forwarding adapter|source removal|..." ...` | No positive adapter/removal plan language remains. | Only negative guardrails and copy-only statements matched. | pass |
| `rg -n "MCP Gateway|stdio|Streamable|scout|invoke|lab://gateway|..." docs/superpowers/plans/2026-05-25-extract-gateway-server.md` | Gateway plan explicitly covers MCP. | Matches showed the new MCP task and smoke checks. | pass |

## Risks and Rollback

- `.lavra/research/*` is ignored by git in this repo, so durable tracked context is in the Beads comments and `docs/superpowers/plans/*`.
- The only tracked file modified at save time is the Gateway extraction plan. Rollback is `git checkout -- docs/superpowers/plans/2026-05-25-extract-gateway-server.md` if that edit is unwanted.
- The Beads comments are append-only historical corrections; do not delete them unless intentionally rewriting tracker history.

## Decisions Not Taken

- Did not create the three fresh clones yet; this session produced the tracked implementation plans.
- Did not rewire Lab to call the extracted services; the user explicitly clarified that Lab should remain unchanged.
- Did not remove Lab source code; the extraction is copy-only.
- Did not run implementation test suites because no runtime code was changed.

## References

- `docs/superpowers/plans/2026-05-25-extract-gateway-server.md`
- `docs/superpowers/plans/2026-05-25-extract-marketplace-registry-server.md`
- `docs/superpowers/plans/2026-05-25-extract-acp-chat-server.md`
- `.lavra/research/2026-05-25-rmcp-extraction-research.md`
- `.lavra/research/2026-05-25-rmcp-extraction-eng-review.md`
- Beads: `lab-hjhnu`, `lab-hjhnu.1`, `lab-hjhnu.2`, `lab-hjhnu.3`

## Open Questions

- Final repo and binary names are still proposed, not confirmed: `lab-gateway`, `lab-marketplace-registry`, and `lab-acp-chat`.
- The `.lavra/research` artifacts are ignored; decide whether a tracked research summary should be added if Beads comments are not enough.

## Next Steps

1. Review and confirm the proposed repo names, ports, env prefixes, and scopes in the three plans.
2. Start with `docs/superpowers/plans/2026-05-25-extract-gateway-server.md` and create the fresh `lab-gateway` clone.
3. Keep the extraction copy-only: copy required code from Lab into the new repo and leave Lab runtime/source intact.
4. After Gateway is validated, proceed to Marketplace Registry, then ACP Chat.
