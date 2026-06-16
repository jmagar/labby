# dispatch/upstream/ — Upstream MCP Proxy Pool

Surface-neutral upstream MCP server proxy. Manages connections to external MCP servers (HTTP or stdio), discovers their tools, and routes `call_tool` / `read_resource` requests.

## Why dispatch/, not mcp/

Both the MCP surface and the HTTP API surface need access to `UpstreamPool`. The layer contract forbids `api -> mcp` dependencies, so shared types must live in the dispatch layer.

Dependency direction:

- `api -> dispatch/upstream`
- `mcp -> dispatch/upstream`
- `cli -> dispatch/upstream`

## Files

| File | Purpose |
|------|---------|
| `upstream.rs` | Module entrypoint. |
| `pool.rs` | Coordinator (~160 LOC): `UpstreamPool` / `UpstreamConnection` struct defs, `InProcessConnector`/`InProcessRegistration` types, builders (`new`/`with_*`/`Default`), `mod` declarations, and `pub`/`pub(crate)` re-exports. **No business logic** — all method bodies live in the `pool/` child modules as additional `impl UpstreamPool` blocks. |
| `types.rs` | `UpstreamEntry`, `UpstreamTool`, `UpstreamHealth` types and the `CIRCUIT_BREAKER_THRESHOLD` / `REPROBE_INTERVAL` constants. |
| `auth.rs`, `http_client.rs`, `process_guard.rs`, `transport.rs` | Bearer/websocket auth, body-capped HTTP client, process-group guard, websocket transport. |

### `pool/` child modules

`pool.rs` keeps the struct definitions; each child module carries method bodies (private fields are visible to descendant modules, so no `pub(super)` is needed for fields — only for cross-module-called private inherent methods).

| Module | Purpose |
|--------|---------|
| `pool/helpers.rs` | Leaf knobs + constants (`DISCOVERY_TIMEOUT`, `DEFAULT_MAX_RESPONSE_BYTES`, …), error classification, naming, redaction, `UpstreamCachedSummary`, prompt/resource merge/rewrite/`cached_upstream_tool`, `max_response_bytes()`, `estimate_response_size`. |
| `pool/logging.rs` | `UpstreamRequestLog` + `log_upstream_request_{start,finish,error}`, `capability_name`, `is_capability_unsupported`. |
| `pool/entries.rs` | `UpstreamEntry` constructors, `resolve_exposure_policy`, `health_str`. |
| `pool/validate.rs` | `validate_upstream_config` + the `validate_*` tests. |
| `pool/connect.rs` | `connect_upstream` / `_http` / `_websocket`, `runtime_origin_label`, jitter/oauth-log helpers (reads env). All transport fns are generic over the client handler `H: ClientHandler`; `connect_upstream_with_client` passes `()` (the default for pooled connections), while `connect_upstream_with_handler` is the seam the relay path uses to install a `RelayClientHandler`. |
| `pool/connect_stdio.rs` | `connect_stdio_upstream` (child-process spawn + process-group guard) + `connect_in_process_service_peer`. |
| `pool/connection.rs` | `UpstreamConnection` `Debug`/`Drop`/`shutdown` + `UpstreamPool::acquire_peer`. |
| `pool/lifecycle.rs` | `drain_for_swap`. |
| `pool/discover.rs` | `discover_all_inner` + `discover_all*` variants + `routable_upstream_peers`. |
| `pool/ensure.rs` | Lazy seeding + on-demand tool discovery; `replace_catalog_tools` shared mutator. |
| `pool/capability.rs` | `discover_capability_counts`. |
| `pool/probe.rs` | `ensure_probe_task` + `reprobe_upstream` background heartbeat/reconnect. |
| `pool/registration.rs` | In-process service-peer registration. |
| `pool/relay.rs` | `RelayClientHandler` — a `ClientHandler` for *dedicated* upstream connections that forwards server→client requests (elicitation/sampling/roots) down to the agent's `Peer<RoleServer>`, advertising only the capabilities the agent itself declared. `UpstreamPool::call_tool_relayed(config, subject, params, downstream, session_id)` returns a relay connection from a per-`(upstream, session_id)` cache (`relay_connections`) — TTL + dead-transport eviction, LRU cap, single-flight, swept by the shared sweep loop — or opens one via the generic `connect::connect_upstream_with_handler` seam. The `session_id` (minted per `LabMcpServer` session) is what binds a cached connection to one downstream agent so it is never reused across agents. Wired into both MCP proxy branches (`mcp/call_tool_upstream.rs`) behind the `LAB_UPSTREAM_RELAY_ELICITATION` env gate + a downstream-elicitation-capability check. |
| `pool/tools.rs` | Tool queries (`healthy_tools*`, `find_tool*`, `tool_schema`, exposure rows, summaries, runtime metadata, health). |
| `pool/tools_call.rs` | `call_tool` + `subject_scoped_call_tool`. |
| `pool/health.rs` | Circuit breaker: `record_*`, `should_reprobe*`, `*_last_error`, `filter_collisions`, `upstream_status`/`upstream_count`. |
| `pool/resources_list.rs` | Resource listing + synthetic `gateway_*` documents. Native `ui://` (mcp-ui) resources skip the `lab://upstream/{name}/…` rewrite so they stay addressable by the same URI a tool's `_meta.ui.resourceUri` references. |
| `pool/resources_read.rs` | `read_upstream_resource` + `subject_scoped_read_resource` + `read_upstream_ui_resource` (reverse-looks-up the owning upstream by cached native `ui://` `resource_uris`, forwards the read, preserves the native URI — **no** `lab://upstream/` rewrite — for mcp-ui widget resources). |
| `pool/prompts_list.rs` | Prompt listing + ownership lookup (`collect_upstream_prompts`, `find_prompt_owner`, …). |
| `pool/prompts_get.rs` | `subject_scoped_prompts`, `get_prompt`, `subject_scoped_get_prompt`. |
| `pool/testsupport.rs` | `#[cfg(test)]` shared fixtures + mock servers (`pub(super)`). |

**Target preserved by this split: no file should exceed 500 LOC (tests included).**
Known exceptions: `http_client.rs` (~717 LOC) was not split during the initial
refactor; it is the only file that currently violates the target and is tracked
for a follow-up split. All *new* files added to `pool/` must stay under 500 LOC.

## Key Types

- `UpstreamPool` — holds live connections and discovered tool catalogs. Cloneable (Arc internals).
- `UpstreamEntry` — snapshot of a single upstream: name, tools, health state.
- `UpstreamTool` — a discovered tool with its cached input schema and owning upstream name.
- `UpstreamHealth` — `Healthy` or `Unhealthy { consecutive_failures }`.
- `UpstreamConnection` — a live rmcp `Peer<RoleClient>` with its owning `RunningService`.

## Constants

| Constant | Value | Location |
|----------|-------|----------|
| `CIRCUIT_BREAKER_THRESHOLD` | 3 | `types.rs` |
| `REPROBE_INTERVAL` | 30 seconds | `types.rs` |
| `DISCOVERY_TIMEOUT` | 15 seconds | `pool/helpers.rs` |
| `DEFAULT_MAX_RESPONSE_BYTES` | 10 MB | `pool/helpers.rs` |

## Rules

- Do not read env vars outside `pool/helpers.rs` (`max_response_bytes()`, `upstream_discovery_concurrency()`) and the connect modules (`pool/connect.rs`, `pool/connect_stdio.rs`). Keep env reads confined to that small, named set of places.
- Do not import MCP-specific types (envelopes, registry) from `mcp/`.
  The `InProcessConnector` IoC seam (`pool.rs`) is the correct boundary: the
  MCP layer (`crate::mcp::in_process_peer`) injects a connector at startup; the
  pool calls it without knowing about `LabMcpServer`. As of A-M6, `connect_stdio.rs`
  no longer has any `crate::mcp` import — the boundary is clean. Do not re-add
  `mcp/` imports to any `dispatch/upstream/` file.
  **PATH/basename-only spawn-guard caveat (S6):** the spawn-guard allowlist check
  in `spawn_guard.rs` is basename-only — `/tmp/x/node` passes because its basename
  is `node`. This is an accepted residual: the trust boundary is admin-write access
  to the gateway config, and no further PATH resolution is performed at spawn time.
  See `spawn_guard.rs` for the canonical comment.
- Do not import API-specific types (router, state) from `api/`.
- The pool is constructed in `cli/serve.rs` and injected into `AppState` and `LabMcpServer`.
- Circuit breaker state is internal to the pool. Surfaces call `record_failure()` and `record_success()`.
