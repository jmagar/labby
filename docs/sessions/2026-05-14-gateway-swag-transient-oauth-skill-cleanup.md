# 2026-05-14 Gateway SWAG Transient OAuth and Skill Cleanup

## Context

- Repo: `/home/jmagar/workspace/lab`
- Branch: `main`
- Starting commit: `2fbfb1d2`
- Captured: `2026-05-14 00:33:07 EDT`

The user reported that adding a Gateway server named `swag` failed with a missing URI-style validation error, did not appear in the Gateway UI afterward, but retrying the add said an upstream named `swag` already existed.

## Investigation

- Checked `~/.labby/config.toml`: no persisted `[[upstream]]` named `swag`.
- Checked the live API on `http://127.0.0.1:8765` with `LAB_MCP_HTTP_TOKEN`:
  - `gateway.get` for `swag` returned `404`.
  - `gateway.status` for `swag` returned an empty list.
  - `gateway.list` did not include `swag`.
- Confirmed the running service was the Docker `labby` container using the repo-bound `bin/labby`.
- Traced the conflict to the upstream OAuth probe path:
  - OAuth probe can register a transient `UpstreamOauthManager` under the requested upstream name.
  - If the later gateway save fails, the transient manager can remain in memory.
  - Retrying with the same name but a different/corrected URL could hit `transient upstream ... already registered for a different URL`, even though the upstream is not persisted or visible in `gateway.list`.

## Changes Made

- Updated `crates/lab/src/dispatch/gateway/oauth_lifecycle.rs`.
- Preserved normal protection for persisted upstream names: a real configured upstream name still rejects a different URL.
- Changed non-persisted transient OAuth managers so a retry with the same name and a different URL evicts the stale transient manager, clears cached OAuth clients for that name, and registers the new transient manager.
- Rebuilt and restarted the live dev container with `just dev-debug`, which installs `target/debug/labby` to `bin/labby` and restarts Docker Compose.

## Skill Cleanup

- The earlier `git diff --check` failure came from trailing whitespace in `plugins/unifi/skills/unifi/SKILL.md`.
- Loaded the system `skill-creator` guidance because the user asked to address the skill issue.
- Removed only the trailing whitespace on the flagged Markdown lines.
- Left the broader existing UniFi skill rewrite intact.

## Verification

- `cargo fmt --check --all` passed.
- `cargo check --manifest-path crates/lab/Cargo.toml --all-features` passed.
- `git diff --check` passed after the UniFi skill whitespace cleanup.
- `just dev-debug` completed successfully and restarted the `labby` container.
- `GET /health` returned `{"status":"ok","mode":"master","pid":7,"uptime_s":0}` after restart.
- Live API after restart:
  - `gateway.get` for `swag` still returns `404`, confirming no hidden persisted gateway was created.
  - `gateway.list` includes the existing upstreams only: `chrome-devtools`, `claude-in-mobile`, `bitwarden`, `open-design`, `syslog`, `axon`, `context7`, `repomix`, `zsh-tool`, `claude-mcp`, and `shadcn`.
  - `/v1/gateway/oauth/upstreams` lists only persisted OAuth upstreams: `syslog` and `axon`.

## Current Dirty Worktree

The worktree had many dirty files before this session's edits. Current dirty files include:

- Gateway/admin frontend files under `apps/gateway-admin/`.
- Gateway and MCP backend files under `crates/lab/src/`.
- Docs under `docs/runtime/CONFIG.md`, `docs/services/GATEWAY.md`, and `docs/services/UPSTREAM.md`.
- `plugins/unifi/skills/unifi/SKILL.md`.
- This session note under `docs/sessions/`.

Known edits from this session:

- `crates/lab/src/dispatch/gateway/oauth_lifecycle.rs`
- `plugins/unifi/skills/unifi/SKILL.md`
- `docs/sessions/2026-05-14-gateway-swag-transient-oauth-skill-cleanup.md`

## Open Questions

- The original "missing uri" frontend/backend error was not reproduced directly. The hidden-name symptom was explained and fixed through the transient OAuth manager path.
- Several dirty files were pre-existing or outside this session's direct scope. They should be reviewed separately before staging a broad commit.
