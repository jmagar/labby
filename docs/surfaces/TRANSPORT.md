---
title: "Transport Contract"
created: "2026-07-30"
updated: "2026-08-01"
---

# Transport Contract

Labby supports three MCP transports over one execution layer: stdio, streamable
HTTP over TCP, and streamable HTTP over a Unix-domain socket. Transport choice
does not change the catalog, schemas, envelopes, or destructive-op policy.

## Stdio

Run:

```bash
labby mcp
```

Stdio is intended for local editor and desktop clients. Protocol messages use
stdin/stdout; logs must never be written to stdout.

### Local bridge to the running daemon

Use `labby mcp` when the MCP client can launch a local command but should use
the same gateway state as a long-running `labby serve` daemon. The client talks
to `labby mcp` over stdin/stdout, so the client configuration does not need an
HTTP URL:

```json
{
  "mcpServers": {
    "labby": {
      "command": "labby",
      "args": ["mcp"]
    }
  }
}
```

Start the daemon once, either in a terminal or as the installed user service:

```bash
labby serve --host 127.0.0.1 --port 8765
```

When `labby mcp` starts, it probes the local daemon. If one is reachable, it
becomes a transparent stdio-to-MCP bridge: tools, resources, prompts,
notifications, cancellation, and task responses are forwarded to the daemon.
The bridge does not create a second gateway manager, upstream pool, or OAuth
state, so the local client sees the same configuration and connections as the
web UI and HTTP clients.

If no explicit remote target is configured and no daemon is found, `labby mcp`
starts a standalone local gateway instead. This is useful for a fully local
setup, but it has its own configuration and runtime state. To require one
specific daemon and prevent that standalone fallback, set `LABBY_SERVER_URL`:

```bash
LABBY_SERVER_URL=http://127.0.0.1:8765 labby mcp
```

Explicit targets fail closed when they are invalid, unreachable, unauthorized,
or not a compatible Labby daemon. For a daemon on another host, use an HTTPS
`LABBY_SERVER_URL` and the daemon's matching `LABBY_MCP_HTTP_TOKEN`; use the
HTTP MCP endpoint directly when the client cannot launch local commands. The
full target and token rules are in [Environment](../runtime/ENV.md#remote-gateway-cli-usage).

## Streamable HTTP

Run:

```bash
labby serve --host 127.0.0.1 --port 8765
```

The native MCP endpoint is `/mcp`. The hosted runtime also mounts supported
`/v1/*` APIs, auth routes, health routes, protected MCP routes, and static web
assets when available.

The generated route inventory in
[../generated/api-routes.md](../generated/api-routes.md) is authoritative.

### Direct stdio proxy

`labby proxy <child>` is a separate foreground transport adapter. It exposes
only that child over a stateless Streamable HTTP router bound to `127.0.0.1`.
Tailscale Serve may publish the loopback router, but the aggregate Labby
catalog and `/v1/*` APIs are not mounted on the proxy endpoint.

The proxy keeps RMCP Host and Origin enforcement enabled. Allowed authorities
and origins are loopback plus the exact public resource host and port when one
exists. JSON and request-scoped SSE responses remain enabled. Child launch uses
an argv vector, a scrubbed environment, continuously drained/redacted stderr,
and owned process trees. See the
[stdio MCP proxy guide](../guides/STDIO_MCP_PROXY.md) for the complete security
and cleanup model.

## Streamable HTTP Over A Unix-Domain Socket

`transport = "unix_socket"` serves the same router and middleware stack as
HTTP/TCP. It is a same-host (or shared-namespace) transport, not a cross-node
one.

```toml
[mcp]
transport = "unix_socket"
socket_path = "/run/labby/labby.sock"
socket_mode = "0660"
peer_uid = 1000
```

- Filesystem listeners require an absolute path whose existing directory chain
  contains only real directories owned by root or the process effective UID.
  Non-sticky group/world-writable ancestors are rejected.
- Labby binds inside a private `0700` staging directory, applies the configured
  mode and ownership there, then atomically publishes the hardened socket.
  Startup reclaims only a verified stale socket; shutdown removes only the exact
  inode this process created.
- Linux abstract sockets use `@name` and have no filesystem owner or mode, so
  `socket_mode`, `socket_uid`, and `socket_gid` must be omitted.
- Bearer and OAuth continue to work over the socket. Linux peer-credential
  authorization (`peer_uid` / `peer_gid`) is an alternative that cannot be
  combined with bearer or OAuth, and is interpreted in the listener's user
  namespace. A Unix listener with none of the three is refused at startup.

## Authentication

- Operator/admin routes use the configured bearer or OAuth mode.
- The downstream MCP endpoint uses the stateless `2026-07-28` lifecycle as its
  primary contract. Its legacy `initialize` compatibility path declares and
  preserves every SDK-known historical protocol version it genuinely adapts,
  so older clients retain that version's request-validation and wire semantics.
  The gateway-to-upstream boundary attempts `server/discover` first
  and performs one bounded fallback to legacy `initialize` for recognized
  lifecycle-compatibility failures; HTTP/TCP and Unix-socket upstreams share
  that policy.
- Protected MCP routes validate route-specific OAuth resources and scopes.
- OAuth metadata and callback routes are public by protocol design.
- Browser session cookies are separate from MCP authorization headers.

## Reverse Proxy

A reverse proxy must preserve the request host/scheme information used to build
OAuth metadata and callback URLs, pass authorization headers, support streaming
responses, and avoid buffering Streamable HTTP traffic. See
[../runtime/REVERSE_PROXY.md](../runtime/REVERSE_PROXY.md).

## Host Validation

State-changing same-origin web/API requests are subject to host validation and
CSRF protections. Do not make a route public merely to simplify browser calls.

## Unsupported Legacy Shapes

The hosted runtime does not expose Fleet/node WebSockets, node enrollment APIs,
Marketplace preview routes, ACP session endpoints, old Agent Artifact Manager
APIs, or MCP Registry compatibility endpoints. Their historical transport
contracts are archived under [../archive/retired-labby](../archive/retired-labby/).
On Linux, File Stash exposes authenticated routes beneath
`/v1/stash`; its bounded upload/download behavior and identity requirements are
specified in [STASH.md](../services/STASH.md). Unsupported platforms omit these
routes rather than mounting a handler that fails after dispatch.
