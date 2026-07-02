---
date: 2026-04-25 19:59:05 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: 3e8db769
agent: Claude (claude-sonnet-4-6)
session id: 82dc0101-18dd-4bd3-ab4a-2dbf1e0e169f
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/82dc0101-18dd-4bd3-ab4a-2dbf1e0e169f.jsonl
working directory: /home/jmagar/workspace/lab
pr: "29 — fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

## User Request

Create scripts that test whether OAuth is properly configured — covering MCP endpoints, env/config variables, secured webapp endpoints, and required public OAuth endpoints.

## Session Overview

Explored the auth middleware stack in the lab HTTP router, clarified the OAuth vs. bearer token model, wrote `scripts/check-oauth.sh` to verify live endpoint security, ran it against `https://lab.example.com`, diagnosed and fixed a false-positive failure in the upstream OAuth callback check, and confirmed the server passes all 31 checks with one expected warning.

## Sequence of Events

1. Read `crates/lab/src/api/router.rs` and `crates/lab/src/api/oauth.rs` to map the full auth middleware stack.
2. Read `crates/lab/src/cli/serve.rs` to understand config loading (`LAB_MCP_HTTP_TOKEN`, `LAB_PUBLIC_URL`, `LAB_WEB_ASSETS_DIR`, etc.).
3. Read `.env.example` to enumerate all relevant env vars.
4. Clarified the security model: API and MCP both accept OAuth JWTs as Bearer tokens; MCP additionally rejects session cookies; static bearer token and OAuth are simultaneously active.
5. Wrote `scripts/check-oauth.sh` — 10 test sections covering config, health probes, protected endpoint gating, static bearer, MCP bearer-only, OAuth discovery metadata, WWW-Authenticate header, dev marketplace, node self-registration, and upstream OAuth callback.
6. Ran the script against `https://lab.example.com` — got 30 pass / 1 fail / 1 warn.
7. Diagnosed the failure: test sent `?state=csrf&code=authcode` to `/auth/upstream/callback`, which triggered the real callback handler that looked up the fake state token in SQLite and correctly returned `kind:auth_failed` — not an auth gate failure.
8. Fixed the probe to send no query params (expecting 400/422 from missing required params) rather than forged OAuth state.
9. Re-ran: 31 pass / 0 fail / 1 warn. All checks clean.
10. User confirmed OAuth + static bearer are already enabled together; clarified the static token grants unconditional `lab:read + lab:admin` scopes.

## Key Findings

- `authenticate_request` in `router.rs:172` tries three credential paths in order: static bearer token → OAuth JWT → browser session cookie (v1 only).
- MCP is mounted with `allow_session_cookie = false` (`router.rs:576`), so session cookies are rejected there by design.
- `browser_routes` (containing `/auth/upstream/callback`) is merged on the outer router outside the auth middleware (`router.rs:623`), so it IS public — but the callback handler itself returns `kind:auth_failed` when the OAuth state token isn't found in SQLite (`upstream_oauth.rs:447`).
- `LAB_WEB_ASSETS_DIR` is set to `/home/jmagar/workspace/lab/apps/gateway-admin/out` in `~/.labby/.env`, meaning the SPA fallback is active on the live server.
- Static bearer token (`LAB_MCP_HTTP_TOKEN`) grants hardcoded `lab:read + lab:admin` scopes unconditionally (`router.rs:195`); OAuth JWTs carry scopes from token claims.
- The MCP GET → 400 warning is expected: MCP uses POST + SSE framing, not GET.

## Technical Decisions

- **Probe `/auth/upstream/callback` with no params, not with fake OAuth state**: sending forged `state=` triggers real SQLite lookup and returns `auth_failed`, which is indistinguishable from an auth gate rejection. Missing required params produces 400/422, which unambiguously confirms the route is mounted and public.
- **Load `~/.labby/.env` inside the script**: allows the script to run without manually exporting vars, matching how the server itself loads config.
- **Shell script over Rust test**: operator-runnable against a live server without building; no compile step, works with just `curl`.
- **Color output gated on `[ -t 1 ]`**: safe for CI pipelines and terminal alike.

## Files Modified

| File | Change |
|------|--------|
| `scripts/check-oauth.sh` | Created — 10-section OAuth/auth verification script |

## Commands Executed

```bash
# Syntax check
bash -n scripts/check-oauth.sh  # → syntax ok

# First run against live server
LAB_BASE_URL=https://lab.example.com bash scripts/check-oauth.sh
# → 30 pass / 1 fail / 1 warn

# Second run after fixing upstream callback probe
LAB_BASE_URL=https://lab.example.com bash scripts/check-oauth.sh
# → 31 pass / 0 fail / 1 warn
```

## Errors Encountered

**False-positive failure on `/auth/upstream/callback`**: the test sent `?state=csrf&code=authcode` expecting any non-401 response to mean "route is public". But the handler validates the state token against SQLite and returns `kind:auth_failed` (401) when it's not found — correct security behavior, not a missing auth bypass. Fixed by probing with no query params instead.

## Behavior Changes (Before/After)

| Surface | Before | After |
|---------|--------|-------|
| `scripts/check-oauth.sh` | Did not exist | 10-section live verification script; exits 0 on pass, 1 on failure |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `GET /health` | 200, no auth | 200 | ✓ |
| `GET /ready` | 200, no auth | 200 | ✓ |
| `GET /v1/extract/actions` (no auth) | 401 `kind:auth_failed` | 401 `kind:auth_failed` | ✓ |
| `GET /mcp` (no auth) | 401 `kind:auth_failed` | 401 `kind:auth_failed` | ✓ |
| `GET /v1/openapi.json` (no auth) | 401 | 401 | ✓ |
| `GET /v0.1/servers` (no auth) | 401 | 401 | ✓ |
| `GET /v1/extract/actions` (Bearer) | 200 | 200 | ✓ |
| `GET /v1/extract/actions` (wrong Bearer) | 401 | 401 | ✓ |
| `GET /mcp` (fake session cookie) | 401 | 401 | ✓ |
| `GET /mcp` (Bearer) | 200/405 | 400 (warn — GET not valid for SSE endpoint) | ⚠ |
| `GET /.well-known/oauth-authorization-server` | 200, issuer matches `LAB_PUBLIC_URL` | 200, issuer=`https://lab.example.com` | ✓ |
| `GET /.well-known/oauth-protected-resource` | 200 | 200 | ✓ |
| `GET /jwks` | 200, keys array | 200, keys present | ✓ |
| `POST /dev/api/marketplace` (read action, no auth) | not 401 | 200 | ✓ |
| `POST /dev/api/marketplace` (mutating, no auth) | 403 `kind:dev_preview_read_only` | 403 | ✓ |
| `GET /auth/upstream/callback` (no params) | 400/422 | 400 | ✓ |
| Overall | 31 pass, 0 fail | 31 pass, 0 fail, 1 warn | ✓ |

## Open Questions

- The MCP endpoint returns 400 on GET — worth confirming whether the reverse proxy (Caddy/Traefik in front of `lab.example.com`) is stripping the `Upgrade` or SSE headers that a real MCP client would send.

## Next Steps

- **Not yet started**: add the script to CI or a `just check-oauth` recipe so it runs automatically against a staging deploy.
- **Not yet started**: consider adding scope-based authorization checks — currently static bearer grants unconditional `lab:admin`; OAuth JWTs could be issued with narrower scopes for read-only clients.
