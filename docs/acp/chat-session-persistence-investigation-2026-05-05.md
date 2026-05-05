# Chat Session Persistence Investigation - 2026-05-05

Bead: `lab-m5sj`

## Result

Chat session metadata and persisted events survive a real backend/container restart when the caller uses the same authenticated principal. The tested session `c51ce807-a097-4aa3-8e70-dcc060858b17` was created through `/v1/acp/sessions`, prompted with `pwd`, persisted 54 events to `~/.lab/acp.db`, survived `docker restart lab-labby-master-1`, and replayed events 39-54 from `/v1/acp/sessions/{id}/events` after restart.

Browser-only UI verification was blocked by the live server's Google OAuth gate. `agent-browser` opened `/chat/`, saw `Sign in to Labby`, and after clicking sign-in was redirected to Google. API/SSE/SQLite evidence below verifies the persistence layer directly, but full authenticated browser reload and route-navigation behavior remains not tested in this run.

One implementation gap was reproduced: a closed session is removed from the live `/sessions` list immediately, but reappears in `/sessions` after process restart because `restore_from_db()` restores closed rows into the in-memory map.

## Storage Map

| Layer | Evidence | Behavior |
| --- | --- | --- |
| Selected chat pointer | `apps/gateway-admin/lib/chat/chat-session-provider.tsx:172` reads `localStorage['lab.chat.last-session-id']`; `apps/gateway-admin/lib/chat/chat-session-provider.tsx:352` writes it in `selectRun()` | Browser-local selected session id only |
| Session list restore | `apps/gateway-admin/lib/chat/chat-session-provider.tsx:254` fetches `/sessions`; `apps/gateway-admin/lib/chat/chat-session-provider.tsx:269` maps backend summaries; `apps/gateway-admin/lib/chat/chat-session-provider.tsx:271` preserves selected id only if returned by backend | Backend list controls visible sessions |
| Frontend event cache | `apps/gateway-admin/lib/chat/session-event-cache.ts:17` and `:18` are module-level `Map`s; `:39` exposes the event cache | In-memory only, not reload-safe |
| Mounted SSE reader | `apps/gateway-admin/lib/chat/chat-session-provider.tsx:484` starts the selected-session stream; `:515` requests a subscribe ticket; `:525` opens `/events?since=...`; `:555` writes the in-memory cache | Active provider path for chat UI |
| Alternate SSE hook | `apps/gateway-admin/lib/chat/use-session-events.ts:129` defines `useSessionEvents`, but `rg "useSessionEvents\\(" apps/gateway-admin/lib apps/gateway-admin/components apps/gateway-admin/app` found only the hook definition in product files | Stale/alternate hook, not mounted in chat UI |
| API auth/principal | `crates/lab/src/api/services/acp.rs:29` requires a principal; `:117`, `:144`, `:183`, `:310`, and `:332` apply it to session routes | Session actions are principal-scoped |
| Registry principal checks | `crates/lab/src/acp/registry.rs:181` rejects empty/mismatched principals; `:204` filters `list_sessions`; `:410`, `:486`, `:604`, `:709`, and `:732` check access on prompt/cancel/close/events/subscribe | Restored sessions remain principal-scoped |
| Durable DB path | `crates/lab/src/dispatch/acp/persistence.rs:203` resolves `LAB_ACP_DB` or default `~/.lab/acp.db` | Host-local SQLite |
| Durable tables | `crates/lab/src/dispatch/acp/persistence.rs:444` creates `acp_sessions`, `acp_session_events`, and `acp_permission_requests` | Metadata, events, permission outcomes |
| Event backfill | `crates/lab/src/acp/registry.rs:750` uses `load_events_since_capped(..., BACKFILL_CAP)` before live SSE fanout | Replay comes from SQLite when available |
| Startup restore | `crates/lab/src/cli/serve.rs:360` creates registry; `:362` calls `restore_from_db()`; `crates/lab/src/acp/registry.rs:1036` reloads sessions from SQLite | Process restart rehydrates registry |
| Docker persistence | `docker-compose.yml:16` starts the volume list; `docker-compose.yml:21` maps `${HOME}/.lab` to `/home/lab/.lab`; `docker-compose.yml:31` mounts `.env` read-only | Default `acp.db` survives container restarts |

## Runtime Evidence

Preconditions:

```text
LAB_ACP_DB=/home/jmagar/.lab/acp.db
/home/jmagar/.lab/acp.db exists, mode -rw-------
LAB_ACP_HMAC_SECRET=<present redacted>
Initial DB baseline: sessions=76, events=2573
```

Created test session:

```text
POST /v1/acp/sessions
id=c51ce807-a097-4aa3-8e70-dcc060858b17
provider=codex-acp
principal=static-bearer
state=idle
```

Prompted session:

```text
POST /v1/acp/sessions/c51ce807-a097-4aa3-8e70-dcc060858b17/prompt
prompt=pwd
response={"ok":true,"session_id":"c51ce807-a097-4aa3-8e70-dcc060858b17"}
```

Pre-restart DB state:

```text
2026-05-05T07:14:36-04:00
pre_sessions=77
pre_events=2627
c51ce807-a097-4aa3-8e70-dcc060858b17|completed|static-bearer|2026-05-05T11:12:55.326575833Z
selected session event range: min_seq=1, max_seq=54, count=54
```

Restart command:

```text
2026-05-05T07:15:03-04:00
docker restart lab-labby-master-1
```

Restore log:

```text
11:15:04 INFO ACP sessions restored from SQLite action=restore restored=77 service=registry surface=acp total_in_db=77
11:15:04 INFO ACP session registry installed phase=ready subsystem=acp
```

Post-restart API/DB state:

```text
GET /health -> {"status":"ok","mode":"master","pid":7,"uptime_s":33}
GET /v1/acp/sessions -> count=29 for principal static-bearer
selected session found:
  id=c51ce807-a097-4aa3-8e70-dcc060858b17
  state=completed
  principal=static-bearer
post_sessions=77
post_events=2627
selected session event range: min_seq=1, max_seq=54, count=54
```

Post-restart SSE replay:

```text
GET /v1/acp/sessions/c51ce807-a097-4aa3-8e70-dcc060858b17/events?since=38
returned seq 39-54
seq 40: prompt_started text="pwd"
seq 43: tool_call_start name="pwd"
seq 45: tool_call_update stdout="/workspace/lab\n" status="completed"
seq 46-51: assistant message chunks forming "`/workspace/lab`"
seq 53: session_update state="completed"
seq 54: stop_reason status="completed"
```

Security evidence:

```text
GET /v1/acp/sessions without Authorization -> 401
{"kind":"auth_failed","message":"missing bearer token or session cookie"}
```

Cross-principal negative testing was not possible in this environment because the live local API exposes one static bearer principal for this route.

Browser evidence:

```text
agent-browser open http://127.0.0.1:8765/chat/
snapshot: heading "Sign in to Labby", button "Sign in"
agent-browser click Sign in
snapshot: Google OAuth sign-in page for tootie.tv
```

The browser could verify the auth gate but not authenticated chat reload/route behavior.

## Restart Matrix

| Scenario | Status | Evidence |
| --- | --- | --- |
| Frontend route navigation away/back | Not tested | Browser was blocked by Google OAuth before reaching chat. |
| Browser refresh on `/chat` | Not tested | Same OAuth gate. Backend-selected session and event replay were verified by API/SSE. |
| Backend process restart without browser reload | Pass for backend/API persistence | `docker restart lab-labby-master-1`; restore log `restored=77`; selected session remained visible and replayed seq 39-54. Browser SSE reconnect not tested because OAuth gate blocked the UI. |
| Docker container restart | Pass | Host `~/.lab/acp.db` counts remained `sessions=77/events=2627`; selected session event count remained 54; API replay worked after restart. |
| Full browser close/open after backend restart | Not tested | Browser returned to OAuth. API cold access after restart found the session and replayed events for the same principal. |
| `LAB_ACP_HMAC_SECRET` absent/present | Pass, present | `~/.lab/.env` contains `LAB_ACP_HMAC_SECRET=<present redacted>`, so cross-restart permission-outcome verification is configured. No permission events were generated in this test. |
| Closed session after restart | Fail | Closed session `9b34d8b9-b8cc-40d1-ace7-d6b2988106eb` disappeared from `/sessions` immediately after `session.close`, persisted as `closed` in SQLite, then reappeared in `/sessions` after `docker restart lab-labby-master-1`. |

## Follow-Up

Created follow-up bead for the closed-session restore gap. The expected behavior should be decided explicitly: either `/sessions` is an active working set and `restore_from_db()` should not restore closed sessions into that list, or the UI/API contract should distinguish active list from durable history.
