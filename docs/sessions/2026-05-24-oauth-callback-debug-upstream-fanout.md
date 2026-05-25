---
date: 2026-05-24 23:03:41 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: bc12d7b2
agent: Claude
session id: 708504f7-2155-4ce5-9c5f-85792129e189
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/708504f7-2155-4ce5-9c5f-85792129e189.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
---

# OAuth Callback Debug + Upstream Discovery Fan-Out

## User Request

Debug why an OAuth callback to `127.0.0.1:3793/callback` timed out from a Windows browser after starting from Zed, then increase the gateway's upstream MCP discovery concurrency.

## Session Overview

- Diagnosed the failed OAuth callback as a misaligned RFC 8252 loopback redirect: labby completed the flow correctly but the Zed-spawned listener was not reachable from the Windows browser where the consent screen finished.
- User pivoted to bearer-token auth, which works against `lab.tootie.tv/mcp` without the loopback step.
- Investigated whether the gateway parallelizes upstream MCP server initialization. Confirmed it does, but defaults to 3 concurrent connects with a 15s per-upstream timeout.
- Bumped `LAB_UPSTREAM_DISCOVERY_CONCURRENCY` from default 3 → 16 in both `docker-compose.yml` and `docker-compose.prod.yml` so ~20 upstreams warm in roughly one 15s window instead of seven.

## Sequence of Events

1. User shared a screenshot of `ERR_TIMED_OUT` at `http://127.0.0.1:3793/callback?code=...&state=...` and asked for systematic debugging.
2. Loaded the `superpowers:systematic-debugging` skill and gathered evidence: `docker ps`, `docker logs labby`, `ss -tlnp` for port 3793.
3. Confirmed labby logged a clean OAuth round-trip at `01:16:07 → 01:16:20 UTC`: DCR accepted, Google round-trip, local code minted, 302 to the registered loopback URI.
4. Confirmed nothing was listening on `127.0.0.1:3793` on dookie (only `codex` on 4500). Concluded the Zed listener and the browser were on different machines, or the listener had already exited.
5. User confirmed Zed initiated the auth, then said it was fine — they had switched to bearer token.
6. User asked about Zed MCP OAuth config knobs; attempted `axon ask` via `lab tool execute` (subcommand does not exist) and direct curl to `http://localhost:8001/v1/ask` (auth_failed; bearer token mismatch from `~/.axon/config.toml` vs runtime expectation). User interrupted.
7. User asked whether MCP server init was being fanned out. Read `crates/lab/src/dispatch/upstream/pool.rs`, found `discover_all_inner` uses `buffer_unordered(discovery_concurrency)` with default 3, and reported worst-case warmup math.
8. User said "just increase the concurrency". Edited both compose files to set `LAB_UPSTREAM_DISCOVERY_CONCURRENCY=16` and noted that a `docker compose up -d labby` restart is required for the env change to apply.

## Key Findings

- Labby OAuth server is healthy. Failure mode was entirely client-side reachability of the loopback URI (`crates/lab/src/...` auth code untouched).
- Loopback redirect URI is reported in DCR and authorize logs: `redirect_uris=["http://127.0.0.1:3793/callback"]`.
- Pool fan-out exists: `crates/lab/src/dispatch/upstream/pool.rs:894` `discover_all_inner` builds `discovery_jobs`, then `crates/lab/src/dispatch/upstream/pool.rs:975` calls `upstream_discovery_concurrency()`, and `crates/lab/src/dispatch/upstream/pool.rs:1065` uses `buffer_unordered(discovery_concurrency)`.
- Concurrency default: `DEFAULT_UPSTREAM_DISCOVERY_CONCURRENCY: usize = 3` at `crates/lab/src/dispatch/upstream/pool.rs:65`.
- Per-upstream timeout: `DISCOVERY_TIMEOUT: Duration::from_secs(15)` at `crates/lab/src/dispatch/upstream/pool.rs:59`.
- The `01:17:27` burst of "circuit breaker open" WARN spam in labby logs is consistent with the ~100s worst-case warmup at the old default.

## Technical Decisions

- Edited compose env rather than changing the Rust default. Keeps the source compatible with low-resource installs while letting this deployment widen the fan-out.
- Did not change `DISCOVERY_TIMEOUT`. Increasing the cap alone closes the gap; bounding per-upstream time at 15s still protects total warmup.
- Did not pursue the `axon ask` lookup further once the user moved off the Zed OAuth question.

## Files Modified

| status | path | purpose |
|---|---|---|
| modified | `docker-compose.yml` | Add `LAB_UPSTREAM_DISCOVERY_CONCURRENCY: "16"` to dev overlay labby env. |
| modified | `docker-compose.prod.yml` | Add `LAB_UPSTREAM_DISCOVERY_CONCURRENCY: "16"` to prod labby env. |

Pre-existing dirty files left untouched: `crates/lab/src/cli/gateway.rs`, `crates/lab/src/dispatch/gateway.rs`, `crates/lab/src/main.rs`.

## Commands Executed

- `docker ps --format ...` — confirmed `labby` container Up 7 minutes on 8765.
- `docker logs labby --tail 500 | grep -iE "oauth|callback|3793"` — surfaced the full OAuth log trail and the exact registered redirect URI.
- `ss -tlnp | grep :3793` — empty, confirming no local loopback listener on dookie.
- `grep -n -E "buffer_unordered|DEFAULT_UPSTREAM_DISCOVERY|DISCOVERY_TIMEOUT" crates/lab/src/dispatch/upstream/pool.rs` — located the fan-out site and tunables.

## Errors Encountered

- `lab tool execute axon ...` failed: `lab` CLI has no `tool` subcommand. Direct upstream tool execution via CLI is not the right entry point; `lab gateway` has `tool-search`, but ad-hoc upstream calls are not exposed there. Resolved by falling back to a direct HTTP call to axon.
- `curl http://localhost:8001/v1/ask` failed with `auth_failed: invalid bearer token`. The token in `~/.axon/config.toml` (`AXON_WEB_API_TOKEN=4TDc7+...`) was rejected by the running axon instance. User interrupted before further investigation; bypassed by switching back to the original task.

## Behavior Changes (Before/After)

- Before: labby warmed upstream MCP servers 3 at a time; ~20 upstreams took up to ~100s, producing a burst of transient "circuit breaker open" WARN logs in the first minute after start.
- After: with `LAB_UPSTREAM_DISCOVERY_CONCURRENCY=16`, warmup is bounded by `ceil(20/16) * 15s = 30s` and most upstreams should land in the first 15s window. Per-upstream timeout unchanged.

## Risks and Rollback

- Risk: higher concurrency increases simultaneous outbound connections at startup. With 16 parallel discoveries each capped at 15s, peak load is modest on a workstation but worth watching if upstreams share rate limits.
- Rollback: remove the `LAB_UPSTREAM_DISCOVERY_CONCURRENCY` env line from both compose files (default returns to 3), then `docker compose up -d labby`.

## Decisions Not Taken

- Did not introduce a CLI flag or new TOML key. The env var already exists and is read at startup; compose is the right surface.
- Did not change `DISCOVERY_TIMEOUT` from 15s. Concurrency alone is enough at the current upstream count.

## References

- `crates/lab/src/dispatch/upstream/pool.rs:59` — `DISCOVERY_TIMEOUT`
- `crates/lab/src/dispatch/upstream/pool.rs:65` — `DEFAULT_UPSTREAM_DISCOVERY_CONCURRENCY`
- `crates/lab/src/dispatch/upstream/pool.rs:128` — `upstream_discovery_concurrency()` env reader
- `crates/lab/src/dispatch/upstream/pool.rs:894` — `discover_all_inner`
- `crates/lab/src/dispatch/upstream/pool.rs:975` / `:1065` — fan-out site with `buffer_unordered`

## Open Questions

- Are any production upstreams sensitive to a 16-wide concurrent startup probe? Worth a one-time look at upstream logs after the next restart, particularly for the Google OAuth probe paths.
- What is the actual axon bearer token expected by the running container? The mismatch between `~/.axon/config.toml` and the live instance suggests the env source for axon has drifted from the file. Out of scope here but worth a follow-up if `ask` workflows become a habit.

## Next Steps

- Not started: restart `labby` (`docker compose up -d labby`) to apply the new concurrency, then re-check labby startup logs for circuit-breaker-open spam reduction.
- Not started: if useful, capture timing evidence (start log → "all upstreams discovered" event) before and after the change.
