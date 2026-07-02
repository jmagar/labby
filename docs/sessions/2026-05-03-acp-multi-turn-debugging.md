---
date: 2026-05-03 07:26:18 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 50824844
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 88e8d4be-5916-447c-8c23-a788dfcb7a62
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/88e8d4be-5916-447c-8c23-a788dfcb7a62.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#40 — Integrate service wave and CI updates (https://github.com/jmagar/lab/pull/40)"
---

# ACP Multi-Turn Debugging

## User Request

Investigate and fix why ACP chat sessions are not multi-turn — the AI has "no idea of my last messages" when a second message is sent, and the user confirmed this is not supposed to be one-shot by design.

## Session Overview

Traced the root cause of ACP multi-turn failure from user report through full protocol stack (lab runtime → ACP SDK → codex-acp subprocess). Found three distinct bugs, applied fixes for all three, rebuilt the Docker container, and verified multi-turn with a live 5-turn session proving conversation history is maintained across prompts.

The actual root cause was a protocol version mismatch: codex-acp sends `usage_update` notifications that `agent-client-protocol-schema 0.12.0` cannot deserialize without the `unstable_session_usage` feature flag — crashing the subprocess connection on every turn.

## Sequence of Events

1. Invoked `/superpowers:systematic-debugging` skill; began Phase 1 root cause investigation by reading `runtime.rs`, `registry.rs`, and ACP SDK source.
2. Confirmed codex-acp subprocess stays alive between turns (outer `while let Some(command) = command_rx.recv().await` loop persists); confirmed `reattach_runtime` was being called on every Turn 2+ from server logs.
3. Identified and fixed **Bug #1**: `biased;` race condition in `tokio::select!` at `runtime.rs:1163` — without it, tokio picks randomly when both `read_update()` and the 5-second idle timer fire simultaneously, leaving `StopReason` unread to poison the next turn.
4. Identified and fixed **Bug #2**: agentic sessions need a pre-prompt drain when the previous turn ended via `idle_completion` — a stale `StopReason` from a late `PromptResponse` (after a long tool call) poisons Turn N+1's read loop.
5. Rebuilt container via `just dev-debug` (nightly + Cranelift + mold); ran live test — Turn 5 still failed ("You didn't tell me a favorite color").
6. Added `tracing::error!` instrumentation to `run_codex_session` at the `run_result.err()` site to capture the exact error string.
7. Rebuilt and tested; error log revealed the true root cause: `unknown variant 'usage_update', expected one of ...` — schema deserialization failure.
8. Traced failure chain: `usage_update` notification → `incoming_actor` dispatch error → `try_join!` propagates → background errors → `run_until` drops foreground loop → `connect_with.await` returns `Err` → `terminate_codex_child` (SIGTERM) → `runtime_exit_without_stop_reason`.
9. Discovered `UsageUpdate` in `agent-client-protocol-schema 0.12.0` is gated behind `#[cfg(feature = "unstable_session_usage")]`; `agent-client-protocol 0.11.1` pins schema at `=0.12.0` (exact), blocking independent upgrade.
10. Applied **fix #3**: added `features = ["unstable"]` to `agent-client-protocol` dependency in `crates/lab/Cargo.toml`.
11. Rebuilt container; ran 5-turn verification session — all turns completed via `stop_reason` (no more `runtime_exit_without_stop_reason`), Turn 5 correctly recalled Turn 1's color.
12. Ran `/lavra:lavra-learn` to structure and store 7 knowledge entries on bead `lab-kvji`.

## Key Findings

- **Root cause** (`runtime.rs`, `Cargo.toml`): `agent-client-protocol = "0"` without `features = ["unstable"]` means `UsageUpdate` (the `usage_update` session notification codex-acp sends after every prompt) cannot be deserialized. Every turn kills the connection. `reattach_runtime` then spawns a fresh subprocess with zero conversation history.

- **Diagnostic event** (`registry.rs:900-934`): `provider_info { type: "runtime_exit_without_stop_reason" }` fires whenever `connect_with.await` returns while `prompt_lifecycle.active=true`. Seeing this on every turn is the definitive indicator of a background actor crash.

- **Failure chain** (`jsonrpc.rs:1241`, `util.rs:124-142`): `run_until(background, foreground)` uses `futures::select!`; when `background` (the `try_join!` of `outgoing_actor`, `incoming_actor`, `task_actor`, `responder`) errors, `bg_result?` propagates and the foreground future is dropped — the inner prompt loop is cut off mid-execution without calling `prompt_lifecycle.finish()`.

- **SDK version constraint** (`Cargo.lock`): `agent-client-protocol 0.11.1` pins `agent-client-protocol-schema = "=0.12.0"` (exact version). `cargo update -p agent-client-protocol-schema --precise 0.12.2` fails. Feature flags on `agent-client-protocol` are the only upgrade lever.

- **`biased;` race** (`runtime.rs:1163-1171`): `tokio::select!` without `biased;` picks randomly when both `session.read_update()` (has `StopReason`) and `sleep(5s)` fire simultaneously. A timer win leaves `StopReason` in the channel to poison Turn N+1. Affected pure-chat sessions with fast responses that happen to complete near the 5-second mark.

- **Turn drain** (`runtime.rs`, new code): For agentic sessions, `idle_completion` can fire during a tool call > 5 seconds. The late `PromptResponse` deposits a stale `StopReason` after the loop breaks. A `previous_turn_idle` flag triggers a bounded drain (up to `LAB_ACP_TURN_DRAIN_TIMEOUT_MS`, default 300 s) before dispatching the next prompt.

## Technical Decisions

- **Enable `features = ["unstable"]` rather than vendoring or patching the schema crate**: The exact-version pin makes schema upgrades impossible without also upgrading the protocol crate (which has no 0.12.x release). Feature flags are the documented mechanism for enabling unstable schema variants and carry no additional code risk.

- **`biased;` over a two-arm select rewrite**: The minimal, semantically correct fix. `biased;` in tokio's `select!` always polls the first arm first when both are ready, which is exactly the correct priority (real updates over the timer).

- **300-second drain timeout (configurable via `LAB_ACP_TURN_DRAIN_TIMEOUT_MS`)**: Agentic tool calls can take minutes. A too-short timeout would fail for long operations; too long delays conversation on a stuck session. 300 s is the operator-adjustable default; the drain exits early on `StopReason` or connection error.

- **Drain discards stale content rather than forwarding it**: Content from after `idle_completion` is orphaned Turn N output that the frontend already declared complete. Forwarding it would duplicate or reorder events in the SSE stream.

- **Log `run_result.err()` at `ERROR` level before SIGTERM**: Added `action = "connect_with.error"` log so the root cause is always visible in structured logs without needing to reproduce or enable TRACE-level logging.

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab/Cargo.toml` | Added `features = ["unstable"]` to `agent-client-protocol` dependency |
| `Cargo.lock` | Updated to reflect feature flag change |
| `crates/lab/src/acp/runtime.rs` | Three changes: (1) `biased;` in `tokio::select!`, (2) `previous_turn_idle`/`ended_via_idle` turn drain logic, (3) `acp_turn_drain_timeout()` helper, (4) `connect_with.error` error log |

## Commands Executed

```bash
# Identify subprocess kill timing
docker logs lab-lab-master-1 --since 5m | grep -v "health report|enrollment"

# Capture exact error message
docker logs lab-lab-master-1 | grep "connect_with.error"
# Result: "unknown variant 'usage_update', expected one of ..."

# Rebuild container
just dev-debug
# Nightly + Cranelift + mold: ~80-100s compile time

# Live 5-turn verification
TOKEN=$(grep LAB_MCP_HTTP_TOKEN ~/.labby/.env | cut -d= -f2-)
curl -s -X POST http://localhost:8765/v1/acp/sessions ...
# (5-turn session, session ID: 29ce2bb5-6de7-4be2-9d00-1f9f5bdad136)

# Read per-turn responses from events
curl -s -X POST http://localhost:8765/v1/acp ...
# Result: all 5 turns clean stop_reason; Turn 5 recalled Turn 1 correctly
```

## Errors Encountered

**Error 1: `lab-lab-master-1` was running old binary**
- Symptom: Multi-turn fixes applied locally but container still failed.
- Root cause: Container mounts `./bin/lab` as a volume; the binary must be rebuilt and the container restarted.
- Resolution: `just dev-debug` rebuilds the debug binary and hot-swaps it via `docker compose restart`.

**Error 2: Test script accumulating responses across turns**
- Symptom: `get_response_since` returned all previous turns concatenated — `before_seq` appeared to be 0.
- Root cause: Extra events (`usage_update` now deserialized as `ProviderInfo("unhandled_provider_message")`) shifted message chunk sequence numbers past the captured `before_seq` value.
- Resolution: Changed verification to parse events by `prompt_started` / `stop_reason` boundaries rather than filtering by `since_seq`.

**Error 3: `cargo update -p agent-client-protocol-schema --precise 0.12.2` failed**
- Root cause: `agent-client-protocol 0.11.1` uses an exact version constraint `="=0.12.0"` on the schema crate, making any independent schema upgrade impossible via lockfile manipulation.
- Resolution: Enabling `features = ["unstable"]` on `agent-client-protocol` activates `unstable_session_usage` (and all other unstable schema variants) without touching the schema version.

## Behavior Changes (Before/After)

| Aspect | Before | After |
|--------|--------|-------|
| ACP session Turn 2+ | `reattach_runtime` called; new subprocess spawned; conversation history lost | Same subprocess, same ACP session; history maintained across turns |
| Session event on every turn | `runtime_exit_without_stop_reason` | `stop_reason` (clean completion) |
| `usage_update` notification | Crashes `incoming_actor`, kills connection | Deserialized as `UsageUpdate`, forwarded as `unhandled_provider_message` provider_info event |
| Race condition (fast responses ≈ 5 s) | `StopReason` randomly skipped, Turn N+1 produces no output | `biased;` always consumes `StopReason` first |
| Long agentic tool calls (> 5 s) | Idle timeout fires, late `StopReason` poisons Turn N+1 | `previous_turn_idle` drain waits for late `PromptResponse` before Turn N+1 |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| 5-turn session, Turn 5: "What is my favorite color?" | Recalls "ultraviolet purple" from Turn 1 | "Your favorite color is ultraviolet purple." | ✅ PASS |
| Session events: completion type on each turn | `stop_reason` (not `runtime_exit_without_stop_reason`) | All 5 turns: `stop_reason` | ✅ PASS |
| `runtime_exit_without_stop_reason` events | None | Zero in 47-event session log | ✅ PASS |
| `reattach_runtime` log lines during 5-turn session | None | Zero reattach calls | ✅ PASS |
| `cargo check -p lab@0.12.1` after all changes | 0 errors, 0 warnings | 0 errors, 0 warnings | ✅ PASS |

Session ID used for verification: `29ce2bb5-6de7-4be2-9d00-1f9f5bdad136`

## Risks and Rollback

- **`features = ["unstable"]` enables all unstable ACP schema variants**: These are documented as "may be removed or changed at any point." If a future codex-acp sends a malformed unstable message, the deserialization will now attempt to parse it rather than failing on an unknown tag. Risk is low — unknown variants within an enabled feature would still fail with a specific field error rather than a silent protocol error.
- **Rollback**: Revert `crates/lab/Cargo.toml` to `agent-client-protocol = "0"` (remove `features`), run `just dev-debug`, restart container.
- **Turn drain timeout**: A 300-second drain that blocks the next prompt could delay conversation response in edge cases where codex-acp's tool call is genuinely stuck. Operator can reduce via `LAB_ACP_TURN_DRAIN_TIMEOUT_MS`. Drain exits early on `StopReason` or connection error in the normal case.

## Decisions Not Taken

- **Patch `agent-client-protocol-schema` locally**: Would require a `[patch.crates-io]` section and maintaining a fork. The `features = ["unstable"]` flag is the documented mechanism and requires zero patch maintenance.
- **Add `#[serde(other)]` fallback to `SessionUpdate`**: Would require patching the upstream schema crate. Rejected for same reason as above.
- **Increase idle timeout from 5 s to handle agentic tool calls**: Would make simple chat sluggish (longer wait before declaring a turn complete). The `previous_turn_idle` drain handles agentic sessions correctly without changing the chat UX.
- **Forward stale Turn N content during drain**: Content arriving after `idle_completion` is orphaned and would duplicate or reorder SSE events already delivered. Discarding is correct.

## References

- ACP SDK source: `~/.cargo/registry/src/.../agent-client-protocol-0.11.1/src/`
  - `jsonrpc.rs:1241` — `run_until(background, foreground)` races both
  - `util.rs:124-142` — `run_until` implementation: `Either::Left` propagates background error and drops foreground
  - `session.rs:560-576` — `send_prompt` uses `on_receiving_result` to spawn response callback
  - `jsonrpc/transport_actor.rs` — `transport_incoming_lines_actor` exits on stdout EOF
- Schema: `~/.cargo/registry/src/.../agent-client-protocol-schema-0.12.0/src/client.rs:84-115` — `SessionUpdate` enum with `UsageUpdate` gated by `#[cfg(feature = "unstable_session_usage")]`
- codex-acp source: `/home/jmagar/workspace/acp/codex-acp/src/codex_agent.rs:732-742` — `prompt()` handler
- codex-acp source: `/home/jmagar/workspace/acp/codex-acp/src/thread.rs:3109-3213` — `handle_prompt` submits `Op::UserInput` to codex-core `CodexThread`
- PR #40: https://github.com/jmagar/lab/pull/40

## Open Questions

- **`unstable` feature stability**: The `UsageUpdate` variant is marked "UNSTABLE — This capability is not part of the spec yet, and may be removed or changed at any point." If codex-acp changes its schema for `usage_update`, lab will encounter a deserialization error again (but with a different, more informative error message now that the `connect_with.error` log is in place).
- **`biased;` and agentic session ordering**: With `biased;`, if multiple `SessionMessage::SessionMessage` dispatch events are queued simultaneously with a `StopReason`, they are all consumed before breaking. This is correct behavior but has not been tested with rapid-fire tool call completions.
- **`UnhandledProviderMessage` events in the SSE stream**: `usage_update` is now forwarded as `ProviderInfo { type: "unhandled_provider_message" }` to the frontend. The gateway-admin UI may or may not handle this gracefully — it was not tested in this session.

## Next Steps

**Started but not completed:**
- The diagnostic `tracing::error!` log at `connect_with.error` was added as part of debugging and should be reviewed — it may be too verbose for production (logs on every normal session teardown if the connection closes with an error). Consider downgrading to `WARN` or scoping to non-clean exits only.

**Follow-on tasks not yet started:**
- Commit and push the three file changes (`Cargo.toml`, `Cargo.lock`, `runtime.rs`) to PR #40.
- Verify the `unhandled_provider_message` SSE events from `usage_update` do not cause visible issues in the gateway-admin chat UI.
- Consider adding a multi-turn integration test to `crates/lab/tests/` that exercises 2+ turns against a mock codex-acp to prevent regression.
- Check if `agent-client-protocol` 0.12.x (when released) will handle `UsageUpdate` by default, allowing removal of the explicit `features = ["unstable"]` opt-in.
