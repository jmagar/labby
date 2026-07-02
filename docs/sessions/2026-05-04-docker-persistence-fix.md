---
date: 2026-05-04 07:58:04 EST
repo: git@github.com:jmagar/lab.git
branch: bd-work/mcp-gateway-review-remediation
head: 60939ce2
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 6114b37e-4f0b-4f91-81de-ad33c5cdbef7
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/6114b37e-4f0b-4f91-81de-ad33c5cdbef7.jsonl
working directory: /home/jmagar/workspace/lab
pr: "40 — Integrate service wave and CI updates — https://github.com/jmagar/lab/pull/40"
---

## User Request

OAuth tokens weren't persisting across container restarts, and after the initial fix was identified, the user asked for a full audit of everything that should be persisted in the Docker container.

## Session Overview

Identified and fixed a fundamental Docker persistence misconfiguration: the named volume covered `~/.local` but virtually all runtime state defaults to `~/.labby`. Replaced piecemeal per-path env var workarounds with a single bind-mount of the host's `~/.labby` directory. Also generated and wrote `LAB_ACP_HMAC_SECRET` to `~/.labby/.env` to prevent ACP permission signature invalidation on restart. Resolved a stale incremental build artifact error that blocked `just dev-debug` after the config changes.

## Sequence of Events

1. User reported re-authentication required on every container restart
2. Inspected `config/Dockerfile`, `.env.example`, `docker-compose.yml` to understand mount layout
3. Identified the initial problem: auth DB (`auth.db`) and JWT key (`auth-jwt.pem`) defaulted to `~/.labby/` but the named volume only covered `~/.local/`
4. Added `LAB_AUTH_SQLITE_PATH` and `LAB_AUTH_KEY_PATH` env vars redirecting those into the named volume (later superseded)
5. User asked to audit all persistent state, not just auth
6. Spawned an Explore subagent to inventory every runtime write path across the codebase
7. Discovered that `acp.db`, `registry.db`, `stash/`, `node-enrollments.json`, `node-token` also defaulted to `~/.labby/` and were all ephemeral
8. Identified that `LAB_ACP_HMAC_SECRET` was unset, causing HMAC key rotation on every restart
9. Replaced the partial fix with a full bind-mount of `${HOME}/.labby:/home/labby/.labby`, removed the per-path env var workarounds
10. Generated `LAB_ACP_HMAC_SECRET` via `openssl rand -hex 32` and wrote it to `~/.labby/.env`
11. User ran `just dev-debug`; build failed with mass `clang: error: no such file or directory` for `.rlib` files
12. Diagnosed as stale incremental build artifacts; ran `cargo clean -p labby`; rebuild succeeded in 2m 16s

## Key Findings

- `crates/lab-auth/src/config.rs:221` — `default_auth_dir()` returns `$HOME/.labby/`, not `$HOME/.local/share/labby/`
- `crates/lab/src/dispatch/acp/persistence.rs:1088-1096` — `resolve_db_path()` defaults to `$HOME/.labby/acp.db`; env override is `LAB_ACP_DB`
- `crates/lab/src/config.rs:1021-1026` — `registry_db_path()` is hard-coded to `$HOME/.labby/registry.db` with no env override
- `crates/lab/src/dispatch/acp/persistence.rs:35-40` — `LAB_ACP_HMAC_SECRET` comment explicitly warns: ephemeral fallback rotates on restart
- `crates/lab/src/dispatch/acp/dispatch.rs:418` — SSE ticket HMAC also falls back to process-ephemeral key
- `docker-compose.yml:29` — previous `${HOME}/.labby/acp:/home/labby/.labby/acp` mount only covered the `acp/` subdirectory, not `acp.db` one level up

## Technical Decisions

**Bind-mount `~/.labby` rather than redirecting per-path via env vars**
The alternative was adding `LAB_ACP_DB`, `LAB_REGISTRY_DB` env vars (registry.db has no override at all, requiring a code change). The bind-mount approach requires zero code changes, covers every current and future path that defaults to `~/.labby/`, and matches how local `labby serve` already uses the host's `~/.labby`. File-level mounts (`.env` `:ro`, `acp-providers.json` Docker-specific `:ro`) override the parent directory mount for those specific paths, preserving the existing access controls.

**Keep the named volume for logs only**
The log DB is ephemeral (TTL-pruned, 7-day retention) and redirected to `~/.local/share/labby/logs.db` via `LAB_LOCAL_LOGS_STORE_PATH`. Keeping it off the host's `~/.labby` avoids filling host storage with churn data.

**Remove the `acp` subdirectory bind mount**
`${HOME}/.labby/acp:/home/labby/.labby/acp` was redundant once the parent directory bind mount was added. Docker resolves this cleanly — the parent directory mount covers all paths including `acp/`.

## Files Modified

| File | Change |
|------|--------|
| `docker-compose.yml` | Added `${HOME}/.labby:/home/labby/.labby` bind mount; removed `${HOME}/.labby/acp:/home/labby/.labby/acp`; removed `LAB_AUTH_SQLITE_PATH` and `LAB_AUTH_KEY_PATH` env vars; updated comments |
| `.env.example` | Added `LAB_ACP_HMAC_SECRET=` with generation instructions |
| `~/.labby/.env` | Added `LAB_ACP_HMAC_SECRET=<generated>` (host file, not in-repo) |

## Commands Executed

```bash
# Generate HMAC secret
openssl rand -hex 32
# → 2e5bc11c3e0a9cf8c4082c50340eca66b20d19a37ffa600ad084d663badaf3e0

# Clear stale incremental artifacts after the build failed
cargo clean -p labby
# → Removed 18 files, 286.3MiB total

# Rebuild and restart container
just dev-debug
# → Finished `dev` profile in 2m 16s; container restarted
```

## Errors Encountered

**`clang: error: no such file or directory: '.../target/debug/deps/lib*.rlib'` (mass)**
Occurred when running `just dev-debug` after the docker-compose changes. Root cause: cargo's incremental fingerprints referenced `.rlib` artifacts that had been deleted (likely by a previous `cargo clean` or file rotation). Fix: `cargo clean -p labby` cleared stale metadata; full rebuild succeeded.

## Behavior Changes (Before/After)

| State Item | Before | After |
|-----------|--------|-------|
| `auth.db` + `auth-jwt.pem` | Ephemeral (lost on restart) | Persisted via `~/.labby` bind mount |
| `acp.db` (ACP sessions) | Ephemeral | Persisted |
| `registry.db` (installed MCP servers) | Ephemeral | Persisted |
| `stash/` (plugin artifacts) | Ephemeral | Persisted |
| `node-enrollments.json` | Ephemeral | Persisted |
| `node-token` | Ephemeral | Persisted |
| ACP HMAC key | Rotated every restart (ephemeral PID+timestamp hash) | Stable across restarts (`LAB_ACP_HMAC_SECRET`) |
| `logs.db` | Ephemeral (unchanged) | Ephemeral — redirected to named volume, TTL-pruned |

## Risks and Rollback

**Shared host/container state:** Both local `labby serve` and the Docker container now write to the same `~/.labby/` directory. Running both simultaneously could cause SQLite write contention (WAL mode mitigates this but does not eliminate it). Mitigation: don't run local and Docker `labby serve` concurrently.

**acp-providers.json override:** The Docker-specific `./config/acp-providers.docker.json` is mounted as a file-level override at `/home/labby/.labby/acp-providers.json`. If Docker file-level mount semantics change or a compose version doesn't honor the override, the container could see the host's `~/.labby/acp-providers.json` instead. This is standard Docker behavior and not a real risk, but worth noting.

**Rollback:** Revert `docker-compose.yml` to the previous volume layout, re-add `LAB_AUTH_SQLITE_PATH`/`LAB_AUTH_KEY_PATH`, and restore the `acp` subdirectory mount. Remove `LAB_ACP_HMAC_SECRET` from `~/.labby/.env` if desired (reverting to ephemeral key).

## Decisions Not Taken

**Per-path env var redirects into the named volume** — Would have required adding `LAB_REGISTRY_DB` support to `config.rs` (registry has no env override) and setting `[workspace].root` in `config.toml` for stash. More surgical but more invasive and incomplete without code changes.

**Adding `LAB_REGISTRY_DB` env var support to `config.rs`** — Not needed once the bind-mount approach was chosen. Would have been the right fix if keeping state isolated from the host.

## Open Questions

- If both local `labby serve` and the Docker container run simultaneously, will SQLite WAL mode handle concurrent writes cleanly or will there be locking issues on `acp.db`?
- Should `node-enrollments.json` and `node-token` be documented in `docs/runtime/NODES.md` as requiring persistence?

## Next Steps

- Verify on next container restart that auth session is preserved end-to-end (no re-auth prompt)
- Verify ACP sessions survive restart (no new permission prompts for previously-granted tools)
- Consider documenting the `LAB_ACP_HMAC_SECRET` requirement in `docs/runtime/OAUTH.md` or a new ops doc
