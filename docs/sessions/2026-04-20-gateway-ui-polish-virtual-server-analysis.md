---
date: 2026-04-20 18:27:42 EST
repo: git@github.com:jmagar/lab.git
branch: fix/auth
head: 24327d2
agent: Claude (claude-sonnet-4-6)
session_id: 2fa8bbe9-64d7-4a3a-976e-d645c14c8fb8
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/2fa8bbe9-64d7-4a3a-976e-d645c14c8fb8.jsonl
working_directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  24327d2 [fix/auth]
pr: "#25 — fix(auth): gateway admin auth, upstream OAuth, and dispatch fixes — https://github.com/jmagar/lab/pull/25"
---

## User Request

Fix a misleading probe error message shown for the qdrant virtual server, then extend the fix to all lab services generically. Followed by a series of gateway list view and detail view UI polish requests, architectural analysis of the virtual server system, and config cleanup.

---

## Session Overview

- Fixed misleading MCP error message for lab virtual service health failures (root cause: `humanizeProbeError` applied indiscriminately to `lab_service` warnings)
- Polished gateway list view: removed status text labels, simplified warning badge, updated lab server endpoint display
- Documented a comprehensive gateway detail page redesign spec and created 9 tracking beads
- Identified and analyzed the virtual server architecture as fundamentally broken; designed the replacement (in-process MCP peers)
- Fixed `~/.labby/config.toml`: removed recursive plex upstream, fixed github-chat missing args
- Killed 26 orphaned lab processes left from the infinite recursion bug

---

## Sequence of Events

1. Continued from a prior session that had identified the root cause of a misleading probe error on the qdrant virtual server
2. Implemented the error message fix in `gateway-adapter.ts` — added `extractReqwestUrl`, `humanizeLabServiceHealthError`, branched `normalizeServerView` on `view.source === 'lab_service'`
3. Removed "HEALTHY"/"DISCONNECTED" text labels from gateway list rows (both mobile and desktop), keeping only the status dot with `aria-label`/`title`
4. Simplified warnings badge from `N issue(s)` to just `N`
5. Updated lab virtual server endpoint preview from `"<name> virtual server"` to `lab serve mcp --stdio --services <name>`
6. User shared screenshots of the gateway detail page; discussed multiple UI improvements
7. Wrote comprehensive redesign spec to `apps/gateway-admin/docs/gateway-detail-redesign.md`
8. Discussed why virtual servers don't expose resources/prompts (hardcoded zeros, no MCP handshake)
9. Discussed in-process MCP transport as a replacement for the virtual server hack
10. User directed: all services should be first-class in-process MCP peers, no virtual server layer
11. Created 9 beads covering all UI changes and the architecture refactor
12. User shared server logs showing infinite recursion (lab spawning itself for plex gateway) and 20+ orphaned processes
13. Identified two bugs in logs: infinite recursion (plex upstream), github-chat empty args, discovery running 3-4x
14. Removed plex upstream from `~/.labby/config.toml`, fixed github-chat args to `["github-chat-mcp"]`
15. Force-killed all 26 orphaned `target/debug/lab` processes

---

## Key Findings

- `gateway-adapter.ts:297` — `humanizeProbeError` was called on ALL `view.warnings` in `normalizeServerView`, including `lab_service` warnings whose raw error strings contain `url (...)` from reqwest, triggering the wrong MCP-specific message
- `manager.rs:1744-1747` — `discovered_resource_count`, `exposed_resource_count`, `discovered_prompt_count`, `exposed_prompt_count` are hardcoded to `0` for all virtual servers
- `manager.rs:1718-1719` — virtual server tool counts read from `virtual_server_tool_registry()` (local ToolRegistry), not via MCP discovery
- `~/.labby/config.toml:68-71` — plex upstream was configured as `command = "lab" args = ["serve", "--services", "radarr", "mcp", "--stdio"]`, causing lab to spawn itself recursively; 20 generations spawned before detection
- `~/.labby/config.toml:62-64` — github-chat had `args = []`, meaning `uvx` ran with no arguments, printed help text to stdout, causing serde parse failure on the MCP initialize response
- `pool.rs` — custom upstreams run full MCP handshake (initialize + list_tools + list_resources + list_prompts); virtual servers do not
- Discovery running 3-4x per server in logs — likely a reprobe timer stacking bug, not investigated this session

---

## Technical Decisions

- **`humanizeLabServiceHealthError` as a separate function** rather than patching `humanizeProbeError` — the two cases are semantically different (REST health check vs MCP initialize), and mixing them would require passing transport type through the call chain
- **`extractReqwestUrl` shared helper** — both humanizers need the `url (...)` regex; extracting it eliminates duplication and makes the difference in message explicit
- **Branch in `normalizeServerView` on `view.source === 'lab_service'`** — `source` is already available, clean discriminator, no new fields needed
- **In-process MCP peers over self-probing** — for virtual servers, static introspection (reading from the registry) was considered but rejected in favor of full in-process MCP transport so all lifecycle management (circuit breaker, reconnect, health) applies uniformly
- **Remove plex upstream entirely** — rather than add self-detection logic, just remove the config entry since the virtual server already covers plex

---

## Files Modified

| File | Change |
|------|--------|
| `apps/gateway-admin/lib/server/gateway-adapter.ts` | Added `extractReqwestUrl()`, `humanizeLabServiceHealthError()`; updated `normalizeServerView` to branch on `view.source`; deduplicated regex in `humanizeProbeError` |
| `apps/gateway-admin/components/gateway/gateway-table.tsx` | Removed status text label spans from mobile and desktop rows; moved label to `aria-label`/`title` on dot |
| `apps/gateway-admin/components/gateway/warnings-pill.tsx` | Changed `{N} issue(s)` to `{N}` |
| `apps/gateway-admin/lib/api/gateway-mobile.ts` | Changed lab service endpoint preview from `"<name> virtual server"` to `` `lab serve mcp --stdio --services ${gateway.name}` `` |
| `apps/gateway-admin/docs/gateway-detail-redesign.md` | New file — comprehensive redesign spec for the gateway detail page |
| `~/.labby/config.toml` | Removed plex upstream block; fixed github-chat `args: [] → ["github-chat-mcp"]` |

---

## Commands Executed

```bash
# Killed all orphaned lab processes
ps aux | grep 'target/debug/lab' | grep -v grep | awk '{print $2}' | xargs kill -9

# Verified clean
ps aux | grep 'target/debug/lab' | grep -v grep
# → no output

# Checked worktree state
git worktree list
# ~/workspace/lab 403d790 [fix/auth]
# ~/workspace/lab/.worktrees/fix-auth b858682 [fix/auth-work]
```

---

## Errors Encountered

**Infinite recursion — lab spawning itself**
- Root cause: `config.toml` had `[[upstream]] name = "plex" command = "lab" args = ["serve", "--services", "radarr", "mcp", "--stdio"]`
- Each lab instance read the config and spawned another lab instance for the plex gateway
- 20 `lab serve --services radarr mcp --stdio` processes accumulated
- Resolution: removed the plex upstream block from config; the virtual server already serves plex

**github-chat serde error on initialize**
- Root cause: `args = []` caused `uvx` to run with no target, printing help text to stdout instead of MCP JSON
- Resolution: changed args to `["github-chat-mcp"]`

**`bd create --prefix` flag rejected**
- `--prefix` caused `cannot use --rig: no routes.jsonl found` error
- Resolution: omit `--prefix`; rig is inferred from CWD automatically

---

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| Lab service health error message | "Could not connect to http://127.0.0.1:53333. The upstream did not complete the MCP initialize request." | "Could not reach qdrant at http://127.0.0.1:53333/healthz. Verify the service is running and the URL is correct." |
| Gateway list row status | `● HEALTHY plex` / `● DISCONNECTED github-chat` | `● plex` / `● github-chat` (dot only, label in title/aria-label) |
| Warnings badge | `⚠ 1 issue` / `⚠ 2 issues` | `⚠ 1` / `⚠ 2` |
| Lab virtual server endpoint text | `apprise virtual server` | `lab serve mcp --stdio --services apprise` |
| plex upstream on startup | Spawns recursive lab child processes, 20+ orphans | Removed; virtual server handles plex natively |
| github-chat on startup | `uvx` with no args → serde error, discovery fails | `uvx github-chat-mcp` → proper MCP handshake |

---

## Risks and Rollback

- `gateway-adapter.ts` change: if a `lab_service` warning message contains text that `humanizeLabServiceHealthError` doesn't handle, it falls back to `return message` (the raw string) — same as before, no regression
- `config.toml` changes are outside the repo; rollback is manual re-addition of the plex upstream block and clearing github-chat args

---

## Decisions Not Taken

- **Static resource/prompt counts for virtual servers** — reading `mcp/resources.rs` and `mcp/prompts.rs` directly to populate the hardcoded zeros. Rejected in favor of the full in-process MCP peer architecture (lab-jt87), which solves the same problem more completely and uniformly
- **Self-detection in the upstream pool** — detecting when `command = "lab"` and routing to in-process transport instead. Rejected as a short-term hack; the proper fix is lab-jt87
- **Fixing the 3-4x discovery repetition** — observed in logs but not investigated; left as an open question

---

## References

- Bead `lab-jt87` — Arch: replace virtual server layer with per-service in-process MCP peers
- `apps/gateway-admin/docs/gateway-detail-redesign.md` — full detail page redesign spec

---

## Open Questions

- Why does discovery run 3-4 times per upstream per startup? Likely reprobe timer stacking in `pool.rs` but not confirmed
- Does `tokio::io::duplex` perform well enough for high-frequency tool calls through in-process MCP peers, or is there meaningful overhead vs direct dispatch?
- Should the `lab_service` source type be renamed to `in_process` once lab-jt87 lands, or does the distinction matter to frontend consumers?

---

## Next Steps

**Unfinished from this session:**
- None — all implemented changes are complete

**Follow-on tasks (beads created):**
- `lab-i8fo` — Move tabs under header, merge SurfaceRatio counts into tab triggers
- `lab-5b72` — Config tab for client JSON (remove from main card)
- `lab-fu9t` — Remove probe result banner; tooltip on warning badge
- `lab-mxpi` — Move surface toggles (Offline/Expose resources/CLI/API/MCP/WEBUI) to AppHeader strip
- `lab-hkm5` — Prompts tab: expandable cards with arguments and description
- `lab-qzio` — Tools vs Actions label differentiation for lab virtual servers
- `lab-bncy` — Fix recursive resource URI construction in `dispatch/upstream/pool.rs`
- `lab-2b8u` — Server name collision detection (backend 409 + frontend inline error)
- `lab-jt87` — Replace virtual server layer with per-service in-process MCP peers (architecture refactor)
