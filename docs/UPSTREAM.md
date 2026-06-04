# Upstream Pool

The upstream pool (`crates/lab/src/dispatch/upstream/pool/`) manages live connections
to upstream MCP servers and caches their discovered capabilities.

## Overview

Each upstream in the gateway config gets one `UpstreamEntry` in the pool's catalog,
plus one live `UpstreamConnection` when connected.  The catalog is kept in memory as
an `Arc<RwLock<HashMap<String, UpstreamEntry>>>`.

## Transport Types

### HTTP upstreams

HTTP upstreams connect via `StreamableHttpClientTransport` to a `url` endpoint.  The
connection is persistent and shared across requests.

### Stdio upstreams

Stdio upstreams spawn a child process using the `command` and `args` fields from the
upstream config.  A new process is started for each connection attempt.

**The spawned command is executed without sandboxing.**  See [GATEWAY.md](./GATEWAY.md)
for the full security model around stdio upstreams and the `gateway.test` action.

## Catalog Size Limits

To prevent runaway allocations when a misconfigured or malicious upstream exposes an
unusually large catalog, the pool enforces hard caps on returned catalog sizes:

| Cap constant           | Value | Applies to         |
|------------------------|-------|--------------------|
| `MAX_UPSTREAM_TOOLS`    | 1000  | `healthy_tools()`  |
| `MAX_UPSTREAM_RESOURCES`| 1000  | `list_upstream_resources()` |
| `MAX_UPSTREAM_PROMPTS`  | 1000  | `collect_upstream_prompts()` |

When the combined catalog across all upstreams exceeds a cap, the excess is dropped and
a `tracing::warn!` is emitted at the truncation site.

Callers that need to inspect the full catalog of a single upstream (e.g. gateway admin
exposure-policy auditing via `tool_exposure_rows()`) bypass these caps because they
operate on a single upstream, not the combined pool.

## Health and Circuit Breakers

Each upstream has independent health state for tools, resources, and prompts.  The health
state is tracked per capability:

- **`Healthy`** — the upstream is responding and routable.
- **`Degraded`** — the upstream has had recent failures but is still attempted.
- **`Unhealthy`** — the upstream has exceeded the circuit-breaker failure threshold.

Failure counts are tracked per `UpstreamCapability` (`Tools`, `Resources`, `Prompts`).
After `CIRCUIT_BREAKER_THRESHOLD` consecutive failures, the capability is marked
unhealthy and removed from routable listings until a successful response is recorded.

`record_success_for` and `record_failure_for` advance the circuit-breaker state.

## Cached Discovery vs. Live RPC

Many gateway inspection actions return data from the catalog cache to avoid fan-out RPC
bursts to all upstream servers:

- **`cached_upstream_resource_uris()`** — returns the URI list from the last successful
  `resources/list` probe.  Updated each time `list_upstream_resources()` runs.
- **`cached_upstream_prompt_names_by_upstream()`** — returns prompt names per upstream
  from the last successful `prompts/list` probe.  Updated each time
  `collect_upstream_prompts()` runs.
- **`healthy_tools()`**, **`find_tool()`**, and related tool queries — serve from the
  in-memory `entry.tools` map, which is populated on connect and updated on rediscovery.

Use `gateway.reload` to trigger a fresh probe cycle and repopulate these caches.

## Exposure Policy

Each upstream has a `ToolExposurePolicy` that controls which of its discovered tools
are forwarded to clients.  A tool that exists in the upstream's catalog but does not
match the exposure policy is discovered but hidden:

- Hidden tools are returned by `tool_exposure_rows()` (admin auditing).
- Hidden tools are excluded from `healthy_tools()`, `find_tool()`, and all client-facing
  listings.
- Direct calls to a hidden tool via `gateway.call_tool` are rejected.

## OAuth Upstreams

Upstreams with an `oauth` config field connect with per-subject OAuth credentials.
The pool's `OauthClientCache` holds one client per `(upstream_name, subject)` pair.
Subject-scoped tool discovery uses short-lived connections: one connect per subject per
probe cycle, not a shared long-lived connection.

## Testing with Stdio Upstreams

Tests that exercise `gateway.test` with a `spec` param for a stdio upstream **will
execute the `command` field as a real child process** on the test host.  These tests
use safe benign commands (e.g. `echo`) so that the test passes in any environment.

Test function names in `dispatch/gateway/dispatch.rs` that involve stdio execution are
named to make the behavior explicit (e.g. `gateway_test_spec_stdio_executes_command`).

Contributors adding new tests for `gateway.test` with stdio specs must:

1. Use a safe command that exits cleanly on all platforms (e.g. `echo`, `true`).
2. Name the test to reflect that a command is being executed.
3. Include a comment citing this document and the `SECURITY NOTE` in the handler.
