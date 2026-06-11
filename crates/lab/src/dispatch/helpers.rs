//! Shared dispatch helpers used across all service modules.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path};

use lab_apis::core::action::ActionSpec;
use serde_json::Value;

use crate::dispatch::error::ToolError;

/// Resolve the lab home directory: `$LAB_HOME` if set, else `$HOME/.lab/`.
///
/// Lives in `helpers` (a leaf module) rather than `setup` so peer services
/// can resolve the path without importing the `setup` orchestrator — see
/// `dispatch/CLAUDE.md` § Orchestrator Exception.
#[must_use]
pub fn lab_home() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("LAB_HOME")
        && !home.is_empty()
    {
        return std::path::PathBuf::from(home);
    }
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base).join(".lab")
}

/// Replace the user's home-directory prefix with literal `~` so paths
/// embedded in log events, response bodies, and error messages don't leak
/// the OS username.
///
/// Preserves per-runtime subdirs (`~/.claude/plugins/` vs `~/.codex/plugins/`
/// vs `~/.lab/bin/<agent_id>/` remain distinguishable). Safe on any input:
/// if `HOME` is unset or the path doesn't sit under it, the input is
/// returned unchanged.
///
/// lab-zxx5.27: promoted to shared helpers so `node/` install paths can
/// call it without reaching into a sibling service's private module.
#[must_use]
pub fn redact_home(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return path.to_string();
    }
    if let Some(rest) = path.strip_prefix(home) {
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            return "~".to_string();
        }
        return format!("~/{rest}");
    }
    path.to_string()
}

/// Reject any path input that contains a `Component::ParentDir` (`..`) segment.
///
/// This is a **lexical** check only. Callers that join the input against a
/// trusted root MUST additionally `canonicalize` + `starts_with(root)` after
/// writing to protect against symlinks escaping the jail (TOCTOU-weak, but
/// strictly better than skipping). Windows UNC / absolute paths are rejected
/// upstream by callers via `Path::is_absolute`.
pub fn reject_path_traversal(rel_path: &str) -> Result<(), ToolError> {
    for component in Path::new(rel_path).components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "path traversal rejected: `{rel_path}` must be a relative path with only normal components"
                ),
                param: "path".to_string(),
            });
        }
    }
    Ok(())
}

/// Read an environment variable, returning `None` if absent or empty.
pub fn env_non_empty(name: &str) -> Option<String> {
    ENV_OVERRIDE
        .with(|override_map| {
            override_map
                .borrow()
                .as_ref()
                .and_then(|values| values.get(name).cloned())
        })
        .or_else(|| std::env::var(name).ok())
        .filter(|v| !v.is_empty())
}

thread_local! {
    static ENV_OVERRIDE: RefCell<Option<HashMap<String, String>>> = const { RefCell::new(None) };
}

pub fn with_env_override<T>(values: HashMap<String, String>, f: impl FnOnce() -> T) -> T {
    ENV_OVERRIDE.with(|slot| {
        let previous = slot.replace(Some(values));
        let result = f();
        slot.replace(previous);
        result
    })
}

/// Serialize any `Serialize` value to `serde_json::Value`.
pub fn to_json<T: serde::Serialize>(v: T) -> Result<Value, ToolError> {
    serde_json::to_value(v).map_err(|e| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: e.to_string(),
    })
}

/// Rough char-based token estimator for dispatch telemetry logs.
///
/// Uses the conventional ~4-chars-per-token heuristic — cheap, dependency-free,
/// and accurate enough for capacity/cost tracking; do NOT use for LLM budget
/// enforcement. Lives in this shared dispatch leaf so the MCP, HTTP, and CLI
/// surfaces can all attribute tokens without crossing the `api -> mcp` boundary.
#[must_use]
pub fn estimate_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

/// Token estimate for a JSON value, computed against its serialized form.
#[must_use]
pub fn estimate_tokens_value(value: &Value) -> usize {
    estimate_tokens(&serde_json::to_string(value).unwrap_or_default())
}

/// Token estimate for an MCP arguments map (`request.arguments`).
#[must_use]
pub fn estimate_tokens_args(arguments: &serde_json::Map<String, Value>) -> usize {
    estimate_tokens(&serde_json::to_string(arguments).unwrap_or_default())
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

/// Extract an optional string parameter from a JSON object.
///
/// Empty strings are rejected as `invalid_param` so callers do not silently
/// treat `instance=` as if the field were absent.
pub fn optional_str<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match params.get(key) {
        None => Ok(None),
        Some(v) => {
            let value = v.as_str().ok_or_else(|| ToolError::InvalidParam {
                message: format!("parameter `{key}` must be a string"),
                param: key.to_string(),
            })?;
            if value.is_empty() {
                Err(ToolError::InvalidParam {
                    message: format!("parameter `{key}` must not be empty"),
                    param: key.to_string(),
                })
            } else {
                Ok(Some(value))
            }
        }
    }
}

/// Build the standard `help` response payload for a service.
///
/// Produces the canonical `{ service, actions: [...] }` shape returned by every
/// service dispatcher when `action == "help"`.
pub fn help_payload(service: &str, actions: &[ActionSpec]) -> Value {
    serde_json::json!({
        "service": service,
        "actions": actions.iter().map(|a| serde_json::json!({
            "name": a.name,
            "description": a.description,
            "destructive": a.destructive,
            "returns": a.returns,
            "params": a.params.iter().map(|p| serde_json::json!({
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
    Ok(serde_json::json!({
        "action": spec.name,
        "description": spec.description,
        "destructive": spec.destructive,
        "returns": spec.returns,
        "params": spec.params.iter().map(|p| serde_json::json!({
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
/// # Contract (shared dispatch rule)
///
/// `help` and `schema` must succeed without any backing client/manager installed.
/// They answer from the statically-compiled action catalog, so they are always
/// available.  Every service dispatcher MUST call this before its own
/// client-resolution step — failure to do so causes built-in actions to return
/// `internal_error` when no client is wired (the regression fixed by lab-l3cm).
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

/// Create a database file with Unix 0600 permissions if it does not already exist.
///
/// This is a no-op when the file already exists (uses `create_new`, so an
/// `AlreadyExists` error is silently swallowed). On platforms that do not
/// support `OpenOptionsExt`, this function is not compiled and callers must
/// ensure appropriate permissions by other means.
///
/// # Security
///
/// Must be called **before** opening the file via the connection pool so that
/// the creation and permission assignment happen atomically. Subsequent opens
/// by the pool do not change permissions.
#[cfg(unix)]
#[allow(dead_code)]
pub fn create_db_file_0600(path: &std::path::PathBuf) {
    use std::os::unix::fs::OpenOptionsExt;
    // Only set mode on creation; if the file already exists, leave perms
    // alone. Any other failure (permission denied, parent missing, EROFS,
    // etc.) is logged at WARN — silently swallowing every error here can
    // hide real misconfigurations of the secure-DB-file path.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to pre-create secure DB file with 0600 permissions"
            );
        }
    }
}
