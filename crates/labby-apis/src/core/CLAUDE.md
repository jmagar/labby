# core/ — Cross-cutting primitives

This directory is the foundation every service module depends on. Changes here ripple across all service clients — be conservative and align with `docs/ARCH.md`, `docs/CONVENTIONS.md`, `docs/dev/OBSERVABILITY.md`, `docs/dev/ERRORS.md`, and `docs/SERIALIZATION.md` before editing.

## Files

| File | Purpose |
|------|---------|
| `auth.rs` | `Auth` enum: `ApiKey { header, key }`, `Bearer { token }`, `Token { token }`, `Basic { username, password }`, `Session { cookie }`, `None`. Debug impl **must** redact secrets. |
| `http.rs` | `HttpClient` wrapper around `reqwest::Client` — auth injection, `request.start`/`request.finish`/`request.error` tracing events, JSON and GraphQL helpers. No retry logic, no backoff, no spans — callers own those. All service clients build on this. |
| `error.rs` | `ApiError` canonical taxonomy + `kind()` method. See below. |
| `status.rs` | `ServiceStatus { reachable, auth_ok, version, latency_ms, message }` — returned by `ServiceClient::health()`. |
| `action.rs` | `ActionSpec { name, description, destructive, params, returns }` + `ParamSpec { name, ty: &'static str, required, description }`. Drives help/schema/catalog. |
| `plugin.rs` | `PluginMeta` + `Category` (10 variants) + `EnvVar`. Per-service compile-time constants. |
| `traits.rs` | `ServiceClient` trait with **native `async fn in trait`** — no `#[async_trait]`, no `Box<dyn>`. |

## ApiError.kind() — canonical stable tags

These strings appear verbatim in MCP and HTTP error surfaces. Adding a new kind is a **spec change** — update `docs/dev/ERRORS.md`, `docs/surfaces/MCP.md`, and `docs/CONVENTIONS.md` first.

| Variant | `kind()` |
|---------|----------|
| `Auth` | `"auth_failed"` |
| `NotFound` | `"not_found"` |
| `RateLimited { retry_after }` | `"rate_limited"` |
| `Validation { field, message }` | `"validation_failed"` |
| `Network(_)` | `"network_error"` |
| `Server { status, body }` | `"server_error"` |
| `Decode(_)` | `"decode_error"` |
| `Internal(_)` | `"internal_error"` |

Dispatchers in `lab/src/dispatch/` layer additional kinds on top: `unknown_action`, `unknown_subaction`, `missing_param`, `invalid_param`, `unknown_instance`. See `docs/dev/ERRORS.md` for the canonical vocabulary and envelope rules.

## Invariants

- **No `clap`, `rmcp`, `tabled`, `anyhow`** in this directory — ever.
- **No file or env I/O.** `Auth::from_env()` helpers are allowed to *accept* env values, but the binary calls them. `lab-apis` never reads `std::env` on its own.
- **Debug impls for anything holding secrets must redact.** Test this.
- **Keep `ParamSpec.ty` as `&'static str`** (e.g., `"string"`, `"integer"`, `"bool"`). Do not reintroduce a `ParamType` enum — the topic docs standardize on string labels.
- **`ActionSpec.destructive` is the single source of truth** for elicitation + CLI confirm flows. Never hide destructive ops behind a non-destructive action.
