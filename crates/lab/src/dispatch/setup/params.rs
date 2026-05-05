//! Param coercion for the `setup` dispatch service.

use lab_apis::setup::DraftEntry;
use serde_json::Value;

use crate::dispatch::error::ToolError;

/// Parse `entries: [{ key, value }, ...]` from a draft.set request body.
pub fn parse_entries(params: &Value) -> Result<Vec<DraftEntry>, ToolError> {
    let raw = params
        .get("entries")
        .ok_or_else(|| ToolError::InvalidParam {
            message: "missing required parameter `entries`".into(),
            param: "entries".into(),
        })?;
    let array = raw.as_array().ok_or_else(|| ToolError::InvalidParam {
        message: "`entries` must be an array".into(),
        param: "entries".into(),
    })?;
    let mut out = Vec::with_capacity(array.len());
    for (idx, item) in array.iter().enumerate() {
        let key =
            item.get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("entries[{idx}].key must be a string"),
                    param: format!("entries[{idx}].key"),
                })?;
        let value =
            item.get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("entries[{idx}].value must be a string"),
                    param: format!("entries[{idx}].value"),
                })?;
        out.push(DraftEntry {
            key: key.to_string(),
            value: value.to_string(),
        });
    }
    Ok(out)
}

#[must_use]
pub fn parse_force(params: &Value) -> bool {
    params
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[must_use]
pub fn parse_bool(params: &Value, name: &str) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(false)
}

pub fn parse_service(params: &Value) -> Result<String, ToolError> {
    params
        .get("service")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| ToolError::MissingParam {
            message: "missing required parameter `service`".into(),
            param: "service".into(),
        })
}

#[must_use]
pub fn parse_services_filter(params: &Value) -> Option<Vec<String>> {
    let array = params.get("services")?.as_array()?;
    Some(
        array
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
    )
}
