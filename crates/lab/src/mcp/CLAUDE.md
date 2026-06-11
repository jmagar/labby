# mcp/ — MCP protocol surface

This directory is the translation layer between `lab-apis` (pure SDK) and the MCP protocol. It owns dispatch, envelopes, resources, elicitation, and the shared catalog.

## One tool per service

Each enabled service registers exactly one MCP tool in `crates/lab/src/registry.rs` (not `mcp/registry.rs`, which is a thin re-export). The tool name matches the service name (`radarr`, `sonarr`, `gateway`, ...). Normal services register directly from the shared dispatch layer:

```rust
#[cfg(feature = "radarr")]
register_service!(reg, "radarr", radarr);
```

The default macro path reads `crate::dispatch::<service>::ACTIONS` and calls `crate::dispatch::<service>::dispatch`.

## Dispatch pattern

For normal services, `dispatch/<service>/dispatch.rs` owns action routing, catalog, param validation, and client resolution. See `crates/lab/src/dispatch/CLAUDE.md` for the required layout and templates.

`mcp/services/` is now an exception layer, not the default adapter surface. Keep a module there only when it owns MCP-specific behavior that cannot live in shared dispatch. Current examples:

- `deploy` sets the MCP elicitation context before calling shared deploy dispatch.
- `fs` filters `fs.preview` out of MCP discovery and execution.
- `nodes` owns MCP-only enrollment actions.
- `code_mode` and `tool_execute` are registered directly in `mcp/server.rs` as
  gateway meta-tools and bypass both `dispatch/` and `mcp/services/`.
  They expose the upstream MCP proxy surface to clients. Parameter
  shapes (`query`/`top_k`/`include_schema`; `name`/`arguments`) are
  incompatible with the action+params contract. Business logic lives in
  `GatewayManager::search_tools()` / `execute_tool()` in
  `dispatch/gateway/manager.rs`, called directly. No CLI or HTTP
  equivalent is planned. The rejection guard test in
  `dispatch/gateway/dispatch.rs` enforces the non-dispatch boundary. Do
  not add `dispatch/gateway-code-mode/` unless a second surface consumer is
  confirmed.
- `code_search` and `code_execute` are registered directly in
  `mcp/server.rs` as gateway Code Mode meta-tools. MCP owns
  tool registration, scope extraction, MCP request parsing, and
  `CallToolResult` envelope conversion. Code Mode business logic lives
  in `dispatch/gateway/code_mode.rs` so the native CLI can call the same
  broker without routing through MCP.

**No business logic anywhere in `mcp/`.** If you find yourself calling `reqwest`, parsing JSON beyond param extraction, or retrying, move it to `lab-apis/src/<service>/client.rs`.

## Structured error envelopes

`ToolError` in `envelope.rs` is the **single canonical error type** across all three surfaces — MCP, API, and CLI. Every failure returns the same JSON shape:

```jsonc
{ "kind": "missing_param", "message": "missing required parameter `query`", "param": "query" }
{ "kind": "unknown_action", "message": "...", "valid": ["movie.list", ...], "hint": null }
{ "kind": "auth_failed",    "message": "authentication failed" }   // SDK pass-through
```

Dispatcher-layer kinds:

| `kind` | When |
|--------|------|
| `unknown_action` | action not in the service's action table. Include `valid: [...]` and fuzzy `hint`. |
| `unknown_subaction` | subaction segment invalid. |
| `missing_param` | required param absent. Include `param` name. |
| `invalid_param` | param present but wrong type/value. |
| `unknown_instance` | multi-instance label not found. Include `valid: [...]`. |

SDK-layer kinds pass through from `ApiError::kind()` via `From<SdkError> for ToolError`: `auth_failed`, `not_found`, `rate_limited`, `validation_failed`, `network_error`, `server_error`, `decode_error`, `internal_error`.

### Serialization contract

`ToolError` uses a **custom `Serialize`** (not `#[derive(Serialize)]`) so that the `Sdk` variant promotes its `sdk_kind` field to the top-level `kind` field. The result is byte-identical across MCP and HTTP — never `{"kind":"sdk","sdk_kind":"auth_failed"}`.

- `Display` delegates to `serde_json::to_string(&self)` — output is always valid JSON.
- `IntoResponse` serializes `self` directly; HTTP status is derived from `kind()`.
- Tests in `envelope.rs` lock in this contract — do not break them.

### Wiring per service

Each service dispatcher must:
1. Return `Result<Value, ToolError>` (not `anyhow::Result`).
2. Implement `From<ServiceError> for ToolError` mapping via `ApiError::kind()`.
3. Use `ToolError::MissingParam` / `UnknownAction` for dispatcher-layer errors.
4. Never use `anyhow::bail!` or `anyhow::anyhow!` inside a dispatch function.

## Elicitation for destructive ops

When an action's `ActionSpec.destructive == true`, the dispatcher **must** call the MCP elicitation flow before executing. The client confirms, then the dispatcher proceeds.

When the MCP client does not support elicitation (e.g. headless agents, CI, Claude Desktop non-interactive), the dispatcher accepts `params.confirm == true` as a machine-to-machine bypass. Without that flag, destructive actions are refused with a `confirmation_required` error.

## Built-in actions

Every tool automatically supports `help` and `schema` without the service declaring them. The dispatcher intercepts these before the action match.

## Shared catalog — one builder, three surfaces

`build_catalog()` (in `crates/lab/src/catalog.rs`) is the **single source** feeding:

1. The `lab.help` global MCP tool.
2. The `lab://catalog` MCP resource.
3. The `lab help` CLI subcommand.

Never duplicate catalog logic. If you need richer data, extend the builder.

## Resources

- `lab://<service>/actions` — per-service action catalog (name, description, destructive, params).
- `lab://catalog` — the full cross-service catalog.

Resources are read-only. Do not use them for mutations.

### `ui://` resources (MCP Apps / mcp-ui)

`read_resource_impl` splits the `ui://` namespace:

- `ui://lab/code-mode/*` — Lab's own Code Mode app resources, served locally
  from bundled HTML (`read_code_mode_app_resource_impl`).
- any other `ui://<upstream>/…` — an upstream mcp-ui widget resource (referenced
  by a tool result's `_meta.ui.resourceUri`). Routed to the owning upstream peer
  via `pool.read_upstream_ui_resource` (catalog reverse-lookup, native URI
  preserved). See `resource_proxy.rs::read_upstream_ui_resource_impl`.

## Transport auth for fs

The `fs` service exposes workspace filesystem contents (`fs.list`,
`fs.preview`). The HTTP surface refuses to mount `/v1/fs` when
`LAB_WEB_UI_AUTH_DISABLED=true` — see `api/router.rs` and the
corresponding gate in `cli/serve.rs`. The MCP surface has **no**
equivalent env-driven refusal: `fs` is registered unconditionally in
`registry.rs` whenever the `fs` feature is compiled in, regardless of
MCP transport auth posture.

Existing hard checks (enforced in code):

- Router: `/v1/fs` refuses to mount when
  `LAB_WEB_UI_AUTH_DISABLED=true` (`api/router.rs`). This is the only
  enforcement that fires in the LAB_WEB_UI_AUTH_DISABLED + LAN-bind
  scenario, because the bind guard below treats a configured bearer
  token as "auth configured" even though the `/v1` middleware has been
  bypassed.
- Bind: `cli/serve.rs` refuses to bind on a non-loopback address when
  no auth is configured at all (no bearer token, no OAuth). Does NOT
  fire when `LAB_WEB_UI_AUTH_DISABLED=true` is paired with a token —
  that case relies on the router-level fs mount refusal above.

Operator-side (not enforced in code) — must be ensured before exposing
a server that has the `fs` feature enabled:

- `labby serve` (HTTP transport, the default): require
  `LAB_MCP_HTTP_TOKEN` or `LAB_AUTH_MODE=oauth`. Do not relax this
  while `fs` is feature-enabled.
- `labby mcp`: stdio has no transport-level auth. Ensure
  the process is not reachable by untrusted callers — do not expose it
  through a network proxy without front-side auth.

The asymmetry with `/v1/fs` is intentional: MCP registration is not
structured to fail or skip a single service at runtime, and stdio has
no single env var equivalent to `LAB_WEB_UI_AUTH_DISABLED`. Promoting
this to a runtime invariant (e.g. a startup check that refuses to
register `fs` when MCP auth posture is not verified) is tracked as
follow-up work.
