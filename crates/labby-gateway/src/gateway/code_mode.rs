//! Gateway adapter over the extracted `labby-codemode` crate.
//!
//! The Code Mode JS execution kernel, broker, result-shaping helpers, and
//! snippet engine now live in `labby-codemode`. This module is the gateway's
//! thin adapter: it re-exports the crate's public surface under
//! `crate::gateway::code_mode::*` import paths, owns the host-side render
//! caches, and hosts `impl CodeModeHost for GatewayManager`
//! (`code_mode_host.rs`) plus the upstream→`ToolDescriptor` catalog projection
//! (`search.rs`) and the one-shot CLI catalog cache (`catalog_cache.rs`).

pub(crate) mod catalog_cache;
pub(crate) mod code_mode_host;
mod search;

// ── Re-exports of the crate's neutral public surface ────────────────────────
//
pub use labby_codemode::run_code_mode_runner_stdio;
pub use labby_codemode::{
    CodeModeBroker, CodeModeCaller, CodeModeCallerCapabilities, CodeModeHistory,
    CodeModeHistoryEntry, CodeModeHistoryKind, CodeModeSourceLookup, CodeModeSourceStore,
    CodeModeSurface, RunnerPool, code_mode_execute_trace, validate_code_mode_params_against_schema,
};
#[cfg(any(test, feature = "testkit"))]
pub use labby_codemode::{CodeModeExecutedCall, CodeModeExecutionResponse};
pub use labby_codemode::{CodeModeExecutionSource, ToolDescriptor, ToolScope};

pub(crate) use labby_codemode::split_namespaced_id;

// ── Host-side render caches (gateway-owned, keyed on the live tool set) ──────

/// Cached rendered Code Mode discovery catalog.
///
/// Keyed by a fingerprint string (sorted `upstream::tool` ids joined with `\n`
/// plus the snippet fingerprint). When the pool's healthy tool set has not
/// changed between lookups, this avoids re-running `generate_tool_types` and
/// re-serializing the catalog JSON. It does NOT avoid re-generating the
/// discovery/proxy JS strings themselves (`generate_discovery_js` /
/// `generate_js_proxy_from_catalog`) — those are rebuilt from `entries` on
/// every `codemode` execution regardless of cache hit/miss (see
/// `crates/labby-codemode/src/execute.rs`'s `build_code_mode_proxy`). See bead
/// `lab-5cgrz` for the investigation that confirmed this and evaluated (then
/// rejected, at current scale) converting `search`/`describe` to host-RPC to
/// close that gap.
///
/// This cache is a single slot (`Mutex<Option<CatalogRenderCache>>` on
/// `GatewayManager`) with NO caller/scope component in its fingerprint. It is
/// safe today only because it is reached exclusively through the unscoped CLI
/// path (`code_mode_catalog_tools_cached`, gated by
/// `surface == CodeModeSurface::Cli && scope.allowed_namespaces().is_none()` in
/// `execute.rs`). Do not widen its call sites to scoped callers without adding
/// a scope/`allowed_upstreams` component to the fingerprint first — otherwise a
/// scoped caller could receive a different scope's cached catalog.
pub(crate) struct CatalogRenderCache {
    /// Fingerprint of the healthy tool list when this cache was built.
    pub fingerprint: String,
    /// Rendered catalog entries (includes `.signature` / `.dts`).
    pub entries: Vec<ToolDescriptor>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    pub catalog_json: String,
    /// Serialized catalog size in bytes (for the tracing log).
    pub serialized_size: usize,
}

/// Cached snippet metadata for Code Mode discovery.
///
/// Keyed by cheap directory metadata plus the caller visibility policy. Stores
/// metadata only; executable snippet source is resolved lazily per execution
/// when `codemode.run()` asks the host for it.
pub(crate) struct SnippetMetadataCache {
    pub fingerprint: String,
    pub entries: Vec<labby_codemode::snippet::store::SnippetInfo>,
}
