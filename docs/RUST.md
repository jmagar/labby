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

## sccache troubleshooting (cache "poisoning")

sccache is optional but, when configured here, runs as a **long-lived systemd
user service** (`~/.config/systemd/user/sccache.service`, `Restart=always`)
backed by a **mise-pinned binary** behind the stable symlink `~/.local/sccache`.
The cargo `rustc-wrapper` (`~/.local/bin/sccache-wrapper`) resolves the active
rustup toolchain before handing off to sccache.

**Symptom — "poisoning":** builds produce stale or wrong artifacts (code you
deleted still seems present, link errors that don't match source, nondeterministic
failures). The cache is returning artifacts that don't match the current inputs.

**Likely causes, highest first:**

1. **Distributed compilation across mismatched machines.** If `~/.config/sccache/config`
   has a `[dist]` section, compiles are farmed to remote build servers. Artifacts
   built on a remote host with a different toolchain/glibc/linker than the local
   host, then cached and linked locally, are the prime poisoning vector — made
   worse when the dist transport is flaky (e.g. self-signed-cert TLS failures cause
   an inconsistent mix of local and remote artifacts). Check the error log for
   `Could not perform distributed compile` / `certificate verify failed`. Fix:
   either disable `[dist]` (local-only) or ensure every scheduler/build-server/client
   runs an **identical** pinned toolchain with valid certs.
2. **Long-lived daemon + swapped binary.** The systemd daemon does not reload when
   the mise-pinned sccache binary changes. After `mise upgrade`/re-pin, the running
   daemon and the client binary can disagree on cache format. Always
   `just sccache-restart` after the binary changes.
3. **On-disk corruption** (interrupted writes, disk pressure) — least likely;
   only this one needs a cache wipe.

**Recovery — tiered, least-destructive first:**

```bash
just sccache-doctor      # diagnose: daemon health, dist config, errors, stats
just sccache-restart     # fixes daemon-state/version-skew causes (no wipe)
# only if restart doesn't clear it — wipe on-disk artifacts:
systemctl --user stop sccache.service && rm -rf ~/.cache/sccache && systemctl --user start sccache.service
SCCACHE_RECACHE=1 cargo build   # force-overwrite suspect entries without a full wipe
```

**Bypass for a single build** (e.g. release/CI-parity verification you don't want
to trust the cache for):

```bash
cargo build --workspace --all-features --config 'build.rustc-wrapper=""'
```

**Logging:** the daemon writes to `~/.local/state/sccache/error.log`
(`SCCACHE_ERROR_LOG` in the unit). Keep `SCCACHE_LOG=warn` — a `debug` level turns
this file into multi-GB noise that buries real errors and grows unbounded. The
`sccache.service.d/` drop-in directory holds level overrides; remove the debug
drop-in when you're done debugging.
