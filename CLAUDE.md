# Lab — Development Instructions

## What is this?

`lab` is a pluggable homelab CLI + MCP server SDK in Rust. One binary exposes CLI, MCP, HTTP API, and Labby web UI surfaces for product-local control-plane services. Standalone Cargo product slices currently include `gateway`, `marketplace`, `fs`, `deploy`, and `acp_registry`; base services include `doctor`, `setup`, `logs`, `device`, `stash`, and `acp`. MCP dispatch still uses a single tool per runtime service with an `action` + `params` shape instead of hundreds of per-method tools.

Start with `docs/README.md` for the docs index. The topic docs in `docs/` are the source of truth; if this file disagrees with them, this file is stale.

Observability is governed by `docs/dev/OBSERVABILITY.md`. When adding or changing request paths, treat that file as the source of truth for logging boundaries, required fields, correlation, redaction, and verification.
Errors are governed by `docs/dev/ERRORS.md`. Serialization and output-boundary rules are governed by `docs/design/SERIALIZATION.md`.
Shared dispatch ownership and adapter direction are governed by `docs/dev/DISPATCH.md`.

## Long-Lived Branches

- `marketplace-no-mcp` is an intentional long-lived marketplace variant branch,
  not stale cleanup. It keeps Lab/Labby marketplace plugin and skill entries
  available while removing bundled MCP server registrations for environments
  where those servers are already connected through the Labby gateway.
- Do not merge `marketplace-no-mcp` into `main` by default, and do not delete it
  as stale unless Jacob explicitly retires the no-MCP marketplace variant.

**Build assumption.** This repo is developed and verified as an **all-features** binary. Treat `cargo build --all-features`, `cargo nextest run --all-features`, and the equivalent `just` commands as the default truth. Narrow feature-slice builds are supported for `gateway`, `marketplace`, `fs`, `deploy`, and `acp_registry`; use them to catch accidental cross-slice coupling, but check warning/removal decisions against the normal all-features build before deleting shared helpers.

**Service onboarding rule.** When bringing a service online, follow the dispatch/module layout in `docs/dev/SERVICE_ONBOARDING.md`, update generated docs, then validate with the all-features test/build path. The older `labby scaffold service` / `labby audit onboarding` workflow is not part of the current CLI surface unless those commands are restored in code.

**Nested guides.** Subdirectories carry their own `CLAUDE.md` with rules that don't belong at the root. Read the nearest one when working in:
- `crates/labby-apis/src/core/` — trait contracts, error taxonomy, HttpClient invariants
- `crates/labby/src/dispatch/` — product dispatch layer, required service layout, canonical templates
- `crates/labby-gateway/src/upstream/` — upstream MCP proxy pool, circuit breaker, layer contract
- `crates/labby/src/mcp/` — dispatch, envelopes, elicitation, catalog
- `crates/labby/src/cli/` — thin-shim pattern, destructive flags, batch commands
- `crates/labby/src/api/` — axum HTTP surface, status code mapping, middleware stack

## Repository Structure

The workspace is split into reusable crates plus one product binary crate. Pure
SDK/domain clients live in `labby-apis`. HTTP/OAuth auth middleware and
upstream OAuth runtime live in `labby-auth`. Shared transport-neutral contracts
and helpers live in `labby-runtime`. Code Mode execution lives in
`labby-codemode`. Gateway runtime/proxy orchestration lives in `labby-gateway`.
Embedded/static web serving lives in `labby-web`. Windows process-tree reaping
lives in `labby-winjob`. CLI, MCP, HTTP API adapters, config loading, product
dispatch, and the `labby` binary live in `labby`.

```
lab/
├── crates/
│   ├── labby-apis/                   # PURE Rust SDK — reusable in any binary
│   │   ├── Cargo.toml                # deps: reqwest, serde, thiserror, tokio
│   │   └── src/
│   │       ├── lib.rs                # re-exports, feature gates
│   │       ├── core/                 # HttpClient, Auth, errors, traits
│   │       ├── acp/                   # ACP provider/session primitives
│   │       ├── acp_registry/          # SDK-only ACP Registry client
│   │       ├── mcpregistry/           # SDK-only MCP Registry v0.1 client
│   │       ├── marketplace/            # marketplace pure data types
│   │       ├── deploy/                # Deployment/runner primitives
│   │       ├── device_runtime/        # ALWAYS-ON: local device runtime introspection
│   │       ├── doctor/                # doctor pure data/client helpers
│   │       ├── setup/                 # setup pure data/client helpers
│   │       └── stash/                 # stash pure data types
│   │
│   ├── labby-auth/                   # HTTP/OAuth auth middleware and storage
│   ├── labby-runtime/                # ToolError, config DTOs, path/security helpers
│   ├── labby-codemode/               # Code Mode runner kernel + snippet engine
│   ├── labby-gateway/                # Gateway manager, upstream pool, OAuth lifecycle
│   ├── labby-web/                    # Embedded/filesystem web asset serving
│   ├── labby-winjob/                 # Windows Job Object helper crate
│   └── labby/                        # BINARY: cli + mcp + api + product dispatch
│       ├── Cargo.toml                # deps: labby-*, clap, rmcp, axum, anyhow
│       └── src/
│           ├── main.rs
│           ├── api.rs                # axum surface module declaration
│           ├── catalog.rs            # build_catalog() — single source for help/resource/CLI
│           ├── cli/                  # clap subcommands per service (thin shims)
│           ├── cli.rs
│           ├── mcp/
│           │   ├── registry.rs       # runtime tool registration
│           │   ├── resources.rs      # action catalog as MCP resources
│           │   ├── error.rs          # structured JSON errors
│           │   └── services/         # one dispatch module per service
│           ├── mcp.rs
│           ├── api/                  # axum HTTP API
│           │   ├── state.rs          # AppState — Catalog + ToolRegistry (Arc-wrapped)
│           │   ├── error.rs          # ApiError + IntoResponse mapping
│           │   ├── router.rs         # build_router() + middleware stack
│           │   ├── health.rs         # /health + /ready endpoints
│           │   └── services/         # per-service route groups
│           ├── config.rs             # ~/.lab/.env + config.toml loading (CWD → ~/.lab/ → ~/.config/lab/)
│           └── output.rs             # table/json formatting
├── Cargo.toml                        # workspace
├── Justfile
├── deny.toml
├── docs/README.md
└── CLAUDE.md
```

### ACP SDK

The ACP SDK (`agent-client-protocol`) is consumed directly from crates.io at `=0.13.1` with the `unstable` feature. No local vendor patch is in use.

The key API used for model/config discovery is `session_config_options()` — it reads `SessionConfigOption` entries from the raw `NewSessionResponse` before `attach_session` consumes it. Session start bypasses `build_session().start_session()` and calls `send_request_to(Agent, NewSessionRequest::new(&*cwd))` directly to intercept the response. Model switching uses `SetSessionConfigOptionRequest::new(session_id, "model", model_id)`.

When upgrading: pin to an exact version (`=X.Y.Z`), verify the `unstable` feature still compiles, and re-check `session_config_options()` behavior against the new SDK's `SessionConfigOption` / `SessionConfigKind::Select` API.

## Key Patterns

### Per-Service Module Structure (in `labby-apis`)

Every service is a module under `crates/labby-apis/src/`:

```
foo.rs              # module declaration: pub mod client; pub mod types; pub mod error; pub const META: ...
foo/
├── client.rs       # FooClient with async methods — ALL business logic
├── types.rs        # Request/response types (serde)
└── error.rs        # Service-specific errors (thiserror)
```

Modern Rust module style: **no `mod.rs` files anywhere**. A module `foo` is declared in `foo.rs` (sibling to the `foo/` directory), not in `foo/mod.rs`.

Note: `commands.rs` and `tools.rs` do **not** live here. CLI subcommands and MCP dispatch live in the `labby` crate, never in `labby-apis`.

### The Golden Rule

Business logic lives in `labby-apis/src/<service>/client.rs`. Shared product semantics live in `crates/labby/src/dispatch/<service>/`. CLI, MCP, and HTTP are thin adapters over dispatch unless a surface has a genuine protocol-specific exception. If you're writing business logic in a CLI command, MCP handler, or API route, you're doing it wrong — move it to the client or shared dispatch layer.

The crate split enforces this structurally: `labby-apis` doesn't depend on `clap` or `rmcp`, so you literally cannot reach for them while writing business logic.

### One Tool Per Service (MCP) — action + subaction dispatch

Each service exposes exactly **one** MCP tool, named after the service. Operations dispatch via a flat dotted `action` string + free-form `params` object. This keeps total MCP tool count near the service count, not hundreds.

```jsonc
marketplace({ "action": "mcp.list", "params": { "search": "github", "limit": 10 } })
gateway({ "action": "gateway.list" })
marketplace({ "action": "help" })                        // built-in discovery
marketplace({ "action": "schema", "params": { "action": "mcp.install" } })  // per-action schema
```

- **Action naming:** `<resource>.<verb>`, lowercase, dot-separated.
- **Built-in actions:** every tool accepts `help` and `schema` without declaring them.
- **Discovery:** `lab://<service>/actions` MCP resource + `lab://catalog` resource.
- **Shared catalog.** `build_catalog()` is a single function feeding the `lab://catalog` MCP resource and the `lab help` CLI subcommand. Never duplicate catalog logic — extend the builder.
- **Multi-instance services.** When `{SERVICE}_{LABEL}_URL` env vars exist, callers pass `params.instance: "<label>"`. Unknown labels return a structured `unknown_instance` envelope listing valid labels.

### Destructive actions

`ActionSpec.destructive: bool` is the **single source of truth** for dangerous operations. It drives:

- **MCP:** elicitation — the dispatcher prompts the client to confirm before executing.
- **CLI:** requires `-y` / `--yes` to run non-interactively. `--no-confirm` and `--dry-run` are also honored.

Mark actions `destructive: true` whenever they delete, overwrite, spawn local processes, or push state that can't be trivially reversed (`gateway.test`, `gateway.remove`, `marketplace.mcp.install`, `stash.component.export`, etc.).

### Structured error envelopes

Every MCP tool failure returns a JSON envelope with a stable `kind` tag so agents can react programmatically:

```jsonc
{ "kind": "unknown_action", "message": "...", "valid": ["movie.search", ...], "hint": "movie.serch" }
{ "kind": "missing_param",  "message": "...", "param": "query" }
{ "kind": "unknown_instance", "message": "...", "valid": ["default", "node2"] }
{ "kind": "rate_limited", "message": "...", "retry_after_ms": 5000 }
```

See `docs/surfaces/MCP.md` for the MCP surface and `docs/CONVENTIONS.md` for the canonical error vocabulary rules.

`docs/dev/ERRORS.md` is the canonical source of truth for stable kinds, envelope expectations, and status mapping.

### Adding a New Service

1. `mkdir crates/labby-apis/src/foo/`
2. Define types in `types.rs` from API spec/docs
3. Implement `FooClient` methods in `client.rs`
4. Add observability at the shared boundary and confirm it matches `docs/dev/OBSERVABILITY.md`
5. Implement `ServiceClient` trait for health checks
6. Add `#[cfg(feature = "foo")] pub mod foo;` to `labby-apis/src/lib.rs`
7. Add `foo = []` feature to `crates/labby-apis/Cargo.toml`
8. Create the shared dispatch layer in `crates/labby/src/dispatch/foo/` following the required layout in `crates/labby/src/dispatch/CLAUDE.md` (catalog.rs, client.rs, params.rs, dispatch.rs + entry `foo.rs`)
9. Create CLI subcommands in `crates/labby/src/cli/foo.rs` calling the dispatch layer
10. Create API route group in `crates/labby/src/api/services/foo.rs` calling the dispatch layer
11. Register in `crates/labby/src/registry.rs` (via `register_service!` inside `build_default_registry()`), `crates/labby/src/cli.rs`, and `crates/labby/src/api/router.rs`
12. Add `foo = ["labby-apis/foo"]` passthrough to `crates/labby/Cargo.toml`

A service is not fully online until one successful path and one failing path are traceable end to end without leaking secrets.

### Auth

Use the `Auth` enum from `labby_apis::core`. Never hardcode auth handling in a service module.

```rust
use labby_apis::core::{Auth, HttpClient};

impl FooClient {
    pub fn new(base_url: &str, auth: Auth) -> Self {
        Self {
            http: HttpClient::new(base_url, auth),
        }
    }
}
```

### Config Loading

**`labby-apis` never reads files or env vars on its own.** Config loading lives entirely in `crates/labby/src/config.rs`. The library exposes optional `from_env()` helpers; the binary calls them.

Naming convention for env vars (read by `labby`, not `labby-apis`):
- `{SERVICE}_URL` — base URL
- `{SERVICE}_API_KEY` — API key (for ApiKey auth)
- `{SERVICE}_TOKEN` — token (for Token/Bearer auth)
- `{SERVICE}_USERNAME` / `{SERVICE}_PASSWORD` — credentials (for Basic auth)

**Multi-instance services:** append a label before the suffix — `UNRAID_URL` is the default instance, `UNRAID_NODE2_URL` / `UNRAID_NODE2_API_KEY` is an additional named instance `node2`. MCP callers select via `params.instance`; CLI selects via `--instance` or positional label. Never hardcode instance names — derive them from env at startup.

Loaded from `~/.lab/.env`. Product actions that mutate config or env files must use backup-first, atomic-write behavior and preserve unrelated keys/comments where the file format allows it.

### PluginMeta shape

Every service entry-point file that participates in generated metadata declares a `pub const META: PluginMeta` with:

- `category: Category` — one of 10 variants: `Media`, `Servarr`, `Indexer`, `Download`, `Notes`, `Documents`, `Network`, `Notifications`, `Ai`, `Bootstrap`.
- `required_env: &[EnvVar]` / `optional_env: &[EnvVar]` — each `EnvVar { name, description, example, secret }`. `secret: true` marks values to mask in logs, docs, and UI.
- `default_port: Option<u16>` — used by generated docs and doctor/setup hints.

### Error Handling

- `labby-apis`: use `thiserror` for typed errors per service; every service error wraps `ApiError` transparently.
- `lab` binary: use `anyhow` to wrap everything.
- Always return `Result<T>`, never panic.
- `docs/dev/ERRORS.md` is canonical for stable `kind` values, dispatcher-level kinds, MCP and HTTP envelope behavior, and status mapping.
- Do not invent service-local error vocabularies or drift MCP and HTTP error semantics apart.
- Adding or renaming an error `kind` is a spec change and must be reflected in the owning docs and surface code together.

### Logging

Use `tracing` everywhere. Never use `println!` for debug info.

`docs/dev/OBSERVABILITY.md` is the canonical source of truth. Do not invent per-service log shapes.

Minimum required rules:

- CLI, MCP, and HTTP dispatch must emit one structured dispatch event per user-visible action
- `HttpClient` must emit `request.start` and `request.finish` or `request.error` for every outbound request
- request logs must inherit caller context from the invoking surface
- health probes must be distinguishable from normal actions
- destructive actions must log intent and outcome
- secrets, auth headers, tokens, cookies, and secret env values must never be logged

**Standard dispatch fields** — all dispatch events must include these:

| Field | Type | Present when |
|-------|------|--------------|
| `surface` | `&str` | always |
| `service` | `&str` | always (MCP/HTTP/CLI dispatch) |
| `action` | `&str` | always |
| `elapsed_ms` | `u128` | always |
| `kind` | `&str` | errors only — from `ToolError::kind()` |

HTTP dispatch additionally carries `request_id` when available. Outbound request events carry `method`, `path`, `host`, and `status` on success.

**Level conventions:**
- `INFO` — successful dispatch
- `WARN` — user/caller errors (`missing_param`, `unknown_action`, `auth_failed`, etc.)
- `ERROR` — unhandled / fatal errors (panics, internal_error)

**Environment variables:**
- `LAB_LOG` — tracing filter directive (default: `labby=info,labby_apis=warn`)
- `LAB_LOG_FORMAT=json` — emit newline-delimited JSON (for prod/CI)
- `LAB_LOG_COLOR=force` — force ANSI colors even without a TTY (e.g. `docker compose logs -f`); also accepts `plain`/`never`/`0` to disable colors

ANSI colors are enabled only when `stderr` is a TTY (`std::io::stderr().is_terminal()`), or when `LAB_LOG_COLOR=force` is set.

The product API surface uses `surface = "api"` in dispatch logs. Keep docs, tests, and new instrumentation aligned with that label.

### Async trait style

Use **native `async fn in trait`** (stable in Rust 1.75+). Do **not** add the `async-trait` crate. Do **not** use `Box<dyn ServiceClient>` — prefer generics or concrete types. This is a hard rule; PRs that reintroduce `#[async_trait]` will be rejected.

### Output Formatting

All formatting lives in `crates/labby/src/output.rs`. `labby-apis` types are pure data.

`docs/design/SERIALIZATION.md` is the canonical source of truth for serde ownership, stable envelopes, and output boundaries.

- Support `--json` by serializing the underlying `labby-apis` type with `serde_json`
- Use `tracing` for debug/verbose output, never `println!` for debug info

## Tech Stack

| Crate | Purpose | Lives in |
|-------|---------|----------|
| tokio | async runtime | both |
| reqwest | HTTP client (rustls-tls) | labby-apis |
| serde + serde_json | serialization | labby-apis |
| thiserror | library errors | labby-apis |
| wiremock | HTTP mocking (tests) | labby-apis |
| clap | CLI parsing (derive) | lab |
| rmcp | MCP server | lab |
| dotenvy | .env loading | lab |
| toml | config parsing | lab |
| tracing | structured logging | lab |
| anyhow | binary errors | lab |

## Dev Commands

```bash
just check      # cargo check --workspace
just test       # cargo nextest run --workspace --all-features
just lint       # cargo clippy + cargo fmt --check
just deny       # cargo deny check
just build      # cargo build --workspace --all-features
just build-release  # cargo build --workspace --all-features --release
just run        # cargo run --all-features -- <args>
just fmt        # cargo fmt --all
just clean      # cargo clean
just mcp-token  # rotate the MCP bearer token in ~/.lab/.env
```

Releases: push a `vX.Y.Z` tag (after bumping the workspace `version` in
`Cargo.toml` and adding a CHANGELOG entry) — `release.yml` builds the
Linux/Windows archives, publishes the GitHub Release, and pushes the GHCR
image. There is no cargo-release config; the bump/tag is manual.

Default verification targets the all-features build. If you run a reduced feature set for a narrow task, treat any warning cleanup decisions from that mode as provisional until they are checked again with `--all-features`.

### Operator tooling

- **`labby doctor`** — comprehensive health audit: checks env vars, reachability, auth, version for every enabled service. Emits human-readable table by default, `--json` for CI. Exit code reflects worst severity.
- **`bin/health-check`** — repo-level shell helper for CI/CD smoke tests.

### Labby gateway runtime

The primary supported self-hosted Labby gateway runtime is an amd64 Debian 13
Incus system container. The host-side entrypoint is
`scripts/incus-bootstrap.sh --version vX.Y.Z`; the in-box converger is
`labby setup --provision`. The provision command is intentionally local
CLI-only and must not be exposed through MCP, HTTP, Code Mode, or remote admin
actions.

The default service is a hardened system unit at
`/etc/systemd/system/labby.service`, running `User=lab`, `Group=lab`, and
`ExecStart=/usr/local/bin/labby serve`. Do not reintroduce `systemd --user`,
linger, `%h` unit paths, or `~/.local/bin/labby` as the supported self-hosted
gateway service model. Preserve a user-service fallback only if it is explicit
and clearly non-default.

The Docker Compose stack is still supported only for explicit dev-container,
prod-like image smoke, and Docker-specific ACP adapter work. Use
`just dev-container` or `just dev-container-debug` when testing that path.

### Bearer auth in dev (driving the UI with agent-browser)

When OAuth is configured (`LAB_AUTH_MODE=oauth`), browser users still hit the Google login flow. Automation tooling (e.g. `agent-browser`, curl) can pass the static bearer token as a header and be treated as an admin session for both `/v1/*` API calls AND the AuthBootstrap session-state endpoint.

```bash
TOKEN=$(grep "LAB_MCP_HTTP_TOKEN" .env | cut -d= -f2)

# All /v1/* calls
curl -H "Authorization: Bearer $TOKEN" http://localhost:8765/v1/acp/provider

# /auth/session — returns synthetic admin session for the bearer holder.
# Without this the UI's AuthBootstrap renders the sign-in page even though
# the underlying API calls succeed.
curl -H "Authorization: Bearer $TOKEN" http://localhost:8765/auth/session

# agent-browser carries the header into every same-origin request.
agent-browser --session test set viewport 1280 800
agent-browser --session test open http://localhost:8765/chat \
  --headers "{\"Authorization\":\"Bearer $TOKEN\"}"
```

The bearer-via-`/auth/session` path returns `sub: "static-bearer"` so admin-gated UI is reachable. OAuth users see no behavior change — the cookie path is still primary.

Scoped to a single crate:

```bash
cargo nextest run -p labby-apis        # client tests only (fast, wiremock-based)
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features  # CLI/MCP/API tests
```

## Testing

- Unit tests: mock HTTP with `wiremock` in `labby-apis`, run in CI
- Integration tests: hit real services, run locally only (marked `#[ignore]`)
- Test runner: `cargo-nextest` (parallel execution)
- The authoritative test/build signal is the all-features workspace run, not a partial-feature slice
- If a helper or module looks unused in a reduced build, confirm with an all-features search/build before removing it

```bash
# Unit tests (CI-safe)
just test

# Integration tests (requires running services)
just test-integration
```

## CI

- GitHub Actions
- Matrix: linux x86_64
- Checks: clippy, rustfmt, cargo-deny, nextest
- Release: cargo-release → GitHub Releases with pre-built binaries (linux x86_64, linux aarch64)

## Style

- Rust 2024 edition, latest stable toolchain
- `cargo fmt` with default settings
- `cargo clippy` with no allowed warnings
- Treat all-features warnings as real; treat narrow feature-slice warnings as diagnostic only until confirmed in the normal all-features build
- Prefer `impl Trait` over `Box<dyn Trait>` where possible
- Prefer concrete types over generics unless sharing demands it
- Never add `clap`, `rmcp`, `axum`, or `anyhow` to `labby-apis` — they belong in product/runtime crates only
- **No `mod.rs` files.** Modern Rust module style only: a module `foo` is declared in `foo.rs` sibling to its `foo/` directory, never in `foo/mod.rs`

## Plugin setup hooks and install flow

Plugin setup is owned by the binary. `labby setup check` is read-only, `labby setup repair` is idempotent, and `labby setup plugin-hook --no-repair` is audit mode.

**The plugin ships no binary and never auto-installs.** Installation is explicit: `scripts/install.sh` (release download → `~/.local/bin/labby`, cargo fallback) or `cargo install`, then `labby setup` for the first-run flow. The checked-in `plugins/labby` hooks are advisory shims that resolve `labby` from `PATH`: SessionStart runs `labby setup plugin-hook --no-repair` (audit only) and prints an install pointer when labby is absent; ConfigChange runs `labby setup plugin-hook` to sync changed plugin settings. Keep hooks that shape — never re-bundle a binary into `plugins/labby/bin/`, reference `${CLAUDE_PLUGIN_ROOT}/bin/labby`, or make a hook install/repair anything at session start.

Do not add Docker Compose, systemd, or service bootstrap logic to plugin hook scripts.
