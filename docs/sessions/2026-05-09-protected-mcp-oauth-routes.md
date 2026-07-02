---
date: 2026-05-09 01:51:41 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: fd989a12
agent: Codex
session id: c7f3c5ad-9a4d-489b-8768-ed4d125abf5a
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/c7f3c5ad-9a4d-489b-8768-ed4d125abf5a.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab fd989a12 [main]
---

# Protected MCP OAuth Routes Session

## User Request

Implement and debug Lab's inline protected MCP route flow so OAuth-enabled upstream MCP servers such as syslog and Axon can be added in the Gateway UI, connected with upstream OAuth, and published through Lab OAuth at paths such as `https://mcp.example.com/syslog` and `https://mcp.example.com/axon`.

## Session Overview

- Added support for protected MCP routes that target a configured upstream by name instead of duplicating backend URL/path fields.
- Integrated protected route creation into the custom HTTP gateway flow in the Gateway UI.
- Updated Lab OAuth and upstream OAuth behavior so gateway-owned upstream OAuth discovery uses the shared `gateway` subject.
- Rebuilt and hot-swapped the running `labby` container, then verified live public protected routes through `mcp.example.com`.
- Captured durable Lavra knowledge entries for the root cause, debugging path, and operational verification pattern.

## Sequence of Events

- Audited the existing SWAG/nginx OAuth-proxy pattern and Lab gateway implementation.
- Implemented protected MCP route configuration, API, dispatch, CLI, UI, and docs for named upstream routes.
- Configured live `syslog` and `axon-controller-tv` upstreams and protected routes in `/home/jmagar/.labby/config.toml`.
- Fixed upstream OAuth callback allowlists for syslog and Axon so Lab can complete dynamic client registration with `https://lab.example.com/auth/upstream/callback`.
- Debugged the "No capabilities were discovered" UI failure after direct protected-route proxying already worked.
- Patched startup, reload, and test discovery paths to use subject-scoped gateway discovery for OAuth upstreams.
- Ran focused Rust/TypeScript tests, rebuilt the Lab binary, restarted the `labby` container, and verified live routes.

## Key Findings

- `UpstreamPool::discover_all` intentionally skips OAuth upstreams without a subject to avoid globally caching a user-specific OAuth view: `crates/lab/src/dispatch/upstream/pool.rs`.
- `GatewayManager::test` and `GatewayManager::reload` needed to use `discover_all_for_subject_with_in_process_peers(..., SHARED_GATEWAY_OAUTH_SUBJECT, ...)`: `crates/lab/src/dispatch/gateway/manager.rs`.
- `labby serve` has its own initial gateway discovery path before the manager is installed, so fixing reload/test alone did not fix the running web/API server: `crates/lab/src/cli/serve.rs`.
- Protected MCP route proxying was already succeeding while gateway catalog discovery still failed, so the proxy path and discovery path had to be verified separately.
- Static bearer tokens authenticate Lab admin API calls, but public protected MCP route validation expects Lab OAuth JWTs for the protected resource audience.

## Technical Decisions

- Preserved subject-less discovery skipping OAuth upstreams for safety.
- Added an explicit subject-scoped discovery path for deliberate gateway-owned OAuth discovery using `SHARED_GATEWAY_OAUTH_SUBJECT`.
- Allowed protected routes to specify `upstream = "<gateway-name>"` with an empty `backend_url`; the backend target is resolved from the upstream config.
- Kept upstream OAuth authorization separate from public Lab OAuth authorization: Lab authenticates the public client, then uses its gateway-owned upstream token to reach the private upstream MCP server.

## Files Modified

- `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx` - added optional protected route path support to custom HTTP gateway creation.
- `apps/gateway-admin/components/gateway/protected-mcp-routes-panel.tsx` - updated protected route UI to support named upstream targets.
- `apps/gateway-admin/lib/types/gateway.ts` - added route typing for optional `upstream` and optional backend URL.
- `apps/gateway-admin/lib/api/gateway-client.test.ts` and `apps/gateway-admin/lib/hooks/use-gateways.ts` - updated gateway client/test behavior around protected route actions.
- `crates/lab/src/config.rs` - allowed protected route configs with named upstreams and empty backend URLs.
- `crates/lab/src/dispatch/gateway/config.rs` - validated upstream-vs-backend route target rules.
- `crates/lab/src/api/router.rs` - resolved protected route upstream targets and injected upstream auth.
- `crates/lab/src/dispatch/upstream/pool.rs` - added subject-scoped gateway discovery APIs while preserving subject-less OAuth skip behavior.
- `crates/lab/src/dispatch/gateway/manager.rs` - used gateway subject discovery for test/reload and added live protected route index updates.
- `crates/lab/src/cli/serve.rs` - used gateway subject discovery during live `labby serve` startup.
- `docs/runtime/CONFIG.md` and `docs/services/GATEWAY.md` - documented protected routes and upstream-backed route config.
- `.lavra/memory/knowledge.jsonl` - appended seven knowledge entries for this OAuth gateway debugging session.

## Commands Executed

- `cargo check -p labby --all-features` - passed after discovery changes.
- `cargo fmt --all --check` - passed after formatting.
- `cargo test -p labby shared_discovery_skips_oauth_http_upstreams --all-features` - passed; confirms subject-less discovery still skips OAuth upstreams.
- `cargo test -p labby protected_route_named_upstream_allows_empty_backend_url --all-features` - passed; confirms named-upstream protected route config normalization.
- `pnpm --dir apps/gateway-admin test ...` - previously passed for gateway-admin tests during UI work.
- `just dev-debug` - rebuilt the Lab binary and restarted the running `labby` container.
- `docker logs labby` - confirmed live startup discovery success for `syslog` and `axon-controller-tv`.
- `curl` calls to `/v1/gateway` with bearer auth - confirmed `gateway.test` and `gateway.list` runtime views.
- `curl` calls to `https://mcp.example.com/syslog` and `https://mcp.example.com/axon` with Lab JWTs - confirmed public protected routes initialize successfully.

## Errors Encountered

- `unknown action: gateway.protected_route.list`: the UI called protected-route actions before the backend dispatch surface supported them. The backend actions were added.
- `scope must be lab`: protected resource-specific OAuth scopes were not yet accepted for protected MCP resources. Lab auth was updated to support resource-scoped protected routes.
- Upstream OAuth dynamic registration returned redirect URI validation failures for Lab's callback. Syslog and Axon needed `https://lab.example.com/auth/upstream/callback` in their upstream OAuth allowlists.
- Axon Google login returned `redirect_uri_mismatch` until its Google OAuth client/env config was aligned with `https://axon.example.com/auth/google/callback`.
- Gateway UI still reported no capabilities after OAuth completed. Root cause: startup/reload/test discovery used subject-less OAuth upstream discovery, which intentionally skips OAuth upstreams.
- Initial direct public route probe with the static bearer token returned `401 invalid bearer token`; protected MCP resources require Lab OAuth JWTs with the correct resource audience.

## Behavior Changes

| Before | After |
| --- | --- |
| Protected routes required explicit backend URL/path fields. | Protected routes can point at named upstreams and reuse upstream URL/auth config. |
| OAuth upstreams were skipped during gateway catalog discovery. | Gateway-owned OAuth upstreams are discovered with the shared `gateway` subject. |
| Gateway UI/test showed no capabilities for OAuth upstreams behind protected routes. | `syslog` and `axon-controller-tv` show connected state and discovered tools/resources. |
| Public route proxying and UI catalog state could disagree. | Startup, reload, test, and proxy paths now share the gateway-owned upstream OAuth model. |

## Verification Evidence

| command | expected | actual | status |
| --- | --- | --- | --- |
| `cargo check -p labby --all-features` | Lab crate compiles with all features | Finished successfully | pass |
| `cargo fmt --all --check` | No formatting drift | Finished successfully | pass |
| `cargo test -p labby shared_discovery_skips_oauth_http_upstreams --all-features` | Subject-less discovery still skips OAuth upstreams | 1 targeted test passed in lib and main harnesses | pass |
| `cargo test -p labby protected_route_named_upstream_allows_empty_backend_url --all-features` | Named-upstream route config normalizes | 1 targeted test passed in lib and main harnesses | pass |
| `just dev-debug` | Rebuild and restart `labby` container | Container restarted | pass |
| `/v1/gateway gateway.test syslog` | Tools/resources discovered | `tool_count=1`, `resource_count=1`, `last_error=null` | pass |
| `/v1/gateway gateway.test axon-controller-tv` | Tools/resources discovered | `tool_count=1`, `resource_count=2`, `last_error=null` | pass |
| `https://mcp.example.com/syslog initialize` | Public protected route returns upstream initialize result | HTTP 200, `serverInfo.name=syslog-mcp` | pass |
| `https://mcp.example.com/axon initialize` | Public protected route returns upstream initialize result | HTTP 200 SSE, Axon/RMCP initialize payload | pass |

## Risks and Rollback

- Risk: gateway-owned discovery uses a shared subject and should remain limited to explicit gateway-owned contexts, not generalized per-user discovery.
- Risk: live route config currently depends on upstream OAuth tokens stored for the shared gateway subject; clearing those tokens requires reconnecting upstream OAuth in the UI.
- Rollback: revert the protected route/upstream discovery changes and remove the `upstream = ...` route entries from config, or point protected routes back to explicit backend URLs.

## Decisions Not Taken

- Did not disable auth on upstream MCP servers; the goal was to support OAuth upstreams that users do not control.
- Did not hardcode Axon or syslog behavior; the route proxy resolves named upstream config generically.
- Did not rely on SWAG/nginx snippets for per-service OAuth protection; the improved method keeps route registration and metadata in Lab.

## References

- `docs/services/GATEWAY.md`
- `docs/runtime/CONFIG.md`
- `docs/dev/OBSERVABILITY.md`
- `crates/lab/src/dispatch/upstream/pool.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/cli/serve.rs`
- Bead: `lab-mgw9`

## Open Questions

- Whether the stale `axon` protected route pointing `/lab` at `https://lab.example.com/mcp` should be removed from live config.
- Whether a full end-to-end Claude Code reconnect succeeds without restarting Claude Code after the previous failed credential reconnect state.

## Next Steps

- Started but not completed: sweep all docs for stale gateway/OAuth/protected-route content and update them to match the implementation.
- Follow-on: run a broader all-features test/build pass before commit if time allows.
- Follow-on: use browser automation to confirm the Gateway UI displays the now-discovered capabilities for `syslog` and `axon-controller-tv`.
