---
date: 2026-05-14 23:27:56 EST
repo: git@github.com:jmagar/lab.git
branch: main
head: 93392f8a
agent: Codex
session id: 96b07a25-9c53-449b-b4b4-a30205de9a10
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/96b07a25-9c53-449b-b4b4-a30205de9a10.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab  93392f8a [main]
---

# MCP Gateway Sidecar Debug Session

## User Request

Systematically debug why the upstream MCP servers `apprise-api`, `unrust`, `rustifi`, `rustify`, `rmcp-template`, and `rustscale` were not connecting through the Lab gateway, using the provided homelab service map and reverse-proxy details.

## Session Overview

- Traced Lab gateway config, SWAG upstream routing, direct sidecar ports, Docker state, and MCP discovery behavior.
- Restored connectivity for the six requested upstream sidecars by fixing missing sidecar environment settings and recreating containers.
- Found that `unrust` and `rustify` were building/running from stale package identities and corrected their Compose/server metadata to the current repository/image names.
- Verified the gateway now discovers the expected one-tool surfaces for the fixed Rust MCP sidecars, including `unrust` and `rustify`.
- Confirmed Gotify is healthy at both the MCP sidecar layer and the Gotify web app layer.

## Sequence of Events

1. Inspected `~/.labby/config.toml` and `~/.labby/.env` to confirm Lab gateway upstream definitions, public MCP URLs, and bearer token env wiring.
2. Checked SWAG reverse-proxy configuration for `unraid.example.com` and confirmed `/mcp` was routed to `100.64.0.10:40010`.
3. Tested direct ports and public `/mcp` endpoints; found `40010` and `40020` initially refused while `40030`, `40040`, `40050`, and `40060` were open but rejected reverse-proxy Host headers.
4. Added allowed-host configuration to the sidecar `.env` files and recreated the sidecar Docker Compose services.
5. Investigated the unexpected `unrust` tool list and found the running `unraid-mcp` image was the wrong legacy Python/FastMCP image, not the current Rust repo.
6. Corrected `unrust` image/package metadata, rebuilt the container, and verified it exposes only the current `unraid` tool.
7. Repeated the image identity audit for `rustify`, corrected stale `gotify-mcp` image/package metadata to `rustify`, rebuilt it, and verified gateway discovery.
8. Ran focused health checks for Gotify after the fixes.

## Key Findings

- Lab gateway upstreams were configured to public MCP URLs such as `https://unraid.example.com/mcp`, `https://gotify.example.com/mcp`, `https://unifi.example.com/mcp`, `https://ts.example.com/mcp`, `https://apprise.example.com/mcp`, and `https://rmcp.example.com/mcp`.
- SWAG `unraid.subdomain.conf` had the correct MCP upstream target: app `100.64.0.10`, port `40010`, protocol `http`.
- The current `/home/jmagar/workspace/unrust` repo has no Python implementation and defines a single MCP tool, `unraid`; the earlier four-tool result came from the wrong legacy image.
- The running `unrust` container had been using `ghcr.io/jmagar/unraid-mcp:latest`; it now uses `ghcr.io/jmagar/unrust:latest`.
- The running `rustify` container had been using `ghcr.io/jmagar/gotify-mcp:latest`; it now uses `ghcr.io/jmagar/rustify:latest`.
- Gotify itself is healthy: `http://100.64.0.20:8070/health` returned `{"health":"green","database":"green"}`.

## Technical Decisions

- Kept Lab gateway config pointed at the public reverse-proxy URLs because SWAG is the intended auth/routing boundary for these upstreams.
- Fixed sidecar Host allowlists in each sidecar repo `.env` instead of weakening reverse-proxy routing or gateway behavior.
- Treated image identity mismatches as deployment metadata bugs and fixed `docker-compose.yml` plus `server.json` in the owning sidecar repos.
- Reverted temporary Python healthcheck experiments after confirming the rebuilt Rust images include `curl` and can use the existing healthcheck shape.

## Files Modified

- `/home/jmagar/workspace/unrust/docker-compose.yml` - changed the published image from `ghcr.io/jmagar/unraid-mcp:${VERSION:-latest}` to `ghcr.io/jmagar/unrust:${VERSION:-latest}`.
- `/home/jmagar/workspace/unrust/server.json` - changed the OCI package identifier from `ghcr.io/jmagar/unraid-mcp:0.1.0` to `ghcr.io/jmagar/unrust:0.1.0`.
- `/home/jmagar/workspace/rustify/docker-compose.yml` - changed the published image from `ghcr.io/jmagar/gotify-mcp:${GOTIFY_MCP_VERSION:-latest}` to `ghcr.io/jmagar/rustify:${RUSTIFY_VERSION:-latest}`.
- `/home/jmagar/workspace/rustify/server.json` - changed repository URL and OCI package identifier from `jmagar/gotify-mcp` to `jmagar/rustify`.
- Ignored local `.env` files under `/home/jmagar/workspace/{unrust,rustify,rustifi,rustscale,apprise-mcp,rmcp-template}` - added MCP allowed-host entries and, for `unrust`, the missing `UNRAID_MCP_BEARER_TOKEN` alias and port value.
- `/home/jmagar/workspace/lab/docs/sessions/2026-05-14-mcp-gateway-sidecar-debug.md` - this session note.

The Lab worktree already contained unrelated dirty files before this note was written, including gateway UI/API files and rust-bin-tools hook files. Those were not part of this sidecar debugging pass.

## Commands Executed

- `rg` / `sed` against `~/.labby/config.toml`, `~/.labby/.env`, and SWAG proxy confs to inspect configured upstreams and tokens.
- `curl` against direct sidecar health endpoints and public `/mcp` endpoints to separate port/listener failures from reverse-proxy Host/auth failures.
- `docker ps`, `docker inspect`, and `docker logs` for `unraid-mcp`, `gotify-mcp`, and the other sidecar containers.
- `docker compose up -d --build` in `/home/jmagar/workspace/unrust` and `/home/jmagar/workspace/rustify` after image metadata fixes.
- `./target/debug/labby gateway reload --json`, `./target/debug/labby gateway test --name unrust --json`, and `./target/debug/labby gateway test --name rustify --json` to verify Lab gateway discovery.
- `curl http://100.64.0.10:40020/health` and `curl http://100.64.0.20:8070/health` to verify Gotify sidecar and app health.

## Errors Encountered

- Initial `unraid` and `gotify` public `/mcp` checks returned `502` because their sidecar listeners were unavailable.
- Initial direct `40010` and `40020` port checks refused connections because `unraid-mcp` was restarting and `gotify-mcp` was not running.
- Authenticated public `/mcp` checks for the other sidecars returned `403 Forbidden: Host header is not allowed`; adding `*_MCP_ALLOWED_HOSTS` entries resolved that class of failure.
- `unrust` showed four tools because the container was running the wrong legacy `unraid-mcp` image. Rebuilding from the current `unrust` repo corrected discovery to one `unraid` tool.
- `rustify` was similarly using stale `gotify-mcp` image/package metadata. Rebuilding with the `rustify` image corrected the deployment identity.

## Behavior Changes

| Area | Before | After |
| --- | --- | --- |
| `unrust` gateway discovery | Legacy image exposed unexpected tools including `diagnose_subscriptions` and `test_subscription_query`. | Current Rust image exposes one `unraid` tool plus one resource and one prompt. |
| `rustify` gateway discovery | Container identity pointed at `ghcr.io/jmagar/gotify-mcp`. | Container identity points at `ghcr.io/jmagar/rustify` and exposes one `gotify` tool. |
| Reverse-proxy MCP Host checks | Authenticated MCP calls reached sidecars but failed Host validation. | Sidecar allowed-host env entries include Tailscale host:port and public domains. |
| Gotify health | Initially `gotify-mcp` was created but not running. | `gotify-mcp` is running healthy on `40020`; Gotify app health is green on `8070`. |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `./target/debug/labby gateway test --name unrust --json` | `last_error: null`, one current tool | `tool_count: 1`, `resource_count: 1`, `prompt_count: 1`, `last_error: null` | Pass |
| `./target/debug/labby gateway test --name rustify --json` | `last_error: null`, one current tool | `tool_count: 1`, `resource_count: 1`, `prompt_count: 2`, `last_error: null` | Pass |
| `gateway.discovered_tools unrust` through Lab API | Only current `unraid` tool | Returned `unraid true` | Pass |
| `gateway.discovered_tools rustify` through Lab API | Only current `gotify` tool | Returned `gotify true` | Pass |
| `docker ps --filter name=gotify-mcp` | Healthy Rustify image on `40020` | `Up ... (healthy)`, `0.0.0.0:40020->40020/tcp`, `ghcr.io/jmagar/rustify:latest` | Pass |
| `docker inspect gotify-mcp` | No restart loop | `restart=0 health=healthy` | Pass |
| `curl http://100.64.0.10:40020/health` | MCP sidecar health 200 | `200`, `{"status":"ok"}` | Pass |
| `curl http://100.64.0.20:8070/health` | Gotify app health 200 | `200`, `{"health":"green","database":"green"}` | Pass |

## Risks and Rollback

- The `.env` edits are machine-local and ignored by git; they must be preserved on `node-a` for these sidecars to keep accepting reverse-proxy Host headers.
- `unrust` and `rustify` tracked image metadata changes should be committed in their respective repos if the new image names are the desired canonical deployment identities.
- Rollback for `unrust`: restore `ghcr.io/jmagar/unraid-mcp` in `docker-compose.yml` and `server.json`, then rebuild/recreate the container.
- Rollback for `rustify`: restore `ghcr.io/jmagar/gotify-mcp` and repository URL `https://github.com/jmagar/gotify-mcp`, then rebuild/recreate the container.

## Decisions Not Taken

- Did not change Lab gateway URLs from public domains to direct Tailscale IPs; the supplied infra map indicates SWAG is the intended gateway route.
- Did not change SWAG upstream routing after it was confirmed correct for `unraid.example.com` and the symptoms pointed at sidecar/runtime state.
- Did not commit or push sidecar repo changes during this session.

## Open Questions

- `rmcp-template` still appears to use the template image name `ghcr.io/your-org/example-mcp:latest`; it was connected after the allowlist fix, but its image identity was not audited in the same depth as `unrust` and `rustify`.
- `rustifi`, `rustscale`, and `apprise-mcp` were verified as connected, but their image labels and package metadata were not audited for stale repo names.
- The user-facing name `apprise-api` may refer to `apprise-mcp` in the provided service map; no separate `apprise-api` upstream was found in the documented port registry during this session.

## Next Steps

- Commit the `unrust` metadata corrections in `/home/jmagar/workspace/unrust`.
- Commit the `rustify` metadata corrections in `/home/jmagar/workspace/rustify`.
- Audit `rustifi`, `rustscale`, `apprise-mcp`, and `rmcp-template` for the same stale image/repository metadata pattern.
- Decide whether the local `.env` allowed-host additions should be documented in each sidecar README or deployment docs so future recreates do not drop the reverse-proxy Host fix.
