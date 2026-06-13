# Dispatch

This document is the canonical dispatch-layer contract for `lab`.

It defines:

- the layer model between product surfaces and `lab-apis`
- which layer owns operation metadata and execution
- the shared operation schema used across CLI, MCP, and API
- allowed dependency direction
- what each surface adapter owns
- how typed CLI, MCP, and API relate to the same shared backend

## Goal

Every service operation must have one shared execution path regardless of which product surface invokes it.

The contract is:

- humans use a typed CLI
- machines use `action + params` through MCP and API
- all three surfaces call the same surface-neutral dispatch layer

This prevents:

- `CLI -> MCP` coupling
- `API -> MCP` coupling
- repeated client-resolution logic per surface
- repeated operation validation and execution logic per surface

## Layer Model

The stack is:

- `lab-apis`
- `crates/lab/src/dispatch`
- `crates/lab/src/cli`
- `crates/lab/src/mcp`
- `crates/lab/src/api`

### `lab-apis`

`lab-apis` owns:

- upstream API clients
- upstream request and response types
- shared transport behavior
- shared transport error taxonomy

It does not own product-surface dispatch.

### `crates/lab/src/dispatch`

`dispatch` is the shared product dispatch layer.

It owns:

- operation catalog per service
- operation schema per service
- param metadata and validation
- destructive-op metadata
- client and instance resolution
- calling the SDK client
- surface-neutral results
- surface-neutral dispatch errors

It does not own:

- `clap` parsing
- MCP tool registration
- MCP envelopes
- HTTP status codes
- axum response types
- table rendering

Synthetic gateway workflows such as Code Mode still follow this ownership rule:
the schema-first IDs, search/schema composition, sandbox parent broker, and
tool-call dispatch live in `dispatch/gateway/`; MCP and CLI only adapt their
surface-specific inputs and outputs.

### Surface Adapters

The three product surfaces are adapters over `dispatch`.

#### CLI

CLI owns:

- typed command and flag parsing
- human-facing command UX
- output formatting
- confirmation prompts

CLI does not own shared operation semantics.

CLI must consume the shared operation schema for:

- help text
- validation consistency
- destructive-op behavior

The CLI does not need to expose machine-oriented `action + params` syntax to humans in order to consume the shared schema.

#### MCP

MCP owns:

- tool registration
- one-tool-per-service exposure
- MCP envelopes
- protocol-level `help` and `schema` exposure
- elicitation behavior

MCP does not own shared operation semantics.

MCP must project the shared operation schema rather than acting as the source of truth for it.

#### API

API owns:

- axum routing
- request extraction
- status-code mapping
- HTTP response shaping

API does not own shared operation semantics.

API must use the shared operation schema for validation. When API documentation is exposed, it must derive from that same shared schema.

## Allowed Dependency Direction

Allowed:

- `cli -> dispatch -> lab-apis`
- `mcp -> dispatch -> lab-apis`
- `api -> dispatch -> lab-apis`

Forbidden:

- `cli -> mcp`
- `api -> mcp`
- `cli -> api`
- `mcp -> api`

The MCP and API layers are sibling adapters, not shared backends for each other.

## Operation Contract

Each service has one canonical operation catalog in `dispatch`.

That catalog owns:

- operation name
- description
- param schema metadata
- destructive flag
- result description

Operation names must remain stable and machine-oriented. Dotted names such as `movie.get` or `sites.list` are appropriate for shared internal identity even when the CLI exposes a different typed syntax.

## Shared Operation Schema

The operation catalog must be represented as a shared surface-neutral schema.

That schema must define:

- operation name
- description
- params
- required versus optional params
- param types
- destructive flag
- result description

One acceptable shape is a shared `OperationSpec` plus `ParamSpec` family, but the exact type names are less important than the ownership rule:

- the schema belongs to `dispatch`
- surfaces project it
- surfaces do not redefine it independently

The shared schema is the semantic contract that keeps:

- typed CLI help and validation
- MCP `help` and `schema`
- API validation and documentation

aligned over time.

## Metadata Ownership

Semantic metadata belongs to `dispatch`, not to any single transport.

That includes:

- operation names
- descriptions
- params metadata
- destructive flags
- return descriptions

In practice, this must be modeled as the shared operation schema rather than as transport-local copies.

Transport layers may project that metadata into:

- CLI help
- MCP `help`
- MCP `schema`
- API documentation

They must not redefine it independently.

## Error Contract

`dispatch` returns `Result<Value, ToolError>` directly.

**Design decision (2026-04-09):** A separate `DispatchError` type was considered and rejected. Both `dispatch/` and the surface adapters live in the same `lab` crate — there is no structural enforcement benefit to a parallel error vocabulary. A `DispatchError → ToolError` mapping layer adds a catch-all arm trap (any unmatched variant silently becomes `internal_error`) with no architectural gain. `ToolError` already has the correct vocabulary: `UnknownAction`, `MissingParam`, `InvalidParam`, `UnknownInstance`, `Sdk`. Using it directly keeps the error path exhaustively checked by the compiler at every call site.

Those errors may represent:

- shared SDK failures (`ToolError::Sdk { sdk_kind, message }` — passthrough from `ApiError::kind()`)
- missing or invalid params (`ToolError::MissingParam`, `ToolError::InvalidParam`)
- unknown operations (`ToolError::UnknownAction`)
- unknown instances (`ToolError::UnknownInstance`)
- missing destructive confirmation (`ToolError::ConfirmationRequired`) — enforced by the API dispatch wrapper

Surface adapters receive `ToolError` directly and handle it for their transport:

- CLI: serialize to JSON string or format for human display
- MCP: already the native envelope type
- API: `IntoResponse` impl on `ToolError` maps `kind()` to HTTP status

`ToolError` must not be constructed or pattern-matched inside `lab-apis`. It belongs to the `lab` crate product layer.

The canonical shared error vocabulary remains defined by [ERRORS.md](./ERRORS.md).

## Result Contract

The dispatch layer must return a surface-neutral result.

For initial migration, returning `serde_json::Value` is acceptable if it reduces churn and keeps the refactor incremental.

Longer term, the dispatch layer may grow a more typed result wrapper if needed, but the contract does not require that immediately.

The important rule is:

- surfaces must not re-execute operation logic to reshape results

The canonical serialization rules remain defined by [design/SERIALIZATION.md](./design/SERIALIZATION.md).

## Client Resolution

Client and instance resolution belong below or inside `services`.

Rules:

- surfaces must not read env directly to construct service clients
- default-instance and named-instance behavior must be consistent across CLI, MCP, and API
- client construction must use shared helpers

This is a primary reason the dispatch layer exists.

## CLI Contract

Typed CLI is the human-facing contract.

Rules:

- new services must default to typed subcommands
- typed CLI commands may map to shared machine-oriented operation names internally
- CLI syntax must not force MCP-style `action + params` onto human users

The CLI remains free to choose ergonomic command names and flags as long as those map to the canonical service operations.

## MCP Contract

MCP remains the machine-facing one-tool-per-service contract.

Rules:

- one tool per service
- input remains `action + params`
- `help` and `schema` are projections of the shared operation schema
- elicitation behavior is driven by the shared destructive metadata

MCP must not be the owner of shared operation execution.

## API Contract

API mirrors the machine-facing dispatch model.

Rules:

- request shape remains `action + params`
- API owns routing, extraction, and status mapping only
- API must use the same semantic operation catalog and execution path as MCP and CLI

API must not call MCP dispatchers directly.

## Observability Boundary

Dispatch observability must be centered around the shared `services` execution boundary.

That means:

- adapters add surface context
- the dispatch layer knows the canonical operation and instance
- SDK request logs inherit that context downstream

The canonical observability rules remain defined by [OBSERVABILITY.md](./OBSERVABILITY.md).

## Testing Contract

The dispatch layer must be testable independently of the surfaces.

That allows:

- operation validation tests
- client-resolution tests
- dispatch error tests
- service execution tests

Surface layers must then need only:

- adapter tests
- envelope/status mapping tests
- a small number of integration verifications

## Migration Rule

Existing MCP service modules may be the source material for the shared dispatch layer because they already contain much of the operation matching and validation logic.

The target state is:

- move shared orchestration into `services`
- let MCP wrap `services`
- let CLI wrap `services`
- let API wrap `dispatch`

The end state must not preserve `CLI -> MCP` or `API -> MCP` dependencies.

## Suggested Layout

One acceptable layout is:

```text
crates/lab/src/
  dispatch.rs
  dispatch/
    context.rs
    params.rs
    gateway.rs
    marketplace.rs
    stash.rs
```

The exact file breakdown may evolve, but every migrated service must start directory-first: thin `<service>.rs` entrypoint plus a `<service>/` directory with `catalog.rs`, `client.rs`, `params.rs`, and `dispatch.rs`, plus optional domain modules.

## Related Docs

- [ARCH.md](./ARCH.md)
- [SERVICE_ONBOARDING.md](./SERVICE_ONBOARDING.md)
- [OBSERVABILITY.md](./OBSERVABILITY.md)
- [ERRORS.md](./ERRORS.md)
- [design/SERIALIZATION.md](./design/SERIALIZATION.md)
