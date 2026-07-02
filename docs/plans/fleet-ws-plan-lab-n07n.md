# Brainstorm: WebSocket fleet transport — device↔master↔peer comms via gateway

Bead: `lab-n07n` · Type:  · Status: open · Labels: brainstorm, plan-reviewed

## Vision
A bidirectional WebSocket fleet transport that integrates with the existing gateway's UpstreamPool. Each deployed lab binary holds one persistent WS to master over Tailscale, carrying both MCP JSON-RPC (routed through the gateway pool) and fleet-specific methods (log streaming, status push, enrollment, peer relay). Fleet == gateway — one catalog, one circuit breaker, one admin UI. Unblocks remote plugin.install, enables live log streaming, and makes device↔device control mediated by master a native capability.

## Requirements
1. WebSocket as a third UpstreamPool transport alongside HTTP streamable and stdio
2. Device→master persistent WS connection over Tailscale with heartbeat + exponential-backoff reconnect
3. JSON-RPC 2.0 wire format; MCP methods routed through gateway dispatch; fleet/* methods routed through fleet handler
4. Durable JSONL queue retained; WS is fast drain path; replay-all FIFO on reconnect
5. Tailnet node identity + device-provisioned token for auth
6. Master-controlled enrollment with policy (tailnet ACL / allowlist); revocable without disconnect
7. Disconnect leaves device in gateway catalog marked Unhealthy (reuses circuit breaker)
8. fleet/peer.invoke: deny-by-default allowlist policy on master; every attempt audited
9. Live log streaming via fleet/log.event notifications
10. Remote command invocation with streaming output via fleet/command.invoke + fleet/command.output notifications
11. Admin UI surfaces connected devices, their advertised MCP tools, peer policy editor, live log tail
12. HTTP phone-home endpoints (/v1/device/{hello,status,metadata,syslog/batch}) deprecated once WS parity achieved

## Non-Requirements
- Direct device↔device WS connections (all peer comms relayed through master)
- Full-mesh topology
- TLS/WSS on the fleet WS (WireGuard already encrypts the tailnet)
- Bearer tokens on the fleet WS (tailnet identity + device token replaces)
- Multi-connection pools per upstream (one WS per device is sufficient for now)
- Offline command queueing (peer.invoke to offline device fails fast; no queue)

## Locked Decisions
- Topology: hub-and-spoke over Tailscale; no direct peer-to-peer
- Direction: device dials master; master accepts N connections
- Wire format: JSON-RPC 2.0
- Two method namespaces on one connection: MCP (gateway-routed) + fleet/* (fleet-handler-routed)
- Auth: tailnet node identity + device-provisioned token; WireGuard handles transport encryption
- Fleet is gateway: WS is the pool's third transport; device enrollment is dynamic upstream registration
- Durable JSONL queue retained; replay-all FIFO on reconnect
- Enrollment: master-controlled via policy; revocable without disconnect
- Disconnect: Unhealthy in catalog (circuit breaker pattern)
- peer.invoke authz: deny-by-default allowlist policy on master; audit log every attempt
- MCP bidirectional features (sampling, elicitation, roots) work natively on WS

## Agent Discretion
- Heartbeat cadence and timeout thresholds (reasonable defaults: 30s ping, 90s dead)
- Reconnect backoff curve (reasonable defaults: 1s→60s exponential with jitter)
- Command output chunk size and line-buffering strategy
- fleet/* method schema versioning approach (method prefix vs params.version)
- Config location for peer policy (reuse config.toml or new fleet.toml)
- Error envelope kinds specific to fleet/* methods (within existing ERRORS.md vocabulary)

## Deferred
- Direct device↔device WS connections — rationale: master-relay covers all stated use cases; revisit if a bulk-transfer use case emerges
- Multi-connection pool per upstream — rationale: one WS per device sufficient for envisioned load
- Offline peer.invoke queueing — rationale: adds complexity; fail-fast is clearer semantics
- gateway-admin live terminal / interactive shell — rationale: log tail is sufficient for phase 1
- Capability-based fine-grained peer permissions — rationale: allowlist is enough for v1; capability tokens can layer on top later

## Sources
- Brainstorm: lab-n07n — self (locked decisions: hub-and-spoke over Tailscale, JSON-RPC 2.0 framing, fleet-is-gateway, durable-queue-retained, tailnet-identity-plus-token auth, master-controlled enrollment, circuit-breaker disconnect handling, deny-by-default peer allowlist)
- File: crates/lab/src/dispatch/upstream/pool.rs:306-2155 — UpstreamPool and existing transport implementations
- File: crates/lab/src/dispatch/upstream/types.rs:13,50-71 — REPROBE_INTERVAL (dead constant) and ToolExposurePolicy
- File: crates/lab/src/config.rs:143-203 — UpstreamConfig and scheme validator
- File: crates/lab/src/device/{runtime,queue,master_client,checkin,log_event}.rs — device phone-home stack
- File: crates/lab/src/api/device/{hello,status,metadata,syslog,fleet,logs}.rs — HTTP phone-home handlers (to deprecate)
- File: crates/lab-apis/src/tailscale/ — Tailscale API client for identity verification
- File: apps/gateway-admin/ — existing admin UI app to extend
- Doc: docs/DEVICE_RUNTIME.md, docs/FLEET_LOGS.md, docs/ERRORS.md, docs/OBSERVABILITY.md, docs/DISPATCH.md
- Research: rmcp 1.4 has no WebSocket transport — must implement custom tokio-tungstenite adapter
- Research: UpstreamPool has no dynamic insert API today — P4 adds one
- Research: Axum included without WS feature — enable in P1

## Phases
- lab-n07n.1: Phase 1 — Gateway WS upstream transport + heartbeat/reconnect (also retrofits REPROBE_INTERVAL)
- lab-n07n.2: Phase 2 — Device-side fleet client + durable queue drain + reconnect
- lab-n07n.3: Phase 3 — fleet/* method namespace (device.enroll, log.event, status.push, peer.invoke, ping, command.invoke)
- lab-n07n.4: Phase 4 — Dynamic gateway enrollment + peer.invoke allowlist policy + audit log
- lab-n07n.5: Phase 5 — gateway-admin UI surfaces + HTTP phone-home deprecation

---

## Review Comments (Epic)

- No additional review comments were recorded in this placeholder block.
- **2026-04-22T22:17:22Z**
- **2026-04-22T22:17:22Z**
- **2026-04-22T22:21:16Z**
- **2026-04-22T22:21:17Z**
- **2026-04-22T22:43:40Z**
- **2026-04-22T22:50:01Z**
- **2026-04-22T23:11:59Z**
- **2026-04-22T23:11:59Z**
- **2026-04-22T23:12:00Z**
- **2026-04-22T23:55:59Z**
- **2026-04-22T23:56:00Z**
- **2026-04-22T23:56:00Z**

---

## Phase 1 — Gateway WS upstream transport + heartbeat/reconnect

Bead: `lab-n07n.1` · Status: open · Parent: lab-n07n

## What

Add WebSocket as a third transport to `UpstreamPool` alongside HTTP streamable and stdio. Implement a generic heartbeat + exponential-backoff reconnect loop that applies to all transports, closing the dead `REPROBE_INTERVAL` gap for HTTP and stdio too. This is the foundation that makes a fleet-connected device appear in the master's gateway catalog as just another upstream.

Because `rmcp` does not ship a WebSocket client transport, this phase includes a custom transport adapter that satisfies rmcp's internal transport contract and carries JSON-RPC 2.0 frames over a `tokio-tungstenite` WebSocket.

## Context

### Current state
- `UpstreamPool` at `crates/lab/src/dispatch/upstream/pool.rs:306` holds `connections: Arc<RwLock<HashMap<String, UpstreamConnection>>>` (line 311).
- Two transports implemented: `connect_http_upstream` (pool.rs:2033-2117) uses `rmcp::transport::StreamableHttpClientTransport`; `connect_stdio_upstream` (pool.rs:2120-2155) uses `rmcp::transport::child_process::TokioChildProcess`.
- Dispatcher `connect_upstream` (pool.rs:2018-2030) picks based on `config.url.is_some()` vs `config.command.is_some()`.
- Scheme validator at `config.rs:188-203` rejects anything other than `http`/`https`.
- `UpstreamConfig` schema: `config.rs:143-172` (name, url, bearer_token_env, command, args, proxy_resources, proxy_prompts, expose_tools, oauth).
- `REPROBE_INTERVAL = 30s` at `types.rs:13` is defined but never scheduled.
- `UpstreamHealth::Healthy` / `Unhealthy { consecutive_failures }` circuit breaker via `record_success_for` / `record_failure_for` (pool.rs:1201).
- Axum 0.7 is in workspace Cargo; **WebSocket feature is NOT enabled**. `tokio-tungstenite` is not a dependency yet.

### Relevant MCP / rmcp
- rmcp 1.4 features enabled: `transport-io`, `transport-child-process`, `transport-streamable-http-server`, `transport-streamable-http-client-reqwest` (lab/Cargo.toml:56-67).
- rmcp transport abstraction is `RoleClient` + `Peer<RoleClient>`; the client side is driven by `RunningService<RoleClient>`.
- We must implement a `WebSocketClientTransport` that behaves like `StreamableHttpClientTransport` at the `RoleClient` service boundary. Review how `StreamableHttpClientTransport` frames requests/responses and mirror it over WS text frames carrying JSON-RPC 2.0.

### Locked decisions from brainstorm
- JSON-RPC 2.0 wire format on the WS
- WireGuard encrypts the tailnet, so plain `ws://` is acceptable (no TLS/wss required when target is a tailnet hostname)
- Disconnect leaves device `Unhealthy` in catalog (do not remove)
- Fleet == gateway: WS is UpstreamPool's third transport
- Circuit breaker reuse — no new health machinery

## Decisions

### Locked
- Transport implemented using `tokio-tungstenite` (client) + `axum` WS feature (server, needed in later phases).
- Framing: JSON-RPC 2.0 text frames, one message per WebSocket text frame. No binary frames in v1.
- Heartbeat: JSON-RPC method `fleet/ping` (the fleet method exists only for side-effect of a response, defined in P3 — for P1 use a placeholder method or rmcp's builtin if applicable).
- Reconnect: exponential backoff 1s → 60s max with ±20% jitter.
- Generic heartbeat/reprobe loop added at the pool level, not per-transport; reuses `record_success_for` / `record_failure_for`.

### Discretion
- Exact heartbeat cadence default (suggest 30s) and dead-threshold (suggest 90s = 3 missed).
- Whether to enable ping/pong WebSocket control frames in addition to application-level heartbeat.
- Internal module layout for the WS transport (e.g., `transport/websocket.rs` vs nested under `upstream/`).
- Exact error envelope kinds emitted by the reprobe loop (stay within `docs/ERRORS.md` vocabulary).

## Testing

Full testing scope (lavra.json = `full`):

- [ ] Unit: `parse_ws_url` accepts `ws://host:port/path` and `wss://host:port/path`, rejects missing scheme or trailing garbage.
- [ ] Unit: JSON-RPC 2.0 frame encoder/decoder round-trips requests, responses, notifications, and error objects.
- [ ] Unit: Exponential backoff calculator returns [1s, 2s, 4s, 8s, 16s, 32s, 60s, 60s, ...] with jitter bounds respected.
- [ ] Integration (wiremock-style WS echo server): `connect_websocket_upstream` succeeds against a mock that mirrors rmcp MCP handshake (`initialize` / `initialized`).
- [ ] Integration: connection drop triggers reconnect loop; `Unhealthy` status set after dead-threshold; `Healthy` restored on next successful heartbeat.
- [ ] Integration: HTTP upstream also gains the reprobe loop — a dead HTTP upstream goes `Unhealthy`, then returns to `Healthy` when the mock comes back up, without manual reload.
- [ ] Negative: malformed JSON frame → `record_failure_for` called; connection not poisoned unless threshold reached.
- [ ] Negative: backoff caps at 60s even on infinite failure sequence.

## Validation

- [ ] `cargo build --all-features` compiles.
- [ ] `cargo test --all-features` passes all new tests.
- [ ] `ws://` and `wss://` schemes accepted by `UpstreamConfig` validation.
- [ ] An `UpstreamConfig { url: Some("ws://localhost:9999/mcp"), ... }` entry in config.toml is discovered by the pool and its tools appear in `lab gateway list-tools` output.
- [ ] Manual verification: kill the mock WS server; after dead-threshold, `lab gateway status` shows the upstream as Unhealthy; bring it back; pool self-heals within one reprobe cycle.
- [ ] No regression in HTTP or stdio transports.
- [ ] `cargo clippy --all-features -- -D warnings` clean.

## Files

- `crates/lab/src/dispatch/upstream/pool.rs` — add `connect_websocket_upstream`; extend `connect_upstream` dispatcher; add reprobe loop orchestrator
- `crates/lab/src/dispatch/upstream/transport/websocket.rs` (new) — custom rmcp transport adapter using `tokio-tungstenite`
- `crates/lab/src/dispatch/upstream/transport/mod.rs` or `transport.rs` (new) — module declaration
- `crates/lab/src/dispatch/upstream/types.rs` — wire `REPROBE_INTERVAL` into the new reprobe loop
- `crates/lab/src/config.rs:188-203` — extend scheme validator to accept `ws`/`wss`
- `crates/lab/src/config.rs:143-172` — no field changes; `url` field already accepts the new schemes
- `crates/lab/Cargo.toml` — add `tokio-tungstenite` dep; enable axum `ws` feature (server WS lands in P3 but feature gate here is fine)

## Dependencies

None. This is the foundation phase.

## References

- File: `crates/lab/src/dispatch/upstream/pool.rs:2018-2155` — reference patterns for the two existing transports
- File: `crates/lab/src/dispatch/upstream/pool.rs:311` — connections HashMap to manage
- File: `crates/lab/src/dispatch/upstream/pool.rs:1201` — circuit breaker API
- File: `crates/lab/src/dispatch/upstream/types.rs:13` — `REPROBE_INTERVAL` constant
- File: `crates/lab/src/config.rs:143-203` — UpstreamConfig + scheme validator
- File: `lab/Cargo.toml:56-67` — rmcp feature flags
- Doc: `docs/ERRORS.md` — error kind vocabulary
- Brainstorm: lab-n07n — locked decisions (JSON-RPC 2.0, fleet-is-gateway, circuit-breaker reuse)


## Research Findings (Phase 1)

### Transport shape (locked)
- rmcp 1.4 ships NO WebSocket transport; confirmed via crate docs. Use rmcp's `IntoTransport` blanket impl over `futures::Sink + futures::Stream` — wrap `WebSocketStream<TokioStream>` from `tokio-tungstenite`. This is the path of least resistance; avoid implementing the `Transport` trait or `Worker` pattern directly unless a specific need emerges.
- Axum `ws` feature is not enabled in `lab/Cargo.toml` today — add it in this phase (server WS lands in P3 but feature gate belongs here).

### Concurrency pattern (locked)
- Split socket via `StreamExt::split()` into reader+writer tasks. Reader → bounded `mpsc` (capacity ~100) → dispatcher. Writer task serialized (preserves JSON-RPC id correlation).
- Per-connection writer loop uses `tokio::select!` racing `socket.recv()` vs `heartbeat_ticker.tick()`.
- On inbound-channel overflow: log WARN + drop oldest (heartbeats first). Do NOT use unbounded channels.

### Heartbeat (refines Discretion)
- Use **WebSocket Ping/Pong control frames** (RFC 6455) for liveness, NOT an application-level `fleet/ping` request-response. Application-level `fleet/ping` stays in the catalog for reprobe semantics at the UpstreamPool layer, but dead-connection detection is at the WS layer.
- Ping cadence: 30s. Pong timeout: 90s → mark Unhealthy.

### Reprobe (new risk mitigation)
- Thundering-herd risk: at 1000x scale, 10K devices simultaneously reprobe an unhealthy upstream every 30s. REQUIRED: exponential backoff 30s → 60s → 120s → 300s (reset on success) + ±5s jitter. Add `reprobe_attempt_count` to `UpstreamEntry`.
- UpstreamPool `RwLock<HashMap>` contention emerges at 100x+ (~10% CPU lost to locking at 100x; 20-30% at 1000x). **Accepted into P1 scope** via the Engineering Review Addendum below: land the `DashMap` swap here rather than deferring it to P4, and avoid adding new hot-path writes while the transition is in progress.

### Frame-level security limits (new, REQUIRED)
Set on `tokio_tungstenite::tungstenite::protocol::WebSocketConfig`:
- `max_message_size: 10 MiB`
- `max_frame_size: 128 KiB`
- Idle connection timeout: 5 min
- Pong timeout: 30 s → auto-close
- Rate limit: 100 msgs/sec per device (token bucket)

Rationale: without these, a hostile or buggy device can exhaust master RAM via fragmentation-bomb or slowloris attacks. These are P1 concerns because the transport is where they must be enforced.

### Auth fields for handshake (new)
- Device token passed in `Sec-WebSocket-Protocol` header on the upgrade request — **NEVER** query string (URLs leak to browser history, proxy logs, Referer). Master parses subprotocol list in the custom axum extractor.
- Origin header validation for browser-facing WS (lands in P5 but precedent set here).

### Additional tests (append to Testing)
- [ ] Frame-limit tests: 11 MiB message → connection closed with clean status; 129 KiB frame → same.
- [ ] Slowloris: write 1 byte/minute → idle timeout fires.
- [ ] Reprobe storm under induced failure: 100 concurrent connections, kill upstream, verify reprobes backoff exponentially (not all firing at T+30s).

### Risks
- **HIGH (perf)** — UpstreamPool RwLock contention at 100x+ is real and is in P1 scope via the DashMap swap required by the Engineering Review Addendum. Do not defer this to P4 or the Deferred tracker.
- **MEDIUM (correctness)** — If `max_message_size` is unset, a single rogue upstream can OOM the master. Always set config limits before accepting frames.
- **MEDIUM (impl risk)** — `IntoTransport` adapter over `tokio-tungstenite` is the intended path but unvalidated in this codebase. First task: spike a minimal round-trip `initialize` → `initialized` against a wiremock WS server to prove the adapter shape before building the reprobe loop on top.

## CEO Review Addendum (Phase 5a)

### Backpressure policy (CRITICAL)
Device WS outbound: bounded mpsc with **1s send timeout**. On timeout:
- Close WS connection (graceful Close frame if possible)
- Log `WARN fleet.backpressure_disconnect`
- Reconnect + durable-queue replay resumes FIFO
- No silent drops (Prime Directive #1)

### Thundering-herd protection
Master SIGTERM handler: broadcast `fleet/server.drain` notification to all connected devices **before** shutdown. Devices apply existing ±5s jitter + exp backoff on reconnect, staggering re-convergence.

### Observability (first-class scope)
Emit these metrics from master:
- `fleet.devices_connected` (gauge, labels: tailnet_node) — current connected device count
- `fleet.ws_reconnect_count` (histogram, labels: device_id, reason) — reconnect distribution
- `fleet.ws_accept_total` / `fleet.ws_close_total` (counters)

WS-specific tracing events (parity with HttpClient request.start/finish):
- `ws.accept` (master side, device_id, tailnet_node)
- `ws.close` (code, reason, connected_duration_ms)
- `ws.reconnect_attempt` (device side, attempt_num, backoff_ms)

### Docs
- Add `docs/FLEET_WS.md` (new): transport spec, frame limits, heartbeat, reconnect semantics
- Update `docs/DISPATCH.md`: WS as third upstream transport

## Engineering Review Addendum (Phase 5b)

### DashMap swap (moved from P4 — perf CRITICAL)
Swap `Arc<RwLock<HashMap<UpstreamId, UpstreamConnection>>>` → `DashMap<UpstreamId, UpstreamConnection>` in `UpstreamPool` as part of P1 (not deferred to P4). Trivial refactor, eliminates 20-30% CPU at 100× scale. Catalog map gets the same treatment. Benchmark: 1000 concurrent enrollments < 30s, < 10% CPU overhead.

### WebSocketConfig enforcement (CRITICAL)
Apply `WebSocketConfig { max_message_size: Some(10 * 1024 * 1024), max_frame_size: Some(128 * 1024), accept_unmasked_frames: false, .. }` at socket accept (master) and dial (device) sites explicitly. Add unit test: craft 11 MiB message, assert connection closes with status code 1009 (message too big).

### Reprobe fairness (HIGH)
Add `Semaphore(16)` gating concurrent reprobe attempts in the pool. Per-device initial reprobe offset: `hash(device_id) % 5000ms`, so convergence after partition spreads over 5s. WARN log if `reprobe_queue_depth > 100`.

### Sec-WebSocket-Protocol header redaction (MED security)
Add tower/axum middleware on `/v1/fleet/ws` route that strips `Sec-WebSocket-Protocol` header value before any tracing span or request log emits. Device side: tracing `fmt` filter excludes the header field. Unit test: INFO log of a handshake must not contain the token substring.

### Thundering herd + drain signal (moved from CEO §5a — clarified)
Initial reconnect on timeout-detected disconnect applies `random(0, 5s)` initial jitter in addition to exponential backoff; prevents cold-start burst after master crash where no drain signal was sent.

### Child Bead Comments

- No additional child-bead comments were recorded in this placeholder block.

---

## Phase 2 — Device-side fleet client + durable queue drain

Bead: `lab-n07n.2` · Status: open · Parent: lab-n07n

## What

Device-side WebSocket client that dials master over Tailscale, performs the auth handshake (tailnet identity proof + device-provisioned token), maintains the connection with heartbeat and exponential-backoff reconnect, and drains the durable JSONL queue (`~/.labby/device-runtime-queue.jsonl`) into the WS as fleet notifications. On outage, writes accumulate on disk and replay FIFO on reconnect with zero data loss.

## Context

### Current state
- Device runtime at `crates/lab/src/device/runtime.rs:15-209` orchestrates startup (`send_initial_hello`, `upload_initial_metadata`) and calls `flush_queue_once()` manually.
- Durable queue at `crates/lab/src/device/queue.rs:1-181`: JSONL with atomic rewrite on ack; two envelope kinds today (`syslog_batch`, `status`), both HTTP-bound via `MasterClient`.
- `MasterClient` at `crates/lab/src/device/master_client.rs:1-89` wraps `DeviceRuntimeClient` (HTTP only).
- `DeviceLogEvent` at `crates/lab/src/device/log_event.rs` — normalized event schema.
- Tailscale integration exists as a read-only service client (`lab-apis/tailscale`, feature-gated in `lab/Cargo.toml:101`) — can be used to resolve the master's tailnet hostname and to fetch our own tailnet identity for the handshake.
- No existing WebSocket client code in the workspace.

### Handshake design
On connect, device sends first JSON-RPC 2.0 request:
```json
{"jsonrpc":"2.0","id":1,"method":"initialize",
 "params":{"protocolVersion":"2024-11-05",
           "capabilities":{},
           "clientInfo":{"name":"lab-device","version":"..."},
           "_meta":{"lab.device_id":"<hostname>",
                    "lab.device_token":"<base64>",
                    "lab.tailnet_identity":{"node_key":"...", "login_name":"..."}}}}
```
Master validates tailnet identity against peer IP via Tailscale API, validates token signature, and either accepts (sends `initialize` response) or rejects (closes WS with structured error). `_meta` namespace keeps the fields out of MCP's spec surface.

### Locked decisions from brainstorm
- Device dials master (not reverse)
- Tailnet identity + device-provisioned token for auth
- Durable queue retained; WS is the fast drain path
- Replay everything in FIFO order on reconnect
- Graceful degradation: if WS unavailable, keep writing to queue; existing HTTP phone-home path remains as secondary until P5 deprecation

## Decisions

### Locked
- Client uses `tokio-tungstenite` (shared crate with P1, enabled in P1's Cargo.toml edit).
- Reconnect backoff matches pool-side curve (1s→60s jittered).
- Heartbeat cadence matches P1 default (30s ping; 90s dead-threshold).
- Device provisioning token stored at `~/.labby/device-token` with 0600 perms; generated on first connect if absent (master issues on first successful hello); renewed on master reset.
- Queue drain: on connect, read all envelopes in order, send each as the appropriate fleet method (`fleet/log.event` for `syslog_batch`, `fleet/status.push` for `status`), ack on master-confirmed receipt, remove from disk.

### Discretion
- Whether to retain a single persistent connection or re-dial on every reconnect event (suggest single persistent).
- Whether to cap in-memory pending-ack buffer size (suggest yes, with backpressure if queue grows faster than drain).
- Exact shape of device token provisioning flow (REST endpoint vs first-WS-message).
- Whether to use `tsnet` library or shell out to `tailscale status --json` for own-identity lookup.

## Testing

- [ ] Unit: JSON-RPC request/response/notification builder helpers produce spec-valid objects.
- [ ] Unit: handshake builder includes all required `_meta` fields.
- [ ] Unit: queue envelope → fleet method mapping (`syslog_batch` → `fleet/log.event`, `status` → `fleet/status.push`).
- [ ] Integration (mock WS server echoing acks): device drains a pre-seeded queue file in order, disk state reduces to empty after successful flush.
- [ ] Integration: mock WS drops connection mid-drain; device retries with backoff; completes drain on reconnect without duplicates.
- [ ] Integration: WS unavailable at startup; device continues to enqueue; reconnect on return drains accumulated queue.
- [ ] Integration: malformed auth token → master rejects → device does not retry storm (respects exponential backoff, logs WARN once per backoff window).
- [ ] Negative: queue file corruption (truncated line) → device skips bad line, logs WARN, drains remainder.

## Validation

- [ ] `cargo build --all-features` compiles.
- [ ] `cargo test -p lab` passes all new device-side tests.
- [ ] `lab serve` on a non-master device with master reachable opens and maintains a persistent WS connection (verify via master's `lab gateway status`).
- [ ] Disk-level test: create 100 synthetic queue entries, start WS client, verify queue empties and 100 events arrive at mock master in order.
- [ ] Kill the mock master → device queues events → restart master → device reconnects and drains.
- [ ] `lab doctor` on device reports WS connection status.
- [ ] Existing HTTP phone-home path (`send_initial_hello`, `post_status`, etc.) still works — do not delete.
- [ ] `cargo clippy --all-features -- -D warnings` clean.

## Files

- `crates/lab/src/device/ws_client.rs` (new) — WS client, handshake, heartbeat, reconnect, queue drain orchestrator
- `crates/lab/src/device/master_client.rs` — add a `maybe_ws_client: Option<Arc<WsClient>>` slot; prefer WS when connected
- `crates/lab/src/device/runtime.rs:99-150` — spawn the WS client task at startup; keep `flush_queue_once` as fallback when WS is down
- `crates/lab/src/device/queue.rs` — extend envelope kinds if needed; keep durability invariants
- `crates/lab/src/device/token.rs` (new) — load/generate/rotate device-provisioned token
- `crates/lab/Cargo.toml` — no new deps (tungstenite already added in P1)

## Dependencies

- Depends on `lab-n07n.1` — shares the WebSocket transport wrapper and JSON-RPC framing helpers introduced in P1 (pool-side), re-exported or duplicated for device-side use.

## References

- File: `crates/lab/src/device/runtime.rs:65-150` — startup sequence to integrate with
- File: `crates/lab/src/device/queue.rs:1-181` — durable queue API
- File: `crates/lab/src/device/master_client.rs:1-89` — existing HTTP client wrapper
- File: `crates/lab-apis/src/tailscale/` — Tailscale API client for tailnet identity lookup
- Doc: `docs/DEVICE_RUNTIME.md` — role resolution + auth current state
- Brainstorm: lab-n07n — device-dials-master; durable-queue-retained; replay-all-FIFO


## Research Findings (Phase 2)

### Queue write-amplification (CRITICAL, scope change)
Current `crates/lab/src/device/queue.rs:126-180` rewrites the **entire** JSONL file on every ack. At realistic scale this becomes:
- 100-msg queue: ~30 KB rewrite per ack (negligible)
- 10K-msg queue (10x): ~3 MB rewrite per ack (noticeable)
- 100K-msg queue (100x): ~30 MB rewrite + fsync per ack; batches take ~600 ms each
- 1M-msg queue (1000x): full reconnect drain takes **~1.6 hours**

**Required P2 mitigation (was: "keep existing JSONL"):** switch to a **segment-based queue**:
- 1 MiB rotating segments; immutable after rotation.
- Append always writes to current segment.
- Ack deletes fully-drained segments (no rewrite).
- Memory cost per drain: O(1 segment), not O(queue).
- Expected reduction in write amplification: **~1000×**.
- Implementation complexity: MEDIUM. Counts as ~300–500 LOC and pushes this bead near its ~1000 LOC budget. If budget risk materializes, split into P2a (WS client + drain with current queue) and P2b (segment queue) — P2a depends on P2b for correctness at scale but can be shipped first with a known ceiling.

### Fsync cadence (new decision)
- Batch acks: buffer 10 acks → single `fsync`. ~80% reduction in ack-latency.
- Append-path stays un-fsync'd (OS buffer) — acceptable for telemetry; device replay handles crash loss.

### At-least-once delivery (refines handshake)
- Add `idempotency_id: Uuid` field to every `QueuedEnvelope`.
- Master dedupes on `(device_id, idempotency_id)` via LRU (24 h TTL).
- Drain uses JSON-RPC **Requests** (with ids), NOT Notifications. Device must see master-ack before removing from disk; Notifications cannot acknowledge by spec.
- Monotonic sequence number per device so master can reject out-of-order replays (`409 Conflict` → device re-fetches and continues).

### Concurrency discipline (new, REQUIRED)
- All queue reads/writes use `tokio::fs` or `spawn_blocking`. Never `std::fs` in the async context — blocks tokio workers and starves unrelated tasks. Codified in lab CLAUDE guidance (lab-yh7/us3/jcq precedents).

### Handshake auth (security, new)
- Bearer token must be scrubbed from `system.vars`, metadata snapshots, and debug logs (lab-e27 precedent applies). Allowlist pattern: only approved env keys exported.
- Provisioning token storage: **out of plaintext `~/.labby/.env`**. Use OS keyring — `libsecret` (Linux), `Keychain` (macOS), `Credential Manager` (Windows). If keyring integration is too large for this bead, fall back to `~/.labby/device-token` with `0600` perms as already planned, but flag as follow-up hardening.
- Token lifetime: 15–60 min with refresh endpoint. Master keeps revocation allowlist in-memory (or SQLite for multi-master). Revoked token denied on next reconnect; active connections are NOT force-closed by revocation alone.

### Additional tests (append to Testing)
- [ ] Segment rotation: fill a 1 MiB segment → new segment created; old segment deletable after its last ack.
- [ ] Idempotency: master receives same `(device_id, idempotency_id)` twice → second is a no-op, returns cached result.
- [ ] Sequence number out-of-order: master returns `409 Conflict` → device resets sequence to master's last-seen + 1.
- [ ] Crash during drain: kill device mid-drain → on restart, remaining segment replayed; master dedupes any double-sends.
- [ ] Token scrubbing: `system.vars` output and metadata snapshots contain no token material.

### Risks
- **CRITICAL (perf)** — current full-file rewrite is a production-blocking bug at 100x+ fleet size. Segment queue is a P2 scope addition, not a follow-up.
- **HIGH (security)** — plaintext `~/.labby/device-token` is acceptable for lab-grade trust but document the keyring upgrade path.
- **MEDIUM (scope)** — P2 may exceed ~1000 LOC budget; pre-plan the P2a/P2b split.

## CEO Review Addendum (Phase 5a)

### ENOSPC handling (CRITICAL)
Segment queue disk-full policy:
- On `io::Error` with `kind() == ErrorKind::StorageFull` (or platform equivalent) during segment rotate or append:
  - Emit `ERROR fleet.queue_disk_full` with bytes_used, bytes_needed
  - Set `fleet.queue_disk_full` gauge = 1
  - In-memory ring buffer holds last N=1000 events as fallback
  - New events dropped with `WARN` (rate-limited 1/sec)
  - Device reports `device.status.degraded = true` in next heartbeat
  - Device stays connected; does NOT crash

### Observability (first-class scope)
- `fleet.queue_depth` (gauge) — segments × entries-per-segment estimate
- `fleet.queue_disk_bytes` (gauge) — total queue dir size
- `fleet.queue_drain_rate` (histogram) — entries drained per second
- `fleet.queue_disk_full` (gauge, 0/1)

Structured events:
- `queue.segment.rotate` (old_segment, new_segment, bytes, entries)
- `queue.replay.start` (segments, total_entries)
- `queue.replay.finish` (entries, duration_ms)

### Docs
- Update `docs/DEVICE_RUNTIME.md`: segment queue layout, rotate threshold (1 MiB), replay semantics, ENOSPC policy

## Engineering Review Addendum (Phase 5b)

### Per-connection session UUID (CRITICAL security)
Handshake flow:
1. Device sends `initialize` with device token
2. Master validates token + tailnet identity, generates `session_uuid: Uuid::new_v4()`, returns in `initialize` response
3. Device includes `session_uuid` in every outbound frame's `_meta.session_id`
4. Master validates session_uuid on every frame; mismatch → close with kind `session_invalid`
5. Revocation by device_id closes all sessions for that device in one operation (`pool.close_device_sessions(device_id)`)

Session UUIDs are ephemeral (in-memory); no persistence needed.

### Segment queue fsync via spawn_blocking (CRITICAL perf)
Segment writes (append + rotate) MUST run on blocking pool:
```rust
tokio::task::spawn_blocking(move || {
    file.write_all(&bytes)?;
    file.sync_all()?;  // fsync
    Ok(())
}).await?;
```
Reader task never blocks on fsync. Rotation index update is its own spawn_blocking, ordered after segment fsync completes. Test: simulate slow disk (100ms fsync), assert reader task continues enqueuing.

### Token redaction in device logs (MED security)
Device tracing fmt filter excludes any field matching `token`, `auth`, `secret`, `session` (case-insensitive). Token file read path never logs the value.

### Child Bead Comments

- No additional child-bead comments were recorded in this placeholder block.

---

## Phase 3 — fleet/* method namespace handlers

Bead: `lab-n07n.3` · Status: open · Parent: lab-n07n

## What

Define the `fleet/*` method namespace and implement handlers on both sides (master + device). These are custom JSON-RPC 2.0 methods that share the WebSocket with standard MCP methods but route to a separate fleet dispatch handler. MCP-strict peers return `-32601 method_not_found` for `fleet/*` by design — no spec extension, no namespace pollution.

Also mount the master-side WebSocket accept endpoint at `/v1/fleet/ws` and wire the JSON-RPC method demux that splits traffic between the gateway `UpstreamPool` (MCP methods) and the fleet handler (`fleet/*` methods).

## Context

### Current state
- Axum router assembly at `crates/lab/src/api/router.rs` mounts device and MCP routes around line 527-530; no WS endpoint exists yet.
- MCP server is an rmcp `StreamableHttpService` at `/mcp` (cli/serve.rs:702, 759); not a pure axum WS.
- Device HTTP handlers under `crates/lab/src/api/device/` (hello.rs, status.rs, metadata.rs, syslog.rs, fleet.rs, logs.rs, oauth.rs) must continue to work — this phase does NOT deprecate them.
- Device-side WS client from P2 expects `fleet/log.event` and `fleet/status.push` methods to be handled on master.

### Method catalog
All methods use JSON-RPC 2.0. `params` is always a named object.

| Method | Direction | Kind | Purpose |
|---|---|---|---|
| `fleet/ping` | both | request | Heartbeat; response carries `{ts, device_id}`. Used by P1 reprobe loop. |
| `fleet/device.enroll` | device→master | request | Advertise tools/resources/prompts + metadata; master applies policy (P4 hook). |
| `fleet/log.event` | device→master | notification | Stream one `DeviceLogEvent`. Replaces POST /v1/device/syslog/batch. |
| `fleet/status.push` | device→master | notification | Stream `DeviceStatus` delta. Replaces POST /v1/device/status. |
| `fleet/peer.invoke` | device→master→device | request | Relay an action to another device; master does authz (P4) + audit. |
| `fleet/command.invoke` | master→device | request | Invoke a local command on device with streaming output. |
| `fleet/command.output` | device→master | notification | Partial output chunk; correlated via `params.correlation_id`. |
| `fleet/command.result` | device→master | response | Final command result; uses JSON-RPC `result` with matching `id`. |

### Locked decisions from brainstorm
- JSON-RPC 2.0 wire format; two method namespaces on one connection
- Fleet methods are NOT MCP extensions; shared envelope only
- Peer operations are master-relayed (no direct device-to-device)
- Command output streamed as correlated notifications

## Decisions

### Locked
- Method names use `fleet/` prefix with `.` sub-separator (`fleet/command.invoke`) — grep-friendly, not confused with MCP's `/` sub-separator (`tools/call`).
- Demux at the JSON-RPC router level: method starts with `fleet/` → fleet handler; otherwise → MCP (gateway) dispatch.
- Schema validated at handler entry; invalid params return `-32602 invalid_params` with a structured `data.kind` matching `docs/ERRORS.md`.
- `fleet/command.invoke` streams output as `fleet/command.output` notifications until a `fleet/command.result` response closes the request.
- `fleet/device.enroll` in this phase is a stub: it accepts the enrollment payload and returns success. Real policy evaluation lands in P4.
- `fleet/peer.invoke` in this phase routes the call to a placeholder handler that returns `method_not_found` unless the action is specifically allowed; full allowlist engine lands in P4.

### Discretion
- Exact params schema per method (document in `docs/FLEET_METHODS.md` or similar).
- Whether to generate method schemas from Rust types via serde or hand-author JSON Schema.
- Handler module layout (suggest `crates/lab/src/fleet/methods/*.rs` per method family).

## Testing

- [ ] Unit: JSON-RPC demux routes `fleet/*` → fleet handler, non-fleet → MCP dispatch.
- [ ] Unit: each `fleet/*` method's params parser accepts valid input, rejects invalid with `invalid_params`.
- [ ] Integration: device sends `fleet/log.event` notification; master's DeviceFleetStore receives the event.
- [ ] Integration: device sends `fleet/status.push` notification; master's DeviceFleetStore updates status.
- [ ] Integration: master sends `fleet/command.invoke { command: "echo hi" }`; device responds with output notifications then `result`.
- [ ] Integration: concurrent `fleet/command.invoke` calls with different ids multiplex correctly — outputs routed via correlation_id without cross-talk.
- [ ] Integration: `fleet/peer.invoke` stub returns `method_not_found`-equivalent until P4 enables it.
- [ ] Integration: unknown `fleet/*` method returns `-32601 method_not_found` with a helpful hint.
- [ ] Integration: MCP `tools/call` on the same WS connection still reaches the gateway pool (demux proved).
- [ ] Negative: oversized output chunk → master backpressures or drops with a clear error kind.
- [ ] Contract test: payload size limit per frame enforced (suggest 1 MiB default).

## Validation

- [ ] `cargo build --all-features` compiles.
- [ ] `cargo test --all-features` passes.
- [ ] `/v1/fleet/ws` endpoint accepts WebSocket upgrade with valid handshake.
- [ ] A connected device appears in the master's connection list (fleet state) after sending `fleet/device.enroll`.
- [ ] Log events flowing via `fleet/log.event` are visible in `GET /v1/device/devices/{id}` response alongside HTTP-path events.
- [ ] `lab gateway list-tools` shows no duplication from the fleet WS (enrollment is stubbed in P3; real catalog registration lands in P4).
- [ ] `cargo clippy --all-features -- -D warnings` clean.
- [ ] `docs/FLEET_METHODS.md` (new) documents every method's shape and error kinds.

## Files

- `crates/lab/src/fleet/mod.rs` or `crates/lab/src/fleet.rs` (new) — module entry
- `crates/lab/src/fleet/methods/mod.rs` + per-method files: `ping.rs`, `enroll.rs`, `log_event.rs`, `status_push.rs`, `peer_invoke.rs`, `command.rs`
- `crates/lab/src/fleet/demux.rs` (new) — JSON-RPC method router splitting fleet vs MCP
- `crates/lab/src/fleet/frame.rs` (new) — JSON-RPC 2.0 frame types, serde helpers (may merge with P1's helpers)
- `crates/lab/src/api/fleet/ws.rs` (new) — axum handler that upgrades to WS and wires demux
- `crates/lab/src/api/router.rs` — mount `/v1/fleet/ws` route alongside device routes (~line 527-530)
- `crates/lab/src/device/ws_client.rs` — extend with outbound notification senders for `fleet/log.event` and `fleet/status.push` (and handler for inbound `fleet/command.invoke`, `fleet/ping`)
- `docs/FLEET_METHODS.md` (new) — method catalog documentation

## Dependencies

- Depends on `lab-n07n.1` (gateway WS upstream transport: shares frame types and transport primitives)
- Depends on `lab-n07n.2` (device client: sends `fleet/log.event` / `fleet/status.push` notifications; handles inbound `fleet/command.invoke`)

## References

- File: `crates/lab/src/api/router.rs:527-530` — route mount location
- File: `crates/lab/src/cli/serve.rs:702,759` — MCP service mounting pattern (for reference; fleet WS is pure axum, not rmcp)
- File: `crates/lab/src/device/log_event.rs` — `DeviceLogEvent` shape for `fleet/log.event`
- File: `crates/lab/src/device/checkin.rs` — `DeviceStatus` / `DeviceMetadataUpload` shapes
- Doc: `docs/ERRORS.md` — error kind vocabulary
- Doc: `docs/DISPATCH.md` — dispatch layer contracts
- Brainstorm: lab-n07n — method namespace design, command streaming, peer relay


## Research Findings (Phase 3)

### JSON-RPC 2.0 method routing (codified)
- Route by method-name prefix on the master demux:
  - `fleet/*` → fleet handler
  - `gateway/*` or standard MCP (`tools/call`, `tools/list`, `resources/read`, `prompts/*`, etc.) → UpstreamPool dispatcher
  - anything else → `-32601 method_not_found`
- JSON-RPC reserves only `rpc.*` for system; `fleet/*` and `gateway/*` are user conventions — safe to use.
- `id` collision discipline on a bidirectional socket:
  - Device uses u32 counter in `[1, 2^31)` for its outbound requests.
  - Master uses u32 counter in `[2^31, 2^32)` for its outbound requests.
  - rmcp's Peer correlator handles response routing natively; no custom correlator needed.

### Request vs Notification discipline (codified)
- `fleet/log.event`, `fleet/status.push`, `fleet/command.output` → **Notifications** (unsolicited, fire-and-forget; no id).
- `fleet/ping`, `fleet/device.enroll`, `fleet/peer.invoke`, `fleet/command.invoke` → **Requests** (id required; response expected).
- Durable-queue drain MUST use Requests so the device sees master-ack before deletion. Notifications cannot acknowledge and silently drop failures per spec.

### Audit correlation (new, REQUIRED)
- `device_id` threaded through every dispatch event as a **tracing field**, never global state. Use `task_local!` or structured span context so async handlers see the right device under concurrent connections. Precedent: `lab-qhzr` activity-log tracing captures this via `FieldVisitor`.
- Span structure: `fleet.dispatch { surface="fleet", service="fleet", action=<method>, device_id=..., elapsed_ms=..., kind=<error_kind on failure> }` — conforms to `docs/OBSERVABILITY.md` dispatch-log conventions.

### Enrollment single-use tokens (P4 precursor, surfaced here)
- `fleet/device.enroll`'s session/state tokens should use `DELETE ... RETURNING` single-use semantics (lab-iwtf.12 OAuth precedent). Replay window closes automatically on both success and failure paths.

### Backpressure on command output (new)
- `fleet/command.output` can stream unbounded. Bounded `broadcast` (or `mpsc`) with 100-frame capacity. Lag beyond capacity → emit a `fleet/command.error { kind: "output_truncated" }` and cancel the call. Do NOT let a slow subscriber stall the command task.

### Additional tests (append to Testing)
- [ ] Concurrent `fleet/command.invoke` from master to 10 devices — correlation_id fan-out verified end-to-end.
- [ ] Request id isolation: device sends id=1, master sends id=(2^31)+1 simultaneously — both correlate correctly.
- [ ] Notification with id field present → treated as Request OR rejected with `invalid_request`, not silently acked (behavior chosen and documented in FLEET_METHODS.md).
- [ ] Tracing field assertion: in an integration test, assert every `fleet/*` log line contains `device_id`.

### Risks
- **MEDIUM (correctness)** — id-space split (`1..2^31` vs `2^31..2^32`) is enforceable only if both sides agree. Codify in `docs/FLEET_METHODS.md` and unit-test the boundary.
- **LOW (obs.)** — if `device_id` isn't threaded through spans, audit logs lose their join key and P4's `fleet/peer.invoke` becomes forensically unusable.

## Engineering Review Addendum (Phase 5b)

### Command output redaction + size cap (HIGH security+perf)
`fleet/command.output` notification handling on master:
1. **Pre-log scrubber** — before emitting tracing event, redact substrings matching: `/\.ssh/`, `/\.aws/`, `token[=:][^\s]+`, `api[_-]?key[=:][^\s]+`, `password[=:][^\s]+`, `Authorization:\s+\S+`. Replace with `<redacted>`.
2. **Per-command cumulative cap** — track bytes_emitted per `request_id`. On exceed 256 MiB:
   - Send `fleet/command.error { kind: "output_truncated", bytes_emitted: N }` to caller
   - Send `fleet/command.cancel { request_id }` back to device (bidirectional cancel — device stops spawning)
   - Rate-limited WARN log: 1 per 10s per device
3. **Test:** device streaming `dd if=/dev/zero bs=1M count=300` must terminate with `output_truncated` at ~256 MiB, device receives cancel, does not OOM master.

### Bidirectional cancel protocol (arch HIGH)
On slow subscriber drop / output cap / client disconnect, master sends `fleet/command.cancel { request_id }` to device. Device handler polls for cancel between chunks (`select!` over output chunk + cancel channel). Prevents orphaned subprocess output flooding.

### Child Bead Comments

- No additional child-bead comments were recorded in this placeholder block.

---

## Phase 4 — Dynamic gateway enrollment + peer.invoke allowlist policy

Bead: `lab-n07n.4` · Status: open · Parent: lab-n07n

## What

Two closely-related capabilities:

1. **Dynamic gateway enrollment** — when a device sends `fleet/device.enroll`, master evaluates a policy and, if allowed, hot-inserts the device into `UpstreamPool.connections` as a new upstream. Its advertised tools/resources/prompts become available in the master catalog without a restart. Enrollment is revocable without disconnecting the WS.

2. **`fleet/peer.invoke` allowlist + audit** — deny-by-default policy engine that decides whether device A may invoke action X on device B. Every attempt (allowed or denied) emits a structured audit log entry.

Together, these turn the WS plumbing from P1–P3 into a working fleet control plane.

## Context

### Current state
- After P1, `UpstreamPool` has WS transport support and a reprobe loop, but `connections` is only populated by `discover_all()` at startup — no post-startup insert API.
- After P3, `fleet/device.enroll` is a stub that accepts and returns success without affecting the pool.
- After P3, `fleet/peer.invoke` is a stub that always returns `method_not_found`.
- Tool exposure allowlist precedent: `UpstreamConfig.expose_tools: Option<Vec<String>>` (config.rs:167) + `ToolExposurePolicy` enum (types.rs:50-71) — patterns either exact names or `*` wildcard.
- Tailscale API client (`lab-apis/tailscale`) can enumerate tailnet nodes — can back tailnet-ACL-based enrollment policies.
- OAuth manager has a known "restart required" limitation (manager.rs:1226) — making the pool hot-mutable here may fix that as a side effect (note in testing but out of scope).

### Policy shapes
```toml
# ~/.labby/config.toml additions

[fleet.enrollment]
# Explicit allowlist. Devices not listed are connected but not enrolled in the catalog.
allow = ["controller", "node-b", "workstation-wsl"]
# Optional: tailnet-ACL-based (if unset, falls back to `allow`)
# require_tailnet_tag = "tag:lab-fleet"

[[fleet.peer_policy]]
# Device A can invoke action X on device B
source = "controller"
target = "node-b"
actions = ["radarr.queue.list", "sonarr.*"]

[[fleet.peer_policy]]
source = "workstation-wsl"
target = "*"
actions = ["marketplace.install"]  # power user: install anywhere
```

Deny-by-default: if no matching `peer_policy` entry, the call is rejected.

### Locked decisions from brainstorm
- Master-controlled enrollment with policy; revocable without disconnect
- Disconnect → `Unhealthy` (circuit breaker, not removal) — already in P1
- `fleet/peer.invoke` deny-by-default allowlist; audit every attempt
- No direct device↔device; all peer ops relayed through master

## Decisions

### Locked
- `UpstreamPool` gains an `insert_upstream(config: UpstreamConfig, connection: UpstreamConnection)` method; write-locks `connections`.
- Revoke path: `pool.revoke_upstream(name)` marks the entry `Unhealthy` permanently and stops routing through it, without closing the WS.
- Enrollment policy config reuses the existing TOML file (`~/.labby/config.toml`). No separate fleet.toml (keeps the deployment story simple).
- Peer policy patterns use the same glob-style matching as existing tool exposure patterns — prefer reuse of `ToolExposurePolicy` matcher if shape fits, otherwise factor the matcher into a shared helper.
- Audit log emits one structured tracing event per decision at `INFO` (allowed) or `WARN` (denied), with fields `surface="fleet", action="peer.invoke", source_device, target_device, target_action, decision, reason`.
- Allowed `fleet/peer.invoke` resolves the target via pool lookup; if target is `Unhealthy` or absent, returns structured error (`kind = "target_offline"` for disconnected, `"unknown_instance"` for not-enrolled) without retry — caller decides to retry.

### Discretion
- Whether to cache parsed peer policy per invocation or reload on every call.
- Policy hot-reload strategy (SIGHUP? config file watcher? explicit CLI action?).
- Whether to surface audit entries as a dedicated `/v1/fleet/audit` endpoint in this phase or defer to P5.
- Whether to use `ToolExposurePolicy` directly or create `FleetActionPolicy` mirror (if the shape drifts significantly, separate types).

## Testing

- [ ] Unit: enrollment policy matcher — allow list exact match, tailnet tag match, deny-by-default.
- [ ] Unit: peer policy matcher — exact action, glob action, source/target combinations, deny-by-default.
- [ ] Unit: `insert_upstream` is thread-safe under concurrent inserts (no lost updates, no deadlock).
- [ ] Unit: `revoke_upstream` flips health to `Unhealthy` without touching the underlying connection.
- [ ] Integration: device sends `fleet/device.enroll` with allowed device_id → appears in `lab gateway list-tools` within one refresh cycle.
- [ ] Integration: device sends `fleet/device.enroll` with denied device_id → WS stays connected, device absent from catalog, WARN log emitted.
- [ ] Integration: allowed `fleet/peer.invoke` routes through master to target device, output returns to caller.
- [ ] Integration: denied `fleet/peer.invoke` returns structured error; audit log entry emitted with `denied` decision.
- [ ] Integration: `fleet/peer.invoke` to offline target returns `target_offline`; no retry; audit entry with reason.
- [ ] Integration: revoke-then-re-enroll a device without closing the WS — catalog reflects both transitions.
- [ ] Negative: malformed policy config → startup fails with a clear error message pointing to the offending line.
- [ ] Negative: policy referencing non-existent device — warn, do not fail.

## Validation

- [ ] `cargo build --all-features` compiles.
- [ ] `cargo test --all-features` passes.
- [ ] `lab-rrkm` scenario works end-to-end: master invokes `fleet/peer.invoke { target: "<device>", action: "marketplace.install" }` and the target device runs `claude plugin install <...>` successfully (requires target device to expose a `marketplace.install` action — may be stubbed for this test).
- [ ] `lab gateway status --json` reflects per-upstream health, including dynamically-enrolled devices.
- [ ] Audit log entries grep-able by `surface=fleet action=peer.invoke`.
- [ ] Policy hot-reload (via `lab fleet policy reload` or equivalent) applies without restart.
- [ ] `cargo clippy --all-features -- -D warnings` clean.
- [ ] `docs/FLEET_POLICY.md` (new) documents enrollment + peer policy TOML schema with examples.

## Files

- `crates/lab/src/dispatch/upstream/pool.rs` — add `insert_upstream`, `revoke_upstream`; wire into fleet enrollment handler
- `crates/lab/src/fleet/policy.rs` (new) — policy types, loader, matcher
- `crates/lab/src/fleet/enroll.rs` (replaces P3 stub) — real enrollment handler using policy
- `crates/lab/src/fleet/methods/peer_invoke.rs` (replaces P3 stub) — real authz + audit + routing
- `crates/lab/src/fleet/audit.rs` (new) — structured audit emit helpers
- `crates/lab/src/config.rs` — extend `Config` with `fleet: FleetConfig { enrollment, peer_policy }`
- `crates/lab/src/cli/fleet.rs` (new) — CLI subcommand `lab fleet {policy reload | list-peers | show-allowlist}`
- `crates/lab/src/cli.rs` — register `fleet` subcommand
- `docs/FLEET_POLICY.md` (new) — policy schema + examples
- `docs/FLEET_METHODS.md` — update enroll + peer.invoke sections with real error kinds

## Dependencies

- Depends on `lab-n07n.3` (fleet method handlers exist; this phase replaces stubs with real logic)

## References

- File: `crates/lab/src/dispatch/upstream/pool.rs:311,562,668` — connections HashMap + current mutation sites
- File: `crates/lab/src/dispatch/upstream/types.rs:50-71` — `ToolExposurePolicy` pattern matcher to potentially reuse
- File: `crates/lab/src/config.rs:143-172` — `UpstreamConfig` precedent for new `FleetConfig` shape
- File: `crates/lab/src/dispatch/upstream/manager.rs:1226` — known OAuth restart limitation (side-effect fix opportunity, not blocking)
- Doc: `docs/ERRORS.md` — error kind vocabulary (add `target_offline` if not present; otherwise reuse existing)
- Doc: `docs/OBSERVABILITY.md` — dispatch log conventions (audit entries conform to these)
- Brainstorm: lab-n07n — deny-by-default allowlist; revocable enrollment


## Research Findings (Phase 4)

### Confused deputy in `fleet/peer.invoke` (CRITICAL, new authz gate)
**Vulnerability class**: if the handler only verifies "source A can reach target B", an action that B's local policy forbids can still be invoked through the master relay. Master authentication implicitly bypasses B's local checks.

**Required two-stage gate in the peer.invoke handler:**
1. **Caller→target allowlist** — policy matches `(source=A, target=B, action_glob)`. This is what the current plan covers.
2. **Destructive-flag gate** — every `ActionSpec.destructive` invocation crossing `fleet/peer.invoke` requires explicit per-`(source, target)` opt-in (e.g., `allow_destructive = true` in the TOML), AND an audit entry that records both the destructive flag and the target's local policy outcome.

Add to the TOML schema:
```toml
[[fleet.peer_policy]]
source = "controller"
target = "node-b"
actions = ["radarr.*"]
allow_destructive = false  # default: false. Explicit opt-in required.
```

Deny-by-default for destructive actions regardless of action glob match.

### Policy hot-reload TOCTOU (new, REQUIRED)
- Policy mutation must hold a write lock around the matcher itself, not just the file. Reloading the TOML while an enrollment is in flight corrupts the matcher snapshot (lab-l840 OAuth precedent).
- Pattern: `Arc<RwLock<FleetPolicy>>` in the handler state; reload swaps the inner under a write lock. Invocations take a read lock snapshot, not a per-field lookup.

### GlobMatcher reuse (codified)
- `ToolExposurePolicy` (types.rs:50-71) already implements the exact-name + `*` glob match. Factor out `GlobMatcher` into a shared helper rather than duplicating logic inside fleet policy. Keeps one source of truth for glob semantics.

### UpstreamPool mutation API (new)
- `insert_upstream` and `revoke_upstream` must be **atomic relative to discovery**: a device in mid-enroll must not appear half-registered in `connections` while missing from the catalog, and vice versa. Use the P1 concurrent-map baseline and a single pool-level mutation API so callers never update `connections` and the catalog independently.
- Revocation flips health to `Unhealthy` and clears the entry from the routing table, but leaves the WS connection live. Reconnect-after-revoke requires the device to re-enroll (explicit, not automatic).
- At 100x+ scale, `RwLock<HashMap>` contention on `catalog` is a known bottleneck (10–30% CPU at 1000x). The `DashMap` swap was pulled forward into Phase 1 by the Engineering Review Addendum, so Phase 4 should assume the concurrent-map baseline is already in place rather than re-scoping it here.

### Backpressure on peer.invoke (new)
- `Semaphore` at the pool level bounds concurrent peer-invoke relays (suggest default: 64). Without it, a single compromised device can starve master's tokio executor (lab-e27 precedent).
- Per-source rate limit (token bucket, 10/s default) as additional defense.

### Token storage hardening (cross-cut with P2)
- Device tokens: OS keyring when available, `~/.labby/device-token` 0600 as fallback. **Do not** store in `~/.labby/.env` where they become visible to `system.vars` scrubbing audits.
- Token rotation: 15-60 min TTL with refresh endpoint. Revocation allowlist persists in SQLite for multi-master deployments.

### Audit log integrity (new)
- Current `DeviceFleetStore` is in-memory; an audit log written to JSONL with no HMAC chain is tamper-editable once persisted. For this phase, keep audit entries as tracing events only (volatile by design). A durable, tamper-evident audit log is tracked as a follow-up bead (Deferred on epic).
- Every peer.invoke audit entry MUST log: `source_device`, `target_device`, `action`, `destructive: bool`, `decision: allow|deny`, `reason`, `correlation_id` (for tracing join).

### Additional tests (append to Testing)
- [ ] Confused-deputy: policy allows `(controller → node-b, radarr.*)` with `allow_destructive=false`; invoking `radarr.movie.delete` returns `denied` with `reason: destructive_not_permitted`.
- [ ] Hot-reload race: start a long-running `fleet/peer.invoke`; reload policy mid-flight; request completes under the snapshot it started with, subsequent requests see new policy.
- [ ] Concurrent-map baseline regression: with the P1 DashMap swap already in place, concurrent 1000-device enrollment bench shows no deadlocks, no lost updates, and clippy remains clean.
- [ ] Semaphore backpressure: spam 10k peer.invoke from one source; concurrent in-flight caps at 64; excess returns `rate_limited` kind, not 503.
- [ ] Audit log completeness: every allow/deny/offline/rate-limited path emits exactly one audit entry; entry contains all required fields.

### Risks
- **CRITICAL (security)** — destructive-action gate is non-negotiable. Without it, `fleet/peer.invoke` is a privilege-escalation channel across the fleet.
- **HIGH (perf)** — dynamic enrollment depends on the P1 DashMap swap being complete; if the concurrent-map baseline is missing, block P4 rather than re-scoping the swap here.
- **MEDIUM (op)** — policy hot-reload TOCTOU is subtle and easy to miss in tests; the async integration test is the load-bearing gate.

## CEO Review Addendum (Phase 5a)

### SSRF reuse in peer.invoke params (HIGH)
If `params` to `fleet/peer.invoke` carry any URL-shaped values, validate via existing `mcpregistry::ssrf::is_blocked()` (CGNAT 100.64/10 + RFC1918 blocked). Applies even for params destined for the target device's MCP action — confused-deputy gate alone does not cover SSRF vector.

Add test: `peer.invoke` with private-IP URL param → envelope `kind: "url_blocked"`.

### Policy reload (simplification)
**Replace file-watcher + debounce with SIGHUP-triggered reload.**
- Master registers SIGHUP handler
- On SIGHUP: atomically read policy file, parse, swap `Arc<RwLock<FleetPolicy>>` content
- On parse error: keep old policy, log `ERROR fleet.policy_reload_failed` with serde error
- Simpler, fewer moving parts, clearer operator semantics

### E2E integration test (business case verification)
Add `#[ignore]` integration test `test_remote_plugin_install_via_ws`:
1. Start test master (in-process axum)
2. Start test device binary subprocess
3. Device enrolls via WS
4. Master dispatches `gateway({"action":"plugin.install","instance":"<device_id>",...})` — routes to device via UpstreamPool
5. Assert install completes on device, response returns to master
6. Marked `#[ignore]` for local-only; covered in `just test-integration`

### Docs
- Update `docs/SECURITY.md`: peer.invoke threat model, confused-deputy gate, SSRF reuse
- Add `docs/FLEET_POLICY.md`: allowlist format, SIGHUP reload, audit log path

## Engineering Review Addendum (Phase 5b)

### Revocation closes active sessions (CRITICAL security)
`revoke_upstream(device_id)` now:
1. Send `fleet/server.revoke { reason: "enrollment_revoked" }` notification on the device's active WS
2. Close WS connection immediately with Close frame code 4001 (custom: revoked)
3. Mark all sessions for device_id invalid in `Pool::close_device_sessions`
4. On subsequent reconnect attempt, device's `initialize` returns error envelope `kind: "enrollment_revoked"`, no retry

Test: revoke_upstream invoked while device has active WS → connection closes within 500ms; subsequent peer.invoke routed to that device returns `kind: "upstream_unreachable"`.

### PolicySnapshot struct (HIGH arch — TOCTOU fix)
```rust
struct PolicySnapshot {
    inner: Arc<FleetPolicyInner>,  // cheap Arc clone
    version: u64,                   // monotonic
}

impl FleetPolicy {
    fn snapshot(&self) -> PolicySnapshot { /* read-lock, clone Arc, release */ }
}
```
Every peer.invoke handler calls `policy.snapshot()` once at entry; evaluates entire request under that snapshot. Audit event records `policy_version_at_decision: u64`. SIGHUP reload bumps version atomically.

### Audit log tamper-evidence (moved in-scope per eng review)
Audit path:
1. In-memory ring buffer (10k entries) for real-time queries (P5 UI reads this)
2. Append-only JSONL at `~/.labby/fleet-audit.jsonl` with daily rotation
3. Each line carries: `{timestamp, request_id, device_id, action, decision, policy_version, prev_hash, hmac}`
4. `prev_hash` = SHA-256 of prior line; `hmac` = HMAC-SHA256(key, line_content) with key in `~/.labby/audit-hmac-key` (0600, generated on first audit init)
5. Daily rotation: on rotation, final line of prior file hashes-chains into first line of new file
6. Verification tool: `lab audit verify --since 2026-04-22` walks chain, reports first tampering

Test: append 100 entries, corrupt one line, assert verify detects.

### E2E integration test (from CEO addendum, detailed here)
`tests/fleet_e2e.rs::test_remote_plugin_install_via_ws` (`#[ignore]`):
1. Spawn in-process axum master (random port, bound to 127.0.0.1)
2. Spawn device binary subprocess with `LAB_MASTER_URL=ws://127.0.0.1:<port>/v1/fleet/ws`, test token
3. Wait for enrollment (poll `GET /v1/fleet/devices` until device appears)
4. Master issues `gateway({"action":"plugin.install","instance":"<device_id>","params":{"url":"..."}})`
5. Assert response arrives within 30s with `kind: ok`
6. Assert device-side plugin directory contains installed artifact

### Child Bead Comments

- No additional child-bead comments were recorded in this placeholder block.

---

## Phase 5 — gateway-admin UI + HTTP phone-home deprecation

Bead: `lab-n07n.5` · Status: open · Parent: lab-n07n

## What

Surface the fleet transport in the gateway-admin Next.js app and mark the legacy HTTP phone-home endpoints as deprecated (do not remove yet).

Admin UI additions:
- Connected-devices view with enrollment status, tailnet identity, last-seen, health, and the device's advertised MCP tools (already present via gateway pool once enrolled in P4).
- Live log tail component streaming `fleet/log.event` from master to browser.
- Peer policy editor: source→target→actions allowlist grid with live validation + diff preview before save.
- Audit log viewer for `fleet/peer.invoke` decisions.

Deprecation:
- Add deprecation headers to HTTP phone-home endpoints (`Deprecation: true`, `Sunset: <future>`, `Link: <ws-docs>; rel="successor-version"`).
- Update docs (`docs/DEVICE_RUNTIME.md`) to mark the HTTP phone-home path as deprecated.
- Keep the HTTP handlers functional for one release; a follow-up bead (not this phase) removes them.

## Context

### Current state
- gateway-admin is a Next.js app under `apps/gateway-admin/` with pages under `(admin)/` and components under `components/gateway/`.
- Admin UI calls the master's HTTP API via `gateway.add`, `gateway.remove`, `gateway.status`, etc. (MCP action dispatch over HTTP).
- Gateways list page: `apps/gateway-admin/(admin)/gateways/page.tsx`; detail: `(admin)/gateway/page.tsx`.
- No WebSocket support in the UI today.
- Device HTTP phone-home handlers at `crates/lab/src/api/device/{hello,status,metadata,syslog}.rs`.
- Live log tail in the UI requires a browser→master WebSocket channel (separate from the device-side WS). Master acts as a fanout hub: log events from connected devices get forwarded to connected browser viewers.

### Locked decisions from brainstorm
- Device stays in catalog when disconnected (`Unhealthy`) — UI renders this state, doesn't hide
- HTTP phone-home stays functional for one release after deprecation marker

## Decisions

### Locked
- Admin UI extensions land in the existing `apps/gateway-admin/` — no new app.
- Browser→master WS uses a new endpoint `/v1/fleet/ui/ws` with subscription-based multiplexing (subscribe to `logs:device_id`, `audit`, etc.) — separate from the device-side `/v1/fleet/ws` to keep auth surfaces distinct (browser uses operator session auth; device uses tailnet + token).
- Peer policy editor writes to the same `~/.labby/config.toml` via a master API endpoint (`POST /v1/fleet/policy`), which validates and triggers hot-reload.
- Deprecation is soft — handlers keep working. A follow-up bead (tracked in epic Deferred) removes them after one release.
- Audit log viewer is read-only in this phase; any remediation actions (e.g., revoke enrollment) are separate admin actions already available from P4 APIs.

### Discretion
- UI component library choice for the policy editor grid (match existing gateway-admin patterns).
- Pagination vs virtual scrolling for the log tail.
- Whether to ship a `/v1/fleet/audit` endpoint in this phase or build the viewer directly on audit-log tracing backend.
- Look-and-feel of the "device offline" badge.

## Testing

- [ ] Unit (frontend): log tail subscription manages reconnect + backpressure correctly under mock WS feeds.
- [ ] Unit (frontend): peer policy editor validates shape before save (catch typos, unknown device_ids).
- [ ] Integration (frontend): connected-devices view renders healthy, unhealthy, and not-enrolled states distinctly.
- [ ] Integration (backend): `/v1/fleet/ui/ws` subscription to `logs:<device>` receives events within 500ms of device-side emit.
- [ ] Integration (backend): `POST /v1/fleet/policy` applies hot-reload; peer.invoke behavior reflects the new policy without restart.
- [ ] Integration (backend): HTTP phone-home endpoints respond with `Deprecation: true` and `Sunset` headers.
- [ ] Contract: `X-Lab-Fleet-Transport: ws|http` response header on device-origin endpoints so we can monitor migration progress.
- [ ] E2E (smoke): operator opens gateway-admin, sees 3+ connected devices, tails logs from one, edits a peer policy, saves, confirms a subsequent `fleet/peer.invoke` behaves per new policy.

## Validation

- [ ] `cargo build --all-features` compiles.
- [ ] `pnpm -C apps/gateway-admin build` succeeds.
- [ ] `pnpm -C apps/gateway-admin test` passes.
- [ ] Admin UI connected-devices view loads and reflects live device state.
- [ ] Live log tail shows events streaming in <500ms median latency.
- [ ] Peer policy editor round-trip: edit → save → hot-reload → behavior changes verified.
- [ ] HTTP phone-home responses include deprecation headers.
- [ ] `docs/DEVICE_RUNTIME.md` updated with deprecation notice and WS cutover plan.
- [ ] `cargo clippy --all-features -- -D warnings` clean.
- [ ] A backlog bead "Remove HTTP phone-home endpoints" is filed (tracked in epic Deferred, created in final lock pass).

## Files

- `apps/gateway-admin/app/(admin)/fleet/page.tsx` (new) — connected devices + log tail + policy editor tabs
- `apps/gateway-admin/components/fleet/` (new) — ConnectedDevices, LogTail, PeerPolicyEditor, AuditLogViewer
- `apps/gateway-admin/lib/fleet-ws-client.ts` (new) — browser-side WS client for `/v1/fleet/ui/ws`
- `apps/gateway-admin/components/layout/sidebar.tsx` or equivalent — add "Fleet" nav entry
- `crates/lab/src/api/fleet/ui_ws.rs` (new) — axum WS handler for browser consumers with subscription mux
- `crates/lab/src/api/fleet/policy.rs` (new) — REST endpoints for policy list/update/reload
- `crates/lab/src/api/device/{hello,status,metadata,syslog}.rs` — add deprecation headers in responses
- `crates/lab/src/api/router.rs` — mount `/v1/fleet/ui/ws` and `/v1/fleet/policy` routes
- `docs/DEVICE_RUNTIME.md` — deprecation notice
- `docs/FLEET_UI.md` (new) — UI architecture notes

## Dependencies

- Depends on `lab-n07n.4` (real enrollment + policy + audit must be working; UI surfaces them)

## References

- File: `apps/gateway-admin/app/(admin)/gateways/page.tsx` — existing admin pattern to mirror
- File: `apps/gateway-admin/components/gateway/` — existing components as style reference
- File: `crates/lab/src/api/router.rs:527-530` — route mount region
- File: `crates/lab/src/api/device/` — phone-home handlers to add deprecation to
- Brainstorm: lab-n07n — UI surfaces + deprecation strategy


## Research Findings (Phase 5)

### Browser fanout disaster (CRITICAL, new transport decision)
`tokio::sync::broadcast` has no per-subscriber backpressure. At 100x (20 browsers × 1000 devices × 1 msg/s × 300 B = 6 MB/s), a single browser on a slow link (500 ms lag) fills the 32-message buffer, and **all** subscribers get `Lagged` and disconnect — cascade failure.

**Required replacement (was: broadcast channel):**
- Per-subscriber bounded `mpsc` (capacity ~1000 msgs ≈ 300 KB each).
- Slow subscribers drop their own tail (controlled degradation); fast subscribers unaffected.
- Master keeps `Arc<DashMap<SubscriptionKey, mpsc::Sender<Event>>>` keyed by `(subscriber_id, topic)`.
- Drop policy: on `TrySendError::Full`, emit `{ warning: "subscription_lagging" }` control frame and drop the oldest message. After N consecutive drops, disconnect that subscriber cleanly.

### Client-side filter protocol (new, REQUIRED for 100x+)
Without a filter, every browser receives every log line from every device. Add subscription protocol on `/v1/fleet/ui/ws`:
```json
// browser → master
{"op":"subscribe","topic":"logs","devices":["id1","id2"]}
{"op":"subscribe","topic":"audit"}
{"op":"unsubscribe","topic":"logs","devices":["id1"]}
```
Master routes events only to matching subscriptions. Reduces fanout 100×+ in typical admin usage (operator tails 1-2 devices at a time).

### Compression (new, RECOMMENDED)
- Enable **RFC 7692 `permessage-deflate`** on both `/v1/fleet/ws` (device-side, landed in P3) and `/v1/fleet/ui/ws` (browser-side, this phase). Log traffic is ~70% compressible; CPU cost ~5%.
- `tokio-tungstenite` supports via the `deflate` feature; axum WS supports via config.
- Transparent to application code.

### Browser WS auth (CRITICAL, security)
- **NEVER** pass tokens in query string. URLs are logged in browser history, HTTP Referer, proxy logs, server access logs. Use `Sec-WebSocket-Protocol` subprotocol negotiation:
  ```
  Sec-WebSocket-Protocol: lab-bearer, <base64-token>
  ```
  Master extracts token from subprotocol in a custom axum extractor and validates against the operator session.
- Required `Origin` header validation to prevent CSWSH (cross-site WebSocket hijacking via session cookies). Allowlist: `LAB_PUBLIC_URL` value. Reject if `Origin` absent or mismatched.
- Redact `Sec-WebSocket-Protocol` from request logs — it carries the token.

### SSRF in policy editor (HIGH, new)
- `POST /v1/fleet/policy` can include device URLs for enrollment overrides (tailscale fallback hostnames, etc.). SSRF validation must live in the dispatch-layer helper (`validate_gateway_url`), NOT CLI-only (lab-77y5 precedent).
- RFC1918 blocklist MUST include Tailscale CGNAT `100.64.0.0/10` (lab-fstf precedent).

### AbortController discipline (UI)
- Every in-flight mutation from the UI dialogs (policy edit, enrollment action, audit query) uses `AbortController`. Unmount cancels in-flight requests. Prevents stale-response races after navigation.

### Additional tests (append to Testing)
- [ ] Slow-subscriber isolation: one browser with simulated 2s lag does NOT affect other subscribers' latency.
- [ ] Subscription filter: subscribing to `logs:device-A` receives no `logs:device-B` traffic (verified with message counter).
- [ ] Compression round-trip: enable deflate, verify payload bytes are <50% of uncompressed for 100-event log burst.
- [ ] Browser WS auth: token in query string → rejected; token in subprotocol → accepted; missing Origin → rejected.
- [ ] CSWSH: connection with `Origin: https://evil.example` → rejected.
- [ ] SSRF: policy POST with `url: http://169.254.169.254/...` or `http://100.64.0.1/...` → rejected with structured error.
- [ ] AbortController: unmount policy editor mid-save → no React warning, no stale-response commit.

### Risks
- **CRITICAL (perf)** — broadcast channel fanout is a production-blocking bug at 100x+. Per-subscriber mpsc is mandatory, not optional.
- **CRITICAL (security)** — tokens in query strings are a one-shot route to credential leak via logs. Subprotocol-only, no fallback.
- **HIGH (security)** — missing `Origin` validation turns the admin WS into a CSRF/CSWSH vector through a logged-in operator's cookies.
- **MEDIUM (scope)** — client-side filter protocol adds ~200 LOC to UI + backend routing. Budget-aware but worth it; without it, the feature is unusable beyond lab-scale.

## CEO Review Addendum (Phase 5a)

### Rollback plan (explicit)
P5 adds `fleet_http_deprecated` feature flag. Rollback procedure:
1. Set `fleet_http_deprecated=false` in config → restart master → HTTP phone-home endpoints re-enabled alongside WS
2. If device binary broken: redeploy prior binary (HTTP-only path still works until flag set true)
3. Full rollback: set `fleet_ws_listener_enabled=false`, restart master — device binaries queue to disk indefinitely (until rolled back or flag re-enabled)

### Observability runbook (first-class scope)
Add `docs/runbooks/FLEET.md` covering:
- Disconnected device triage (check circuit breaker, check tailnet reachability, check device logs)
- Queue backlog alert response (queue depth > 10k, disk bytes > 100 MiB)
- peer.invoke denied spike (check policy, check audit log)
- Thundering herd detection (reconnect_count histogram spike)

### Docs
- Add `docs/runbooks/FLEET.md`: operator runbook for fleet incidents
- Update `docs/OBSERVABILITY.md`: register new fleet.* metrics
- Update `README.md`: deprecation notice for HTTP phone-home

### Child Bead Comments

- No additional child-bead comments were recorded in this placeholder block.
