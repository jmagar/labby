//! Surface-neutral dispatch helpers shared by every service dispatcher.
//!
//! These build the canonical `help`/`schema` JSON payloads and the small param
//! extractors that every `action + params` dispatcher relies on. They live here
//! (rather than in the `lab` binary) so the extracted runtime crates — notably
//! `lab-gateway`'s dispatch surface — share one source of truth with the binary.
//! The `lab` binary re-exports them from its own `dispatch::helpers` module so
//! existing call sites are unchanged.

use labby_apis::core::ActionSpec;
use serde_json::{Value, json};

use crate::error::ToolError;

/// Serialize any `Serialize` value to `serde_json::Value`.
pub fn to_json<T: serde::Serialize>(v: T) -> Result<Value, ToolError> {
    serde_json::to_value(v).map_err(|e| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: e.to_string(),
    })
}

/// Extract a required string parameter from a JSON object.
pub fn require_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam {
            message: format!("missing required parameter `{key}`"),
            param: key.to_string(),
        })
}

/// Build the standard `help` response payload for a service.
///
/// Produces the canonical `{ service, actions: [...] }` shape returned by every
/// service dispatcher when `action == "help"`.
pub fn help_payload(service: &str, actions: &[ActionSpec]) -> Value {
    json!({
        "service": service,
        "actions": actions.iter().map(|a| json!({
            "name": a.name,
            "description": a.description,
            "destructive": a.destructive,
            "returns": a.returns,
            "params": a.params.iter().map(|p| json!({
                "name": p.name,
                "type": p.ty,
                "required": p.required,
                "description": p.description,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Return the schema for one named action.
///
/// Used to implement the `"schema"` built-in action in every service dispatcher.
/// Returns `ToolError::UnknownAction` if `action_name` is not in `actions`.
pub fn action_schema(actions: &[ActionSpec], action_name: &str) -> Result<Value, ToolError> {
    let spec = actions
        .iter()
        .find(|a| a.name == action_name)
        .ok_or_else(|| ToolError::UnknownAction {
            message: format!("no schema for unknown action `{action_name}`"),
            valid: actions.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        })?;
    Ok(json!({
        "action": spec.name,
        "description": spec.description,
        "destructive": spec.destructive,
        "returns": spec.returns,
        "params": spec.params.iter().map(|p| json!({
            "name": p.name,
            "type": p.ty,
            "required": p.required,
            "description": p.description,
        })).collect::<Vec<_>>(),
    }))
}

/// Handle the `help` and `schema` built-in actions that every service dispatcher
/// must respond to **before** resolving any service-specific client or manager.
///
/// Returns `Some(result)` when the action was handled; `None` to let the caller
/// continue with service-specific dispatch.
pub fn handle_builtin(
    action: &str,
    params: &Value,
    service: &str,
    actions: &[ActionSpec],
) -> Option<Result<Value, ToolError>> {
    match action {
        "help" => Some(Ok(help_payload(service, actions))),
        "schema" => Some(require_str(params, "action").and_then(|a| action_schema(actions, a))),
        _ => None,
    }
}
