# dispatch/ — shared dispatch layer

This directory is the shared semantic layer between the product adapters and `lab-apis`.

## Core Rule

CLI, MCP, and API surfaces are thin adapters over `dispatch/`. `mcp/services/` exists only for MCP-specific exceptions.

If multiple surfaces need the same action semantics, that code belongs here.

## Required Service Layout

Every migrated service must be directory-first from day one.

Required shape:

- `crates/lab/src/dispatch/<service>.rs`
- `crates/lab/src/dispatch/<service>/catalog.rs`
- `crates/lab/src/dispatch/<service>/client.rs`
- `crates/lab/src/dispatch/<service>/params.rs`
- `crates/lab/src/dispatch/<service>/dispatch.rs`

Optional:

- extra domain modules such as `devices.rs`, `wifi.rs`, `movies.rs`, `queue.rs`

Rules:

- `<service>.rs` is a thin entrypoint only
- `catalog.rs` is the single source of truth for `ActionSpec` / `ParamSpec`
- `client.rs` owns env lookup, instance lookup, auth construction, and client construction
- `params.rs` owns param coercion and request-body/query construction helpers
- `dispatch.rs` owns top-level action routing and help payload generation
- broad services may add domain modules, but not instead of the standard four files

Do not start a migrated service as one large file and split it later.

## Shared subsystems and sanctioned layout exceptions

Not everything under `dispatch/` is an action-dispatched service. The following
are **shared subsystems, not services — they are exempt from the 4-file layout**
(`catalog.rs`/`client.rs`/`params.rs`/`dispatch.rs`) and from the action catalog:
`node`, `security`, `upstream`, and `gateway/code_mode` (a submodule of
`gateway`). They are the common runtime substrate other services build on, not
peers with their own MCP tool. The architecture test
(`crates/lab/tests/architecture_orchestrator.rs`) classifies them in
`SHARED_NON_SERVICES` and always permits imports of them.

`snippets` is a **sanctioned exception** to the required layout: it has no
`client.rs` or `params.rs` because it wraps no upstream API — it reuses
`gateway::code_mode` (the shared JS execution kernel) and the local snippet
store instead of constructing an HTTP client. Do not cite `snippets` as
precedent for skipping `client.rs`/`params.rs` in a service that DOES front an
upstream API.

## Ownership

`dispatch/` owns:

- action catalogs
- param metadata and validation
- destructive metadata
- client and instance resolution
- shared action execution
- surface-neutral `Result<Value, ToolError>` behavior

`dispatch/` does not own:

- `clap` parsing
- MCP registration or envelopes
- API routing or status mapping
- output formatting
- upstream request/response parsing that belongs in `lab-apis`

## Error Rule

`ToolError` lives in `dispatch/error.rs`.

Keep all `From<ServiceError> for ToolError` impls there.
Do not create adapter-local conversion paths.

## Canonical `client.rs` Template

Every `dispatch/<service>/client.rs` must follow this exact shape:

```rust
use lab_apis::<service>::<Service>Client;
use lab_apis::core::Auth;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::env_non_empty;

/// Build a `<Service>` client from the default-instance env vars.
///
/// Returns `None` if any required env var is absent or empty.
/// Called by `AppState` at startup — keep pure (no side effects, no logging).
pub fn client_from_env() -> Option<<Service>Client> {
    let url = env_non_empty("<SERVICE>_URL")?;
    let key = env_non_empty("<SERVICE>_API_KEY")?;
    <Service>Client::new(&url, Auth::ApiKey { header: "X-Api-Key".into(), key }).ok()
}

/// Return a client or a structured error distinguishing missing config from init failure.
///
/// Do NOT collapse both cases into `not_configured_error()` — a service whose
/// URL is set but whose TLS init fails should surface as `internal_error`, not
/// as a missing-config error. Keep `client_from_env()` for the `None`-means-absent
/// startup path, and use this pattern for any code path that must report to a user.
pub fn require_client() -> Result<<Service>Client, ToolError> {
    let url = env_non_empty("<SERVICE>_URL").ok_or_else(not_configured_error)?;
    <Service>Client::new(&url, Auth::None).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("<SERVICE> client init failed: {e}"),
    })
}

/// Structured error for callers that hold a pre-built `Option<ServiceClient>`.
/// The API handler calls this directly instead of re-reading env vars.
pub fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: "<SERVICE>_URL or <SERVICE>_API_KEY not configured".to_string(),
    }
}
```

Rules:
- `client_from_env()` is called by `AppState::ServiceClients::from_env()` at startup — keep it pure (no side effects, no logging).
- `require_client()` is the MCP/CLI fallback when `AppState` is not available.
- `not_configured_error()` is exposed separately so API handlers can produce the same structured error without re-reading env vars.
- Always use `env_non_empty` — never inline `std::env::var(...).ok().filter(|v| !v.is_empty())`.
- Never read env vars inside `dispatch.rs` or `params.rs` — always go through `client.rs`.
- When the service supports multiple instances, use `InstancePool<C>` from `dispatch::helpers` instead of a bespoke `OnceLock`. Implement `client_from_instance(label: Option<&str>)` as the public entry point:

```rust
use std::sync::OnceLock;
use crate::dispatch::helpers::{env_non_empty, InstancePool};

static POOL: OnceLock<InstancePool<<Service>Client>> = OnceLock::new();

fn pool() -> &'static InstancePool<<Service>Client> {
    POOL.get_or_init(|| {
        InstancePool::build("<SERVICE>", |url, key| {
            <Service>Client::new(&url, Auth::ApiKey { header: "X-Api-Key".into(), key }).ok()
        })
    })
}

pub fn client_from_instance(label: Option<&str>) -> Result<&'static <Service>Client, ToolError> {
    pool().resolve(label)
}
```

`InstancePool::build(prefix, closure)` scans for `{PREFIX}_URL` (default instance) and `{PREFIX}_{LABEL}_URL` (named instances) at first call, caching all clients in a single `OnceLock`. `resolve(None)` returns the default instance; `resolve(Some("label"))` returns the named one. Both return `ToolError::UnknownInstance` if the label is absent.

> **Header casing:** The default is `X-Api-Key` (Servarr convention). Some APIs enforce specific casing:
> - Unraid: `X-API-Key` — matches the Unraid server's exact validation
> - UniFi: `X-API-KEY` — all caps, matches UniFi Network Application spec
> HTTP headers are case-insensitive on the wire, but some servers validate exact casing.
> Check the upstream API spec before setting.

## `dispatch.rs` Template

Every `dispatch/<service>/dispatch.rs` must handle the two built-in actions before
dispatching to service-specific logic:

```rust
use crate::dispatch::helpers::{action_schema, help_payload, require_str};
use super::catalog::ACTIONS;
use super::client::require_client;

pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help"   => Ok(help_payload("<service>", ACTIONS)),
        "schema" => { let a = require_str(&params, "action")?; action_schema(ACTIONS, a) }
        _        => dispatch_with_client(&require_client()?, action, params).await,
    }
}

pub async fn dispatch_with_client(
    client: &<Service>Client,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        // ... service-specific arms ...
        unknown => Err(ToolError::UnknownAction {
            service: "<service>".into(),
            action: unknown.to_string(),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
        }),
    }
}
```

`dispatch_with_client` is what the API handler calls with the pre-built client from
`AppState`. `dispatch` is what MCP and CLI call.

## Always-on meta-service registration

Always-on meta-services (those without a feature flag) register directly from
`dispatch::*` in `registry.rs` without a `mcp/services/` shim. This is the
**canonical pattern** for these services, not an exception. Current examples:

- `gateway` — `crate::dispatch::gateway::dispatch`
- `doctor` — `crate::dispatch::doctor::dispatch`
- `logs` — `crate::dispatch::logs::dispatch`
- `marketplace` — `crate::dispatch::marketplace::dispatch`
- `acp` — `crate::dispatch::acp::dispatch::dispatch`

`mcp/services/` is the **exception layer**, not the default. An adapter lives
there only when it needs MCP-specific behavior that cannot be represented in
shared dispatch alone — for example:

- `deploy` sets the MCP elicitation context (`McpContext`) before calling the
  shared dispatch, a protocol detail meaningless to CLI/API.
- `fs` filters `fs.preview` out of MCP discovery.
- `nodes` owns MCP-only enrollment actions with no CLI/API equivalent.

Do not add a `mcp/services/<service>.rs` shim for a service unless it genuinely
needs MCP-specific behavior. A pass-through shim that only delegates to dispatch
adds indirection without value.

There is no `Category::Acp` variant. ACP uses `Category::Ai` (set in
`lab_apis::acp::META`), which is coherent: ACP is the agent protocol layer
that fronts AI providers. Adding a new category variant for a single service
would violate the stable 10-variant catalog defined in `core/plugin.rs`.

## `fs` registration

`fs` registers unconditionally when the `fs` feature is enabled; runtime
resolution returns `workspace_not_configured` when `[workspace].root` in
`config.toml` is invalid. The catalog and `lab help` stay discoverable
regardless of workspace state, and `cli::serve` emits a single WARN at startup
when the configured root cannot be resolved.

## Naming

Use `API` for the product surface name in comments and docs.

Reserve `HTTP` for actual transport concerns such as:

- `HttpClient`
- upstream HTTP requests
- HTTP status codes


## Orchestrator Exception (lab-bg3e.3)

`Category::Bootstrap` services may invoke peer dispatch actions when the
operation is intrinsically composite. The current sanctioned cross-calls:

- `setup.draft.commit` invokes `doctor::dispatch("audit.full", _)` to gate
  the merge of `.env.draft` into `.env` on a clean health audit.
- `marketplace` forks persist through `stash`: `marketplace/stash_bridge.rs`
  reuses `stash::store::StashStore` + `stash::service::adopt_component_from_path`
  so a fork lands as a `StashOrigin::Marketplace` Stash component instead of a
  marketplace-private store.

Dependency direction is one-way:

- setup may depend on doctor; doctor MUST NOT depend on setup.
- marketplace may depend on stash; stash MUST NOT import or resolve marketplace.

Surface adapters (CLI/MCP/HTTP) MUST NOT chain dispatch calls themselves —
that work belongs in the shared dispatch layer so all three surfaces share
identical orchestration semantics. If you find yourself reaching into
another service from a surface module, move the orchestration into
`dispatch/` instead.

The one-way direction is enforced by
`crates/lab/tests/architecture_orchestrator.rs`. That test now enforces a
**general cross-service import allowlist** (`ALLOWED_EDGES`): every edge
`dispatch::<a> → dispatch::<b>` must be listed with a rationale, and no
`* → setup` edge is allowed. The `setup → doctor` orchestrator edge and the
`marketplace → stash` fork-persistence edge (alongside `marketplace → gateway`
and `marketplace → node`) are encoded there as sanctioned entries. If you need to
extend the exception (or add any new cross-service edge), add the
`(consumer, target)` pair to `ALLOWED_EDGES` with a one-line rationale and
explain why in the same PR. The same test also lints action names to
`<resource>.<verb>` dotted form.
