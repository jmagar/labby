use serde_json::Value;

use crate::dispatch::error::ToolError;

pub fn optional_u32(params: &Value, name: &'static str) -> Result<Option<u32>, ToolError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let Some(n) = value.as_u64() else {
                return Err(ToolError::InvalidParam {
                    message: format!("`{name}` must be an integer"),
                    param: name.to_owned(),
                });
            };
            Ok(Some(u32::try_from(n).unwrap_or(u32::MAX).min(500)))
        }
    }
}

pub fn optional_status(params: &Value) -> Result<Option<String>, ToolError> {
    match params.get("status") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let Some(status) = value.as_str() else {
                return Err(ToolError::InvalidParam {
                    message: "`status` must be a string".into(),
                    param: "status".into(),
                });
            };
            let status = status.trim();
            if matches!(
                status,
                "open" | "in_progress" | "blocked" | "deferred" | "closed"
            ) {
                Ok(Some(status.to_owned()))
            } else {
                Err(ToolError::InvalidParam {
                    message: "`status` must be one of open, in_progress, blocked, deferred, closed"
                        .into(),
                    param: "status".into(),
                })
            }
        }
    }
}

pub fn optional_project(params: &Value) -> Result<Option<String>, ToolError> {
    match params.get("project") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let Some(project) = value.as_str() else {
                return Err(ToolError::InvalidParam {
                    message: "`project` must be a string".into(),
                    param: "project".into(),
                });
            };
            let project = project.trim();
            if project.is_empty() {
                return Ok(None);
            }
            for ch in project.chars() {
                if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
                    return Err(ToolError::InvalidParam {
                        message: "`project` may only contain ASCII letters, digits, `_`, and `-`"
                            .into(),
                        param: "project".into(),
                    });
                }
            }
            Ok(Some(project.to_owned()))
        }
    }
}
