---
date: 2026-05-10 22:53:25 EDT
repo: git@github.com:jmagar/lab.git
branch: fix/protected-route-edit-state
head: 151605c0
agent: Codex
session id: 019e14a1-a5e1-7723-8c48-1a87dc020134
transcript: /home/jmagar/.codex/sessions/2026/05/10/rollout-2026-05-10T21-23-18-019e14a1-a5e1-7723-8c48-1a87dc020134.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab 151605c0 [fix/protected-route-edit-state]
pr: 55 - fix(gateway-admin): rename gateways to servers - https://github.com/jmagar/lab/pull/55
---

# Codex MCP OAuth Refresh Debugging

## User Request

Investigate why Codex could not keep `axon` and `syslog` MCP servers connected through Lab-protected path routes, despite OAuth login completing successfully.

## Session Overview

- Traced multiple OAuth/MCP startup failures across Codex credentials, Lab protected routes, upstream OAuth, Axon OAuth, and syslog.
- Restored the intended model: Codex authenticates to Lab-protected route resources, and Lab proxies to named OAuth upstreams at configured public paths.
- Fixed Lab OAuth refresh-token handling so route-scoped refresh tokens continue to mint route-scoped access tokens when clients omit `resource` on refresh.
- Verified refreshed tokens initialize both `https://mcp.example.com/axon` and `https://mcp.example.com/syslog`.

## Sequence of Events

1. Reproduced Codex startup failures and inspected cached Codex MCP credentials under `/home/jmagar/.codex/.credentials.json`.
2. Confirmed Lab protected-route metadata advertised route resources `https://mcp.example.com/axon` and `https://mcp.example.com/syslog` with `mcp:read mcp:write`.
3. Fixed Codex MCP config so `axon` and `syslog` include explicit `oauth_resource` values matching their protected-route resources.
4. Reconfigured Lab local protected routes to use named upstreams instead of raw `backend_url` proxying for OAuth upstreams.
5. Fixed Axon container OAuth env drift: `/home/jmagar/.axon/.env` had Lab Google OAuth credentials while `AXON_MCP_PUBLIC_URL` was `https://axon.example.com`.
6. Identified the recurring refresh failure: Codex sends refresh-token grants without `resource`; Lab incorrectly defaulted those refreshes to `https://lab.example.com/mcp`.
7. Updated `lab-auth` refresh-token handling and verified the live deployed Lab container after rebuilding the debug binary.

## Key Findings

- Codex route config needs explicit route resources: `/home/jmagar/.codex/config.toml:86` and `/home/jmagar/.codex/config.toml:93`.
- Lab route config now publishes named OAuth upstreams: `/home/jmagar/workspace/lab/config.toml:86`, `/home/jmagar/workspace/lab/config.toml:101`, `/home/jmagar/workspace/lab/config.toml:155`, and `/home/jmagar/workspace/lab/config.toml:172`.
- Axon canonical runtime env is `/home/jmagar/.axon/.env`, not repo `.env`; the running container had Lab Google OAuth credentials until corrected.
- Lab logs showed refresh requests with `requested_resource=https://lab.example.com/mcp` while stored route refresh tokens were bound to `https://mcp.example.com/axon` and `https://mcp.example.com/syslog`.
- The refresh-token bug was in [crates/lab-auth/src/token.rs](/home/jmagar/workspace/lab/crates/lab-auth/src/token.rs:276): omitted refresh `resource` was treated as canonical Lab resource instead of the stored refresh-token resource.

## Technical Decisions

- Kept protected routes on named upstreams, not `backend_url`, because named upstreams carry the upstream OAuth manager and stored credentials.
- Kept `oauth_resource` explicit in Codex config because Lab serves multiple protected resources from the same authorization server.
- Preserved explicit `resource` validation on refresh when a client supplies `resource`, but changed omitted `resource` to mean "use the refresh token's stored resource."
- Used debug `labby` rebuild for hot-swap because release build failed during LLVM optimization with out-of-memory.

## Files Modified

- [crates/lab-auth/src/token.rs](/home/jmagar/workspace/lab/crates/lab-auth/src/token.rs:276): refresh-token grants now preserve the stored refresh-token resource when `resource` is omitted.
- [crates/lab-auth/src/token.rs](/home/jmagar/workspace/lab/crates/lab-auth/src/token.rs:783): regression test for omitted-resource refresh preserving `aud` and `scope`.
- `/home/jmagar/.codex/config.toml`: restored `oauth_resource` for `axon` and `syslog`.
- `/home/jmagar/workspace/lab/config.toml`: local ignored config updated so `syslog` and `axon` protected routes publish named OAuth upstreams.
- `/home/jmagar/.axon/.env`: corrected Axon Google OAuth client ID/secret for `https://axon.example.com`.
- `/home/jmagar/workspace/lab/bin/labby`, `/home/jmagar/.local/bin/labby`, `/home/jmagar/.local/bin/lab`: installed rebuilt debug binary for live verification.

## Commands Executed

- `codex mcp login axon` / `codex mcp login syslog`: completed browser login, then exposed route/audience drift during startup.
- `curl https://mcp.example.com/.well-known/oauth-protected-resource/{axon,syslog}`: confirmed route resources and scopes.
- `sqlite3 /home/jmagar/.labby/auth.db ... refresh_tokens ...`: confirmed stored refresh-token resources were route-specific.
- `docker inspect axon ...`: confirmed running Axon env had the wrong Google OAuth client before correction.
- `docker compose --env-file /home/jmagar/.axon/.env -f docker-compose.yaml up -d --no-deps --force-recreate axon`: recreated Axon with corrected env.
- `cargo test -p lab-auth token_endpoint_refresh_grant --all-features`: ran focused regression coverage.
- `RUSTC_WRAPPER= cargo build -p labby --all-features`: built patched debug binary.
- `docker compose -f docker-compose.yml -f docker-compose.dev.yml restart labby-master`: restarted Lab against the patched binary.

## Errors Encountered

- `OAuth token refresh failed: invalid_grant: resource does not match the refresh token`: Lab defaulted omitted refresh `resource` to canonical Lab MCP resource; fixed in `lab-auth`.
- `Deserialize error: JsonRpcMessage`: earlier route proxy returned non-JSON-RPC auth errors to Codex when upstream auth was miswired.
- `redirect_uri_mismatch`: Axon container used Lab Google OAuth credentials with Axon public URL; corrected `/home/jmagar/.axon/.env`.
- Release rebuild via `just install` failed with `rustc-LLVM ERROR: out of memory`; debug build was used for live hot-swap verification.
- A full Axon compose up attempted to recreate dependency containers and hit existing-name conflicts; resolved by recreating only `axon` with `--no-deps --force-recreate`.

## Behavior Changes

| Before | After |
| --- | --- |
| Codex login could mint access tokens with the wrong audience when `oauth_resource` was absent. | Codex config explicitly requests route resources for `axon` and `syslog`. |
| Lab protected route using raw backend proxy could not apply upstream OAuth. | Local route config publishes named OAuth upstreams at `/axon` and `/syslog`. |
| Axon OAuth redirected Google with a client/redirect mismatch. | Axon container uses Axon Google client credentials with `https://axon.example.com/auth/google/callback`. |
| Codex refresh without `resource` failed for route-scoped tokens. | Lab refresh uses the stored refresh-token resource when `resource` is omitted. |

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `cargo test -p lab-auth token_endpoint_refresh_grant --all-features` | focused refresh-token tests pass | 2 passed, 0 failed | pass |
| Manual POST `/token` refresh for cached `axon` credential without `resource` | 200 and route-scoped token | `aud=https://mcp.example.com/axon`, `scope=mcp:read mcp:write` | pass |
| Manual POST `/token` refresh for cached `syslog` credential without `resource` | 200 and route-scoped token | `aud=https://mcp.example.com/syslog`, `scope=mcp:read mcp:write` | pass |
| Refreshed `axon` token initialize to `https://mcp.example.com/axon` | JSON-RPC initialize succeeds | HTTP 200 `text/event-stream`, initialized Axon MCP | pass |
| Refreshed `syslog` token initialize to `https://mcp.example.com/syslog` | JSON-RPC initialize succeeds | HTTP 200 `application/json`, initialized syslog-mcp | pass |
| `codex exec --skip-git-repo-check ...` | Codex starts without axon/syslog startup failures | completed; no axon/syslog startup failures | pass |

## Risks and Rollback

- Risk: the live Lab container currently runs a debug build, not a release build. Roll back by reinstalling the prior release binary or rerunning the normal release build when memory permits.
- Risk: local ignored config and host env changes are machine-local; another host will need the same route/upstream/auth configuration.
- Rollback for code change: revert the `refresh_token_grant` change and test in `crates/lab-auth/src/token.rs`, then rebuild and restart Lab.
- Rollback for Codex config: remove `oauth_resource` from `/home/jmagar/.codex/config.toml`, though that would reintroduce route-audience ambiguity.

## Decisions Not Taken

- Did not restore static backend bearer-token proxying for `axon`/`syslog`; that bypasses upstream OAuth and is not the intended model.
- Did not delete historical refresh-token rows from Lab's auth DB; the server-side refresh behavior now handles existing route-scoped refresh tokens.
- Did not pursue a release build after the LLVM OOM; debug hot-swap was sufficient for live verification.

## Open Questions

- Codex still logged an unrelated `relative URL without a base` MCP worker error during the startup probe; the failing MCP server was not identified in this session.
- Codex also logged an unrelated Lab session-delete failure after the probe; axon/syslog startup was unaffected.
- Confirm whether the Axon Google OAuth client in Google Cloud permanently includes `https://axon.example.com/auth/google/callback`.

## Next Steps

- Re-run a release build/install for Lab when host memory pressure is lower.
- Investigate the unrelated Codex `relative URL without a base` MCP startup log if it remains noisy.
- Decide whether to commit this session note or keep it local under ignored `docs/sessions/`.
