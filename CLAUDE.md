# Lab — Development Instructions

## What is this?

`lab` is a pluggable homelab CLI + MCP server SDK in Rust. One binary, feature-gated upstream integrations plus always-on operator tools like `gateway`, `logs`, `device`, `marketplace`, `acp`, `extract`, and `stash`; MCP dispatch still uses a single tool per runtime service with an `action` + `params` shape instead of hundreds of per-method tools.

Start with `docs/README.md` for the docs index. The topic docs in `docs/` are the source of truth; if this file disagrees with them, this file is stale.

Observability is governed by `docs/dev/OBSERVABILITY.md`. When adding or changing request paths, treat that file as the source of truth for logging boundaries, required fields, correlation, redaction, and verification.
Errors are governed by `docs/dev/ERRORS.md`. Serialization and output-boundary rules are governed by `docs/design/SERIALIZATION.md`.
Shared dispatch ownership and adapter direction are governed by `docs/dev/DISPATCH.md`.

**Build assumption.** This repo is developed and verified as an **all-features** binary. Treat `cargo build --all-features`, `cargo nextest run --all-features`, and the equivalent `just` commands as the default truth. Do not delete or rewrite shared helpers just because they appear unused in a narrow feature slice; first verify whether they are used by other feature-gated services in the normal all-features build.

**Service onboarding rule.** When bringing a service online, prefer scaffold first, audit second, and all-features verification last. New onboarding work should be generated with `labby scaffold service`, checked with `labby audit onboarding`, and only then validated with the all-features test/build path.

**Nested guides.** Subdirectories carry their own `CLAUDE.md` with rules that don't belong at the root. Read the nearest one when working in:
- `crates/lab-apis/src/core/` — trait contracts, error taxonomy, HttpClient invariants
- `crates/lab-apis/src/servarr/` — shared *arr primitives
- `crates/lab-apis/src/extract/` — synthetic-service rules, `.env` merge algorithm
- `crates/lab/src/dispatch/` — shared dispatch layer, required service layout, canonical templates
- `crates/lab/src/dispatch/upstream/` — upstream MCP proxy pool, circuit breaker, layer contract
- `crates/lab/src/mcp/` — dispatch, envelopes, elicitation, catalog
- `crates/lab/src/cli/` — thin-shim pattern, destructive flags, batch commands
- `crates/lab/src/tui/` — plugin manager UX, `.mcp.json` patching
- `crates/lab/src/api/` — axum HTTP surface, status code mapping, middleware stack

## Repository Structure

Two crates. Pure API clients live in `lab-apis`. Everything else (CLI, MCP, TUI, binary) lives in `lab`.

```
lab/
├── crates/
│   ├── lab-apis/                     # PURE Rust SDK — reusable in any binary
│   │   ├── Cargo.toml                # deps: reqwest, serde, thiserror, tokio
│   │   └── src/
│   │       ├── lib.rs                # re-exports, feature gates
│   │       ├── core/                 # HttpClient, Auth, errors, traits
│   │       ├── servarr/              # shared *arr primitives
│   │       ├── radarr/               # { client.rs, types.rs, error.rs }
│   │       ├── sonarr/
│   │       ├── prowlarr/
│   │       ├── plex/
│   │       ├── tautulli/
│   │       ├── sabnzbd/
│   │       ├── qbittorrent/
│   │       ├── tailscale/
│   │       ├── linkding/
│   │       ├── memos/
│   │       ├── bytestash/
│   │       ├── paperless/
│   │       ├── arcane/                # Docker management UI
│   │       ├── unraid/                # Unraid GraphQL API
│   │       ├── unifi/                 # UniFi Network Application local API
│   │       ├── overseerr/              # Media request manager
│   │       ├── gotify/                # Push notifications
│   │       ├── openai/                # OpenAI API (+ OpenAI-compatible)
│   │       ├── qdrant/                # Vector database
│   │       ├── tei/                   # HF Text Embeddings Inference
│   │       ├── apprise/               # Universal notification dispatcher
│   │       ├── mcpregistry/           # MCP Registry v0.1 (server discovery + install)
│   │       ├── deploy/                # Deployment/runner primitives
│   │       ├── device_runtime/        # ALWAYS-ON: local device runtime introspection
│   │       └── extract/                # ALWAYS-ON synthetic service: scan local/SSH hosts for service creds
│   │
│   └── lab/                          # BINARY: cli + mcp + tui + main
│       ├── Cargo.toml                # deps: lab-apis, clap, rmcp, ratatui, anyhow, tabled
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
│           ├── api/                  # axum HTTP API (mirrors MCP action dispatch)
│           │   ├── state.rs          # AppState — Catalog + ToolRegistry (Arc-wrapped)
│           │   ├── error.rs          # ApiError + IntoResponse mapping
│           │   ├── router.rs         # build_router() + middleware stack
│           │   ├── health.rs         # /health + /ready endpoints
│           │   └── services/         # per-service route groups (feature-gated)
│           ├── tui/                  # ratatui plugin manager
│           ├── tui.rs
│           ├── config.rs             # ~/.lab/.env + config.toml loading (CWD → ~/.lab/ → ~/.config/lab/)
│           └── output.rs             # table/json formatting
├── Cargo.toml                        # workspace
├── Justfile
├── deny.toml
├── crates/vendor/
│   └── agent-client-protocol/        # vendored fork of agent-client-protocol (see Vendored ACP SDK below)
├── docs/README.md
└── CLAUDE.md
```

### Vendored ACP SDK

`crates/vendor/agent-client-protocol/` is a local fork of `agent-client-protocol` 0.11.1 with one patch: `ActiveSession` preserves the `models: Option<SessionModelState>` field that upstream's `attach_session()` was discarding. Without the patch, `session.response()` rebuilds a `NewSessionResponse` without the model list and the chat UI's adapter model picker stays empty even when codex/claude/gemini advertise models in their `NewSessionResponse`.

Wired via `[patch.crates-io]` in the workspace `Cargo.toml`. Drop the patch when upstream gains a public accessor for session models on `ActiveSession` — the change is small enough that an upstream PR is the right long-term fix. Don't edit this directory casually; if you need to bump the underlying SDK version, copy `~/.cargo/registry/src/index.crates.io-*/agent-client-protocol-<VER>` over the directory, re-apply the same three deltas in `src/session.rs` (struct field, `attach_session`, `response()`, and the proxy-mode destructure stub), and re-run `just build`.

## Key Patterns

### Per-Service Module Structure (in `lab-apis`)

Every service is a module under `crates/lab-apis/src/`:

```
foo.rs              # module declaration: pub mod client; pub mod types; pub mod error; pub const META: ...
foo/
├── client.rs       # FooClient with async methods — ALL business logic
├── types.rs        # Request/response types (serde)
└── error.rs        # Service-specific errors (thiserror)
```

Modern Rust module style: **no `mod.rs` files anywhere**. A module `foo` is declared in `foo.rs` (sibling to the `foo/` directory), not in `foo/mod.rs`.

Note: `commands.rs` and `tools.rs` do **not** live here. CLI subcommands and MCP dispatch live in the `lab` crate, never in `lab-apis`.

### The Golden Rule

Business logic lives in `lab-apis/src/<service>/client.rs`. Shared product semantics live in `crates/lab/src/dispatch/<service>/`. CLI, MCP, and HTTP are thin adapters over dispatch unless a surface has a genuine protocol-specific exception. If you're writing business logic in a CLI command, MCP handler, or API route, you're doing it wrong — move it to the client or shared dispatch layer.

The two-crate split enforces this structurally: `lab-apis` doesn't depend on `clap` or `rmcp`, so you literally cannot reach for them while writing business logic.

### One Tool Per Service (MCP) — action + subaction dispatch

Each service exposes exactly **one** MCP tool, named after the service. Operations dispatch via a flat dotted `action` string + free-form `params` object. This keeps total MCP tool count near the service count, not hundreds.

```jsonc
radarr({ "action": "movie.search", "params": { "query": "The Matrix" } })
radarr({ "action": "queue.list" })
radarr({ "action": "help" })                        // built-in discovery
radarr({ "action": "schema", "params": { "action": "movie.add" } })  // per-action schema
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

Mark actions `destructive: true` whenever they delete, overwrite, or push state that can't be trivially reversed (`extract.apply`, `radarr.movie.delete`, `sabnzbd.queue.purge`, etc.).

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

1. `mkdir crates/lab-apis/src/foo/`
2. Define types in `types.rs` from API spec/docs
3. Implement `FooClient` methods in `client.rs`
4. Add observability at the shared boundary and confirm it matches `docs/dev/OBSERVABILITY.md`
5. Implement `ServiceClient` trait for health checks
6. Add `#[cfg(feature = "foo")] pub mod foo;` to `lab-apis/src/lib.rs`
7. Add `foo = []` feature to `crates/lab-apis/Cargo.toml`
8. Create the shared dispatch layer in `crates/lab/src/dispatch/foo/` following the required layout in `crates/lab/src/dispatch/CLAUDE.md` (catalog.rs, client.rs, params.rs, dispatch.rs + entry `foo.rs`)
9. Create CLI subcommands in `crates/lab/src/cli/foo.rs` calling the dispatch layer
10. Create API route group in `crates/lab/src/api/services/foo.rs` calling the dispatch layer
11. Register in `crates/lab/src/registry.rs` (via `register_service!` inside `build_default_registry()`), `crates/lab/src/cli.rs`, and `crates/lab/src/api/router.rs`
12. Add `foo = ["lab-apis/foo"]` passthrough to `crates/lab/Cargo.toml`
13. Confirm TUI coverage: service listing comes from the shared registry, but health/config helpers in `crates/lab/src/tui/metadata.rs` and `crates/lab/src/tui/services.rs` may need service-specific entries.

A service is not fully online until one successful path and one failing path are traceable end to end without leaking secrets.

### Auth

Use the `Auth` enum from `lab_apis::core`. Never hardcode auth handling in a service module.

```rust
use lab_apis::core::{Auth, HttpClient};

impl FooClient {
    pub fn new(base_url: &str, auth: Auth) -> Self {
        Self {
            http: HttpClient::new(base_url, auth),
        }
    }
}
```

### Config Loading

**`lab-apis` never reads files or env vars on its own.** Config loading lives entirely in `crates/lab/src/config.rs`. The library exposes optional `from_env()` helpers; the binary calls them.

Naming convention for env vars (read by `lab`, not `lab-apis`):
- `{SERVICE}_URL` — base URL
- `{SERVICE}_API_KEY` — API key (for ApiKey auth)
- `{SERVICE}_TOKEN` — token (for Token/Bearer auth)
- `{SERVICE}_USERNAME` / `{SERVICE}_PASSWORD` — credentials (for Basic auth)

**Multi-instance services:** append a label before the suffix — `UNRAID_URL` is the default instance, `UNRAID_NODE2_URL` / `UNRAID_NODE2_API_KEY` is an additional named instance `node2`. MCP callers select via `params.instance`; CLI selects via `--instance` or positional label. Never hardcode instance names — derive them from env at startup.

Loaded from `~/.lab/.env`. **`extract.apply` writes to this file** using a strict merge algorithm (backup first, atomic write, dedupe by key, preserve order and comments, default conflict policy is skip-and-warn, `--force` overwrites). See `crates/lab-apis/src/extract/CLAUDE.md`.

### PluginMeta shape

Every service entry-point file (e.g., `radarr.rs`) declares a `pub const META: PluginMeta` with:

- `category: Category` — one of 10 variants: `Media`, `Servarr`, `Indexer`, `Download`, `Notes`, `Documents`, `Network`, `Notifications`, `Ai`, `Bootstrap`.
- `required_env: &[EnvVar]` / `optional_env: &[EnvVar]` — each `EnvVar { name, description, example, secret }`. `secret: true` marks values to mask in TUI/logs.
- `default_port: Option<u16>` — used by `labby doctor` and the TUI for hints.

### Error Handling

- `lab-apis`: use `thiserror` for typed errors per service; every service error wraps `ApiError` transparently.
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
- `LAB_LOG` — tracing filter directive (default: `labby=info,lab_apis=warn`)
- `LAB_LOG_FORMAT=json` — emit newline-delimited JSON (for prod/CI)
- `LAB_LOG_COLOR=force` — force ANSI colors even without a TTY (e.g. `docker compose logs -f`); also accepts `plain`/`never`/`0` to disable colors

ANSI colors are enabled only when `stderr` is a TTY (`std::io::stderr().is_terminal()`), or when `LAB_LOG_COLOR=force` is set.

The product API surface uses `surface = "api"` in dispatch logs. Keep docs, tests, and new instrumentation aligned with that label.

### Async trait style

Use **native `async fn in trait`** (stable in Rust 1.75+). Do **not** add the `async-trait` crate. Do **not** use `Box<dyn ServiceClient>` — prefer generics or concrete types. This is a hard rule; PRs that reintroduce `#[async_trait]` will be rejected.

### Output Formatting

All formatting lives in `crates/lab/src/output.rs`. `lab-apis` types are pure data.

`docs/design/SERIALIZATION.md` is the canonical source of truth for serde ownership, stable envelopes, and output boundaries.

- Derive `Tabled` on wrapper types in `lab` (not on `lab-apis` types — keeps `tabled` out of the SDK)
- Support `--json` by serializing the underlying `lab-apis` type with `serde_json`
- Use `tracing` for debug/verbose output, never `println!` for debug info

## Tech Stack

| Crate | Purpose | Lives in |
|-------|---------|----------|
| tokio | async runtime | both |
| reqwest | HTTP client (rustls-tls) | lab-apis |
| serde + serde_json | serialization | lab-apis |
| thiserror | library errors | lab-apis |
| wiremock | HTTP mocking (tests) | lab-apis |
| clap | CLI parsing (derive) | lab |
| rmcp | MCP server | lab |
| ratatui + crossterm | TUI | lab |
| tabled | table rendering | lab |
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
just release    # cargo release
just mcp-token  # rotate the MCP bearer token in ~/.lab/.env
```

Default verification targets the all-features build. If you run a reduced feature set for a narrow task, treat any warning cleanup decisions from that mode as provisional until they are checked again with `--all-features`.

### Operator tooling

- **`labby doctor`** — comprehensive health audit: checks env vars, reachability, auth, version for every enabled service. Emits human-readable table by default, `--json` for CI. Exit code reflects worst severity.
- **`bin/health-check`** — repo-level shell helper for CI/CD smoke tests.

### Docker dev container

`docker-compose.yml` + `docker-compose.dev.yml` run `labby:dev` with the host's `~/.lab/`, `~/.gemini/`, and the repo workspace bind-mounted in. The image at `config/Dockerfile.fast` pre-installs the three ACP adapters (`claude-agent-acp`, `codex-acp`, `gemini`) into `/opt/acp-adapters/node_modules` and symlinks them into `/usr/local/bin/`, so each chat session spawn calls a deterministic local binary instead of paying the `npx -y` round-trip. The provider config at `config/acp-providers.docker.json` therefore uses `command: "claude-agent-acp"` (etc.) directly.

The Claude SDK is held forward of `claude-agent-acp`'s pinned version via an `overrides` entry in `/opt/acp-adapters/package.json` (currently `^0.2.131`). The bundled Claude Code binary version must match credential format expectations from the host's `claude` CLI, otherwise the underlying binary `SIGILL`s on session start. Bump both when upgrading.

`just dev-debug` rebuilds the labby binary with nightly + cranelift codegen and hot-swaps it into the running container without rebuilding the Docker image. Image rebuilds are only needed when changing `Dockerfile.fast` or the pre-installed package set.

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
cargo nextest run -p lab-apis        # client tests only (fast, wiremock-based)
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features  # CLI/MCP/TUI tests
```

## Testing

- Unit tests: mock HTTP with `wiremock` in `lab-apis`, run in CI
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
- Never add `clap`, `rmcp`, `ratatui`, `anyhow`, or `tabled` to `lab-apis` — they belong in `lab` only
- **No `mod.rs` files.** Modern Rust module style only: a module `foo` is declared in `foo.rs` sibling to its `foo/` directory, never in `foo/mod.rs`

## Plugin setup hooks

Plugin setup is owned by the binary. `labby setup check` is read-only, `labby setup repair` is idempotent, and `labby setup plugin-hook --no-repair` is audit mode. If a lab plugin hook is added later, keep it as a thin adapter that maps `CLAUDE_PLUGIN_OPTION_*` values to environment variables, prepares appdata, ensures `labby` is on `PATH`, and then calls `labby setup plugin-hook "$@"`.

Do not add Docker Compose, systemd, or service bootstrap logic to plugin hook scripts.
