//! The `lab.help` global MCP meta-tool. Returns the full catalog in
//! envelope form so agents can discover every enabled service and
//! action in one call.
#![allow(dead_code)]

use crate::{
    catalog::{Catalog, build_catalog},
    mcp::{
        envelope::{ToolEnvelope, ToolError},
        registry::ToolRegistry,
    },
};

/// Dispatch the `lab.help` meta-tool.
///
/// Returns `Result` to match the repo-wide contract that all dispatch helpers
/// are fallible, even though this particular helper is currently infallible.
#[allow(clippy::unnecessary_wraps)]
pub fn help(registry: &ToolRegistry) -> Result<ToolEnvelope<Catalog>, ToolError> {
    let filtered;
    let registry = if crate::registry::lab_show_all_enabled() {
        registry
    } else {
        filtered = crate::registry::filter_by_configured_env(registry);
        &filtered
    };
    Ok(ToolEnvelope::new(build_catalog(registry)))
}
