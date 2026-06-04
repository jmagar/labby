# Gateway

The gateway manages upstream MCP server connections.
It connects to configured upstreams via HTTP (`StreamableHttpClientTransport`) or stdio
(child process), discovers their tools, and caches schemas and health state.

## Gateway Actions

All gateway actions dispatch through a single `gateway` MCP tool and CLI subcommand using
the `action + params` pattern described in [dev/DISPATCH.md](./dev/DISPATCH.md).

### `gateway.test` — Probing an upstream connection

`gateway.test` verifies that a gateway can connect to an upstream and list its tools.
It accepts either `name` (a saved upstream config) or `spec` (an unsaved inline config),
not both.

**SECURITY: This action may execute a local command.**

When `spec` is provided and the spec is for a stdio-backed upstream (a spec with a
`command` field and no `url`), `gateway.test` passes that `command` directly to the
child process launcher.  There is no sandbox.

Operators must treat `spec`-mode `gateway.test` as equivalent to running the named binary.
Only callers with gateway admin privileges should be able to reach this action.

When `name` is provided, the command comes from the persisted config file, which is under
operator control.  The same execution risk applies — the test action spawns the stdio
process and probes it exactly as the gateway would during live operation.

### `gateway.add` and `gateway.update`

Stdio-backed upstream configs can be added and updated without an additional
acknowledgment prompt.  The execution risk of the stored command is accepted implicitly
by the operator when they add or update the config.

### `gateway.reload`

Reloads the live connection pool from the current config.  Does not perform live RPC
fan-out to upstream servers.  Tool, resource, and prompt counts reflect the cached state
from the most recent successful probe or discovery cycle.

### `gateway.discovered_resources` and `gateway.discovered_prompts`

These actions return cached data from the most recent successful discovery cycle.
They do **not** issue live RPCs to upstream servers.  Use `gateway.reload` to trigger
a fresh probe cycle when stale data is suspected.

## Stdio Upstream Security Model

Stdio-backed upstreams spawn a child process using the configured `command` and `args`.
The process is started fresh for each connection attempt.

**The command is executed without sandboxing.**  An operator who can configure a stdio
upstream (via `gateway.add`, `gateway.update`, or direct config file edits) can cause
the gateway to run any binary on the host.

This is intentional: MCP stdio servers are by nature local programs.  The security
boundary is who can configure the gateway, not what commands the gateway will run.

Operator checklist when using stdio upstreams:

- Restrict gateway admin access to trusted users.
- Treat `gateway.test` with a `spec` param as equivalent to `exec <command>`.
- Review the `command` and `args` fields of stdio configs before adding or updating them.
- Do not expose gateway admin actions (add, update, test) over unauthenticated channels.

## Upstream Pool

The upstream pool (`dispatch/upstream/pool/`) is the runtime state manager for all
upstream connections.  See [UPSTREAM.md](./UPSTREAM.md) for pool internals, health
circuit breakers, and catalog size limits.
