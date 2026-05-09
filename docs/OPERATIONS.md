# Operations

This document covers operator-facing workflows, verification surfaces, CI, and release behavior.

## Repo-Level Helpers

The repo includes helper tooling outside the shipped binary.

### `bin/health-check`

Purpose:

- smoke-test configured services from the repo env file
- validate reachability quickly
- provide operator-friendly shell output

It is distinct from the product-level `labby health` surface.

It is intended as a repo-local smoke test, not as the canonical SDK-level health API.

### `scripts/check-oauth.sh`

Purpose:

- verify OAuth/auth configuration against a **running server** from outside the process
- confirm all protected endpoints return 401 without auth and accept valid tokens
- validate OAuth discovery metadata, issuer, JWKS, and RFC 9728 WWW-Authenticate header
- confirm public endpoints (health, node self-registration, OAuth callbacks) are not auth-blocked

Usage:

```bash
./scripts/check-oauth.sh                          # auto-loads ~/.lab/.env, defaults to localhost:8080
./scripts/check-oauth.sh https://lab.example.com  # explicit URL
LAB_BASE_URL=https://lab.example.com ./scripts/check-oauth.sh
```

Exit codes: `0` = pass, `1` = one or more failures. Suitable for post-deploy CI gates.

Complements `labby doctor`, which checks internal state (config, file permissions, SQLite) before a server is running. `scripts/check-oauth.sh` is the external black-box probe; `labby doctor` is the internal pre-flight check.

### `just mcp-token`

Purpose:

- generate or rotate `LAB_MCP_HTTP_TOKEN`
- update the env file safely

## OAuth Auth State

When `LAB_AUTH_MODE=oauth`, `lab` persists local auth state on disk:

- SQLite database: `~/.lab/auth.db` by default
- JWT signing key: `~/.lab/auth-jwt.pem` by default

Rules:

- `LAB_AUTH_ADMIN_EMAIL` must be set to the bootstrap admin's Google email; startup fails closed if it is missing so no Google account can authenticate without explicit permission
- both files must use restrictive permissions; on Unix, `lab` requires they are not group- or world-readable
- new files are created with `0600` permissions on Unix
- the SQLite store is opened in WAL mode with a non-zero busy timeout
- the current auth store opens a small local SQLite pool, so login/code/token traffic is no longer funneled through one in-process mutex lane
- Google tokens stay server-side only; clients always receive `lab` access tokens and receive `lab` refresh tokens only when Google granted an upstream refresh token

Recovery guidance:

- deleting `auth-jwt.pem` invalidates every previously issued `lab` access token and refresh token exchange path tied to those access tokens
- deleting `auth.db` removes registered clients, pending authorization requests, authorization codes, and refresh tokens
- if you back up either file, back up both together to preserve a coherent auth state snapshot

## Browser-Local OAuth Callback Forwarding

Some MCP clients can pin the OAuth callback port but still redirect the browser to
`http://127.0.0.1:<port>/...`. When the real callback listener lives on another machine, run
`labby oauth relay-local` on the browser machine to accept that loopback redirect and forward it to
the actual listener.

Named-machine workflow:

```bash
labby oauth relay-local --machine dookie --port 38935
```

Ad hoc workflow:

```bash
labby oauth relay-local \
  --forward-base http://node.internal.example:38935/callback/dookie \
  --port 38935
```

Operational rules:

- the remote callback listener must already be running
- the helper is transport-only; it does not exchange codes or mint tokens
- the listener is loopback-only and normally run on demand for the active login flow
- startup output shows the resolved forwarding target before the first callback arrives
- failures map to HTTP responses on the local callback port: unreachable target -> `502`, timeout -> `504`

Recommended setup checklist:

1. Configure the browser-side machine target in `~/.lab/config.toml`:

```toml
[oauth.machines.dookie]
target_url = "http://node.internal.example:38935/callback/dookie"
description = "dookie Codex callback listener"
default_port = 38935
```

2. Start the real OAuth client listener on the remote machine.
3. Start `labby oauth relay-local` on the browser machine.
4. Complete the OAuth login flow in the browser before either listener exits.

If you need public redirect URIs for a relay or browser-facing callback domain, remember to
allowlist them in `lab-auth` with `LAB_AUTH_ALLOWED_REDIRECT_URIS` or
`[auth].allowed_client_redirect_uris`.

## Product-Level Health Tooling

### `labby doctor`

`labby doctor` is the main read-only validation command.

It should audit:

- required env vars
- URL validity
- connectivity
- auth
- version visibility

It should support:

- all services
- single-service runs
- JSON output
- quick mode

Typical checks include:

- required env presence
- optional env visibility
- DNS/URL validity
- TCP reachability
- health endpoint success
- auth acceptance
- version reporting

### `labby health`

`labby health` should expose normalized health status using shared service contracts.

## Device Runtime Operations

In the current Linux `x86_64` v1 target, every supported fleet member runs `labby serve` as a node runtime.

Setup order:

1. Pick one machine as the master and start it first with `labby serve`.
2. If you use bearer auth, set `LAB_MCP_HTTP_TOKEN` on the master before starting it and reuse that same token on every non-master device that reports to it.
3. On each non-master, set the master machine name in `~/.lab/config.toml`:

```toml
[node]
controller = "tootie"
```

4. Start each non-master with `labby serve`.
5. Only use `labby mcp` when you explicitly want a local stdio MCP session instead of the default HTTP runtime.

Operationally:

- one device is the `master`
- non-controller nodes report to the master over `/v1/nodes/*`
- node inventory and node logs are queried from the master

Useful commands:

```bash
labby nodes list
labby nodes get dookie
labby logs search dookie oauth
```

Useful HTTP checks:

```bash
curl http://<device>:8765/health
curl -H "Authorization: Bearer $LAB_MCP_HTTP_TOKEN" http://<controller>:8765/v1/nodes/devices
```

Current operational limits:

- fleet state is in-memory on the master
- non-master background uploads reuse the shared static bearer token when bearer auth is enabled
- non-controller nodes intentionally do not expose Web UI, gateway management, or MCP
- the master should be reachable on its configured HTTP port before non-masters start reporting to it

## Install and Patch Workflows

Install and uninstall operations should:

- validate env requirements
- prompt for missing values when appropriate
- patch `.mcp.json` atomically
- back up before write
- support dry-run behavior

## CI

CI should verify:

- workspace builds
- formatting
- linting
- deny checks
- CI-safe tests
- docs when rustdoc verification is enabled

Expected job split:

- fast correctness and style checks on pushes and PRs
- release builds on tags
- publishing after successful release builds

Live service integration tests are intentionally excluded from normal CI.

## Release Process

Locked release expectations:

- single workspace version
- tagged releases
- release artifacts per supported platform
- GitHub Releases as the artifact distribution surface
- `cargo-release` for version bumps and tagging
- GitHub-generated release notes

Tag format should stay `vX.Y.Z`.

## Privacy Rule

Operator workflows must respect the project-wide privacy rule:

- no telemetry
- no analytics
- no phone-home traffic except explicit service calls or explicit update operations
