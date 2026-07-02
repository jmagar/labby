# 2026-04-22 — Fleet WebSocket Design Pipeline (lab-n07n)

## Session Overview

Resumed the `/lavra-design` 6-phase pipeline for epic `lab-n07n` (Rust WebSocket fleet transport for deployed lab binaries). Completed Phase 4 (Revise), Phase 5a (CEO review / HOLD SCOPE), and Phase 5b (4-agent engineering review). Locked plan with `plan-reviewed` label. Assembled the final post-review plan as a single markdown document.

No code changes. Planning + review only.

## Timeline

1. Applied 5 staged Phase 3 research-findings append files (`/tmp/n07n{1..5}-append.md`) to bead descriptions in parallel via `bd update -d`.
2. Ran CEO review (Phase 5a) — pre-audit → Step 0 premise/existing-code/dream-state → mode selection → 10-section sweep → 8 trade-offs resolved via two AskUserQuestion batches.
3. Ran Engineering review (Phase 5b) — dispatched 4 `haiku` agents (architecture-strategist, code-simplicity-reviewer, security-sentinel, performance-oracle) in parallel. 7 CRITICAL/HIGH findings resolved via three AskUserQuestion batches (3+4+4 questions).
4. Applied engineering review addenda to P1–P4 via parallel `bd update -d`. P5 left unchanged (simplicity-reviewer YAGNI cuts rejected).
5. Added `plan-reviewed` label to epic.
6. User interrupted subsequent `/lavra-work lab-n07n.1` launch and requested the assembled plan as a markdown doc.
7. Wrote assembled plan to `docs/fleet-ws-plan-lab-n07n.md` (1115 lines) by concatenating epic + all 5 child beads via `jq`.

## Key Findings

- rmcp 1.4 has no shipped WebSocket transport — must write custom `IntoTransport` adapter over `tokio-tungstenite`.
- Multi-agent review converged independently on two critical issues: DashMap swap for `UpstreamPool` (arch + perf) and fsync-blocking on tokio runtime (perf + simplicity).
- Simplicity-reviewer's YAGNI cuts (defer segment queue, per-sub mpsc, audit viewer, subscriptions, compression) conflicted with research-driven production requirements; research and perf-oracle findings took precedence.
- Existing SSRF blocklist at `crates/lab/src/dispatch/mcpregistry::ssrf::is_blocked()` (CGNAT 100.64/10 + RFC1918) is reusable for `peer.invoke` params.
- Existing queue at `~/.labby/device-runtime-queue.jsonl` has O(N) rewrite-on-ack — replaced by segment-based design (1 MiB immutable segments).

## Technical Decisions

- **HOLD SCOPE mode** (not expansion/reduction) — WebSocket is the right-sized solution; no competing premise (SSE/gRPC/NATS) survived Step 0.
- **Per-connection session UUID handshake** — added for replay-attack hardening.
- **DashMap for `UpstreamPool` in P1** (moved from deferred P4) — lock contention is a P1 blocker, not a scale optimization.
- **SIGHUP-triggered policy reload** instead of file-watcher — simpler, matches operator mental model.
- **PolicySnapshot struct** with `policy_version: u64` — avoids TOCTOU on hot-reload.
- **Audit HMAC chain moved into P4** (from deferred) — SHA-256 prev_hash + HMAC-SHA256 per line, `~/.labby/fleet-audit.jsonl`, `lab audit verify` tool.
- **Semaphore(16) on reprobes + per-device `hash(device_id) mod 5000ms` jitter** — thundering-herd mitigation.
- **Backpressure policy** — 1s `send_timeout` then disconnect (not unbounded).
- **Token redaction at tracing layer** for `Sec-WebSocket-Protocol` header — prevents leakage in structured logs.
- **Engineering review agents used `model: "haiku"`** due to prior-session Sonnet rate limits.

## Files Modified

| File | Purpose |
|------|---------|
| `docs/fleet-ws-plan-lab-n07n.md` | Full assembled plan (epic + 5 child beads, 1115 lines) — created this session |
| `docs/sessions/2026-04-22-lab-n07n-fleet-ws-design-pipeline.md` | This session log |
| Bead `lab-n07n` (description + comments + labels) | Epic — added `plan-reviewed` label; logged 6 DECISION/INVESTIGATION/FACT/LEARNED comments |
| Bead `lab-n07n.1` (description) | Phase 1 — appended Research Findings + CEO Addendum + Eng Review Addendum |
| Bead `lab-n07n.2` (description) | Phase 2 — appended Research Findings + CEO Addendum + Eng Review Addendum |
| Bead `lab-n07n.3` (description) | Phase 3 — appended Research Findings + Eng Review Addendum |
| Bead `lab-n07n.4` (description) | Phase 4 — appended Research Findings + CEO Addendum + Eng Review Addendum |
| Bead `lab-n07n.5` (description) | Phase 5 — appended Research Findings + CEO Addendum only (no Eng Addendum — kept as specified) |

No source code files in `crates/` or `apps/` modified.

## Commands Executed

| Command | Purpose / Result |
|---------|---|
| `rtk bd update lab-n07n.{1..5} -d "$(cat ...)"` | Applied Phase 3/5a/5b staged findings to bead descriptions |
| `rtk bd update lab-n07n --add-label plan-reviewed` | Locked plan (initial `--labels` flag failed; `--add-label` worked) |
| `rtk bd show lab-n07n --json` and `rtk bd show lab-n07n.{1..5} --json` | Fetched current post-review bead state for assembly |
| `jq -r ...` pipeline → `docs/fleet-ws-plan-lab-n07n.md` | Assembled final plan markdown (1115 lines) |
| `rtk bd comments add lab-n07n "..."` (×6) | Logged review summary comments on epic |

## Behavior Changes (Before/After)

No runtime behavior changes. Plan artifacts in the beads database moved from "research-complete, unreviewed" to "post-CEO + eng-review, `plan-reviewed` label set, ready for implementation."

## Verification Evidence

| Command | Expected | Actual | Status |
|---|---|---|---|
| `rtk bd show lab-n07n --json \| jq -r '.[0].labels'` | `["brainstorm", "plan-reviewed"]` | `["brainstorm", "plan-reviewed"]` | PASS |
| `wc -l docs/fleet-ws-plan-lab-n07n.md` | >1000 lines (all 5 phases with addenda) | 1115 | PASS |
| `rtk bd swarm validate lab-n07n` (prior turn) | DAG valid, swarmable | 5 waves, swarmable YES | PASS (carried) |
| `wc -l docs/sessions/2026-04-22-lab-n07n-fleet-ws-design-pipeline.md` | File written | Pending (this file) | N/A |

## Source IDs + Collections Touched

Beads CLI only — no vector store / embedding collections used this session.

| Source ID | Outcome |
|---|---|
| `lab-n07n` (epic) | Updated description, comments, labels |
| `lab-n07n.1`–`lab-n07n.5` (children) | Updated descriptions (addenda appended) |

## Risks and Rollback

- **Risk:** Bead descriptions now carry three stacked addenda; readers may miss the most recent (Eng Review) overrides inside P1 (DashMap was deferred in the original, moved to P1 in addendum).
- **Rollback:** Bead history is preserved by `bd` internally. `rtk bd update lab-n07n.N -d "$(original description)"` reverts. Plan markdown can be deleted without affecting the source of truth (beads).
- **Risk:** `plan-reviewed` label signals "ready to implement"; removing it requires `bd update --remove-label plan-reviewed` if the plan needs another review round.

## Decisions Not Taken

- **Rejected: SSE / gRPC / NATS** as transport — WebSocket confirmed in Step 0 (bidirectional RPC + Tailscale tailnet).
- **Rejected: Simplicity-reviewer YAGNI cuts for P5** (defer audit viewer / subscriptions / compression) — user kept P5 as specified.
- **Rejected: File-watcher for policy hot-reload** — chose SIGHUP instead (simpler, explicit operator action).
- **Rejected: Unbounded mpsc backpressure** — chose 1s send-timeout + disconnect.
- **Rejected: Broadcast channel for browser UI fanout** — chose per-subscriber bounded mpsc (1000 cap, 10-drop disconnect).

## Open Questions

- Whether `/lavra-work lab-n07n.1` should launch with `--no-parallel` or default parallel dispatch when the user resumes implementation — user interrupted before selecting.
- Whether `docs/fleet-ws-plan-lab-n07n.md` should be committed as an in-repo artifact or treated as a scratch export (beads remain source of truth).

## Next Steps

- User to resume `/lavra-work lab-n07n.1` (Phase 1 — Gateway WS upstream + heartbeat/reconnect) when ready.
- Implementation order follows `bd swarm` wave layout (5 waves, P1 → P2 → P3 → P4 → P5).
- Consider trimming `docs/fleet-ws-plan-lab-n07n.md` or committing it alongside the first P1 PR so reviewers can see the design context.

## Neo4j Memory Integration

Skipped — `mcp__neo4j-memory__*` tools are not available in this session's deferred tool list. Session knowledge persists via beads (`lab-n07n` comments) and this markdown file.
