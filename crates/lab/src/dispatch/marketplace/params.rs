use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{optional_str, require_str};

/// Conflict-resolution strategy for a marketplace artifact update.
///
/// Lives here (the marketplace param layer) because it is a request parameter
/// that the dispatch/merge code consumes; there is no separate "stash meta"
/// schema module.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictStrategy {
    KeepMine,
    TakeUpstream,
    #[default]
    AlwaysAsk,
    AiSuggest,
}

/// The single canonical relative-path validator for marketplace artifact paths.
///
/// Rejects empty strings, null bytes, backslashes, and any non-`Normal` path
/// component (so `..`, absolute paths, `.`, and Windows drive prefixes are all
/// rejected) on every platform. Do NOT reintroduce a per-call-site copy — every
/// artifact-path entry point (params parsing, the fork bridge, update preview/
/// apply) must funnel through this function so the rules cannot diverge.
pub(crate) fn validate_rel_path(rel_path: &str, param: &str) -> Result<(), ToolError> {
    let invalid = |message: &str| ToolError::InvalidParam {
        param: param.into(),
        message: message.into(),
    };
    if rel_path.is_empty() {
        return Err(invalid("must not be empty"));
    }
    if rel_path.as_bytes().contains(&0) {
        return Err(invalid("must not contain null bytes"));
    }
    if rel_path.contains('\\') {
        return Err(invalid("path traversal not allowed"));
    }
    for component in Path::new(rel_path).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid("path traversal not allowed"));
        }
    }
    Ok(())
}

pub(super) struct CherryPickParams {
    pub plugin_id: String,
    pub components: Vec<String>,
    pub node_ids: Vec<String>,
    pub scope: String,
    pub project_path: Option<String>,
}

pub(super) struct UpdateCheckParams {
    pub plugin_id: Option<String>,
}

pub(super) struct UpdatePreviewParams {
    pub plugin_id: String,
    pub artifact_path: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct ForkParams {
    pub plugin_id: String,
    pub artifacts: Option<Vec<String>>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct ArtifactListParams {
    pub plugin_id: Option<String>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct UnforkParams {
    pub plugin_id: String,
    pub artifacts: Option<Vec<String>>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct ArtifactResetParams {
    pub plugin_id: String,
    pub artifacts: Option<Vec<String>>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct ArtifactDiffParams {
    pub plugin_id: String,
    pub artifact_path: Option<String>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct PatchParams {
    pub plugin_id: String,
    pub artifact_path: String,
    pub patch: String,
    pub description: Option<String>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct UpdateApplyParams {
    pub plugin_id: String,
    pub artifact_path: Option<String>,
    pub strategy: Option<ConflictStrategy>,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct MergeSuggestParams {
    pub plugin_id: String,
    pub artifact_path: String,
    pub instance: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct ConfigSetParams {
    pub plugin_id: String,
    pub artifact_path: Option<String>,
    pub strategy: Option<ConflictStrategy>,
    pub notify: Option<bool>,
    pub instance: Option<String>,
}

pub(super) fn parse_update_check_params(params: &Value) -> Result<UpdateCheckParams, ToolError> {
    let plugin_id = params
        .get("plugin_id")
        .or_else(|| params.get("pluginId"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if let Some(plugin_id) = &plugin_id {
        parse_plugin_id(plugin_id)?;
    }
    Ok(UpdateCheckParams { plugin_id })
}

pub(super) fn parse_update_preview_params(
    params: &Value,
) -> Result<UpdatePreviewParams, ToolError> {
    let plugin_id = params
        .get("plugin_id")
        .or_else(|| params.get("pluginId"))
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam {
            param: "plugin_id".into(),
            message: "`plugin_id` is required".into(),
        })?
        .to_string();
    parse_plugin_id(&plugin_id)?;
    let artifact_path = optional_owned_str(params, "artifact_path")?;
    if let Some(path) = &artifact_path {
        validate_rel_path(path, "artifact_path")?;
    }
    Ok(UpdatePreviewParams {
        plugin_id,
        artifact_path,
    })
}

pub(super) fn parse_fork_params(params: &Value) -> Result<ForkParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifacts = optional_artifact_paths(params, "artifacts")?;
    Ok(ForkParams {
        plugin_id,
        artifacts,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_artifact_list_params(params: &Value) -> Result<ArtifactListParams, ToolError> {
    let plugin_id = optional_owned_str(params, "plugin_id")?;
    if let Some(plugin_id) = &plugin_id {
        parse_plugin_id(plugin_id)?;
    }
    Ok(ArtifactListParams {
        plugin_id,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_unfork_params(params: &Value) -> Result<UnforkParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifacts = optional_artifact_paths(params, "artifacts")?;
    Ok(UnforkParams {
        plugin_id,
        artifacts,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_artifact_reset_params(
    params: &Value,
) -> Result<ArtifactResetParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifacts = optional_artifact_paths(params, "artifacts")?;
    Ok(ArtifactResetParams {
        plugin_id,
        artifacts,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_artifact_diff_params(params: &Value) -> Result<ArtifactDiffParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifact_path = optional_owned_str(params, "artifact_path")?;
    if let Some(path) = &artifact_path {
        validate_rel_path(path, "artifact_path")?;
    }
    Ok(ArtifactDiffParams {
        plugin_id,
        artifact_path,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_patch_params(params: &Value) -> Result<PatchParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifact_path = require_str(params, "artifact_path")?.to_string();
    validate_rel_path(&artifact_path, "artifact_path")?;
    let patch = require_str(params, "patch")?.to_string();
    Ok(PatchParams {
        plugin_id,
        artifact_path,
        patch,
        description: optional_owned_str(params, "description")?,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_update_apply_params(params: &Value) -> Result<UpdateApplyParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifact_path = optional_owned_str(params, "artifact_path")?;
    if let Some(path) = &artifact_path {
        validate_rel_path(path, "artifact_path")?;
    }
    Ok(UpdateApplyParams {
        plugin_id,
        artifact_path,
        strategy: parse_strategy(params)?,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_merge_suggest_params(params: &Value) -> Result<MergeSuggestParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifact_path = require_str(params, "artifact_path")?.to_string();
    validate_rel_path(&artifact_path, "artifact_path")?;
    Ok(MergeSuggestParams {
        plugin_id,
        artifact_path,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_config_set_params(params: &Value) -> Result<ConfigSetParams, ToolError> {
    let plugin_id = parse_required_plugin_id(params)?;
    let artifact_path = optional_owned_str(params, "artifact_path")?;
    if let Some(path) = &artifact_path {
        validate_rel_path(path, "artifact_path")?;
    }
    let notify = match params.get("notify") {
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => {
            return Err(ToolError::InvalidParam {
                param: "notify".into(),
                message: "`notify` must be a boolean".into(),
            });
        }
        None => None,
    };
    Ok(ConfigSetParams {
        plugin_id,
        artifact_path,
        strategy: parse_strategy(params)?,
        notify,
        instance: optional_owned_str(params, "instance")?,
    })
}

pub(super) fn parse_cherry_pick_params(params: &Value) -> Result<CherryPickParams, ToolError> {
    let plugin_id = require_str(params, "plugin_id")?.to_string();
    parse_plugin_id(&plugin_id)?;

    // Parse `components` strictly: a non-string entry is rejected rather than
    // silently dropped (which would proceed with a partial cherry-pick).
    // Defense-in-depth: every accepted string is funnelled through the single
    // canonical validator so the path-traversal rules cannot diverge from the
    // rest of the marketplace artifact-path entry points.
    let components: Vec<String> = match params.get("components") {
        Some(Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for value in arr {
                let Some(component) = value.as_str() else {
                    return Err(ToolError::InvalidParam {
                        param: "components".into(),
                        message: "`components` must be an array of strings".into(),
                    });
                };
                validate_rel_path(component, "components")?;
                out.push(component.to_string());
            }
            out
        }
        Some(_) => {
            return Err(ToolError::InvalidParam {
                param: "components".into(),
                message: "`components` must be an array of strings".into(),
            });
        }
        None => Vec::new(),
    };
    if components.is_empty() {
        return Err(ToolError::MissingParam {
            param: "components".into(),
            message: "`components` must be a non-empty array".into(),
        });
    }

    let node_ids: Vec<String> = params
        .get("node_ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    if node_ids.is_empty() {
        return Err(ToolError::MissingParam {
            param: "node_ids".into(),
            message: "`node_ids` must be a non-empty array".into(),
        });
    }

    let scope = require_str(params, "scope")?.to_string();
    if scope != "global" && scope != "project" {
        return Err(ToolError::InvalidParam {
            param: "scope".into(),
            message: "`scope` must be `global` or `project`".into(),
        });
    }

    let project_path = params
        .get("project_path")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if scope == "project" {
        match &project_path {
            None => {
                return Err(ToolError::MissingParam {
                    param: "project_path".into(),
                    message: "`project_path` is required when `scope` is `project`".into(),
                });
            }
            Some(p) if !p.starts_with('/') => {
                return Err(ToolError::InvalidParam {
                    param: "project_path".into(),
                    message: "`project_path` must be an absolute path".into(),
                });
            }
            _ => {}
        }
    }

    Ok(CherryPickParams {
        plugin_id,
        components,
        node_ids,
        scope,
        project_path,
    })
}

fn parse_required_plugin_id(params: &Value) -> Result<String, ToolError> {
    let plugin_id = params
        .get("plugin_id")
        .or_else(|| params.get("pluginId"))
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam {
            param: "plugin_id".into(),
            message: "`plugin_id` is required".into(),
        })?
        .to_string();
    parse_plugin_id(&plugin_id)?;
    Ok(plugin_id)
}

fn optional_owned_str(params: &Value, key: &str) -> Result<Option<String>, ToolError> {
    optional_str(params, key).map(|value| value.map(ToString::to_string))
}

fn optional_artifact_paths(
    params: &Value,
    key: &'static str,
) -> Result<Option<Vec<String>>, ToolError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let Some(values) = value.as_array() else {
        return Err(ToolError::InvalidParam {
            param: key.into(),
            message: format!("`{key}` must be an array of strings"),
        });
    };
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let Some(path) = value.as_str() else {
            return Err(ToolError::InvalidParam {
                param: key.into(),
                message: format!("`{key}` must be an array of strings"),
            });
        };
        validate_rel_path(path, key)?;
        out.push(path.to_string());
    }
    Ok(Some(out))
}

fn parse_strategy(params: &Value) -> Result<Option<ConflictStrategy>, ToolError> {
    optional_str(params, "strategy")?
        .map(|strategy| match strategy {
            "keep_mine" => Ok(ConflictStrategy::KeepMine),
            "take_upstream" => Ok(ConflictStrategy::TakeUpstream),
            "always_ask" => Ok(ConflictStrategy::AlwaysAsk),
            "ai_suggest" => Ok(ConflictStrategy::AiSuggest),
            other => Err(ToolError::InvalidParam {
                param: "strategy".into(),
                message: format!("unknown value: {other}"),
            }),
        })
        .transpose()
}

/// Parse a plugin id in `name@marketplace` form.
///
/// Both components are validated against path traversal: only `Normal` path
/// components are accepted, rejecting `..`, absolute paths, and `.`.
pub fn parse_plugin_id(id: &str) -> Result<(&str, &str), ToolError> {
    let (name, marketplace) = id
        .split_once('@')
        .filter(|(n, m)| !n.is_empty() && !m.is_empty() && !m.contains('@'))
        .ok_or_else(|| ToolError::InvalidParam {
            message: format!("plugin id `{id}` must be in `name@marketplace` form"),
            param: "id".into(),
        })?;
    for part in [name, marketplace] {
        for component in Path::new(part).components() {
            if !matches!(component, Component::Normal(_)) {
                return Err(ToolError::InvalidParam {
                    message: format!("plugin id `{id}` contains invalid path characters"),
                    param: "id".into(),
                });
            }
        }
    }
    Ok((name, marketplace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_rel_path_accepts_normal_relative_paths() {
        assert!(validate_rel_path("agents/foo.md", "p").is_ok());
        assert!(validate_rel_path("skills/bar/baz.md", "p").is_ok());
    }

    #[test]
    fn validate_rel_path_rejects_traversal_on_all_platforms() {
        // The backslash and null-byte cases are the regression guard for the
        // formerly-weaker update.rs validator: `agents\..\x` must be rejected
        // even on Unix, where it would otherwise be one Normal component.
        for bad in [
            "../secrets",
            "/etc/passwd",
            "a/../b",
            "bad\0path",
            "",
            r"C:\windows",
            r"agents\..\secret",
            r"a\b",
            ".",
            "./x",
        ] {
            let err =
                validate_rel_path(bad, "artifact_path").expect_err(&format!("must reject {bad:?}"));
            assert_eq!(err.kind(), "invalid_param", "input {bad:?}");
        }
    }

    fn base_params() -> Value {
        json!({
            "plugin_id": "demo-plugin@demo-market",
            "components": ["agents/my-agent.md"],
            "node_ids": ["node-1"],
            "scope": "global",
        })
    }

    #[test]
    fn accepts_relative_normal_component_paths() {
        let result = parse_cherry_pick_params(&base_params());
        assert!(
            result.is_ok(),
            "valid params must parse: {:?}",
            result.err()
        );
    }

    #[test]
    fn rejects_component_path_with_parent_dir() {
        let mut params = base_params();
        params["components"] = json!(["agents/../../etc/passwd"]);
        let err = parse_cherry_pick_params(&params)
            .err()
            .expect("must reject");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn rejects_absolute_component_path() {
        let mut params = base_params();
        params["components"] = json!(["/etc/passwd"]);
        let err = parse_cherry_pick_params(&params)
            .err()
            .expect("must reject");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn rejects_empty_node_ids() {
        let mut params = base_params();
        params["node_ids"] = json!([]);
        let err = parse_cherry_pick_params(&params)
            .err()
            .expect("must reject");
        assert_eq!(err.kind(), "missing_param");
    }

    #[test]
    fn rejects_relative_project_path() {
        let mut params = base_params();
        params["scope"] = json!("project");
        params["project_path"] = json!("relative/path");
        let err = parse_cherry_pick_params(&params)
            .err()
            .expect("must reject");
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn rejects_project_scope_without_project_path() {
        let mut params = base_params();
        params["scope"] = json!("project");
        let err = parse_cherry_pick_params(&params)
            .err()
            .expect("must reject");
        assert_eq!(err.kind(), "missing_param");
    }

    #[test]
    fn update_check_accepts_optional_plugin_id() {
        let parsed =
            parse_update_check_params(&json!({ "plugin_id": "demo-plugin@demo-market" })).unwrap();
        assert_eq!(parsed.plugin_id.as_deref(), Some("demo-plugin@demo-market"));
    }

    #[test]
    fn update_preview_requires_plugin_id() {
        let err = parse_update_preview_params(&json!({}))
            .err()
            .expect("must reject");
        assert_eq!(err.kind(), "missing_param");
    }
}
