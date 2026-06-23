//! Small Code Mode helpers: error constructors, catalog sizing, and the
//! self-contained filesystem/env helpers the kernel needs (artifact root,
//! snippet dir, size caps). These are pure stdlib so the crate stays free of a
//! dependency on the host binary's helper module.

use std::path::PathBuf;

use crate::error::ToolError;
use crate::lab_action_unknown_tool_hint;

use super::types::ToolDescriptor;

/// Resolve the Lab home directory (`$LAB_HOME`, else `$HOME/.lab`, else a fixed
/// temp-dir fallback). Self-contained copy of the host helper so the kernel can
/// locate its artifact root and user snippet dir without depending on `lab`.
///
/// Fail-closed: never anchors artifact/snippet storage to the process CWD
/// (CWE-426/377).
#[must_use]
pub(crate) fn lab_home() -> PathBuf {
    if let Ok(home) = std::env::var("LAB_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home);
    }
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join(".lab"),
        _ => {
            let fallback = std::env::temp_dir().join("lab");
            tracing::warn!(
                fallback = %fallback.display(),
                "neither LAB_HOME nor HOME is set; using a temp-dir fallback for lab home"
            );
            fallback
        }
    }
}

/// Replace the user's home-directory prefix with literal `~` so paths embedded
/// in logs/responses don't leak the OS username. Self-contained copy of the
/// host helper.
#[must_use]
pub(crate) fn redact_home(path: &str) -> String {
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

/// Read an environment variable, returning `None` if absent or empty.
#[must_use]
pub(crate) fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub(crate) fn invalid_code_mode_id(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_code_mode_id".to_string(),
        message: message.into(),
    }
}

pub(crate) fn lab_action_unknown_tool() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unknown_tool".to_string(),
        message: format!(
            "lab:: IDs are not supported by Code Mode. {}",
            lab_action_unknown_tool_hint()
        ),
    }
}

/// Serialized byte size of a catalog (host render-size accounting).
pub fn serialized_catalog_size(entries: &[ToolDescriptor]) -> Result<usize, ToolError> {
    serde_json::to_vec(entries)
        .map(|bytes| bytes.len())
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to serialize Code Mode catalog: {err}"),
        })
}
