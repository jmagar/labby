---
title: ACP Design — Research Findings
created_at: 2026-04-23
status: evidence-gathered
agents: architecture-strategist, security-sentinel, performance-oracle, julik-frontend-races-reviewer, kieran-typescript-reviewer, pattern-recognition-specialist, agent-native-reviewer, best-practices-researcher, learnings-researcher
---

# ACP Design — Research Findings

Research gathered from 9 domain-matched agents covering the design document at `docs/acp/design.md`.
No plan modifications made — evidence only, ready for `/lavra-design` to integrate.

---

## Critical Blockers (Must Resolve Before Phase 1)

### C1 — ContentBlock[] Destroyed at Emission (Violates Locked Decisions 12 & 13)

**Source:** agent-native-reviewer, architecture-strategist  
**File:** `crates/lab/src/acp/runtime.rs:671–680`

`content_to_text()` converts every `ContentBlock` to a flat string before the event is stored in
`BridgeEvent`. Images become `"[image]"`, resources become `"[resource] uri"`. This happens before
persistence and SSE delivery — the structured data is gone at source. `BridgeEvent` has no
`content: Option<Vec<Value>>` field to hold preserved blocks.

**Locked decision 12:** "ACP must preserve raw `usage_update` payloads and raw `ContentBlock[]`"  
**Locked decision 13:** "ACP must invest in full `ContentBlock[]` rendering rather than flattening"

**Fix:** Add `content: Option<Vec<Value>>` to `PendingBridgeEvent` and `BridgeEvent` in
`types.rs`. In `push_session_update`, serialize the raw `ContentBlock` via `serde_json::to_value`
into that field. Keep `text` as a derived convenience field. No persistence changes needed — the
field serializes automatically.

---

### C2 — HTTP API Is REST-Shaped, Violates Locked Decision 24

**Source:** agent-native-reviewer  
**File:** `crates/lab/src/api/services/acp.rs:18–25`

Current routes: `GET /provider`, `GET /sessions`, `POST /sessions`, `POST /sessions/:id/prompt`,
`POST /sessions/:id/cancel`. Locked decision 24 states machine-facing ACP routes must follow the
`action + params` dispatch model. Current shape prevents `dispatch/acp/` from being the shared path
for both MCP and HTTP surfaces.

**Fix:** Migrate command routes to `POST /v1/acp` with `{"action": "...", "params": {...}}`
following `api/services/helpers.rs::handle_action()`. Retain SSE as the exception:
`GET /v1/acp/events/:session_id?since=<seq>`.

---

### C3 — subscribe() Only Reads In-Memory Events (Silent Data Loss Above 500 Events)

**Source:** performance-oracle, architecture-strategist  
**Files:** `crates/lab/src/acp/registry.rs:174–185`, `336–339`

`subscribe(since_seq)` reads the backlog exclusively from the in-memory `Vec`, which is capped at
500 by FIFO drain. A client reconnecting with a `since` below the oldest in-memory seq receives a
silent partial backlog — no gap marker, no error. The design's resume contract ("server returns
backlog with seq > since") implies complete delivery.

**Fix:** `subscribe()` must fall back to disk when `since` predates the oldest retained in-memory
seq. Switch `save_events` from full-rewrite to `OpenOptions::append(true)` so the JSONL file is the
authoritative source and can be read efficiently by seq range.

---

### C4 — Subprocess Inherits All Lab Credentials

> **RESOLVED** (2026-06) — Provider subprocesses now spawn with `env_clear()` and a fixed allowlist (`PATH`, `HOME`, locale vars, terminal vars, Windows `SystemRoot`). Per-provider entries can extend the allowlist explicitly via the structured `env` field on `AcpProviderEntry`. See `docs/acp/README.md` security section.

**Source:** security-sentinel  
**File:** `crates/lab/src/acp/runtime.rs:153–161`  
**Severity: HIGH**

`tokio::process::Command::new(&command)` with no `.env_clear()` spawns Codex (`npx
@zed-industries/codex-acp`) with the full parent environment including every service key loaded from
`~/.labby/.env`: `RADARR_API_KEY`, `SONARR_API_KEY`, `OPENAI_API_KEY`, etc. The subprocess can read
all of them via `process.env`.

**Fix:** Call `.env_clear()` before `.spawn()`. Explicitly pass only what Codex needs (`HOME`,
`PATH`, `TMPDIR`, `ACP_SESSION_CWD`). Document forwarded variables.

---

### C5 — Permission Auto-Selection Bypasses User Consent

> **RESOLVED** (2026-06) — A pending-permission state machine is now implemented. Permission decisions are explicit with no auto-approval path. Each request emits an event and waits for an authenticated decision bounded by `LAB_ACP_PERMISSION_TIMEOUT_MS` (default 60 s). `session.permission.approve` and `session.permission.reject` are Phase 1 dispatch actions. HMAC-signed `permission_outcome` payloads detect tampering. See `docs/acp/README.md` security section.

**Source:** security-sentinel, agent-native-reviewer  
**File:** `crates/lab/src/acp/runtime.rs:196–257`  
**Severity: HIGH**

The `RequestPermissionRequest` handler resolves the permission synchronously with `AllowOnce` >
`AllowAlways` > `first()` before the browser or any MCP consumer can see or act on the
`permission.requested` event. There is no pending-permission state machine, no held channel, no
timeout.

**Fix:** Introduce a `pending_permissions: HashMap<tool_call_id, oneshot::Sender<PermissionResponse>>`
in the registry. On `RequestPermissionRequest`, store the responder, emit the event, and block.
Expose `session.permissions.respond` as a Phase 1 dispatch action. Add timeout defaulting to
`RejectOnce`. Promote this from "Likely later additions" in the action catalog.

---

### C6 — lab-apis Promotion Blockers (3 Hard Issues)

**Source:** pattern-recognition-specialist, architecture-strategist  

**C6a — Direct env reads in `persistence.rs` and `registry.rs`:**  
`persistence.rs:15,18` reads `LAB_ACP_SESSION_DIR` and `HOME`. `registry.rs:35–39` reads
`ACP_SESSION_CWD`. `lab-apis` must never read env vars — that rule is explicit in CLAUDE.md.
Fix: `JsonFileAcpPersistence::new(base_dir: PathBuf)`. Env reads move to `lab/src/config.rs`.

**C6b — `ToolError` coupling in what should be SDK code:**  
`registry.rs:6` imports `crate::dispatch::error::ToolError`, a `lab`-crate type. Moving the
registry to `lab-apis` would create a circular dependency.  
Fix: Define `AcpError` in `lab-apis::acp` using `thiserror`. Dispatch layer converts via `From`.

**C6c — `agent_client_protocol` subprocess launch should stay in `lab`:**  
`runtime.rs` spawns OS threads, creates embedded Tokio runtimes, and uses 20+ types from
`agent_client_protocol` — an `npx` subprocess protocol. This is too infrastructure-heavy for a
pure SDK. The design plan already hedges this correctly (`design.md:622–628`).  
Fix: Define a `Provider` trait in `lab-apis::acp` covering `health()`, `start_session()`,
`prompt()`, `cancel()`. The `CodexProvider` implementation stays in `lab`, behind that trait.

---

## Design Gaps (Require Decisions Before Implementation)

### D1 — GatewayManager Has No `call_tool` Method

**Source:** architecture-strategist  
**File:** `crates/lab/src/dispatch/gateway/manager.rs`  

`GatewayManager` exposes `discovered_tools()` and `discovered_resources()` but no tool execution
method. The in-process ACP→gateway bridge the design relies on (Phase 2) doesn't exist yet.
Open question #1 in the design doc acknowledges this. The global `OnceLock` static wiring pattern
already exists; the missing piece is the execution surface.

**Design.md open question #1:** "What exact trait or interface should ACP use for gateway tool execution?"

---

### D2 — Subprocess-ACP and HTTP-REST Providers Have Incompatible Session Shapes

**Source:** architecture-strategist, best-practices-researcher  

`RuntimeHandle` uses `mpsc::UnboundedSender<SessionCommand>` over stdin/stdout for the Codex
subprocess. HTTP providers (Claude API, Copilot OAuth) have no subprocess, no ACP initialize
handshake, and no `provider_session_id`/`agent_name`/`agent_version` from a startup response.
These are ACP protocol concepts, not generic provider concepts.

The design plan's provider trait surface (`design.md:823–853`) does not address this structural
mismatch. A unified `Provider` trait must either make these fields `Option<T>` or define two
distinct session lifecycle models.

**Best-practices finding:** `async fn in trait` is stable (Rust 1.75+) but `dyn Trait` with `async
fn` is NOT object-safe (no stable solution as of 2026). Recommended approach: **concrete types or
generics** for the known provider set (Codex, Claude, Gemini, Copilot, OpenCode), with **enum
dispatch** (`enum_dispatch` crate) as an ergonomic option for the closed set.

Repository conventions in `CLAUDE.md` are precise about what is forbidden: `Box<dyn ServiceClient>`
is banned outright, `#[async_trait]` is banned, and `Box<dyn Trait>` is disfavored where `impl
Trait` or generics are sufficient. These rules do not ban all dynamic dispatch — they ban the
`ServiceClient` trait in particular from being object-erased. For a dynamic-registry extension path
(third-party or plugin providers not in the closed set), an erased-trait adapter using a crate like
`dynosaur` is acceptable at the extension boundary, precisely because it is not a `Box<dyn
ServiceClient>`. Use concrete/enum dispatch for all known providers; reserve the erased adapter for
the extension slot only.

---

### D3 — `Bridge*` Type Names Encode Codex Implementation Detail

**Source:** pattern-recognition-specialist  
**Severity: Medium**

`BridgeSessionSummary`, `BridgeEvent`, `BridgePermissionOption`, `BridgeSessionStatus`,
`PendingBridgeEvent` all reflect the "bridge to Codex ACP subprocess" identity. Locked decision 9
says ACP is provider-agnostic. When Claude or Gemini sessions emit events, "Bridge" is a misnomer.

**Rename targets:** `AcpSessionSummary`, `AcpEvent`, `AcpSessionStatus`, `AcpPermissionOption`,
`PendingAcpEvent` (or `AcpEventBuilder`).

---

### D4 — `session.load` Semantics Undefined

**Source:** agent-native-reviewer  

`session.load` is listed as Phase 1 but has no implementation sketch, parameter definition, or
distinction from `session.start`. The `reattach_runtime()` method in `registry.rs:223` is the
likely target. Needs definition: does it differ from `session.get` (read-only) or does it
re-attach a runtime to a persisted session?

---

### D5 — `target.list` Needs a `Target` Struct Before Phase 1 Ships

**Source:** agent-native-reviewer  

`target.list` is listed as Phase 1 but no `Target` struct exists. Minimum schema needed before
catalog registration: `{target_id, kind: "agent"|"skill"|"command"|"mcp_package", capabilities,
install_state: "available"|"installed"}`.

---

### D6 — ACP Spec Name Collision

**Source:** best-practices-researcher  

The IBM/BeeAI "Agent Communication Protocol" (ACP) spec was archived August 2025 and merged into
the A2A protocol under the Linux Foundation. The `lab` ACP service is a product-local naming
convention, not an implementation of that spec. The term "ContentBlock[]" in the design document
refers to the **Claude API's** content block format, not any ACP spec type. Should be documented
to prevent future confusion.

---

## Performance Issues

### P1 — O(n) Clone + Full File Rewrite on Every Event

**Source:** performance-oracle  
**File:** `crates/lab/src/acp/registry.rs:276–352`, `persistence.rs:44–64`

Every `push_event` call: (1) clones all retained events for the session (~500 entries) while
holding the write lock, (2) clones all session summaries across every open session, (3) rewrites
the entire event file — despite the `.jsonl` extension implying line-append. At 10 events/sec,
the same 500-event file is rewritten 10 times per second.

**Fix priority:** Make `save_events` append-only (`OpenOptions::append(true)`). Decouple
persistence writes from the write lock critical section — extract clones outside the lock, then
dispatch to a background task.

---

### P2 — Per-Session OS Thread + Node.js Subprocess (~100MB RSS Each)

**Source:** performance-oracle  
**File:** `crates/lab/src/acp/runtime.rs:68–79`

Each active ACP session spawns one OS thread (8MB stack) and one Node.js process (~50–100MB RSS).
At 10 concurrent sessions: ~500MB–1GB from subprocesses alone. This is inherent to the
process-per-session model. Design should document the expected concurrent session ceiling and
whether a session pool or lazy-attach model is needed.

---

### P3 — Unbounded mpsc in Event Forwarder

**Source:** performance-oracle, learnings-researcher  

The provider→registry channel is unbounded (`mpsc::unbounded_channel()`). If persistence I/O
stalls, events accumulate without backpressure. The institutional learnings database confirms a
prior incident where a broadcast fan-out caused cascading failures — recommended pattern is
**per-subscriber bounded mpsc** (cap ~1000 msgs) rather than global broadcast.

---

### P4 — `try_write().expect()` Panic in `hydrate()`

**Source:** performance-oracle, pattern-recognition-specialist  
**File:** `crates/lab/src/acp/registry.rs:207–211`

`hydrate()` uses `try_write().expect("ACP session registry lock unavailable")`. Safe at current
construction time, but would be a panic in library code (lab-apis must not panic per CLAUDE.md).
Fix: use blocking `write()` or enforce single-caller construction through the type system.

---

## Frontend Issues

### F1 — No SSE Reconnect/Lagged Recovery

**Source:** julik-frontend-races-reviewer  
**File:** `apps/gateway-admin/lib/chat/use-session-events.ts:112–153`  
**Severity: HIGH (blocks streaming testing)**

On any failure (network drop, non-OK response, or backend `Lagged` when the 256-slot broadcast
overflows), the code sets `connectionState='error'` and exits permanently. No retry loop, no
backoff, no reconnect with `since=<last_seq>`. The `lastSeqRef` is ready for reconnect but never
used for it. User must reload the page to recover.

**Fix:** Implement an exponential backoff reconnect loop inside `use-session-events.ts`. On
`Lagged` or error, wait a delay, then re-call the SSE endpoint with `?since=<lastSeqRef.current>`.

---

### F2 — Module-Scope Caches Leak Across Mounts

**Source:** julik-frontend-races-reviewer  
**File:** `apps/gateway-admin/lib/chat/use-session-events.ts:12–13`  
**Severity: HIGH**

`sessionEventCache` and `sessionLastSeqCache` are module-scope `Map`s. They accumulate without
eviction and are shared across every `ChatShell` mount, including React StrictMode
double-invocations and HMR. Move to `useRef` or a context provider scoped to component lifetime.

---

### F3 — `handleSend` Has No In-Flight Guard

**Source:** julik-frontend-races-reviewer  
**File:** `apps/gateway-admin/components/chat/chat-shell.tsx:224–232`  
**Severity: HIGH**

No lock, no state check. Double-tap or fast keyboard submit fires two concurrent POST requests to
`/sessions/:id/prompt`. Both responses stream simultaneously into the transcript, interleaving two
assistant turns under the same synthetic `activeAssistantMessageId`.

**Fix:** Set an `isSending` ref or state on submit; guard the handler and disable the send button.

---

### F4 — Bootstrap `useEffect` Can Fire Twice

**Source:** julik-frontend-races-reviewer  
**File:** `apps/gateway-admin/components/chat/chat-shell.tsx:204–211`  
**Severity: HIGH**

`shouldAutoCreateInitialRun()` checks `providerHealth?.ready && runs.length === 0`. But
`providerHealth` is replaced with a new object identity on every `refreshProvider` call, so if
`refreshProvider` fires during the window when conditions are met, the effect re-runs and creates a
second bootstrap session. Result: two unnamed empty sessions on first load.

**Fix:** Add a `hasBootstrappedRef = useRef(false)` guard set to `true` after the first
`createSession` call.

---

### F5 — TypeScript Layer 2 Abstraction Missing

**Source:** kieran-typescript-reviewer  
**Files:** `apps/gateway-admin/components/chat/types.ts`, `lib/chat/session-events.ts`

`ActivityItem = BridgeEvent` is a direct type alias — no Layer 2 canonical render model exists.
This directly violates the design's 3-layer event-to-render contract. `BridgeEvent` is a flat bag
where all 14 event kinds share one interface with every field `?`-optional — TypeScript cannot
narrow which fields are present from `kind`.

**Critical gaps:**
- `TranscriptToolCall.input`, `.output`, `.content` typed as `unknown` — SDK types available
- `toolContent as unknown[]` widening cast strips `ToolCallContent[]` SDK type
- Import alias `~/` in `chain-of-thought.tsx` won't resolve with codebase's `@/` convention

**Fix:** Define Layer 2 discriminated union types in `lib/acp/render-model.ts`. Update
`deriveTranscriptAndActivity` to return `{ turns: AssistantTurn[] }` instead of
`{ messages: ACPMessage[] }`.

---

## Structural Debt

### S1 — PendingBridgeEvent / BridgeEvent Field Duplication

**Source:** pattern-recognition-specialist  
**File:** `crates/lab/src/acp/types.rs`, `registry.rs:276–352`

19 identical `Option<T>` fields across both structs. The `push_event` copy block is 27 lines of
field-by-field assignment. Any new event field requires changes in 4 places.

**Fix:** Add `fn finalize(self, id: String, seq: u64) -> AcpEvent` to `PendingAcpEvent` that
constructs the full event. Eliminates the copy block.

---

### S2 — ACP_SESSION_CWD Default Resolution Duplicated

**Source:** pattern-recognition-specialist  
**Files:** `registry.rs:35–39`, `api/services/acp.rs:51–57`

Same env-read logic appears in both places. The API handler should pass `None` — the registry's
own default at lines 71–74 already handles the fallback.

---

### S3 — Usage Data Field Never Populated

**Source:** agent-native-reviewer  
**File:** `crates/lab/src/acp/types.rs:104`, `runtime.rs:push_session_update`

`BridgeEvent.usage: Option<Value>` exists but `push_session_update` never emits it. The
`SessionUpdate::UsageUpdate` variant likely falls into the `other => debug` arm. Violates locked
decision 12 on usage preservation.

---

### S4 — `get_session` Is Dead Code

**Source:** agent-native-reviewer  
**File:** `crates/lab/src/acp/registry.rs:62`

`#[allow(dead_code)]` — `get_session()` is implemented but never wired to any API route or
dispatch action. Should be exposed as `session.get` in Phase 1.

---

### S5 — Persistence Failures Silently Suppressed

**Source:** pattern-recognition-specialist  
**File:** `registry.rs:348–350`

`drop(self.persistence.save_sessions(...).await)` and `drop(sender.send(...))` swallow errors with
no log. Per `OBSERVABILITY.md`, suppressed errors should emit `tracing::warn!`.

---

## Security Issues (Full List)

| # | Severity | Finding | File |
|---|----------|---------|------|
| C4 | HIGH | Subprocess inherits all lab credentials | `runtime.rs:153` |
| C5 | HIGH | Permission auto-selection bypasses consent | `runtime.rs:196` |
| Sec3 | MEDIUM | `cwd` path not validated before subprocess use | `acp.rs:51`, `runtime.rs:155` |
| Sec4 | MEDIUM | Raw provider payloads stored without redaction | `registry.rs:286`, `persistence.rs:56` |
| Sec5 | MEDIUM | No session ownership check on SSE subscription | `acp.rs:105` |
| Sec6 | LOW | Stderr not rate-limited — can flood event buffer | `runtime.rs:177` |
| Sec7 | LOW | `ACP_CODEX_ARGS` naive whitespace split | `runtime.rs:133` |

**Highest priority security remediation order:**
1. Add `.env_clear()` to subprocess Command (C4) — must be before any real credentials in env
2. Implement pending-permission state machine (C5) — before any user-facing deployment
3. Validate `cwd` against configured root (Sec3)
4. Strip/redact `raw`, `raw_input`, `raw_output` at persistence boundary (Sec4)
5. Add `owner_sub` to sessions, validate in `subscribe()` and command handlers (Sec5)

---

## Framework Best Practices Confirmed

- **axum SSE sequence resume:** Application-layer concern. Use `stream::iter(backlog).chain(live_broadcast)`. Emit seq as SSE `id:` field so browser `Last-Event-ID` stays current. Read `Last-Event-ID` header on reconnect.
- **`RecvError::Lagged` auto-heals:** After `Lagged(n)`, the receiver's cursor advances to the oldest retained message — next `recv()` returns a value. Do NOT break the stream on `Lagged`; log it and continue.
- **Broadcast capacity:** Start at 128 per session (medium-frequency event streams). Tune by measurement.
- **`dyn ProviderRuntime` not available:** `async fn in trait` is not object-safe. Use enum dispatch (`enum_dispatch` crate) for the closed provider set. Use `dynosaur` for the dynamic Registry extension point.
- **In-process ACP→gateway bridge:** `Arc`-shared concrete struct through `AppState` is the canonical axum pattern. No loopback HTTP needed.
- **`serde_json preserve_order` feature:** Required before any config file patching writes.
- **connect_timeout on reqwest clients:** Must be set (5s recommended) or health probes hang 75s.

---

## What's Missing (Greenfield — No Prior Art)

The knowledge base confirms **no prior institutional learnings** exist for:
- ACP session streaming architecture
- ContentBlock[] rendering pipeline
- Chain-of-thought / action-flow UI rendering patterns
- Multi-model provider abstraction (Claude, Gemini, Copilot behind one trait)
- shadcn AI + Aurora design system integration

These are genuine greenfield design areas.

---

## Phase 1 Prerequisite Checklist (Before Any Phase 1 Code)

- [ ] Add `content: Option<Vec<Value>>` to event types; stop flattening ContentBlock (C1)
- [ ] Migrate HTTP routes to `action + params` dispatch (C2)
- [ ] Fix `subscribe()` to read from persistence when in-memory seq is insufficient (C3)
- [ ] Add `.env_clear()` to subprocess Command (C4)
- [ ] Design pending-permission state machine (C5)
- [ ] Define `AcpError` in `lab-apis::acp`; remove `ToolError` coupling from registry (C6b)
- [ ] Inject `base_dir: PathBuf` into persistence; remove env reads from `lab-apis`-bound code (C6a)
- [ ] Define `Provider` trait; keep `CodexProvider` in `lab` (C6c)
- [ ] Rename `Bridge*` types to `Acp*` (D3)
- [ ] Define `session.load` semantics in design doc (D4)
- [ ] Add a stub `Target` struct before `target.list` catalog entry (D5)
- [ ] Implement SSE reconnect in `use-session-events.ts` (F1)
- [ ] Move caches to component lifetime (F2)
- [ ] Add `isSending` guard to `handleSend` (F3)
- [ ] Add `hasBootstrappedRef` guard to bootstrap effect (F4)
- [ ] Fix `~/` import alias in `chain-of-thought.tsx` (F5)
- [ ] Wire `session.get` to API and dispatch (S4)
- [ ] Populate `usage` field from `SessionUpdate::UsageUpdate` variant (S3)
