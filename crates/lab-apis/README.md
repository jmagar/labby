# lab-apis

Last updated: 2026-04-09

Pure Rust capability layer for homelab services and synthetic modules. **Zero binary dependencies** — reusable in any Rust project.

```
tokio | reqwest | serde | thiserror
```

## What It Is

Core library for homelab capability modules: HTTP-backed integrations such as Radarr, Sonarr, Prowlarr, Plex, qBittorrent, UniFi, Unraid, Overseerr, and Tailscale.

HTTP-backed services expose typed async clients with request/response types and structured error handling. Non-HTTP capability modules follow the same crate-level contracts when they are reusable outside the product binary.

Designed to be a library — not a binary, not an MCP server. Use it in your own Rust projects.

## Installation

```toml
[dependencies]
lab-apis = { version = "0.6", features = ["radarr", "sonarr"] }

# or all services:
lab-apis = { version = "0.6", features = ["all"] }
```

## Quick Start

```rust
use lab_apis::radarr::RadarrClient;
use lab_apis::core::Auth;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = RadarrClient::new(
        "http://localhost:7878",
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "your-api-key".into(),
        },
    )?;

    let movies = client.get_movies().await?;
    println!("Found {} movies", movies.len());
    Ok(())
}
```

## Core Abstractions

### HttpClient
Shared HTTP wrapper around `reqwest::Client`. Hardcoded timeouts: 5s connect, 30s request. TLS via rustls. Fallible constructor — TLS init failure returns `ApiError::Internal`.

`HttpClient` is required for HTTP-backed modules. Non-HTTP capability modules are not required to use it.

### Auth Enum
```rust
pub enum Auth {
    None,
    ApiKey { header: String, key: String },
    Token { token: String },
    Bearer { token: String },
    Basic { username: String, password: String },
    Session { cookie: String },
}
```

Header name for `ApiKey` is caller-chosen. Most *arr services use `X-Api-Key`.

### ServiceClient Trait
Every service or capability module implements a health check:
```rust
pub trait ServiceClient: Send + Sync {
    fn name(&self) -> &'static str;
    fn service_type(&self) -> &'static str;
    async fn health(&self) -> Result<ServiceStatus, ApiError>;
}
```

### Error Handling
All errors wrap to `ApiError` with stable `kind()` tags for programmatic handling:
- `auth_failed` — auth headers rejected
- `not_found` — 404
- `rate_limited` — 429, includes `retry_after` if available
- `validation_failed` — request validation error
- `network_error` — connectivity issue
- `server_error` — 5xx response
- `decode_error` — response parsing failed
- `internal_error` — unhandled error

See `docs/ERRORS.md` in the monorepo for the canonical error vocabulary.

## Service Status

| Status | Services |
|--------|----------|
| **Fully Implemented** | Radarr, UniFi, ByteStash |
| **Partially Implemented** | OpenAI (Chat, Embeddings), Overseerr (search, request management) |
| **Client Stub** | Sonarr, Prowlarr, Plex, Tautulli, SABnzbd, qBittorrent, Tailscale, Linkding, Memos, Arcane, Gotify, Qdrant, TEI, Apprise |
"Stub" means the client struct exists and compiles, but methods are minimal or unimplemented. Contributions welcome.

## Module Structure

Every service follows:
```
foo.rs              # pub const META: PluginMeta, ServiceClient impl
foo/
  client.rs         # FooClient or equivalent capability entrypoint
  types.rs          # Request/response or input/output types (serde)
  error.rs          # Service-specific error enum (thiserror)
```

No `mod.rs` files — modern Rust 1.56+ module style.

Large modules (Radarr, OpenAI, and future non-HTTP capability modules) must organize methods into sub-modules rather than accumulating a monolithic `client.rs`.

## Metadata

Every service exports `pub const META: PluginMeta`:
```rust
pub struct PluginMeta {
    pub name: &'static str,
    pub category: Category,
    pub required_env: &'static [EnvVar],
    pub optional_env: &'static [EnvVar],
    pub default_port: Option<u16>,
}

pub struct EnvVar {
    pub name: &'static str,
    pub description: &'static str,
    pub example: &'static str,
    pub secret: bool,  // true = mask in logs/TUI
}
```

Categories: `Media`, `Servarr`, `Indexer`, `Download`, `Notes`, `Documents`, `Network`, `Notifications`, `Ai`, `Bootstrap`.

## Config Loading

**`lab-apis` never reads files or environment variables.** Config lives entirely in the binary (`lab/src/config.rs`). The library exposes `Auth::from_env()` helpers; the binary calls them.

Standard env var naming:
- `{SERVICE}_URL` — base URL
- `{SERVICE}_API_KEY` — API key
- `{SERVICE}_TOKEN` — bearer token
- `{SERVICE}_USERNAME` / `{SERVICE}_PASSWORD` — Basic auth

Multi-instance services append a label: `RADARR_URL` (default), `RADARR_NODE2_URL` (instance `node2`).

Non-HTTP modules may not use these env keys at all. Their config still must be supplied by the caller rather than read implicitly inside `lab-apis`.

## Feature Flags

Optional service integrations are exposed as opt-in features. `core` always compiles.

```rust
radarr, sonarr, prowlarr     // pulls in shared "servarr" types automatically
overseerr, plex, tautulli, sabnzbd, qbittorrent, tailscale
linkding, memos, bytestash, arcane, unraid, unifi
gotify, openai, qdrant, tei, apprise
```

Convenience: `features = ["all"]` enables all service integrations.

## Testing

HTTP-backed unit tests use `wiremock` for HTTP mocking:
```bash
cargo test -p lab-apis
```

Live integration tests (marked `#[ignore]`) require real services or real capability environments:
```bash
cargo test -p lab-apis -- --ignored --nocapture
```

## Invariants

- **No `clap`, `rmcp`, `ratatui`, `anyhow`, `tabled`** — they belong in the `lab` binary only
- **No ambient file or env config I/O** — the caller supplies values and paths explicitly
- **Native async fn in trait** (Rust 1.75+) — no `#[async_trait]`, no `Box<dyn>`
- **Debug impls redact secrets** — test this for any new auth variant
- **TLS is always rustls** for HTTP-backed modules — hardcoded, not configurable

## Architecture

For full context, see the monorepo docs:
- `docs/README.md` — architecture index
- `docs/ARCH.md` — crate boundaries and service model
- `docs/ERRORS.md` — error taxonomy and stability guarantees
- `docs/OBSERVABILITY.md` — logging and tracing rules
- `docs/SERIALIZATION.md` — serde ownership and output boundaries
- `docs/TESTING.md` — testing and TDD contract
- `CLAUDE.md` (this directory) — per-crate development rules

## License

Same as the monorepo. See `LICENSE` at the repository root.
