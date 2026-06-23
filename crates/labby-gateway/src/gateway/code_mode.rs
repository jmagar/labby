//! Gateway adapter over the extracted `lab-codemode` crate.
//!
//! The Code Mode JS execution kernel, broker, result-shaping helpers, and
//! snippet engine now live in `lab-codemode`. This module is the gateway's
//! thin adapter: it re-exports the crate's public surface under the historical
//! `crate::gateway::code_mode::*` import paths, owns the host-side
//! render caches, and hosts `impl CodeModeHost for GatewayManager`
//! (`code_mode_host.rs`) plus the upstream→`ToolDescriptor` catalog projection
//! (`search.rs`) and the one-shot CLI catalog cache (`catalog_cache.rs`).

pub(crate) mod catalog_cache;
pub(crate) mod code_mode_host;
mod search;

// ── Re-exports of the crate's neutral public surface ────────────────────────
//
// Several gateway call sites still use the historical gateway-flavored names
// (`CodeModeCapabilityFilter`, `CodeModeCatalogEntry`, `split_upstream_tool`).
// Those are aliased here so the broad gateway tree compiles unchanged while the
// crate itself stays vocabulary-neutral.

pub use labby_codemode::run_code_mode_runner_stdio;
pub use labby_codemode::{
    CodeModeBroker, CodeModeCaller, CodeModeHistory, CodeModeHistoryEntry, CodeModeHistoryKind,
    CodeModeSourceLookup, CodeModeSourceStore, CodeModeSurface, RunnerPool,
    code_mode_execute_trace, validate_code_mode_params_against_schema,
};
#[cfg(any(test, feature = "testkit"))]
pub use labby_codemode::{CodeModeExecutedCall, CodeModeExecutionResponse};
pub use labby_codemode::{CodeModeExecutionSource, ToolDescriptor, ToolScope};

/// Historical gateway alias for the crate-neutral [`ToolScope`].
pub type CodeModeCapabilityFilter = ToolScope;
/// Historical gateway alias for the crate-neutral [`ToolDescriptor`].
pub type CodeModeCatalogEntry = ToolDescriptor;

pub(crate) use labby_codemode::split_namespaced_id as split_upstream_tool;

// ── Host-side render caches (gateway-owned, keyed on the live tool set) ──────

/// Cached rendered Code Mode discovery catalog.
///
/// Keyed by a fingerprint string (sorted `upstream::tool` ids joined with `\n`
/// plus the snippet fingerprint). When the pool's healthy tool set has not
/// changed between lookups, this avoids re-running `generate_tool_types`,
/// re-serializing the catalog JSON, and re-generating the JS proxy.
pub struct CatalogRenderCache {
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
