//! Marketplace artifact diff/patch actions — not yet implemented.
//!
//! `artifact.diff` and `artifact.patch` are advertised in the catalog but
//! currently return the `not_implemented` error kind. The intended behavior is
//! described in `docs/contracts/marketplace-stash-integration.md`; until it
//! lands, these wire stable signatures and a structured `not_implemented`
//! envelope so callers fail predictably rather than on a missing action.

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::params::{ArtifactDiffParams, PatchParams};

pub(super) async fn artifact_diff(params: ArtifactDiffParams) -> Result<Value, ToolError> {
    Err(not_implemented_error(
        "artifact.diff",
        format!(
            "artifact diff is not implemented yet for `{}`",
            params.plugin_id
        ),
    ))
}

pub(super) async fn artifact_patch(params: PatchParams) -> Result<Value, ToolError> {
    Err(not_implemented_error(
        "artifact.patch",
        format!(
            "artifact patch is not implemented yet for `{}` at `{}`",
            params.plugin_id, params.artifact_path
        ),
    ))
}

fn not_implemented_error(action: &'static str, detail: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_implemented".to_string(),
        message: format!("{action}: {detail}"),
    }
}
