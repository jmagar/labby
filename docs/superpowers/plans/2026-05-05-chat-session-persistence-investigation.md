# Chat Session Persistence Investigation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to execute this investigation task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Determine, with restart evidence, whether gateway-admin chat session metadata and messages survive frontend reloads, backend restarts, container restarts, and full process restarts.

**Architecture:** Gateway-admin chat is a static Next.js client over the Lab ACP HTTP/SSE surface. Frontend state is split between durable browser `localStorage` for the selected session id, module-level in-memory event caches for transcript replay during a single browser lifetime, and backend ACP SQLite persistence for session summaries/events. The investigation must prove which layer restores what after each restart class.

**Tech Stack:** Rust 2024, Axum API routes, Lab ACP registry/runtime, SQLite ACP persistence, Next.js static export, React context, SSE.

---

## Scope Lock

This bead is investigation and planning only. Do not implement product code while executing this plan.

Allowed changes:
- Update bead `lab-m5sj`.
- Add follow-up beads if restart evidence proves a gap.
- Save or update this investigation plan.
- Add a markdown investigation report only if execution needs a durable evidence artifact.

Disallowed changes:
- Do not edit Rust, TypeScript, CSS, tests, generated docs, `.gitignore`, Docker config, or runtime config as part of this bead.
- Do not claim persistence works without restart evidence.
- Do not close `lab-m5sj` unless the matrix below has concrete evidence for every row.

## Code Map

Frontend chat state:
- `apps/gateway-admin/components/admin-layout-client.tsx`: mounts `ChatSessionProvider`, floating chat UI, and persisted floating-popover open/config state.
- `apps/gateway-admin/lib/chat/chat-session-provider.tsx`: owns runs, selected run, provider health, optimistic messages, and an internal SSE reader.
- `apps/gateway-admin/lib/chat/session-event-cache.ts`: module-level in-memory LRU cache for recent session events and last seq values; not durable across browser reload.
- `apps/gateway-admin/lib/chat/use-session-events.ts`: alternate SSE hook with retry/backoff behavior; verify whether any mounted surface still uses it.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`: helper functions for creating sessions and sending prompts.
- `apps/gateway-admin/components/chat/chat-shell.tsx`: `/chat` route surface consuming provider context.
- `apps/gateway-admin/components/floating-chat-popover.tsx`: floating UI localStorage state.

Backend ACP/session persistence:
- `crates/lab/src/api/services/acp.rs`: browser-compatible `/v1/acp/provider`, `/sessions`, `/sessions/{id}/prompt`, `/events`, and subscribe-ticket routes.
- `crates/lab/src/acp/registry.rs`: in-memory session registry, access checks, event fanout, reattach behavior, and `restore_from_db()`.
- `crates/lab/src/dispatch/acp/persistence.rs`: SQLite ACP persistence, schema, `LAB_ACP_DB`, `LAB_ACP_HMAC_SECRET`, event batching, and event replay reads.
- `crates/lab-apis/src/acp/persistence.rs`: persistence trait contract.
- `crates/lab/src/cli/serve.rs`: startup path that installs the registry and calls `restore_from_db()`.
- `docker-compose.yml`: bind-mounts host `~/.labby` to `/home/labby/.lab`, so default `acp.db` should survive container restarts.

Docs:
- `docs/acp/README.md`: current ACP boundary, HTTP/SSE routes, security/runtime posture.
- `docs/acp/design.md`: durable-store design intent.
- `docs/runtime/CONFIG.md`: Docker restart/config notes and `just acp-smoke`.
- `docs/generated/env-reference.md`: `LAB_ACP_DB` and `LAB_ACP_HMAC_SECRET`.
- `docs/sessions/2026-05-04-acp-session-persistence-chat-polish.md`: prior session note claiming `restore_from_db()` work and explicitly deferring real container restart verification.

## Current Repo Facts To Verify During Execution

- `ChatSessionProvider` seeds `selectedRunId` from `localStorage.getItem('lab.chat.last-session-id')` and writes it in `selectRun`.
- `refreshSessions()` calls `GET /v1/acp/sessions`, maps returned sessions to runs, and keeps the current selected id only if the backend list still contains it.
- `sessionEventCache` and `sessionLastSeqCache` are module-level `Map`s capped to 10 sessions; they are not localStorage, IndexedDB, or backend storage.
- The provider's internal SSE reader backfills from `/sessions/{id}/events?since=<lastSeq>&ticket=...` and writes event cache entries in memory.
- `SqliteAcpPersistence` defaults to `~/.labby/acp.db` unless `LAB_ACP_DB` overrides it.
- SQLite schema includes `acp_sessions`, `acp_session_events`, and `acp_permission_requests`.
- `AcpSessionRegistry::restore_from_db()` reloads persisted sessions into the in-memory registry and marks previously running/waiting sessions as failed with synthetic restart events.
- `labby serve` calls `restore_from_db().await` before constructing API state.
- Docker binds host `${HOME}/.labby` into `/home/labby/.lab`; all default ACP SQLite data should persist across `docker compose restart labby-master`.

## Restart Matrix

| Scenario | Expected Durable Layer | Required Evidence | Pass Condition | Failure Follow-Up |
| --- | --- | --- | --- | --- |
| Frontend route navigation away/back | React provider remains mounted in admin layout | Browser screenshot or DOM state before/after route change; network trace showing no session loss | Same selected session and transcript visible; no duplicate session created | Bead: prevent route-change session reset |
| Browser refresh on `/chat` | `localStorage` selected id + backend session list + SSE backfill | Before/after `localStorage['lab.chat.last-session-id']`; `/v1/acp/sessions` response; `/events?since=0` or selected seq replay | Same selected session id is restored and messages replay from backend, not only memory | Bead: persist selected id or reload transcript correctly |
| Backend process restart without browser reload | Backend SQLite restore + frontend SSE reconnect/backfill | Process restart command; server logs containing restore count; browser network retry/open; transcript after reconnect | Existing session remains in sidebar and old messages replay; active session becomes explicit Failed/interrupted if process died mid-turn | Bead: SSE restart reconnection/backfill defect |
| Docker container restart | Host bind-mounted `~/.labby/acp.db` + registry restore | `docker compose restart labby-master`; `sqlite3 ~/.labby/acp.db` row counts before/after; `/v1/acp/sessions` response after restart | Sessions/events still in SQLite and visible in UI after restart | Bead: Docker persistence mount/config gap |
| Full browser close/open after backend restart | Browser localStorage + backend SQLite restore | Fresh browser session or new tab; selected id in localStorage; `/sessions` and `/events` responses | Selected session reappears if id exists; if not selected, session list still contains it and selecting it replays messages | Bead: cold-start selection/transcript gap |
| `LAB_ACP_HMAC_SECRET` absent | SQLite rows survive, but signed permission outcomes may fail verification | `grep LAB_ACP_HMAC_SECRET ~/.labby/.env` result without printing secret; logs for ephemeral-key warning; replay of permission outcome event | Investigation reports degraded cross-restart permission-outcome verification truthfully | Bead: require or auto-provision persistent ACP HMAC secret |
| Closed session after restart | SQLite state plus registry restore policy | Close a session, restart, inspect `/sessions` and DB state | Closed sessions remain queryable only if current product contract says they should; otherwise documented discrepancy with `design.md` | Bead: align closed-session retention/list behavior |

## Research Findings

Repo-local docs and mirrored upstream references agree on the core pattern: durable chat persistence needs a server-side store for session/message records plus a resumable stream or event replay path. Vercel AI SDK's chat persistence guide frames message persistence as loading prior messages from storage before handling a new message and validating messages server-side before model use. OpenACP's session persistence docs say sessions should survive restarts by storing session records to disk and restoring them automatically; active sessions after restart need explicit reconnect/error behavior.

For Lab, the intended server-side durable store is SQLite. Browser storage should be treated as a pointer/cache only. The investigation must distinguish:
- session metadata durability: `acp_sessions`
- message/event durability: `acp_session_events`
- UI selection durability: `localStorage['lab.chat.last-session-id']`
- UI transcript cache: in-process only `sessionEventCache`
- live runtime continuity: not durable; runtime handles are intentionally in-memory

## CEO Review: HOLD SCOPE

Mode: HOLD SCOPE.

Premise: The right problem is not "add persistence"; the right problem is "prove restart-survivable chat behavior end to end and identify any remaining gaps." Prior work already added SQLite restore. Re-implementing before evidence would risk rebuilding solved pieces.

Minimum scope:
- Build an evidence-backed restart matrix.
- Identify exactly where session metadata and transcript messages live today.
- Prove frontend reload, backend restart, Docker restart, and full cold-start behavior.
- File follow-up beads only for proven gaps.

Out of scope:
- New database schema.
- New browser persistence layer.
- UI redesign.
- Retention policy implementation unless the investigation proves current behavior violates a committed contract.

Decisions implementers will otherwise hit:
- Treat active in-flight provider processes as non-durable. Restored sessions may preserve history, but the runtime handle itself cannot survive a process/container restart.
- Treat permission outcome replay as dependent on stable `LAB_ACP_HMAC_SECRET`.
- Treat `sessionEventCache` as a performance cache, not a persistence layer.
- Treat Docker restart evidence as required because a prior session note explicitly deferred that test.

## Engineering Review

Architecture:
- Good: the current split is sensible if SQLite is the source of truth and frontend caches are only accelerators.
- Risk: there appear to be two frontend SSE paths (`ChatSessionProvider` internal stream and `useSessionEvents`). The investigation must prove only the intended path is mounted or explain differences in retry behavior.
- Risk: backend restore reloads sessions into memory, but `close_session()` removes closed sessions from the live map. Verify whether closed sessions should remain visible after restart according to `docs/acp/design.md`.

Simplicity:
- Do not add IndexedDB or localStorage transcript persistence unless backend replay fails and a follow-up bead justifies it.
- Prefer black-box API/SSE evidence plus SQLite row checks over broad code churn.

Security:
- Verify access remains principal-scoped after restore: a session created under one authenticated principal must not be visible to another.
- Do not print secrets. For `LAB_ACP_HMAC_SECRET`, report presence/absence and length/format only.
- Verify subscribe-ticket failure behavior for stale/wrong session ids if restart testing exposes 401/404 loops.

Performance:
- Use bounded sessions and small prompts for restart tests.
- Inspect row counts and event seq ranges; do not dump large event payloads into tracker comments.
- Confirm SSE backfill uses the capped SQL path for large transcripts if the existing DB has high event counts.

Failure modes to record:

| Codepath | Failure Mode | Expected Visibility |
| --- | --- | --- |
| `refreshSessions()` | Backend unavailable or returns non-JSON | UI shows provider/session unavailable; investigation captures response/status |
| `restore_from_db()` | SQLite unavailable | Logs `persistence_unavailable`; `/sessions` returns empty; investigation captures DB path and log |
| SSE subscribe | Ticket invalid or session missing | 401/404/error state; no infinite silent spinner |
| Event replay | HMAC verification fails for permission outcome | Synthetic persisted-event error or logged warning; transcript does not silently claim approval |
| Active turn restart | Provider process dies | Session marked Failed/interrupted with synthetic event |

## Task 1: Baseline Static Evidence

**Files:**
- Read: `apps/gateway-admin/lib/chat/chat-session-provider.tsx`
- Read: `apps/gateway-admin/lib/chat/session-event-cache.ts`
- Read: `apps/gateway-admin/components/admin-layout-client.tsx`
- Read: `crates/lab/src/acp/registry.rs`
- Read: `crates/lab/src/dispatch/acp/persistence.rs`
- Read: `crates/lab/src/api/services/acp.rs`
- Read: `crates/lab/src/cli/serve.rs`
- Read: `docker-compose.yml`

- [ ] **Step 1: Capture code-backed storage map**

Record exact line references for:
- selected session id source/write: `localStorage['lab.chat.last-session-id']`
- frontend transcript cache: module-level `Map`s in `session-event-cache.ts`
- session list load: `GET /sessions`
- SSE backfill: `/sessions/{id}/events?since=...`
- SQLite path: `LAB_ACP_DB` or `~/.labby/acp.db`
- tables: `acp_sessions`, `acp_session_events`, `acp_permission_requests`
- startup restore: `restore_from_db()`

- [ ] **Step 2: Confirm mounted SSE path**

Run:

```bash
rg -n "useSessionEvents\\(|ChatSessionProvider|sessionEventCache|events\\?since" apps/gateway-admin/lib apps/gateway-admin/components apps/gateway-admin/app
```

Expected:
- Identify whether `ChatSessionProvider` is the only mounted event reader.
- If `useSessionEvents()` is unused, report it as a stale or alternate hook, not as active behavior.

- [ ] **Step 3: Capture prior-art docs**

Read:

```bash
sed -n '79,140p' docs/references/ai-sdk/domains/ai-sdk.dev/latest/markdown/0081-ai-sdk-dev-docs-ai-sdk-ui-chatbot-message-persistence.md
sed -n '91,144p' docs/references/openacp/domains/docs.openacp.ai/latest/markdown/0017-docs-openacp-ai-features-session-persistence.md
```

Expected:
- Report that durable chat requires server-side message/session storage and explicit restart restore semantics.

## Task 2: Runtime Preconditions

**Files:**
- Read only: `~/.labby/.env`
- Read only: `~/.labby/acp.db`
- Read only: `docker-compose.yml`

- [ ] **Step 1: Record clean working context**

Run:

```bash
git status --short
bd show lab-m5sj --json
```

Expected:
- Preserve unrelated dirty files, especially `.gitignore`.
- Save bead status and current acceptance criteria in the evidence notes.

- [ ] **Step 2: Resolve runtime paths**

Run:

```bash
printf 'LAB_ACP_DB=%s\n' "${LAB_ACP_DB:-$HOME/.labby/acp.db}"
test -f "${LAB_ACP_DB:-$HOME/.labby/acp.db}" && ls -l "${LAB_ACP_DB:-$HOME/.labby/acp.db}" || true
grep -E '^LAB_ACP_HMAC_SECRET=' "$HOME/.labby/.env" | sed -E 's/=(.).*/=<present redacted>/' || true
```

Expected:
- Do not print the secret value.
- Record whether the DB exists and whether HMAC secret is configured.

- [ ] **Step 3: Record DB baseline**

Run:

```bash
sqlite3 "${LAB_ACP_DB:-$HOME/.labby/acp.db}" \
  "select 'sessions', count(*) from acp_sessions union all select 'events', count(*) from acp_session_events;"
sqlite3 "${LAB_ACP_DB:-$HOME/.labby/acp.db}" \
  "select id, state, principal, provider, updated_at from acp_sessions order by updated_at desc limit 5;"
```

Expected:
- Counts and latest session metadata are recorded.
- Do not dump event payloads unless needed for a specific failure, and redact raw fields if viewed.

## Task 3: Frontend Reload Evidence

**Surfaces:**
- Browser `/chat`
- Browser devtools localStorage
- API `/v1/acp/sessions`
- API `/v1/acp/sessions/{id}/events`

- [ ] **Step 1: Create or select a test session**

Use the UI or API to create a session, send a short prompt such as `pwd`, and wait for visible assistant output.

Record:
- selected session id
- localStorage `lab.chat.last-session-id`
- one visible message/event identifier or seq

- [ ] **Step 2: Refresh `/chat`**

Reload the page.

Expected:
- selected id remains in localStorage
- `/v1/acp/sessions` contains the selected id
- transcript reloads from backend events, not only module memory

- [ ] **Step 3: Close and reopen browser tab**

Open `/chat` again in a new tab.

Expected:
- same selected session is restored if localStorage is shared
- if the selected id was deleted or missing, UI falls back to first returned session without creating an empty duplicate

## Task 4: Backend Restart Evidence

**Surfaces:**
- Lab server process or Docker container
- `/v1/acp/provider`
- `/v1/acp/sessions`
- `/v1/acp/sessions/{id}/events`
- SQLite row counts

- [ ] **Step 1: Record pre-restart state**

Run DB baseline queries from Task 2 and capture the selected session id.

- [ ] **Step 2: Restart backend**

For Docker:

```bash
docker compose restart labby-master
```

For a local `labby serve` process, stop and restart the same command that was running it.

Expected:
- Do not change source code or config.
- Capture exact command and timestamp.

- [ ] **Step 3: Verify backend restore**

Run:

```bash
curl -fsS http://127.0.0.1:8765/v1/acp/provider >/tmp/lab-acp-provider.json
curl -fsS http://127.0.0.1:8765/v1/acp/sessions >/tmp/lab-acp-sessions.json
sqlite3 "${LAB_ACP_DB:-$HOME/.labby/acp.db}" \
  "select 'sessions', count(*) from acp_sessions union all select 'events', count(*) from acp_session_events;"
```

Adjust host/port/auth headers to match the running environment.

Expected:
- provider route is healthy or reports an explicit provider error
- sessions route includes the selected session for the same principal
- DB counts did not drop

- [ ] **Step 4: Verify transcript replay**

Open the selected session in the UI and capture:
- visible prior user prompt
- visible prior assistant output or restart/failure marker
- network evidence that `/events?since=...` returned events

Expected:
- Past transcript survives backend restart.
- In-flight turns are marked interrupted/failed rather than silently appearing live.

## Task 5: Security And Principal Evidence

**Files:**
- Read: `crates/lab/src/api/services/acp.rs`
- Read: `crates/lab/src/acp/registry.rs`

- [ ] **Step 1: Confirm principal propagation**

Record line references showing:
- API requires authenticated principal for session routes
- registry checks principal before list/prompt/subscribe/cancel/close

- [ ] **Step 2: Negative access test**

If test auth allows it, call `/v1/acp/sessions` or `/events` with missing/invalid auth.

Expected:
- missing auth returns an auth failure
- wrong principal cannot enumerate or subscribe to another principal's session

If the environment only has static-bearer local auth, record that cross-principal testing is not possible in this run and keep the gap open in the bead.

## Task 6: Report And Follow-Up Beads

**Files:**
- Modify: bead `lab-m5sj`
- Optional create: `docs/reports/chat-session-persistence-investigation.md`

- [ ] **Step 1: Write final investigation result**

Update `lab-m5sj` with:
- storage map
- restart matrix with Pass/Fail/Not tested
- exact commands run
- exact evidence file paths or copied response snippets
- any untested rows and why

- [ ] **Step 2: Create follow-up beads only for proven gaps**

Each follow-up bead must include:
- failure scenario
- affected files
- expected behavior
- acceptance criteria
- evidence link or command output reference

- [ ] **Step 3: Verify tracker and plan**

Run:

```bash
bd show lab-m5sj --json
test -f docs/superpowers/plans/2026-05-05-chat-session-persistence-investigation.md
git status --short
```

Expected:
- bead contains the investigation/restart matrix
- plan file exists
- `.gitignore` remains whatever it was before this work

## Completion Criteria

`lab-m5sj` is ready to close only when:
- every matrix row has Pass/Fail/Not tested status with evidence
- backend storage path and frontend storage/cache layers are identified with code references
- restart behavior is verified with real reload/restart commands
- any gap has a follow-up bead
- no product code was changed by this bead
