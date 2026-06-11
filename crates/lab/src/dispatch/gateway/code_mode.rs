//! Code Mode dispatch for the gateway.
//!
//! Split into focused submodules. This root module owns the `CodeModeBroker`
//! struct (so every `impl` submodule is a descendant and can touch the private
//! `gateway_manager` field) and re-exports the public surface consumed by the
//! MCP/CLI adapters and integration tests.

// Tool-name consts come from the layer-neutral `crate::tool_names` module, not
// the MCP surface — the dispatch layer must not import `crate::mcp` (enforced by
// tests/architecture_boundaries.rs).
use crate::tool_names::{CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME};

// Used in-crate by the `CodeModeBroker` struct/`new`; also re-exported so the
// in-crate test modules can reach them via `super::` exactly as the old nested
// `mod tests` did.
pub(crate) use crate::dispatch::gateway::manager::GatewayManager;
pub(crate) use crate::registry::ToolRegistry;

mod artifacts;
pub(crate) mod catalog_cache;
mod execute;
mod normalize;
pub mod preamble;
mod protocol;
mod runner;
mod runner_drive;
mod runner_io;
mod schema;
mod search;
mod trace;
mod truncate;
/// Live TypeScript signature / `.d.ts` generator for Code Mode catalog entries.
/// Previously named `types_legacy` — renamed to reflect its actual role.
pub mod ts_signatures;
mod types;
/// Backward-compat alias for `ts_signatures`. Previously the live generator
/// lived here; the file now delegates entirely to `ts_signatures`.
pub mod types_legacy;
mod util;
mod wrapper;

// `wasm_runner` is dead code — the live runner is Javy/QuickJS via subprocess
// stdio. Compile it only in test builds so it remains reachable from
// integration tests / reference while being invisible to the production binary.
// See `docs/dev/CODE_MODE.md` and `dispatch/gateway/CLAUDE.md` trust-model note.
#[cfg(test)]
mod wasm_runner;

#[cfg(test)]
mod tests_broker;
#[cfg(test)]
mod tests_ids_schema;
#[cfg(test)]
mod tests_normalize;
#[cfg(test)]
mod tests_runtime;
/// Tests for the live TypeScript signature generator (previously `tests_types_legacy`).
#[cfg(test)]
mod tests_ts_signatures;
// Keep the old module name so `cargo test tests_types_legacy` still works —
// the test module was renamed to `tests_ts_signatures` in Q-L2.
// The old file `tests_types_legacy.rs` is now identical to `tests_ts_signatures.rs`
// with updated module paths; both modules reference the same functions.
#[cfg(test)]
mod tests_types_legacy;

pub use normalize::normalize_user_code;
pub use runner::run_code_mode_runner_stdio;
pub(crate) use trace::{code_mode_execute_trace, code_mode_search_trace};
pub(crate) use types::split_upstream_tool;
pub use types::{CodeModeCaller, CodeModeCapabilityFilter, CodeModeSurface, upstream_tool_id}; // shared upstream::tool splitter
// Re-export so `GatewayManager` (in `manager.rs`, a sibling of `code_mode.rs`)
// can name the type in `cached_catalog_render`'s return signature without
// reaching into the private `types` submodule.
pub(crate) use types::CodeModeCatalogEntry;

/// Cached rendered Code Mode search catalog.
///
/// Keyed by a fingerprint string (sorted `upstream::tool` ids joined with `\n`).
/// When the pool's healthy tool set has not changed between searches, this
/// avoids re-running `generate_tool_types` for every entry, re-serializing the
/// catalog JSON, and re-generating the JS proxy string.
pub struct CatalogRenderCache {
    /// Fingerprint of the healthy tool list when this cache was built.
    pub fingerprint: String,
    /// Rendered catalog entries (includes `.signature` / `.dts` from `generate_tool_types`).
    pub entries: Vec<CodeModeCatalogEntry>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    pub catalog_json: String,
    /// Serialized catalog size in bytes (for the tracing log).
    pub serialized_size: usize,
}
#[cfg(test)]
pub(crate) use types::{CodeModeExecutionError, CodeModeExecutionResponse};
pub(crate) use types::{CodeModeHistory, CodeModeHistoryEntry, CodeModeHistoryKind};

// Re-exported for the in-crate test modules (`tests_*`), which reference these
// types/helpers via `super::*`. Gated to the test build so the non-test lib does
// not carry an unused public re-export (clippy runs `-D warnings`). This block
// reconstructs the exact `super::` surface the old single `mod tests` had.
#[cfg(test)]
pub(crate) use crate::dispatch::error::ToolError;
#[cfg(test)]
pub(crate) use crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
#[cfg(test)]
pub(crate) use types::{
    CodeModeExecutedCall, CodeModeToolId, CodeModeToolRef, sanitize_code_mode_schema,
};
// These items are declared `pub(in ...code_mode)`; re-export at the same
// restricted visibility (a wider `pub(crate)` re-export is rejected by E0364).
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) use schema::{
    unwrap_code_mode_upstream_result, validate_code_mode_params_against_schema,
};
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) use truncate::{
    apply_log_caps, truncate_execution_response,
};
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) use types::destructive_permitted;

// Tool name strings are sourced from mcp/catalog.rs constants at runtime to
// avoid stale literal references when tool names change.
pub(in crate::dispatch::gateway::code_mode) fn lab_action_unknown_tool_hint() -> String {
    format!(
        "Code Mode handles upstream MCP tools only. For Lab actions, use the `{TOOL_EXECUTE_TOOL_NAME}` MCP tool \
         (use `{CODE_MODE_SEARCH_TOOL_NAME}` first to discover available tools): \
         name=<service> (e.g. \"radarr\"), arguments={{action: \"<dotted.action>\", params: {{...}}}}. \
         Example: {TOOL_EXECUTE_TOOL_NAME}(name=\"radarr\", arguments={{action:\"movie.search\", params:{{query:\"Matrix\"}}}})."
    )
}

pub struct CodeModeBroker<'a> {
    gateway_manager: Option<&'a GatewayManager>,
    /// Run-scoped sink for the last upstream MCP Apps (mcp-ui) widget link seen
    /// during this execution. Recorded at the `call_upstream_tool` boundary
    /// (last-wins) before the envelope is unwrapped, then surfaced via the
    /// `{ __ui: <result> }` opt-in in `execute()`. A fresh broker is constructed
    /// per request, so this is naturally scoped to a single run.
    ui_capture: std::sync::Arc<std::sync::Mutex<Option<types::UiLink>>>,
}

impl<'a> CodeModeBroker<'a> {
    #[must_use]
    pub fn new(_registry: &'a ToolRegistry, gateway_manager: Option<&'a GatewayManager>) -> Self {
        Self {
            gateway_manager,
            ui_capture: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}
