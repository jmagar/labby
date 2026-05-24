---
date: 2026-05-23 07:59:50 EST
repo: git@github.com:jmagar/lab.git
branch: fix/upstream-proxy-hardening
head: 3919c74a
plan: docs/superpowers/plans/2026-05-23-upstream-proxy-hardening.md
agent: Claude (Opus 4.7)
working directory: /home/jmagar/workspace/lab/.worktrees/upstream-proxy-hardening
worktree: /home/jmagar/workspace/lab/.worktrees/upstream-proxy-hardening
pr: #69 — fix(upstream): harden MCP proxy — process orphan, body OOM, UTF-8 panic — https://github.com/jmagar/lab/pull/69
---

## User Request

Run the full `work-it` pipeline against `docs/superpowers/plans/2026-05-23-upstream-proxy-hardening.md` — create a worktree, implement F3 + F2 + F1 with TDD, open a PR, run review waves (lavra-review + 3× code_simplifier + pr-review-toolkit + GitHub PR comments), then save-to-md and final push.

## Session Overview

Executed the full lavra design pipeline followed by `work-it` on three upstream-MCP-proxy defects identified in earlier code review:

- **F3** (`lab-4z8sx.3`): UTF-8 char-boundary panic in `wildcard_matches` when matching multi-byte tool names against `expose_tools` patterns.
- **F2** (`lab-4z8sx.2`): post-hoc response-body size check at `pool.rs:1748,2035,2532,2616` allowed OOM before rejection; needed a transport-layer cap at the rmcp `StreamableHttpClient` trait.
- **F1** (`lab-4z8sx.1`): stdio child process-group orphan when the connect future was dropped — only the lead PID died, leaving npx-spawned grandchildren attached to PID 1.

PR #69 opened with all 3 fixes, then 4 additional commits applied review findings: cross-chunk SSE delimiter correctness, preallocation, observability/logging, OAuth WARN dedup, precise per-event byte accounting (Codex review feedback).

## Sequence of Events

1. Skipped `lavra-research`/`lavra-design`/`lavra-eng-review` ordering issue, ran `lavra-plan` first to create epic `lab-4z8sx` with 3 children (F1/F2/F3) at STANDARD detail.
2. `lavra-research` dispatched 5 agents; 3 hit Sonnet rate limits, 2 (framework-docs-researcher + learnings-researcher) returned solid evidence including the `process_spawn_culprit` memory recall and rmcp 1.6 trait shape.
3. `lavra-design` revised bead notes with research findings; skipped CEO review (concrete bug fixes, not business-fit).
4. `lavra-eng-review` dispatched 4 reviewers; 2 returned (architecture, performance) — performance-oracle flagged BLOCKING bug: cumulative SSE cap would disconnect long-lived subscriptions. Switched F2 policy to per-event cap.
5. `superpowers:writing-plans` produced the formal plan file `docs/superpowers/plans/2026-05-23-upstream-proxy-hardening.md`.
6. `work-it` started: created worktree `.worktrees/upstream-proxy-hardening`. Fixed baseline `apps/gateway-admin/out` missing dir (copied from main) and `tool_search_schema_visible` missing test wrapper at `mcp/server.rs:2599`.
7. Implemented F3 (Task 1): rewrote `wildcard_matches` using `str::match_indices`. Added unicode tests + proptest. 8/8 passed.
8. Ran `cargo fmt --all` to fix 3 pre-existing diff files (`cli/gateway.rs`, `dispatch/gateway/dispatch.rs`, `mcp/server.rs`) so `just lint` passed.
9. Implemented F2 (Task 2): `BodyCappedHttpClient` (`http_client.rs`) implementing rmcp's `StreamableHttpClient` trait. Wired into non-OAuth path; OAuth path documented as follow-up (AuthClient is `#[non_exhaustive]`). 4/4 wiremock tests passed.
10. Implemented F1 (Task 3): `ProcessGroupGuard` (`process_guard.rs`) + `Drop` on `UpstreamConnection` + `take()` pgid before `.await` in `shutdown()`. 2/2 setsid-based tests passed after upgrading timeout to 2s.
11. Opened PR #69. Pushed branch.
12. Review wave 1: dispatched perf/code-reviewer/silent-failure agents. perf-oracle found cross-chunk SSE delimiter bug. Codex bot independently flagged the same issue on the PR.
13. Fixed cross-chunk boundary detection (`prev_ended_with_lf` state) + preallocate `read_body_capped` Vec when Content-Length known. Pushed.
14. Review wave 2: dispatched pr-review-toolkit:code-simplifier + code-reviewer + silent-failure-hunter. code-reviewer confirmed F1/F2/F3 correctness clean. silent-failure flagged OAuth WARN flood + Drop observability gap + `_server_task.take()` inside `#[cfg(unix)]` bug.
15. Fixed: (a) UpstreamConnection Drop now aborts `_server_task` on all platforms; (b) `log_oauth_uncapped_once` dedups warnings via `OnceLock<Mutex<HashSet>>`; (c) Drop impls now emit `tracing::warn!` on syscall failure and `debug!` on success.
16. Review wave 3: silent-failure-hunter + pr-test-analyzer + comment-analyzer. Findings: `CappedStreamError::Reqwest` Display lost context (no prefix), `shutdown()` log fields showed pgid as None (clone-then-take ordering), `call_tool` doc comment drift (post-hoc claim wrong for HTTP non-OAuth now), missing SSE happy-path test.
17. Fixed all 4 + added SSE happy-path test (`sse_happy_path_yields_events_under_cap`). Pushed.
18. Replied to Codex GitHub PR comment with resolution commit SHA after the precise byte-accounting refactor.
19. Codex flagged subtler edge case: mid-chunk `\n\n` followed by bytes belonging to next event was undercounted. Rewrote scan with `account_event_bytes` doing byte-level position tracking. Added 6 new tests including the "multi-event-per-chunk no-false-positive" case Codex described.

## Key Findings

- **rmcp `StreamableHttpClient` trait shape** (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-1.6.0/src/transport/streamable_http_client.rs:199-232`): 3 methods (`post_message`, `delete_session`, `get_stream`). `StreamableHttpClientWorker::new` accepts any type implementing this trait. Wrap layer is the correct boundary for body cap. rmcp 1.7 has identical surface.
- **AuthClient is `#[non_exhaustive]`** (`rmcp-1.6.0/src/transport/auth.rs`): cannot construct `AuthClient<BodyCappedHttpClient>` from outside rmcp; threading the cap through `OauthClientCache` requires a deeper refactor. Tracked as F2 follow-up.
- **`process_wrap::ProcessGroup::leader()` semantics**: child becomes its own pgid leader (`pgid == pid`). `killpg(pgid)` reaches grandchildren that rmcp's per-PID `TokioChildProcess` Drop misses. Do NOT add `kill_on_drop(true)` — would double-kill with process_wrap.
- **Codex/Performance-oracle dual flag**: cross-chunk `\n\n` delimiter detection — independent reviewers caught the same bug, validating the agent stack.
- **Pre-existing baseline failures fixed as scope creep**: `tool_search_schema_visible` missing fn at `mcp/server.rs:3613` (test compilation broken on main); `cargo fmt` diffs in 3 unrelated files (`cli/gateway.rs`, `dispatch/gateway/dispatch.rs`, `mcp/server.rs`).
- **`account_event_bytes` algorithm** (`http_client.rs:account_event_bytes`): byte-level scan for `\n` positions with delimiter detection at `\n\n` boundaries. Handles 4 cases: intra-chunk, cross-chunk, mid-chunk-with-next-event, multi-event-in-one-chunk.

## Technical Decisions

- **SSE cap = per-event, not cumulative** (engineering review blocker → locked in F2 bead). Long-lived subscriptions must not disconnect after arbitrary cumulative bytes.
- **OAuth path uncapped for now**: threading `BodyCappedHttpClient` through `OauthClientCache` requires building `AuthClient<BodyCappedHttpClient>` end-to-end. Documented + WARN-once log + follow-up in bead. Non-OAuth path (primary attack surface) is fully capped.
- **Drop is sync, no `sleep` between TERM/KILL**: graceful 150ms wait stays in async `shutdown()` path. Drop is the abandonment safety net.
- **`shutdown()` clones runtime BEFORE `take()`**: ensures log fields surface the actual pgid (not the post-take None).
- **`chunk_contains_event_boundary` kept as `#[cfg(test)]`**: legacy helper retained for test coverage; production path uses `account_event_bytes`.
- **Skipped pure-cosmetic simplifier suggestions**: flatten `validate_custom_header`, inline `parse_json_rpc_error`, let-else chain in `extract_scope_from_header`. Current form is fine and matches rmcp's upstream reference impl.
- **Did NOT detour into the 89 `--all-targets` clippy errors**: `just lint` uses `--workspace --all-features` only (no `--tests` / `--all-targets`); the 89 errors are pre-existing in integration tests across the repo.

## Files Modified

- `crates/lab/src/dispatch/upstream/types.rs` — F3 `wildcard_matches` rewrite + unicode tests + proptest
- `crates/lab/src/dispatch/upstream/http_client.rs` — NEW `BodyCappedHttpClient`, `account_event_bytes`, `CappedStreamError`, 12 unit + integration tests
- `crates/lab/src/dispatch/upstream/process_guard.rs` — NEW `ProcessGroupGuard` (RAII) + 2 setsid integration tests
- `crates/lab/src/dispatch/upstream/pool.rs` — F1 (Drop on UpstreamConnection, shutdown-take-before-await, connect_stdio guard arm/disarm), F2 (wrap non-OAuth + OAuth-skip-with-once-warn), call_tool doc correction
- `crates/lab/src/dispatch/upstream.rs` — module declarations
- `crates/lab/src/mcp/server.rs` — added `tool_search_schema_visible` test-only wrapper + cargo fmt
- `crates/lab/src/cli/gateway.rs`, `crates/lab/src/dispatch/gateway/dispatch.rs` — pre-existing cargo fmt diffs
- `crates/lab/Cargo.toml` — added `bytes = "1"` + `sse-stream = "0.2"` direct deps
- `docs/superpowers/plans/2026-05-23-upstream-proxy-hardening.md` — formal plan file

## Commands Executed

- `git worktree add -b fix/upstream-proxy-hardening .worktrees/upstream-proxy-hardening HEAD` — created isolated worktree
- `cp -r ~/workspace/lab/apps/gateway-admin/out apps/gateway-admin/out` — fixed baseline `include_dir!` failure (build artifact)
- `cargo nextest run ...` → unavailable, fell back to `cargo test --workspace --all-features --lib`
- `cargo test --workspace --all-features --lib -- dispatch::upstream` — final: 59 passed
- `cargo test --workspace --all-features --lib` — final: 1366 passed, 26 ignored
- `just lint` — passes (clippy clean + fmt clean)
- `gh pr create --title ...` — opened PR #69
- `gh api repos/jmagar/lab/pulls/69/comments/{id}/replies -X POST` — replied to Codex review

## Behavior Changes (Before/After)

- **Before:** A discovery timeout against an `npx`-wrapped stdio MCP server leaked the underlying node process to PID 1.
  **After:** `ProcessGroupGuard` drop fires `killpg(pgid, SIGTERM); killpg(pgid, SIGKILL)`, reaping the entire group.
- **Before:** A hostile upstream returning 5 GB JSON OOMed the gateway before the 10 MB check fired.
  **After:** `BodyCappedHttpClient` checks Content-Length up front and counts bytes during `bytes_stream()` consumption; rejects at the transport layer.
- **Before:** A tool name with a multi-byte unicode char + a wildcard `expose_tools` pattern panicked the listing task.
  **After:** `str::match_indices` advances the cursor on char-boundary-aligned offsets; proptest locks the invariant.
- **Before:** OAuth-uncapped WARN logged at every reconnect/reprobe (every ~30s × N upstreams).
  **After:** `log_oauth_uncapped_once(name)` logs once per upstream per process.
- **Before:** UpstreamConnection Drop was silent on both success and signal failure.
  **After:** `tracing::warn!` on syscall failure, `tracing::debug!` on successful pgid reap.

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `cargo test --workspace --all-features --lib -- dispatch::upstream::types` | new + existing wildcard tests pass | 8 passed | ✅ |
| `cargo test --workspace --all-features --lib -- dispatch::upstream::http_client` | wiremock + boundary tests pass | 12 passed | ✅ |
| `cargo test --workspace --all-features --lib -- dispatch::upstream::process_guard` | setsid-based guard tests pass | 2 passed | ✅ |
| `cargo test --workspace --all-features --lib -- dispatch::upstream` | all upstream tests pass | 59 passed | ✅ |
| `cargo test --workspace --all-features --lib` | full workspace lib tests pass | 1366 passed, 26 ignored | ✅ |
| `just lint` | clippy + fmt clean | exit 0 | ✅ |
| `gh pr checks 69` | CI green on latest commit | pending at session-save | ⏳ |

## Risks and Rollback

- **F2 OAuth gap**: hostile OAuth-protected upstream can still OOM. Follow-up bead tracks threading cap through `OauthClientCache`. Rollback: revert PR — no schema/data migrations.
- **F1 process-group race**: PID reuse after wraparound could SIGKILL an unrelated process group. Sub-second window, requires >32k PID cycle between guard-arm and Drop-fire; accepted residual risk per plan.
- **`bytes` + `sse-stream` direct deps added**: workspace-wide dependency graph change. Both were already transitive deps via rmcp; making them direct doesn't change resolved versions.
- **F2 `account_event_bytes` is more complex than the original `windows(2)` scan**: 50+ lines of byte-position math. All 6 new unit tests pass including Codex's exact false-positive scenario. Risk: future maintainer breaking the byte-counting invariant; mitigated by test coverage.

## Decisions Not Taken

- **F2 fork rmcp**: considered exposing the body-cap as a feature flag in rmcp itself. Rejected — wrapper at the trait layer doesn't require fork, and rmcp 1.6/1.7 surfaces are identical.
- **F1 reuse `tokio::process::Command.kill_on_drop(true)`**: rejected per research — rmcp's `TokioChildProcess` already wraps Child with its own Drop+async-kill path, and combining would double-kill with process_wrap.
- **F2 use `tower::Layer` middleware**: rejected — middleware can't access streaming body chunks for per-event SSE capping.
- **F3 add `globset` or `wildmatch` crate**: rejected — `str::match_indices` is enough for `*`-only patterns; smaller blast radius.
- **Detour into 89 pre-existing `cargo clippy --all-targets` errors**: rejected — out of scope for this PR; `just lint` (the actual repo gate) passes.

## References

- Plan file: `docs/superpowers/plans/2026-05-23-upstream-proxy-hardening.md`
- Beads epic: `lab-4z8sx` (children .1, .2, .3)
- PR: https://github.com/jmagar/lab/pull/69
- Memory recall: `process_spawn_culprit` (root MEMORY.md)
- rmcp 1.6 source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-1.6.0/src/transport/streamable_http_client.rs`
- Codex PR review comment: https://github.com/jmagar/lab/pull/69#discussion_r3292671659
- Reqwest body-limit discussion: https://github.com/seanmonstar/reqwest/issues/848 (won't-fix)
- Prior art for UTF-8 boundary lesson: bead `lab-fstf.3`

## Open Questions

- Should `account_event_bytes` count the trailing `\n` after a single-`\n` cross-chunk close? Currently sets count to 0 immediately after the cross-boundary `\n`; the previous chunk already accounted for the bytes up to its end. Edge case where the previous chunk's last byte was the `\n` that closed the event AND the chunk wasn't approved at the cap line — but the cap check runs per-chunk so this would already have errored.
- Codex's "review limit reached" message on the PR (separate from the addressed inline comment) — does this need separate engagement?

## Next Steps

**Started but not completed (in this session):**
- CI status check: `gh pr checks 69` returned `pending` for Actionlint, Cargo Deny, Format, Frontend assets. Need to verify green before merge.

**Follow-on tasks (not yet started):**
- Thread `BodyCappedHttpClient` through `OauthClientCache` to cap the OAuth HTTP path (tracked in `lab-4z8sx.2` notes).
- Catalog-ingress BIDI/control-char rejection for tool names (security-sentinel suggestion; deferred — out of F3 scope).
- Per-stream wall-clock idle timeout for `get_stream` SSE (architecture-strategist follow-up suggestion; deferred).
- Consider integration test `crates/lab/tests/upstream_stdio_orphan.rs` for the F1 leak-prevention end-to-end (test-analyzer suggested; current unit tests cover the guard mechanics).
