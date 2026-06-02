//! Small Code Mode helpers: error constructors and catalog sizing.

use crate::dispatch::error::ToolError;

use super::lab_action_unknown_tool_hint;
use super::types::CodeModeCatalogEntry;

pub fn invalid_code_mode_id(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_code_mode_id".to_string(),
        message: message.into(),
    }
}

pub(in crate::dispatch::gateway::code_mode) fn lab_action_unknown_tool() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unknown_tool".to_string(),
        message: format!(
            "lab:: IDs are not supported by Code Mode. {}",
            lab_action_unknown_tool_hint()
        ),
    }
}

pub(in crate::dispatch::gateway::code_mode) fn serialized_catalog_size(
    entries: &[CodeModeCatalogEntry],
) -> Result<usize, ToolError> {
    serde_json::to_vec(entries)
        .map(|bytes| bytes.len())
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to serialize Code Mode catalog: {err}"),
        })
}
