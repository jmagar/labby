---
date: 2026-05-25 22:20:53 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: df097f26
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
beads: none
---

# Lab MCP OAuth callback debug

## User Request

Investigate why OAuth callbacks for logging into the Lab HTTP MCP server were failing, including the `callback.tootie.tv` SWAG routing and the Codex `labby` MCP startup error.

## Session Overview

We traced the callback failure across Cloudflare, SWAG, the `callback-relay` container on `squirts`, the relay registry, Dookie's Tailscale address, and Codex configuration. The root cause was the Codex MCP server URL missing `/mcp`; Codex was handshaking against `https://lab.tootie.tv` instead of `https://lab.tootie.tv/mcp`, which also prevented the OAuth callback flow from reaching the expected live listener state.

## Sequence of Events

1. Confirmed the browser failure was a Cloudflare 502 on `https://callback.tootie.tv/callback/dookie?...`.
2. Verified Lab's public health endpoint and OAuth protected-resource metadata were reachable.
3. Inspected Codex OAuth callback settings and found Dookie configured for `mcp_oauth_callback_port = 38935` and `mcp_oauth_callback_url = "https://callback.tootie.tv/callback/dookie"`.
4. Queried SWAG via Lab gateway tools and found `callback.subdomain.conf` forwarding to Docker DNS name `callback-relay` on port `39001`.
5. SSHed to `squirts` and inspected `/mnt/compose/mcp-oauth-gateway`, confirming `callback-relay` runs there and persists its registry under `.cache/callback-relay/registry.json`.
6. Confirmed the relay registry maps `dookie` to `http://100.88.16.79:38935/callback/dookie`.
7. Proved Squirts can reach Dookie over Tailscale when a controlled listener binds `0.0.0.0:38935`.
8. Investigated the separate Codex startup error and found `[mcp_servers.labby] url = "https://lab.tootie.tv"` was missing `/mcp`.
9. Updated Dookie's host-level `~/.codex/config.toml` to `https://lab.tootie.tv/mcp`, after which the user confirmed it fixed OAuth too.

## Key Findings

- SWAG does not hardcode a public or Tailscale IP for `callback.tootie.tv`; it proxies to `callback-relay:39001` over Docker DNS.
- On `squirts`, Docker resolved `callback-relay` to `10.6.0.12` on `jakenet`; SWAG was `10.6.0.100` on the same network.
- The relay registry entry for `dookie` points to `http://100.88.16.79:38935/callback/dookie`.
- Dookie's callback listener on `38935` is expected to be Codex's temporary OAuth listener during `codex mcp login`, not a persistent service deployed by the relay repo.
- The MCP startup error came from the wrong configured endpoint: Codex was using `https://lab.tootie.tv`, not `https://lab.tootie.tv/mcp`.

## Technical Decisions

- Used `superpowers:systematic-debugging` to avoid patching SWAG or the relay before proving which component failed.
- Treated the callback flow as component boundaries: Cloudflare, SWAG, relay container, relay registry, Dookie network reachability, Codex listener, and Codex MCP config.
- Left SWAG and relay configuration unchanged because live evidence showed they were routing correctly.
- Changed only the host Codex MCP URL because that was the observed startup failure and the user confirmed it fixed OAuth.

## Files Changed

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| created | `docs/sessions/2026-05-25-lab-mcp-oauth-callback-debug.md` | - | Save this session log | Current `save-to-md` request |
| modified | `/home/jmagar/.codex/config.toml` | - | Fix host Codex `labby` MCP URL to include `/mcp` | `sed -n '360,372p' ~/.codex/config.toml` showed `url = "https://lab.tootie.tv/mcp"` |

## Beads Activity

No bead activity observed during this session. `bd list --all --sort updated --reverse --limit 50 --json` and `.beads/interactions.jsonl` were read for maintenance context, but no bead was created, edited, claimed, assigned, commented on, or closed.

## Repository Maintenance

### Plans

Checked `docs/plans` and `docs/superpowers/plans`. No plan file was moved because this session was an operational debug/config session and none of the listed plan files were clearly completed by this work.

### Beads

Read recent bead state and interactions. No bead updates were made because the session resolved a host configuration issue, not a tracked repo implementation task.

### Worktrees and branches

Checked `git worktree list --porcelain`, local branches, and remote branches. Only the main worktree was registered, `main` tracked `origin/main`, and no branch/worktree cleanup was safe or relevant.

### Stale docs

Reviewed the remote relay documentation at `/mnt/compose/mcp-oauth-gateway/docs/architecture/callback-relay.md`; it already described the temporary Codex listener requirement and the Dookie Tailscale target pattern, so no doc update was made.

### Dirty state

Pre-existing dirty/untracked repo files were observed and left untouched:

- `docs/superpowers/plans/2026-05-25-extract-gateway-server.md`
- `docs/crate-extract/`
- `docs/sessions/2026-05-25-code-mode-merge-cleanup.md`
- `docs/sessions/2026-05-25-lab-rmcp-extraction-plans.md`
- `docs/superpowers/plans/2026-05-25-gateway-fresh-clone-prune-list.md`

## Tools and Skills Used

- **Skills.** `superpowers:systematic-debugging` for root-cause investigation; `save-to-md` for this session artifact.
- **Shell and SSH.** Used local shell plus SSH to `squirts` and `dookie` to inspect compose files, Docker containers, network listeners, Tailscale IPs, and Codex config.
- **Lab MCP tools.** Used `labby` gateway/scout/invoke tools to find and query SWAG configuration and health surfaces.
- **HTTP tools.** Used `curl` for public and internal endpoint checks.
- **Docker tools.** Used `docker ps`, `docker inspect`, `docker exec`, and `docker logs` on `squirts` to inspect SWAG, `callback-relay`, and `mcp-oauth` state.
- **File tools.** Read and wrote host config plus this session artifact. No repo source files were modified.

## Commands Executed

| command | result |
|---|---|
| `curl https://lab.tootie.tv/health` | Returned `{"status":"ok","mode":"master",...}` |
| `curl https://mcp.tootie.tv/.well-known/oauth-protected-resource` | Returned OAuth protected-resource metadata for `https://lab.tootie.tv/mcp` |
| `curl https://callback.tootie.tv/` | Returned relay 404 JSON, proving generic traffic reached the relay |
| `curl https://callback.tootie.tv/callback/dookie?code=test&state=test` | Returned 502 while the relay could not reach the registered machine target |
| `ssh squirts 'cd /mnt/compose/mcp-oauth-gateway && sed -n "1,220p" docker-compose.yml'` | Confirmed compose includes `auth/docker-compose.yml` and shared network configuration |
| `ssh squirts 'cat /mnt/compose/mcp-oauth-gateway/.cache/callback-relay/registry.json'` | Confirmed Dookie target URL was `http://100.88.16.79:38935/callback/dookie` |
| `ssh squirts 'docker ps --format ... | grep -Ei "callback|oauth|swag"'` | Confirmed `swag`, `callback-relay`, `mcp-oauth`, and `mcp-oauth-redis` were running on `jakenet` |
| `ssh squirts 'docker exec swag getent hosts callback-relay'` | Resolved `callback-relay` to `10.6.0.12` |
| `ssh squirts 'docker exec swag curl http://callback-relay:39001/healthz'` | Returned `{"status":"ok"}` |
| `ssh dookie 'python3 -m http.server equivalent on 0.0.0.0:38935'` plus `curl http://100.88.16.79:38935/callback/dookie?probe=1` from Squirts | Proved Squirts can reach Dookie on `38935` when a listener exists |
| `rg -n "[mcp_servers.labby]|labby|mcp.tootie|lab.tootie" ~/.codex/config.toml` | Found `url = "https://lab.tootie.tv"` missing `/mcp` |
| `curl -D - https://lab.tootie.tv/mcp` | Returned HTTP 401 with `content-type: application/json` and MCP OAuth challenge headers |

## Errors Encountered

- **Cloudflare 502 on callback URL.** Root cause was not SWAG DNS; relay forwarding failed because the live Codex flow had not reached a usable callback listener state.
- **`labby` MCP startup failed with `Unexpected content type: Some("missing-content-type; body: ")`.** Root cause was Codex using the base site URL instead of the Streamable HTTP MCP endpoint.
- **Local `/mnt/compose/mcp-oauth-gateway` path missing.** The compose stack lives on `squirts`, so inspection moved to SSH.
- **Arcane container search returned unrelated containers.** The search/filter behavior was not reliable for exact callback lookup, so Docker inspection over SSH was used instead.
- **Some shell quoting attempts failed during remote inspection.** Retried with simpler commands and direct Docker templates.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Codex `labby` MCP URL | `https://lab.tootie.tv` | `https://lab.tootie.tv/mcp` |
| Codex startup | MCP initialize failed before usable login state | User confirmed the change fixed OAuth |
| Callback relay | Returned 502 because the forwarded Dookie target was not reachable during the broken startup flow | Relay configuration left unchanged; callback path can work once Codex starts the proper MCP OAuth flow |

## Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| `docker exec swag getent hosts callback-relay` on `squirts` | SWAG resolves relay by Docker DNS | `10.6.0.12 callback-relay` | pass |
| `docker exec swag curl http://callback-relay:39001/healthz` on `squirts` | Relay health is OK | HTTP 200 `{"status":"ok"}` | pass |
| Controlled listener on Dookie plus curl from Squirts to `100.88.16.79:38935/callback/dookie?probe=1` | Squirts can reach Dookie over Tailscale when a listener exists | HTTP 200 `ok /callback/dookie?probe=1` | pass |
| `sed -n '360,372p' ~/.codex/config.toml` | `labby` URL includes `/mcp` | `url = "https://lab.tootie.tv/mcp"` | pass |
| User retest | OAuth works | User stated: "that fixed the oauth problem as well" | pass |

## Risks and Rollback

- Risk is limited to host-level Codex config for `labby`. Rollback is to change `/home/jmagar/.codex/config.toml` back to the previous URL, though that would restore the broken MCP startup behavior.
- The generated session note is committed separately from pre-existing dirty repo files.

## Decisions Not Taken

- Did not modify SWAG because live config and health checks showed SWAG was reaching `callback-relay`.
- Did not modify `callback-relay` registry because Dookie's target URL matched the documented design.
- Did not add a persistent listener on Dookie because the documented design expects Codex to own the callback listener during login.
- Did not modify repo source because the issue was host configuration, not Lab code.

## References

- `/mnt/compose/mcp-oauth-gateway/docs/architecture/callback-relay.md`
- `/mnt/compose/mcp-oauth-gateway/auth/docker-compose.yml`
- `/mnt/compose/mcp-oauth-gateway/.cache/callback-relay/registry.json`
- `/home/jmagar/.codex/config.toml`

## Open Questions

- Whether the relay onboarding helper should validate that registered targets are reachable only during an active login, or expose a clearer diagnostic message for no listener on `38935`.

## Next Steps

- Restart or relaunch Codex so the corrected `labby` MCP URL is loaded.
- If the callback flow regresses, first verify Codex has created a temporary listener on Dookie with `ss -ltnp | grep 38935` while `codex mcp login labby` is waiting.
- If the listener exists but relay still fails, test from Squirts with `curl http://100.88.16.79:38935/callback/dookie?...` before changing SWAG or relay config.
