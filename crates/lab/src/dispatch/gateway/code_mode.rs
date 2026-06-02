//! Code Mode dispatch for the gateway.
//!
//! Split into focused submodules. This root module owns the `CodeModeBroker`
//! struct (so every `impl` submodule is a descendant and can touch the private
//! `gateway_manager` field) and re-exports the public surface consumed by the
//! MCP/CLI adapters and integration tests.

use crate::mcp::catalog::{TOOL_EXECUTE_TOOL_NAME, TOOL_SEARCH_TOOL_NAME};

// Used in-crate by the `CodeModeBroker` struct/`new`; also re-exported so the
// in-crate test modules can reach them via `super::` exactly as the old nested
// `mod tests` did.
pub(crate) use crate::dispatch::gateway::manager::GatewayManager;
pub(crate) use crate::registry::ToolRegistry;

mod execute;
mod normalize;
pub mod preamble;
mod protocol;
mod runner;
mod runner_drive;
mod runner_io;
mod schema;
mod search;
mod truncate;
mod types;
pub mod types_legacy;
mod util;
mod wrapper;

#[allow(dead_code)]
mod wasm_runner;

#[cfg(test)]
mod tests_broker;
#[cfg(test)]
mod tests_ids_schema;
#[cfg(test)]
mod tests_normalize;
#[cfg(test)]
mod tests_runtime;
#[cfg(test)]
mod tests_types_legacy;

pub use normalize::normalize_user_code;
pub use runner::run_code_mode_runner_stdio;
pub use types::{CodeModeCaller, CodeModeCapabilityFilter, CodeModeSurface, upstream_tool_id};

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
    CodeModeCatalogEntry, CodeModeExecutedCall, CodeModeExecutionResponse, CodeModeToolId,
    CodeModeToolRef, sanitize_code_mode_schema,
};
// These items are declared `pub(in ...code_mode)`; re-export at the same
// restricted visibility (a wider `pub(crate)` re-export is rejected by E0364).
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) use runner_io::code_mode_upstream_error_info;
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
         (use `{TOOL_SEARCH_TOOL_NAME}` first to discover available tools): \
         name=<service> (e.g. \"radarr\"), arguments={{action: \"<dotted.action>\", params: {{...}}}}. \
         Example: {TOOL_EXECUTE_TOOL_NAME}(name=\"radarr\", arguments={{action:\"movie.search\", params:{{query:\"Matrix\"}}}})."
    )
}

pub struct CodeModeBroker<'a> {
    gateway_manager: Option<&'a GatewayManager>,
}

impl<'a> CodeModeBroker<'a> {
    #[must_use]
    pub fn new(_registry: &'a ToolRegistry, gateway_manager: Option<&'a GatewayManager>) -> Self {
        Self { gateway_manager }
    }
}
