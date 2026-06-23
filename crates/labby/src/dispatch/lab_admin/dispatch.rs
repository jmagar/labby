use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str};
use crate::dispatch::lab_admin::catalog::ACTIONS;

/// Top-level dispatch for the `lab_admin` tool.
///
/// Handles the built-in `help` and `schema` actions, then delegates to
/// `dispatch_inner` for service-specific actions.
///
/// # Note on `surface`
///
/// The tracing field `surface` is hardcoded to `"mcp"` here. This is a known
/// limitation: the dispatch layer does not yet carry surface context. All three
/// adapter surfaces (CLI, MCP, API) call into the same dispatch path. Fixing
/// this would require threading a surface parameter through the entire dispatch
/// call chain — tracked as a systemic gap in `crates/lab/CLAUDE.md`.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let start = std::time::Instant::now();
    let result = dispatch_inner(action, params).await;
    let elapsed_ms = start.elapsed().as_millis();

    match &result {
        Ok(_) => tracing::info!(
            surface = "mcp",
            service = "lab_admin",
            action,
            elapsed_ms,
            "dispatch ok"
        ),
        Err(err) => {
            if err.is_internal() {
                tracing::error!(
                    surface = "mcp",
                    service = "lab_admin",
                    action,
                    elapsed_ms,
                    kind = err.kind(),
                    "dispatch error"
                );
            } else {
                tracing::warn!(
                    surface = "mcp",
                    service = "lab_admin",
                    action,
                    elapsed_ms,
                    kind = err.kind(),
                    "dispatch error"
                );
            }
        }
    }

    result
}

/// Inner dispatch — separated so timing and logging wrap the full call.
async fn dispatch_inner(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("lab_admin", ACTIONS)),
        "schema" => {
            let a = require_str(&params, "action")?;
            action_schema(ACTIONS, a)
        }
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `lab_admin.{unknown}`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}
