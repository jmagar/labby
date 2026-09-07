---
title: "Technology and Rust Build"
created: "2026-07-30"
updated: "2026-08-18"
---

# Technology and Rust Build

This document is the canonical technology and Rust build reference for Labby.
For operational deployment details, see [OPERATIONS.md](./OPERATIONS.md).

## Workspace Baseline

The workspace metadata in the root `Cargo.toml` is authoritative:

- Rust edition 2024
- pinned build toolchain and MSRV: Rust 1.97.1
- Cargo resolver 3
- workspace version shared by the Rust crates
- AGPL-3.0-only license
- release targets: Linux x86_64 GNU, macOS arm64, and Windows x86_64 MSVC

`rust-toolchain.toml` pins the toolchain used locally and in CI. The matching
`rust-version` in `Cargo.toml` is the minimum version Cargo will accept.

## Core Runtime

| Concern | Current choice |
| --- | --- |
| async runtime | `tokio` |
| concurrency helpers | `futures` |
| HTTP client | `reqwest` with rustls |
| TLS provider | `rustls` with `ring` |
| URL handling | `url::Url` |
| serialization | `serde`, `serde_json`, `serde_yaml_ng`, `toml` |
| library errors | `thiserror` |
| application errors | `anyhow` |
| time | `jiff` |
| logging | `tracing`, `tracing-subscriber` |
| embedded state | `rusqlite` |

`url::Url` is the canonical URL type once an endpoint has been parsed and
validated. Do not carry validated service endpoints as raw strings.

## Product Surfaces

| Concern | Current choice |
| --- | --- |
| CLI | `clap` |
| MCP | pinned `rmcp` Git revision `0665dcac` |
| HTTP/WebSocket | `axum`, `tower`, `tower-http`, `tokio-tungstenite` |
| OpenAPI | `utoipa` |
| CLI color | `owo-colors` |
| TTY detection | `is-terminal` |
| progress | `indicatif` |
| web app | Next.js 16, React 19, Tailwind CSS 4 |
| desktop palette | Tauri 2 + React |

Labby does **not** currently ship a Ratatui TUI. Historical plugin-manager TUI
references are retired rather than treated as a supported surface.

## Workspace Crate Boundaries

The current Rust workspace is composed of:

- `labby` — product binary and surface composition
- `labby-apis` — small shared API/core contracts for doctor and setup
- `labby-auth` — inbound and upstream OAuth/authentication primitives
- `labby-codemode` — Code Mode runtime contracts and snippet support
- `labby-gateway` — surface-neutral gateway, upstream MCP, relay, and discovery runtime
- `labby-openapi` — reusable OpenAPI helpers
- `labby-primitives` — leaf metadata and security primitives
- `labby-runtime` — reusable runtime/config/skills contracts
- `labby-web` — embedded static web assets and resolution helpers
- `labby-winjob` — Windows process containment and verified filesystem primitives
- `xtask` — repository automation

The binary should compose these crates; leaf/shared crates should not reach back
into product surface code.

## Feature Gating

`labby` owns product feature slices. `labby-apis` has no product feature matrix
and keeps only empty compatibility aggregates plus test utilities.

Current `labby` rules:

- `default = ["gateway-host"]`
- `all` enables `lab-admin`, `api-docs`, `gateway-host`, `fs`, `systemd`, and `skills`
- `gateway-host` composes gateway support with the embedded web UI
- `skills` enables Agent Skills over MCP support where the gateway is present
- `doctor`, `server_logs`, `setup`, and `snippets` are always-on services
- retired ACP, Registry-browser, Marketplace, Fleet, Deploy-product, and Agent
  Artifact Manager feature names are not compatibility aliases; the approved
  principal-scoped File Stash is current default `gateway-host` functionality
  on Linux, not a revival of the retired `stash` Cargo feature

The generated [feature matrix](./generated/feature-matrix.md) is authoritative
for the exact current Cargo feature projection.

## Build Prerequisites

Required locally:

- the toolchain from `rust-toolchain.toml`
- a C/C++ linker toolchain appropriate to the host
- `just` for the repository task runner when using Justfile workflows

Managed development hosts may provide additional compiler/linker acceleration.
The repository itself does not install a compiler wrapper during a build.

## Repository Cargo Configuration

`.cargo/config.toml` intentionally disables incremental compilation:

```toml
[build]
incremental = false
```

This keeps compiler-cache inputs deterministic across managed and unmanaged
hosts. The host configuration owns any `rustc-wrapper` such as Kache.

Changing source code never refreshes an installed Labby binary as a side effect.
Use explicit install/sync tasks such as `just build-release`, `just install`, or
`just host-sync` when the installed binary should change.

## Kache Troubleshooting

On managed hosts, Kache may be configured as the Cargo `rustc-wrapper`. A cache
failure can degrade performance without failing a build, so a green build is not
evidence that the cache is healthy.

Useful diagnostics:

```bash
kache doctor
kache doctor --verify --repair
kache why-miss <crate>
kache stats
```

For a one-off verification that bypasses the configured cache:

```bash
KACHE_DISABLED=1 cargo build --workspace --all-features
RUSTC_WRAPPER="" cargo build --workspace --all-features
```

Reach for a cache wipe only after diagnostics and repair fail. Cache corruption
and stale source are different problems; preserve evidence before deleting it.

## Testing and Quality

| Concern | Current choice |
| --- | --- |
| unit HTTP mocking | `wiremock` |
| snapshots | `insta` |
| test runner | `cargo-nextest` |
| linting | `clippy`, `rustfmt`, `cargo-deny` |
| task runner | `just` |
| CI | GitHub Actions |

Primary repository checks:

```bash
just check
just test
just lint
just deny
just docs-check
just rustdoc-check
```

### Rustdoc contract

`just rustdoc` builds the complete workspace HTML documentation with all Cargo
features enabled, dependencies omitted, private items rendered, and the workspace
binary/example targets included. `just rustdoc-check` additionally runs all
workspace doctests.
Canonical workspace/default-target HTML lives under `target/doc/`; the stdio MCP fixture and Rust examples live under `target/rustdoc-extra/doc/`. The six-line `labby` launcher delegates directly to `labby::run()` and is compile-tested rather than separately published because Cargo cannot emit same-named library/binary Rustdoc without an output collision (Cargo issue #6313).

The workspace denies missing crate-level documentation. The strict Rustdoc gate
also promotes the configured Rustdoc warning families to errors, covering
broken/private intra-doc links, invalid code blocks or HTML, bare URLs,
redundant explicit links, and unescaped backticks. `just rustdoc-audit`
uses a force-warning pass to inventory missing public API prose without turning
the product crate's historical coverage debt into a blocker for unrelated
changes. CI runs the strict Rustdoc correctness build in its own lane and
uploads both Rustdoc trees as the `rustdoc-html` artifact for inspection.

Useful scoped checks:

```bash
cargo test -p labby-apis
cargo nextest run -p labby --all-features
RUSTDOCFLAGS="-D warnings" cargo doc -p labby-runtime --all-features --no-deps --document-private-items
```

CI additionally validates generated docs, strict Rustdoc/doctests, frontend
assets, the web app, package artifacts, security policy, release paths, and
platform-specific jobs according to the changed-path classifier.

## Release Tooling

GitHub Actions builds release artifacts for the supported targets and publishes
GitHub Releases. Release automation and packaging are described in
[runtime/CICD.md](./runtime/CICD.md).

## Product Rule

Labby does not add analytics or telemetry phone-home behavior. Operational
tracing and local usage records are observability features, not third-party
analytics.
