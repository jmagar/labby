//! Param coercion and security guards for `mcp.*` actions in the marketplace dispatch.
//!
//! Argv/env security guards delegate to the shared
//! [`crate::dispatch::security::spawn_guard`] module — do NOT add local copies
//! of those rules here.
//!
use labby_apis::mcpregistry::types::{
    LabRegistryMetadata, LabRegistrySetupDifficulty, LabRegistryTransportScore, ListServersParams,
};
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::security::spawn_guard;

/// Validate a `runtimeHint` string against the spawn-guard allowlist.
///
/// Delegates to [`spawn_guard::validate_stdio_command`] so both the gateway and
/// marketplace paths share exactly one copy of the allowlist logic.
/// Pass `extra` / `bypass` from `GatewayPreferences` when the caller has config
/// context; pass empty slice / false otherwise.
///
/// Returns `unsupported_runtime_hint` if the hint is not in the allowed list.
pub fn validate_runtime_hint(hint: &str, extra: &[String], bypass: bool) -> Result<(), ToolError> {
    spawn_guard::validate_stdio_command(hint, extra, bypass).map_err(|_| ToolError::Sdk {
        sdk_kind: "unsupported_runtime_hint".to_string(),
        message: format!(
            "runtimeHint '{hint}' is not in the allowed list; must be one of: {}",
            spawn_guard::ALLOWED_RUNTIME_HINTS.join(", ")
        ),
    })
}

/// Validate that none of the argv strings violates runtime-specific security policy.
///
/// Delegates to [`spawn_guard::validate_stdio_argv`]; re-wraps the `InvalidParam`
/// error as `Sdk { sdk_kind: "invalid_param" }` to preserve the existing
/// marketplace error shape.
pub fn validate_stdio_argv(runtime_hint: &str, args: &[String]) -> Result<(), ToolError> {
    spawn_guard::validate_stdio_argv(runtime_hint, args).map_err(rewrap_as_sdk_invalid_param)
}

/// Validate an environment variable name: must match `^[A-Z][A-Z0-9_]*$`.
///
/// Delegates to [`spawn_guard::validate_stdio_env_name`]; re-wraps the error
/// as `Sdk { sdk_kind: "invalid_param" }` to preserve the existing shape.
pub fn validate_env_var_name(name: &str) -> Result<(), ToolError> {
    spawn_guard::validate_stdio_env_name(name).map_err(rewrap_as_sdk_invalid_param)
}

/// Validate an environment variable value: must not contain embedded control separators.
///
/// Delegates to [`spawn_guard::validate_stdio_env_value`]; re-wraps the error.
pub fn validate_env_value(key: &str, value: &str) -> Result<(), ToolError> {
    spawn_guard::validate_stdio_env_value(key, value).map_err(rewrap_as_sdk_invalid_param)
}

/// Re-wrap any `ToolError` as `Sdk { sdk_kind: "invalid_param" }`.
///
/// The marketplace surface historically used `Sdk` for all param errors;
/// the shared spawn_guard uses `InvalidParam`. Both serialize identically
/// (`kind: "invalid_param"`), so this is only a structural normalisation.
fn rewrap_as_sdk_invalid_param(e: ToolError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: e.user_message().to_string(),
    }
}

/// Resolve the effective `search` string from `search` + `owner` dispatch params.
///
/// Precedence:
/// 1. explicit `search` wins if present (owner is silently ignored).
/// 2. `owner` is validated (non-empty after trim, no `/`, no whitespace) and
///    synthesized to `io.github.{owner}/` lowercased.
/// 3. invalid `owner` returns an `invalid_param` error so callers see the
///    problem instead of falling through to an unfiltered list.
///
/// The registry API has no structured owner field — this is a client-side
/// convenience only and does not match non-GitHub publishers.
pub fn resolve_search_for_rest(
    search: Option<&str>,
    owner: Option<&str>,
) -> Result<Option<String>, ToolError> {
    if let Some(s) = search {
        return Ok(Some(s.to_string()));
    }
    let Some(raw) = owner else {
        return Ok(None);
    };
    let owner = raw.trim();
    if owner.is_empty() {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "`owner` must not be empty".to_string(),
        });
    }
    if owner.chars().any(|c| c == '/' || c.is_whitespace()) {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "`owner` must be a bare GitHub username/org (no slashes or whitespace)"
                .to_string(),
        });
    }
    Ok(Some(format!("io.github.{}/", owner.to_ascii_lowercase())))
}

fn optional_string_param<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("`{key}` must be a string"),
        }),
    }
}

pub fn resolve_search(params: &Value) -> Result<Option<String>, ToolError> {
    resolve_search_for_rest(
        optional_string_param(params, "search")?,
        optional_string_param(params, "owner")?,
    )
}

fn optional_bool_param(params: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("`{key}` must be a boolean"),
        }),
    }
}

/// Extract `mcp.list` params from the dispatch params object.
pub fn list_servers_params(params: &Value) -> Result<ListServersParams, ToolError> {
    Ok(ListServersParams {
        search: resolve_search(params)?,
        limit: params["limit"].as_u64().map(|v| v as u32),
        cursor: params["cursor"].as_str().map(str::to_string),
        version: params["version"].as_str().map(str::to_string),
        updated_since: params["updated_since"].as_str().map(str::to_string),
        featured: optional_bool_param(params, "featured")?,
        reviewed: optional_bool_param(params, "reviewed")?,
        recommended: optional_bool_param(params, "recommended")?,
        hidden: optional_bool_param(params, "hidden")?,
        tag: optional_string_param(params, "tag")?.map(str::to_string),
    })
}

pub fn parse_lab_metadata(value: &Value) -> Result<LabRegistryMetadata, ToolError> {
    let metadata: LabRegistryMetadata =
        serde_json::from_value(value.clone()).map_err(|e| ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("invalid Lab metadata: {e}"),
        })?;
    validate_lab_metadata(&metadata)?;
    Ok(normalize_lab_metadata(metadata))
}

fn validate_lab_metadata(metadata: &LabRegistryMetadata) -> Result<(), ToolError> {
    if metadata.audit.is_some() {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "`audit` is managed by Lab and cannot be set manually".to_string(),
        });
    }

    if let Some(curation) = &metadata.curation {
        for tag in &curation.tags {
            if tag.trim().is_empty() {
                return Err(invalid_metadata(
                    "curation.tags must not contain empty values",
                ));
            }
        }
    }
    if let Some(trust) = &metadata.trust {
        validate_timestamp(trust.reviewed_at.as_deref(), "trust.reviewed_at")?;
    }
    if let Some(quality) = &metadata.quality {
        validate_timestamp(
            quality.last_install_tested_at.as_deref(),
            "quality.last_install_tested_at",
        )?;
        match quality.transport_score {
            Some(
                LabRegistryTransportScore::Good
                | LabRegistryTransportScore::Mixed
                | LabRegistryTransportScore::Poor,
            )
            | None => {}
        }
    }
    if let Some(ux) = &metadata.ux {
        match ux.setup_difficulty {
            Some(
                LabRegistrySetupDifficulty::Easy
                | LabRegistrySetupDifficulty::Medium
                | LabRegistrySetupDifficulty::Hard,
            )
            | None => {}
        }
    }
    Ok(())
}

fn normalize_lab_metadata(mut metadata: LabRegistryMetadata) -> LabRegistryMetadata {
    if let Some(curation) = metadata.curation.as_mut() {
        curation.tags = curation
            .tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        curation.notes = curation
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    if let Some(trust) = metadata.trust.as_mut() {
        trust.reviewed_at = normalize_optional_string(trust.reviewed_at.take());
    }
    if let Some(quality) = metadata.quality.as_mut() {
        quality.last_install_tested_at =
            normalize_optional_string(quality.last_install_tested_at.take());
    }
    metadata
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_timestamp(value: Option<&str>, field: &str) -> Result<(), ToolError> {
    let Some(value) = value else {
        return Ok(());
    };
    value
        .parse::<jiff::Timestamp>()
        .map_err(|_| invalid_metadata(&format!("`{field}` must be an RFC3339 timestamp")))?;
    Ok(())
}

fn invalid_metadata(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: message.to_string(),
    }
}

/// Extract a required `name` string param.
pub fn require_name(params: &Value) -> Result<String, ToolError> {
    match params["name"].as_str() {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        Some(_) | None => Err(ToolError::MissingParam {
            message: "missing required parameter `name`".to_string(),
            param: "name".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_hint_rejects_unrecognized_commands() {
        let err = validate_runtime_hint("/tmp/evil", &[], false).unwrap_err();

        assert_eq!(err.kind(), "unsupported_runtime_hint");
    }

    #[test]
    fn stdio_argv_rejects_docker_privileged_flag() {
        let err = validate_stdio_argv("docker", &["--privileged".to_string()]).unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn stdio_argv_rejects_node_inspect_prefix() {
        let err = validate_stdio_argv("npx", &["--inspect=0.0.0.0:9229".to_string()]).unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn stdio_argv_rejects_control_characters() {
        let err = validate_stdio_argv("uvx", &["safe\nunsafe".to_string()]).unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_var_name_rejects_process_and_lab_names() {
        for name in ["PATH", "LD_PRELOAD", "LAB_TOKEN", "foo bar"] {
            let err = validate_env_var_name(name).unwrap_err();
            assert_eq!(err.kind(), "invalid_param", "{name}");
        }
    }

    #[test]
    fn env_value_rejects_null_bytes() {
        let err = validate_env_value("TOKEN", "abc\0def").unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
    }
}
