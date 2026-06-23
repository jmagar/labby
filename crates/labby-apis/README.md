# lab-apis

Pure Rust SDK/data layer for Lab capability modules. This crate is reusable by
other Rust binaries and intentionally has no product-surface dependencies such
as `clap`, `rmcp`, `axum`, or `anyhow`.

## Current Modules

Always compiled:

- `core`
- `acp`
- `device_runtime`
- `doctor`
- `marketplace`
- `setup`
- `stash`

Optional SDK modules:

- `deploy`
- `mcpregistry`
- `acp_registry`

`all` enables the optional SDK modules above. `default` enables no optional SDK
modules.

## Contracts

- `lab-apis` never reads config files or ambient env vars on its own.
- Callers provide URLs, auth, paths, and runtime config explicitly.
- HTTP-backed modules use the shared `core::HttpClient`.
- Wire-facing types use `serde` and keep presentation concerns out of the SDK.
- Errors map to stable `ApiError::kind()` tags for product-surface envelopes.
- Module layout uses `foo.rs` plus `foo/` submodules; no `mod.rs` files.

## Metadata

Modules that participate in generated docs or setup/doctor metadata expose
`pub const META: PluginMeta` with category, env vars, docs URL, and default port
metadata.

## Testing

```bash
cargo test -p lab-apis
cargo check -p lab-apis --no-default-features
cargo check -p lab-apis --no-default-features --features all
```

See the workspace README and `docs/` for the current product surface and
feature-slice contract.
