# lab — Binary Crate

The `lab` binary crate (binary name: `labby`). Depends on `lab-apis` (pure SDK). Provides three surfaces:
CLI (`clap`), MCP server (`rmcp`), HTTP API (`axum`).

Sub-docs for each surface:
- [`src/CLAUDE.md`](src/CLAUDE.md) — layer contract and ownership rules
- [`src/mcp/CLAUDE.md`](src/mcp/CLAUDE.md) — dispatch, envelopes, elicitation
- [`src/cli/CLAUDE.md`](src/cli/CLAUDE.md) — thin-shim pattern, destructive flags
- [`src/api/CLAUDE.md`](src/api/CLAUDE.md) — router, middleware, status mapping

## Feature Flags

Feature flags are service passthroughs to `lab-apis`; count can drift as services are added. Use `Cargo.toml` as the source of truth. Default: `all` (every service enabled). The `all` feature is delegated entirely to `lab-apis/all`.

All surface code (axum, rmcp, ratatui, clap) is compiled unconditionally — feature flags gate service-specific code only, not the surface infrastructure.

## Entry Point

`main.rs`: `init_tracing()` → `config::load()` (non-fatal; warns and continues) → `cli::dispatch()`. ANSI colors are stderr-TTY-gated here. JSON logs via `LAB_LOG_FORMAT=json`.

## Config Loading

Two files loaded in order: `~/.lab/.env` (secrets, `dotenvy`) then `config.toml` (preferences, searched CWD → `~/.lab/` → `~/.config/lab/`, first found wins). A CWD `.env` is also loaded (errors silently discarded). Failures in `config::load()` are non-fatal by design.

`scan_instances(prefix)` parses multi-instance env vars as `{PREFIX}_{LABEL}_{SUFFIX}`. Recognized suffixes: `URL`, `API_KEY`, `TOKEN`, `USERNAME`, `PASSWORD`. Any other suffix is silently ignored.

## MCP/Catalog Registration

Normal services register through `crates/lab/src/registry.rs` using `crate::dispatch::<service>::ACTIONS` and `crate::dispatch::<service>::dispatch`. Add `mcp/services/<service>.rs` only for MCP-specific exceptions that cannot live in shared dispatch.

Full new-service onboarding (CLI, API, TUI, dispatch layer) is covered in the root `CLAUDE.md` "Adding a New Service" checklist.

`build_catalog()` in `catalog.rs` is registry-driven: it iterates `registry.services()` and reads `svc.actions` directly. There is no `actions_for()` match arm and no separate catalog step — registering the service in `registry.rs` is sufficient for catalog coverage.

## Shared Dispatch Layer

`src/dispatch/` is the home for surface-neutral dispatch. Services should live there with the required directory layout (catalog.rs, client.rs, params.rs, dispatch.rs). `mcp/services/<service>.rs` is an exception layer for MCP-specific behavior, not the default bridge.

When adding new services, use the full `dispatch/<service>/` directory layout from the start — see `crates/lab/src/dispatch/CLAUDE.md` for templates.

`api/services/helpers.rs::handle_action()` is the shared dispatch wrapper (unknown-action gate, destructive confirmation, param stripping, timed dispatch, structured logging). All migrated API handlers call this.

## CLI: Two Implementation Tiers

**Tier 1 (typed):** `radarr` — typed `clap` `Subcommand` enum with named variants per operation. `radarr.rs` is the reference. (`audit` and `scaffold` are infrastructure commands, not service clients.)

**Tier 2 (dispatch-backed thin shims):** All other services — call into `dispatch/<service>/dispatch.rs` directly with a flat action string extracted from CLI args. When a service warrants richer UX, replace with typed subcommands.

## ToolError Invariants (Critical)

`ToolError` (`dispatch/error.rs`) is the error type for all three surfaces (MCP, CLI, HTTP). `mcp/envelope.rs` re-exports it for older import paths.

- **Never add `#[derive(Serialize)]` to `ToolError`.** Serialization is hand-written. The `Sdk { sdk_kind }` variant promotes `sdk_kind` to the top-level `kind` field; a derived impl would emit `{"kind":"sdk"}` instead.
- **`Display` on `ToolError` is always JSON**, not a human string. Don't use it for human messages.
- **`IntoResponse` on `ToolError` is shared by MCP and HTTP.** Status code mapping changes affect both transports simultaneously.
- `DispatchError` is a separate type that survives `anyhow::Error` chains and can be downcast at the serve boundary. It is not the same as `ToolError`.

## Known Gaps (Not Yet Implemented)

These are facts about the current state, not the spec:

- **`surface` field** in HTTP handler log events: missing (gap vs `OBSERVABILITY.md`).
- **`/ready` probe**: always returns 200; readiness is not actually checked.
- **Human table rendering** in `output.rs`: `print()` falls back to `serde_json::to_string_pretty` for both `Human` and `Json` formats.
