---
date: 2026-05-04 13:45:41 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 5743e804
plan: none
agent: Claude (claude-sonnet-4-6)
session id: c090271c-28fc-4e25-a9d8-84bc82888c41
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/c090271c-28fc-4e25-a9d8-84bc82888c41.jsonl
working directory: /home/jmagar/workspace/lab
pr: "40 — Integrate service wave and CI updates — https://github.com/jmagar/lab/pull/40"
---

## User Request

Investigate and fix why ACP sessions don't survive container restarts; then investigate why sessions stop mid-work in the chat UI; then do a thorough polishing and design-system-contract alignment pass on the chat page.

## Session Overview

Three major work streams: (1) root-cause investigation and fix for ACP session persistence across container restarts, (2) root-cause investigation of premature session termination during agent work, and (3) a comprehensive frontend polishing and Aurora design system alignment pass across the chat components.

## Sequence of Events

1. Systematic debugging of ACP session persistence — traced the data flow from write path (SQLite) through `AcpSessionRegistry` and confirmed the read path (in-memory map) is never rehydrated on startup
2. Confirmed `LAB_ACP_HMAC_SECRET` was already set in `~/.labby/.env` — secondary root cause pre-fixed
3. Implemented `restore_from_db()` on `AcpSessionRegistry`: added `Session::new_with_seq`, `SqliteAcpPersistence::load_max_seqs()`, and wired the restore call in `cli/serve.rs`
4. Identified three additional gaps and fixed them: SSE no-reconnect, `registry.rs.bak` cleanup, `idle_completion` UI invisibility
5. Investigated chat session stopping mid-work via screenshot; traced to 5-second `PROMPT_IDLE_TIMEOUT` firing during codex-acp's internal tool execution
6. Fixed `docker-compose.yml` with `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS: 30000` and `LAB_ACP_TURN_DRAIN_TIMEOUT_MS: 15000`
7. Ran frontend polishing pass on chat components; applied design system tokens and visual improvements
8. Audited all changes against `docs/design/design-system-contract.md`; found and fixed remaining violations in the same pass and a follow-up sweep

## Key Findings

- `AcpSessionRegistry::new()` at `crates/lab/src/acp/registry.rs:139` always starts with `HashMap::new()` and has no restore path — sessions written to SQLite are never loaded back
- `SqliteAcpPersistence::load_sessions()` existed and worked correctly at `crates/lab/src/dispatch/acp/persistence.rs:251` but was never called at startup
- `next_seq` is initialized to 1 in `Session::new` — without seeding to `MAX(seq)+1` from the DB, new events after restore would collide with the existing `UNIQUE(session_id, seq)` index
- The Docker bind mount `${HOME}/.labby:/home/labby/.lab` covers `acp.db` — the database physically survives restarts but the in-memory map doesn't
- `DEFAULT_PROMPT_IDLE_TIMEOUT = 5s` at `crates/lab/src/acp/runtime.rs:49` fires during codex-acp's internal shell execution (`find`, `cat` etc.) which produces no ACP protocol events for the idle timer to reset against
- `DEFAULT_TURN_DRAIN_TIMEOUT = 300s` at `runtime.rs:70` causes the next prompt after an idle-completion to stall silently for up to 5 minutes
- `use-session-events.ts` SSE loop has no reconnect: after `done` or error it just stops — server restarts leave the UI silently stale
- `shadow-[var(--aurora-highlight-soft)]` in `message-thread.tsx` referenced a non-existent CSS token
- Non-Aurora colors (`emerald-`, `sky-`, `cyan-`, `rose-`, `orange-`, `pink-`, `destructive`) used in `tool-call-display.tsx` and `workspace-picker.tsx`
- Raw `rgba(0,0,0,0.24)` in `chat-input.tsx` shadow strings instead of `var(--aurora-shadow-medium)`
- Multiple `overflow-y-auto` / `overflow-x-auto` containers missing required `aurora-scrollbar` class
- Hand-rolled eyebrow (`text-[10px] font-semibold uppercase tracking-[0.18em]`) in empty state instead of `AURORA_MUTED_LABEL`
- Empty state title using `text-[15px] font-medium` instead of `AURORA_CARD_TITLE` (missing Manrope display font)

## Technical Decisions

- **`next_seq` seeding via bulk `MAX(seq)` query** — one SQL `GROUP BY` round-trip instead of N per-session queries; avoids UNIQUE constraint violations on event insert after restore
- **In-flight session synthetic events** — sessions that were `Running` or `WaitingForPermission` at shutdown get `SessionUpdate{Failed}` + `ProviderInfo{container_restart}` events written to both SQLite and in-memory so SSE replay shows a clean terminal transition
- **Sessions with no `principal` skipped on restore** — they cannot be accessed via `check_principal` anyway; silent skip avoids inserting dead-weight entries into the map
- **30s idle timeout** — gives tool chains room to execute (vs 5s default); still short enough to catch truly hung sessions
- **15s drain timeout** — replaces the 5-minute default; caps the next-prompt stall at a tolerable 15 seconds
- **SSE exponential backoff** — 1s → 2s → 4s → 8s → 30s cap; resets to 1s on successful open so post-restart reconnects are fast
- **`idle_completion` → `kind: 'idle_completion'`** in session-events normalizer — explicit case instead of falling through to the opaque `debug` default; `IdleCompletionCard` uses `aurora-warn` tokens
- **Non-Aurora category colors replaced with Aurora semantic equivalents** — `emerald→success`, `sky/cyan→accent-primary`, `orange→warn`, `pink→accent-deep`; no new tokens needed
- **Diff line colors** (`text-emerald-300`, `text-rose-300`) → `text-aurora-success`, `text-aurora-error` — semantically correct and contract-compliant

## Files Modified

### Rust (backend)
- `crates/lab/src/acp/registry.rs` — added `Session::new_with_seq`; added `restore_from_db()` method with bulk seq query, synthetic failure events for in-flight sessions, and map population
- `crates/lab/src/dispatch/acp/persistence.rs` — added `SqliteAcpPersistence::load_max_seqs()` non-trait method (bulk `MAX(seq) GROUP BY` query)
- `crates/lab/src/cli/serve.rs` — added `acp_registry.restore_from_db().await` after `install_registry`
- `crates/lab/src/acp/registry.rs.bak` — **deleted** (stale development artifact)

### Docker / Config
- `docker-compose.yml` — added `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS: 30000`, `LAB_ACP_TURN_DRAIN_TIMEOUT_MS: 15000`; `init: true` also added by linter

### Frontend — Chat UI
- `apps/gateway-admin/lib/chat/use-session-events.ts` — rewrote single-shot SSE connect to `while (!aborted)` loop with exponential backoff reconnect
- `apps/gateway-admin/lib/chat/session-events.ts` — added explicit `case 'idle_completion'` in `provider_info` switch with title extraction
- `apps/gateway-admin/components/chat/activity-card.tsx` — added `IdleCompletionCard` component; added `idle_completion` dispatch case; fixed amber colors → `aurora-warn` tokens; added `Clock` icon import
- `apps/gateway-admin/components/chat/message-bubble.tsx` — replaced "U"/"A" letter avatars with `UserRound`/`Bot` lucide icons; added 2px left accent strip on assistant messages; fixed `bg-aurora-accent-deep/20` → `/18`
- `apps/gateway-admin/components/chat/message-thread.tsx` — fixed non-existent `--aurora-highlight-soft` token; redesigned `SessionStatusNotice` (spinner + compact single-line); fixed empty-state to use `AURORA_MUTED_LABEL` + `AURORA_CARD_TITLE`; added `cn`, `Loader2`, `ShieldQuestion`, `AURORA_CARD_TITLE`, `AURORA_MUTED_LABEL` imports; fixed `/20`→`/18`, `/10`→`/12` tints
- `apps/gateway-admin/components/chat/session-sidebar.tsx` — selected run: 2px left accent strip; running icon: `animate-ping` halo; added `waiting_for_permission` state; removed wrong `AURORA_MUTED_LABEL` from timestamp; used `AURORA_DENSE_META` + `tabular-nums`; fixed ping halo `/25`→`/30`
- `apps/gateway-admin/components/chat/chat-input.tsx` — replaced raw `rgba(0,0,0,0.24)` shadow strings with `var(--aurora-shadow-medium)`
- `apps/gateway-admin/components/floating-chat-shell.tsx` — added connection state pill (connecting/error) to header; Zap icon dims when provider unavailable; fixed `/5`→`/12`, `/20`→`/18`, `/10`→`/12` tints
- `apps/gateway-admin/components/chat/tool-call-display.tsx` — replaced all non-Aurora category colors with Aurora equivalents; replaced diff line colors; fixed `/50`→`/40` on link hover; fixed `text-aurora-success/70`→`text-aurora-success`
- `apps/gateway-admin/components/chat/tool-artifact-panels.tsx` — fixed `/10`→`/12` on success/error/warn bg tints; added `aurora-scrollbar` to `overflow-auto` terminal output container
- `apps/gateway-admin/components/chat/activity-debug-card.tsx` — added `aurora-scrollbar` to `<pre>` element
- `apps/gateway-admin/components/chat/activity-review-card.tsx` — added `aurora-scrollbar` to `<pre>` element
- `apps/gateway-admin/components/chat/activity-status-card.tsx` — added `aurora-scrollbar` to both `<pre>` elements
- `apps/gateway-admin/components/chat/workspace-picker.tsx` — replaced `destructive` shadcn token with `aurora-error` Aurora token; added `aurora-scrollbar` to file browser container
- `apps/gateway-admin/components/chat/settings-panel.tsx` — added `aurora-scrollbar` to `overflow-y-auto` panel

## Commands Executed

```bash
# Rust build verification (after each change)
rtk cargo check --all-features

# TypeScript build verification (after each change)
cd apps/gateway-admin && rtk tsc --noEmit

# HMAC key verification
grep -i "LAB_ACP_HMAC_SECRET" ~/.labby/.env
# → LAB_ACP_HMAC_SECRET=2e5bc11c3e0a9cf8c4082c50340eca66b20d19a37ffa600ad084d663badaf3e0

# Non-Aurora color audit
rtk grep -rn "rgba\|emerald-|sky-500|cyan-500|rose-300|pink-500|orange-500|destructive" \
  apps/gateway-admin/components/chat --include="*.tsx"
# → No matches (after fixes)

# stale artifact cleanup
rm crates/lab/src/acp/registry.rs.bak
```

## Behavior Changes (Before/After)

| Area | Before | After |
|---|---|---|
| ACP sessions after restart | All sessions lost — registry starts empty | Sessions restored from SQLite; in-flight sessions marked Failed with synthetic events |
| `next_seq` after restore | Would start at 1, collide with existing seq index | Seeded to `MAX(persisted_seq) + 1` per session |
| Session idle timeout | 5s — fires during any tool execution >5s | 30s — accommodates multi-step tool chains on Docker-mounted volumes |
| Next-prompt stall after idle-completion | Up to 5 minutes silent drain | Capped at 15 seconds |
| SSE stream after server restart | Silently stale — no reconnect | Reconnects with 1s→30s exponential backoff; resumes from last `seq` |
| Idle completion in UI | Silent — looks identical to normal completion | Amber-bordered `IdleCompletionCard` with Clock icon and "Send a message to continue" |
| Session sidebar: running state | Static Sparkles icon | Sparkles + `animate-ping` halo ring |
| Session sidebar: selected state | Blue glow only | 2px left accent strip (matches message bubble assistant treatment) |
| Message avatars | "U" / "A" letter badges | `UserRound` / `Bot` lucide icons |
| Assistant message bubbles | No visual distinction from user | 2px `aurora-accent-primary/40` left strip |
| Empty state typography | Hand-rolled `text-[10px]` eyebrow + `text-[15px] font-medium` | `AURORA_MUTED_LABEL` + `AURORA_CARD_TITLE` (Manrope display font) |
| Category tint colors in tool display | Non-Aurora (`emerald-`, `sky-`, etc.) | Aurora semantic equivalents (`aurora-success`, `aurora-accent-primary`, `aurora-warn`) |
| `overflow-auto` containers | Missing `aurora-scrollbar` in 7 places | `aurora-scrollbar` applied consistently |
| Raw rgba in shadows | `rgba(0,0,0,0.24)` in chat-input className | `var(--aurora-shadow-medium)` |
| `destructive` token in workspace-picker | shadcn token (drifts in light mode) | `aurora-error` Aurora token |

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `rtk cargo check --all-features` | 0 errors, 0 warnings | 0 errors, 0 warnings | ✅ |
| `cd apps/gateway-admin && rtk tsc --noEmit` | Clean | `TypeScript compilation completed` | ✅ |
| `grep LAB_ACP_HMAC_SECRET ~/.labby/.env` | Secret present | 64-char hex key found | ✅ |
| Non-Aurora color grep on chat components | No matches | No matches | ✅ |
| `registry.rs.bak` existence check | Deleted | File absent | ✅ |

## Risks and Rollback

- **`restore_from_db` on every startup** — adds one async SQLite round-trip plus N max-seq queries at serve time. On a very large session database this adds latency to `labby serve`. Rollback: remove the `acp_registry.restore_from_db().await` call in `serve.rs:361`.
- **Idle timeout increase (5s → 30s)** — a stuck provider process will now hold a session in `Running` state for up to 30 seconds before the idle timer fires. The 30-minute idle reaper still cleans up eventually. Rollback: remove `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` from docker-compose or set it back to 5000.
- **SSE exponential backoff** — a server that is permanently down will cause the frontend to loop forever at the 30s cap. The loop exits on `abortController.abort()` (unmount / session deselect). No server-side risk.

## Decisions Not Taken

- **Restoring `events` in-memory buffer from SQLite** — the advisor correctly noted "don't preload events into memory"; `Session.events` is a fallback cache only, SQLite is the source of truth. Restoring the full event history into memory would waste RAM for long sessions.
- **Setting `LAB_ACP_PROMPT_IDLE_TIMEOUT_MS` higher (60s+)** — 30s was chosen as the sweet spot; 60s would let clearly-hung sessions accumulate for too long.
- **Adding a new `interrupted` session status** — mapping `idle_completion` to the existing `completed` status + a distinct activity card kind is simpler and avoids a schema migration.
- **Migrating all opacity modifiers to `color-mix()`** — the contract states this as a future migration direction, not an immediate requirement. Existing `/30`, `/40` named-scale suffixes were kept as-is since `color-mix()` in Tailwind classNames requires `[color-mix(...)]` arbitrary value syntax, which is less readable.

## Open Questions

- **`agents.length > 0 ? agents : [selectedAgent]` in `chat-shell.tsx` and `floating-chat-shell.tsx`** — when `selectedAgent` is `null`, this creates `[null]` which is `ACPAgent[]` (TypeScript coerces it). The agent picker would render a null agent. Pre-existing; not fixed this session.
- **Light-mode verification** — the contract requires dark+light screenshots in `/design-system` before shared components are considered complete. The chat surface changes (new `IdleCompletionCard`, revised `SessionStatusNotice`, icon avatars) have not been verified in light mode.
- **`overflow-auto` containers outside chat** — `components/registry/server-detail-panel.tsx`, `components/nodes/node-log-stream.tsx`, `components/ai/*`, `components/marketplace/*` all have `overflow-auto` / `overflow-y-auto` without `aurora-scrollbar`. Deferred to a separate sweep.
- **`plugin-info-panel.tsx` inline scrollbar style** — uses bespoke `[scrollbar-width:thin] [scrollbar-color:...]` instead of `aurora-scrollbar`. Deferred.

## Next Steps

### Incomplete from this session
- Light-mode verification of chat UI changes in `/design-system` sandbox (required by contract before rollout)

### Follow-on tasks
- `agents.length > 0 ? agents : [selectedAgent]` null guard — when `selectedAgent` is null, filter it out before passing to `ChatInput`
- Broader `aurora-scrollbar` sweep across `components/registry/`, `components/nodes/`, `components/ai/`, `components/marketplace/`
- `plugin-info-panel.tsx` inline scrollbar style → migrate to `aurora-scrollbar`
- Broader opacity tint migration to `color-mix()` per contract's stated direction
- Container tests: verify `restore_from_db` correctness with a real `docker compose restart` cycle against the bind-mounted `~/.labby/acp.db`

## References

- `docs/design/design-system-contract.md` — Aurora design system source of truth
- `crates/lab/src/acp/persistence.rs` — legacy JSON persistence (still used by registry sync path)
- `crates/lab/src/dispatch/acp/persistence.rs` — SQLite persistence (new path)
- `crates/lab/src/acp/runtime.rs` — prompt idle timeout implementation and constants
- `docker-compose.yml` — volume mounts, env overrides, init flag
