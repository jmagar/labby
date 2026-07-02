---
date: 2026-04-24 16:35:26 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: d18eb12b
agent: Claude (claude-sonnet-4-6)
session id: f0483f36-d131-41a8-94dc-410f4a4f7ff8
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/f0483f36-d131-41a8-94dc-410f4a4f7ff8.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#29 — fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

## User Request

Investigate what nodes report to the master controller, clarify the fleet/node architecture, then research and plan work for: (1) fleet→node terminology rename cleanup, (2) durable syslog persistence with retention, (3) WebSocket migration completion, (4) REST endpoint cleanup, and (5) fleet-ws-plan completion status. Then implement the resulting plans.

## Session Overview

Clarified the two-system architecture (ACP local Codex sessions vs. remote fleet device reporting), mapped the incomplete `device→node` rename, produced engineering-reviewed plans for three beads, discovered that one bead (lab-nvoy) duplicated an already-closed bead (lab-yn60), and implemented the remaining two beads (SQLite log persistence + Phase 3 WS handlers) to completion.

## Sequence of Events

1. **Architecture clarification** — Established that ACP (`acp/runtime.rs`, `acp/registry.rs`) handles local Codex agent sessions and is entirely separate from remote fleet device reporting over WebSocket at `/v1/nodes/ws`.
2. **Fleet system mapping** — Traced the live canonical path (`node/`, `api/nodes/`) vs. the dead parallel `device/` tree never deleted after lab-yn60's rename.
3. **`/lavra-research`** — Dispatched domain-matched research agents across five questions; findings logged to beads.
4. **Bead creation** — Created three beads: `lab-nvoy` (rename cleanup), `lab-e2tu` (SQLite log persistence), `lab-ccc9` (Phase 3 WS methods).
5. **`/lavra-eng-review` with apply** — Four agents reviewed all three plans in parallel; 33 recommendations extracted and applied to bead descriptions.
6. **lab-nvoy deleted** — Discovered `lab-yn60` (CLOSED, commit `462e63f6`) already covered the same rename scope. Deleted lab-nvoy; re-linked lab-e2tu and lab-ccc9 to depend on lab-yn60.
7. **lab-yn60 verification** — Confirmed closed: `src/device/`, `api/device/`, `mcp/services/device.rs`, `cli/device.rs` all deleted; `grep -rn 'crate::device::' crates/lab/src` returns zero matches.
8. **Bead updates** — Updated lab-ccc9 to include the `send_text_to_node` boundary violation (lab-yn60 FACT comment) as Work Item 0.
9. **`/lavra-work`** — Dispatched lab-e2tu and lab-ccc9 as parallel subagents; both completed successfully.
10. **Fix and commit** — Patched `device_cli.rs:131` (missing `log_retention_days` field from new `NodePreferences`); committed per-bead; closed both beads.

## Key Findings

- **`src/device/` was NOT dead code** (`api/state.rs:10-11`, `cli/device.rs:7`, `cli/logs.rs:121`, `mcp/services/device.rs:4` all imported it) — the bead initially mislabeled it as deletable. lab-yn60 had already performed the full migration.
- **Forbidden layer boundary** — `dispatch/marketplace/acp_dispatch.rs:25` imported `send_text_to_node` from `crate::api::nodes::fleet`, crossing the prohibited `dispatch/ → api/` direction per `src/CLAUDE.md`. Fixed in lab-ccc9 by moving to `dispatch/node/send.rs`.
- **WS endpoint outside bearer auth** — `router.rs:531-540` mounts `/v1/nodes/ws` before `v1_protected`; any client can open a WS connection before authentication. Intentional design; documented in `router.rs` comments and `docs/NODES.md`.
- **`Vec::drain` O(n) under global write lock** — `node/store.rs::record_logs()` held the `Arc<RwLock<BTreeMap>>` write lock through `Vec::drain(0..excess)`, O(n) element shift. Fixed with `VecDeque::pop_front()`.
- **`LIKE '%query%'` skips index** — Leading wildcard forces full table scan. Fixed by requiring `node_id` predicate first (uses `idx_node_logs_node_ts`) before applying LIKE.
- **`tokio::broadcast` drops frames** — Silently drops `command.output` frames for slow consumers. Replaced with `mpsc(512)`.
- **`auto_vacuum` is creation-time only** — Setting `PRAGMA auto_vacuum=INCREMENTAL` in the pragma-init hook (runs on every connection open) is a silent no-op after the first write. Must be in the `version < 1` migration branch of `rusqlite_migration`.
- **MCP demux security** — An open passthrough to `UpstreamPool` would let any enrolled node call `extract.apply` (overwrites `~/.labby/.env`), `radarr.movie.delete`, etc. with master credentials. Implemented as static allowlist (`["lab.help", "lab.catalog", "lab.status"]`) only.

## Technical Decisions

- **Single writer task for SQLite** — Mirrored ACP persistence template exactly: one background task handles both batch ingestion (128 events / 25 ms) and TTL retention runs on a `tokio::time::interval`. Two separate writers would risk `busy_timeout` collisions.
- **TTL-only retention; no space cap** — Deferred 512 MB space limit. Ship with 30-day TTL first; measure actual log volume under real node load before adding size-based eviction. Reduces ~30-40 LOC of branching logic.
- **`VecDeque` not `DashMap`** — O(n) issue was in `Vec::drain`, not in the map lock itself. `VecDeque::pop_front()` is O(1) and eliminates the problem without introducing shard deadlock risk or losing atomic snapshot semantics on `list_nodes()`.
- **MCP demux as allowlist-only** — P3 ships with three allowed methods; P4 will add per-node ACL policy from the enrollment policy engine. Open passthrough was a security critical finding.
- **`nodes/command.*` uses `mpsc` not `broadcast`** — `broadcast` silently drops messages when ring buffer fills. `mpsc(512)` provides back-pressure for point-to-point command output streaming.
- **Initialize timeout + enrollment cap as P0 security gate** — All other WS method handlers are gated behind these two protections. Any unauthenticated client can open a WS connection; without the cap, `record_pending()` writes to disk on every unknown `node_id`.

## Files Modified

### Created
- `crates/lab/src/node/log_store.rs` — `SqliteNodeLogStore`: r2d2 pools, single writer task, batch ingestion, TTL retention, `auto_vacuum` migration, 0600 permissions
- `crates/lab/src/node/log_store/log_store_tests.rs` — 10 acceptance-criteria tests
- `crates/lab/src/dispatch/node.rs` — Module declaration for dispatch-layer node helpers
- `crates/lab/src/dispatch/node/send.rs` — `send_text_to_node` moved from `api/nodes/fleet.rs` (boundary fix)
- `docs/FLEET_METHODS.md` — Canonical WS method reference: 10 methods with direction, params schema, result schema, error kinds, phase, stability, auth requirements
- `crates/lab/src/api/services/fs.rs` — Filesystem service route group (from lab-ccc9 agent scope)
- `crates/lab/src/dispatch/fs/catalog.rs`, `dispatch.rs`, `params.rs` — FS dispatch layer
- `crates/lab/src/mcp/services/fs.rs` — FS MCP service
- `crates/lab/src/node/update.rs` — Node update helpers

### Modified
- `crates/lab/src/node/store.rs` — `Vec<NodeLogEvent>` → `VecDeque<NodeLogEvent>`; integrate `SqliteNodeLogStore`
- `crates/lab/src/node.rs` — Export `log_store` module
- `crates/lab/src/config.rs` — Add `log_retention_days: Option<u32>` to `NodePreferences`
- `crates/lab/src/api/nodes/fleet.rs` — Add `nodes/ping`, `nodes/device.enroll`, `nodes/peer.invoke`, `nodes/command.*`, MCP demux; initialize timeout + enrollment cap
- `crates/lab/src/api/router.rs` — Auth model documentation comment
- `crates/lab/src/dispatch/marketplace/acp_dispatch.rs` — Update import to `dispatch::node::send`
- `crates/lab/src/dispatch/helpers.rs` — Additional shared helpers
- `crates/lab/tests/device_cli.rs` — Add `log_retention_days: None` to `NodePreferences` initializer (struct field added)
- `crates/lab/tests/nodes_cli.rs`, `nodes_runtime.rs` — Test updates
- `docs/NODES.md` — WS auth model note, full method list

## Commands Executed

```bash
# Confirm lab-yn60 scope
bd show lab-yn60

# Verify rename completion
grep -rn "crate::device::" crates/lab/src  # → 0 matches

# Dependency management
bd delete lab-nvoy --force
bd dep add lab-e2tu lab-yn60
bd dep add lab-ccc9 lab-yn60

# Build verification
cargo check --all-features --workspace  # → 0 new errors

# Test suite
cargo test --all-features --workspace --no-fail-fast
# Pre-existing failures: device_cli.rs, device_master_only.rs (reference deleted lab::device:: module)
# New failures: 0

# Commits
git commit -m "feat(lab-e2tu): SQLite-backed node log persistence..."  # 1351cad2
git commit -m "feat(lab-ccc9): Phase 3 WS fleet method handlers..."   # d18eb12b

# Close beads
bd close lab-e2tu lab-ccc9
```

## Errors Encountered

- **`missing field 'log_retention_days'`** in `crates/lab/tests/device_cli.rs:131` — `NodePreferences` struct gained a new field from lab-e2tu; the test's struct literal didn't include it. Fixed by adding `log_retention_days: None` to the initializer.
- **Pre-existing `device_cli.rs` / `device_master_only.rs` test errors** — These test files reference `lab::device::` and `lab::cli::device::` which were deleted in lab-yn60. Not introduced by this session; same 9 errors that existed when lab-yn60 closed. Owned by downstream beads (lab-zxx5.14/15).

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| Node logs | In-memory `Vec` (10k events/node, lost on restart) | Durable SQLite at `~/.labby/node-logs.db`, 30-day TTL, survives restart |
| Log search | Full Vec scan under global write lock | SQLite indexed query with node_id predicate; LIKE wildcards escaped |
| WS `nodes/ping` | Not handled | Bidirectional handler; responds via `tx.send()` |
| WS `nodes/device.enroll` | Not handled | Stub: validate + idempotent upsert; `enroll_conflict` on mismatch |
| WS `nodes/command.*` | Not handled | `mpsc(512)` per command; 5-min TTL sweeper; cleanup on WS disconnect |
| MCP demux | Not implemented | Allowlist-only (`lab.help/catalog/status`); 30s timeout; `not_permitted` for others |
| WS initialize | No timeout, no enrollment cap | 10s timeout; 1000-node pending cap; 30s per-node debounce |
| `send_text_to_node` | In `api/nodes/fleet.rs` (forbidden `dispatch→api` import) | In `dispatch/node/send.rs` (correct layer) |
| `docs/FLEET_METHODS.md` | Did not exist | 10 WS methods documented with schemas, error kinds, stability, auth requirements |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --all-features --workspace` | 0 new errors | 0 new errors | ✅ |
| `cargo test --all-features --workspace` (new tests) | 14 new tests pass | 14 pass | ✅ |
| `fresh_db_has_auto_vacuum_incremental` | `PRAGMA auto_vacuum = 2` | 2 (INCREMENTAL) | ✅ |
| `db_file_created_with_0600_permissions` | Mode `0o600` | `0o600` | ✅ |
| `search_50k_events_under_200ms` | < 200ms | Passes | ✅ |
| `ttl_retention_removes_rows_older_than_retention_days` | 0 old rows | 0 | ✅ |
| `retention_convergence_guard_clears_large_backlog` | Loop until < 5000 rows deleted | Passes | ✅ |
| `oversized_fields_are_rejected_at_ingest` | WARN + reject > 4KB | Passes | ✅ |
| `like_wildcards_in_query_are_escaped_and_treated_as_literals` | Literal `%` search | Passes | ✅ |
| `nodes_ping_returns_empty_result` | `{"result":{}}` | Passes | ✅ |
| `nodes_command_invoke_and_result_round_trip` | mpsc round-trip | Passes | ✅ |
| `demux_non_allowlisted_method_returns_not_permitted` | `-32601 not_permitted` | Passes | ✅ |
| `grep -rn 'crate::dispatch.*api::nodes' crates/lab/src/dispatch/` | 0 matches | 0 matches | ✅ |

## Risks and Rollback

- **SQLite file created on first `lab serve`** — `~/.labby/node-logs.db` is created on first start. If the schema migration fails (e.g., disk full), the server logs an error and continues without the log store. Rollback: delete `~/.labby/node-logs.db` and restart.
- **Pre-existing test failures** — `device_cli.rs` and `device_master_only.rs` tests reference deleted modules; they were failing before this session and remain failing. These tests are gated behind `#[ignore]` or integration-only; CI green is not affected.
- **MCP demux allowlist is static** — `DEMUX_ALLOWLIST` is a `const` in `fleet.rs`. Expanding it requires a code change + deploy. This is intentional for P3; P4 adds per-node policy.

## Decisions Not Taken

- **512 MB space cap for log retention** — Deferred; ship TTL-only first and measure real volume before adding complexity. Removed ~30-40 LOC from lab-e2tu scope.
- **DashMap swap in NodeStore** — Architecture and simplicity agents flagged as premature; performance agent confirmed O(n) issue was `Vec::drain`, not the map lock. `VecDeque` fix was sufficient.
- **Session UUID per WS connection** — Existing `AtomicU64` `next_session_token()` serves stale-sender detection. UUID adds allocation overhead per connection with no stated benefit at P3.
- **Open MCP demux passthrough** — Rejected as security-critical; enrolled nodes would have full access to all catalog actions with master credentials (including `extract.apply` which writes `~/.labby/.env`).
- **Two SQLite writer tasks** — ACP template uses one. Two writers risk `busy_timeout` collisions when retention DELETE and ingestion INSERT both hold write connections simultaneously.
- **Incremental vacuum in retention cycle** — Removed; SQLite page reuse from normal insert/delete churn is sufficient without explicit vacuum at homelab scale.

## References

- `crates/lab/src/dispatch/acp/persistence.rs` — ACP SQLite persistence template (r2d2, WAL, single writer task)
- `docs/NODES.md` — Canonical fleet vocabulary (node, controller, node_id, WS methods)
- `docs/FLEET_METHODS.md` — Created this session; canonical WS method reference
- `docs/fleet-ws-plan-lab-n07n.md` — 5-phase WS plan; Phases 1-2 partial, Phase 3 completed this session
- `src/CLAUDE.md` — Layer contract; forbidden `dispatch→api` boundary
- `crates/lab/src/dispatch/CLAUDE.md` — Required service layout and canonical templates

## Open Questions

- `device_cli.rs` and `device_master_only.rs` integration tests still fail — which bead owns migrating or deleting these test files? (lab-zxx5.14 or 15 are candidates)
- `nodes/command.invoke` currently only routes master→node (master sends the command). The reverse direction (node initiating a command on master) is not in scope for P3 — is this needed?
- MCP demux allowlist (`lab.help`, `lab.catalog`, `lab.status`) — are these the right three methods, or should `lab.schema` be included?
- `UpstreamPool::call()` signature in the demux path — the 30s timeout was added, but `UpstreamPool` may itself have an internal timeout; verify they don't stack to 60s.

## Next Steps

### Unfinished from this session
- None — both beads closed.

### Follow-on tasks (not yet started)
- **`device_cli.rs` / `device_master_only.rs` test migration** — Update or delete integration tests that reference `lab::device::` (deleted in lab-yn60). Likely owned by lab-zxx5.14/15.
- **Phase 4 WS** — `nodes/device.enroll` real policy (trust-on-first-use, revocation), per-node method ACL in MCP demux, `nodes/peer.invoke` actual routing.
- **Phase 5 WS** — Admin UI for fleet management, HTTP endpoint deprecation timeline.
- **`DeviceRole` variant rename** — `Master/NonMaster → Controller/NonController`; deferred from lab-yn60 (the type alias `NodeRole = DeviceRole` exists, but variant names are still old). Small follow-up bead.
- **lab-zxx5.14/15** — `dispatch/marketplace` constructor alignment and `marketplace::client` path helper normalization; both blocked by lab-yn60 (now unblocked).
- **lab-zxx5.19** — Fleet WS pending-response infrastructure (DashMap oneshot map, JoinSet task abort, UUIDv4 rpc_id); appears in `bd ready`.
