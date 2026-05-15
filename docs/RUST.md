---
title: "Rust Build Setup"
doc_type: "guide"
status: "active"
owner: "lab"
audience:
  - "contributors"
  - "agents"
scope: "service"
source_of_truth: false
upstream_refs:
  - "https://github.com/jmagar/rmcp-template/blob/main/docs/RUST.md"
last_reviewed: "2026-05-15"
---

# Rust Build Setup

This repo follows the build conventions of the rmcp server family.
The canonical reference is [rmcp-template/docs/RUST.md](https://github.com/jmagar/rmcp-template/blob/main/docs/RUST.md).

## System prerequisites

- Rust stable ≥ 1.86 (`rustup update stable`)
- `clang` and `mold` for fast Linux builds: `apt install clang mold`
- `just` command runner (optional): `cargo install just`
- `sccache` (optional, if using distributed build caching): `cargo install sccache`

## Global Cargo config

Build performance depends on `~/.cargo/config.toml` on the developer's machine.
See [rmcp-template/docs/RUST.md](https://github.com/jmagar/rmcp-template/blob/main/docs/RUST.md)
for the expected config (mold linker, profile settings, Cranelift backend).

## Local `.cargo/config.toml`

This repo's `.cargo/config.toml` has one intentional override:

```toml
[build]
incremental = false
```

**Why:** `lab` supports sccache for distributed build caching. sccache and
Rust incremental compilation are mutually exclusive — sccache cannot cache
incremental artifacts because they are non-deterministic. The global
`~/.cargo/config.toml` sets `incremental = true` for normal dev; this file
overrides it so developers who have sccache configured get correct caching
behaviour.

Developers without sccache take no penalty from this override — Rust simply
recompiles changed crates in full rather than using incremental fragments.

This repo has no xtask crate, so no `[alias]` section is needed.
