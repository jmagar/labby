# Session: Marketplace Host Validation with OAuth

Date: 2026-05-07
Repo: `/home/jmagar/workspace/lab`
Branch: `main`
Head: `57805b99`

## Prompt

The operator pasted Docker logs showing repeated warnings:

```text
WARN rejecting non-loopback Host header kind=host_validation_failed host=lab.example.com path=/ surface=api
```

They asked why Labby was rejecting a non-loopback Host header when OAuth was enabled, and whether it was a CORS issue.

## Diagnosis

This was not CORS. The request was being rejected before normal request handling by the API DNS-rebinding guard in `crates/lab/src/api/host_validation.rs`.

The relevant route was `/v1/marketplace`; under Axum nesting the middleware logs the stripped path as `/`, which made the log look less specific than the actual mounted route. `crates/lab/src/api/router.rs` mounts `/v1/marketplace` with `host_validation_layer`.

The local effective config already had:

```text
LAB_PUBLIC_URL=https://lab.example.com
```

and `/home/jmagar/.labby/config.toml` had OAuth mode enabled:

```toml
[auth]
mode = "oauth"
```

Docs said `LAB_PUBLIC_URL` feeds allowed-host derivation, but the API host-validation middleware only accepted loopback hosts. The MCP HTTP service already had separate allowed-host derivation that included the configured resource/public URL, but the API middleware did not.

## Change

Updated `crates/lab/src/api/host_validation.rs` so protected API host validation accepts:

- loopback hosts as before: `localhost`, `127.0.0.1`, `::1`
- the host parsed from `LAB_PUBLIC_URL`
- explicit comma-separated hosts from `LAB_MCP_ALLOWED_HOSTS`

The code still rejects missing/malformed hosts, unrelated domains, and wildcard `*`.

Added tests for:

- uppercase loopback host normalization
- accepting the public URL host with and without a port
- rejecting unrelated domains
- accepting explicitly configured extra hosts
- ignoring wildcard `*`

## Verification

Formatting:

```bash
cargo fmt --all
```

Focused all-features test:

```bash
RUSTC_WRAPPER= cargo test -p labby --all-features host_validation
```

Result:

```text
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1179 filtered out
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1184 filtered out
```

The first test attempt using default `RUSTC_WRAPPER` failed because `sccache` returned `Operation not permitted` in the sandbox. Clearing `RUSTC_WRAPPER` made the same focused test pass.

## Files Changed

- `crates/lab/src/api/host_validation.rs`
- `docs/sessions/2026-05-07-marketplace-host-validation.md`

## Current State

`git status --short` before writing this note showed:

```text
 M crates/lab/src/api/host_validation.rs
```

This note is under `docs/sessions/`, which is ignored in this repo unless force-added explicitly.

## Open Questions

- The running `labby-master-1` container still needs a rebuild/restart or hot-swap before the log behavior changes in production.
- No live curl against `https://lab.example.com/v1/marketplace` was run in this session.
