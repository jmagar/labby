//! Typed param parsers for `stash` dispatch actions.
//!
//! Each parser corresponds to one action in `catalog::ACTIONS`. Parsers return
//! a typed struct so `dispatch.rs` never hand-rolls param extraction inline.

use std::path::PathBuf;

use labby_apis::stash::StashOrigin;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{optional_str, require_str};

// ─── Component lifecycle ─────────────────────────────────────────────────────

/// `component.get` — single required `id`.
pub struct GetParams {
    pub id: String,
}

pub fn parse_get_params(params: &Value) -> Result<GetParams, ToolError> {
    Ok(GetParams {
        id: require_str(params, "id")?.to_string(),
    })
}

/// `component.create` — required `kind` + `name`, optional `label`.
pub struct CreateParams {
    pub kind: String,
    pub name: String,
    pub label: Option<String>,
}

pub fn parse_create_params(params: &Value) -> Result<CreateParams, ToolError> {
    Ok(CreateParams {
        kind: require_str(params, "kind")?.to_string(),
        name: require_str(params, "name")?.to_string(),
        label: optional_str(params, "label")?.map(str::to_string),
    })
}

/// `component.import` — required `id` + `source_path`, optional `kind`.
pub struct ImportParams {
    pub id: String,
    pub source_path: PathBuf,
    pub kind: Option<String>,
}

pub fn parse_import_params(params: &Value) -> Result<ImportParams, ToolError> {
    Ok(ImportParams {
        id: require_str(params, "id")?.to_string(),
        source_path: required_absolute_path(params, "source_path")?,
        kind: optional_str(params, "kind")?.map(str::to_string),
    })
}

/// `component.adopt` - create, import, attach origin metadata, and save.
pub struct AdoptParams {
    pub kind: String,
    pub name: String,
    pub label: Option<String>,
    pub source_path: PathBuf,
    pub origin: StashOrigin,
    pub save_label: Option<String>,
}

pub fn parse_adopt_params(params: &Value) -> Result<AdoptParams, ToolError> {
    let path = required_absolute_path(params, "source_path")?;
    let origin_value = params
        .get("origin")
        .cloned()
        .ok_or_else(|| ToolError::MissingParam {
            param: "origin".to_string(),
            message: "`origin` is required".to_string(),
        })?;
    let origin: StashOrigin =
        serde_json::from_value(origin_value).map_err(|error| ToolError::InvalidParam {
            param: "origin".to_string(),
            message: format!("origin is invalid: {error}"),
        })?;
    if let StashOrigin::LocalPath { source_path } = &origin
        && !source_path.is_absolute()
    {
        return Err(ToolError::InvalidParam {
            param: "origin".to_string(),
            message: "origin.source_path must be an absolute path".to_string(),
        });
    }
    Ok(AdoptParams {
        kind: require_str(params, "kind")?.to_string(),
        name: require_str(params, "name")?.to_string(),
        label: optional_str(params, "label")?.map(str::to_string),
        source_path: path,
        origin,
        save_label: optional_str(params, "save_label")?.map(str::to_string),
    })
}

fn required_absolute_path(params: &Value, name: &str) -> Result<PathBuf, ToolError> {
    let value = require_str(params, name)?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: format!("{name} must be an absolute path"),
            param: name.to_string(),
        });
    }
    Ok(path)
}

/// `component.workspace` — single required `id`.
pub struct WorkspaceParams {
    pub id: String,
}

pub fn parse_workspace_params(params: &Value) -> Result<WorkspaceParams, ToolError> {
    Ok(WorkspaceParams {
        id: require_str(params, "id")?.to_string(),
    })
}

/// `component.save` — required `id`, optional `label`.
pub struct SaveParams {
    pub id: String,
    pub label: Option<String>,
}

pub fn parse_save_params(params: &Value) -> Result<SaveParams, ToolError> {
    Ok(SaveParams {
        id: require_str(params, "id")?.to_string(),
        label: optional_str(params, "label")?.map(str::to_string),
    })
}

/// `component.revisions` — single required `id`.
pub struct RevisionsParams {
    pub id: String,
}

pub fn parse_revisions_params(params: &Value) -> Result<RevisionsParams, ToolError> {
    Ok(RevisionsParams {
        id: require_str(params, "id")?.to_string(),
    })
}

/// `component.export` — required `id` + `output_path`, optional bool flags.
pub struct ExportParams {
    pub id: String,
    pub output_path: PathBuf,
    pub include_secrets: bool,
    pub force: bool,
}

pub fn parse_export_params(params: &Value) -> Result<ExportParams, ToolError> {
    let output_path = require_str(params, "output_path")?;
    let path = PathBuf::from(output_path);
    if !path.is_absolute() {
        return Err(ToolError::InvalidParam {
            message: "output_path must be an absolute path".to_string(),
            param: "output_path".to_string(),
        });
    }
    let include_secrets = params
        .get("include_secrets")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let force = params
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(ExportParams {
        id: require_str(params, "id")?.to_string(),
        output_path: path,
        include_secrets,
        force,
    })
}

/// `component.deploy` — required `id` + `target_id`, optional `revision_id`.
pub struct DeployParams {
    pub id: String,
    pub target_id: String,
    pub revision_id: Option<String>,
}

pub fn parse_deploy_params(params: &Value) -> Result<DeployParams, ToolError> {
    Ok(DeployParams {
        id: require_str(params, "id")?.to_string(),
        target_id: require_str(params, "target_id")?.to_string(),
        revision_id: optional_str(params, "revision_id")?.map(str::to_string),
    })
}

// ─── Provider sync ────────────────────────────────────────────────────────────

/// `provider.link` — required `id`, `kind`, `label`, `config`.
pub struct LinkParams {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub config: Value,
}

pub fn parse_link_params(params: &Value) -> Result<LinkParams, ToolError> {
    let config = params.get("config").cloned().unwrap_or(Value::Null);
    if !config.is_object() {
        return Err(ToolError::InvalidParam {
            message: "config must be a JSON object".to_string(),
            param: "config".to_string(),
        });
    }
    Ok(LinkParams {
        id: require_str(params, "id")?.to_string(),
        kind: require_str(params, "kind")?.to_string(),
        label: require_str(params, "label")?.to_string(),
        config,
    })
}

/// `provider.push` / `provider.pull` — required `id` + `provider_id`.
pub struct ProviderSyncParams {
    pub id: String,
    pub provider_id: String,
}

pub fn parse_provider_sync_params(params: &Value) -> Result<ProviderSyncParams, ToolError> {
    Ok(ProviderSyncParams {
        id: require_str(params, "id")?.to_string(),
        provider_id: require_str(params, "provider_id")?.to_string(),
    })
}

// ─── Deploy targets ───────────────────────────────────────────────────────────

/// `target.add` — required `name` + `kind`, optional `path` / `gateway_id`.
pub struct TargetAddParams {
    pub name: String,
    pub kind: String,
    pub path: Option<PathBuf>,
    pub gateway_id: Option<String>,
}

pub fn parse_target_add_params(params: &Value) -> Result<TargetAddParams, ToolError> {
    let path = optional_str(params, "path")?.map(|s| PathBuf::from(s));
    // If path is provided it must be absolute.
    if let Some(ref p) = path {
        if !p.is_absolute() {
            return Err(ToolError::InvalidParam {
                message: "path must be an absolute path".to_string(),
                param: "path".to_string(),
            });
        }
    }
    Ok(TargetAddParams {
        name: require_str(params, "name")?.to_string(),
        kind: require_str(params, "kind")?.to_string(),
        path,
        gateway_id: optional_str(params, "gateway_id")?.map(str::to_string),
    })
}

/// `target.remove` — single required `id`.
pub struct TargetRemoveParams {
    pub id: String,
}

pub fn parse_target_remove_params(params: &Value) -> Result<TargetRemoveParams, ToolError> {
    Ok(TargetRemoveParams {
        id: require_str(params, "id")?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // `required_absolute_path` uses OS-semantic `Path::is_absolute()`, which is
    // correct for real host paths: a Unix `/tmp/demo` is not absolute on Windows
    // (no drive prefix). Use a platform-appropriate absolute path so the test
    // exercises the accept/reject contract on every OS.
    #[cfg(windows)]
    const ABS_PATH: &str = r"C:\tmp\demo";
    #[cfg(not(windows))]
    const ABS_PATH: &str = "/tmp/demo";

    #[test]
    fn parse_adopt_rejects_relative_origin_local_path() {
        let error = match parse_adopt_params(&json!({
            "kind": "skill",
            "name": "demo",
            "source_path": ABS_PATH,
            "origin": {
                "kind": "local_path",
                "source_path": "relative/demo"
            }
        })) {
            Ok(_) => panic!("relative origin local path should be rejected"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), "invalid_param");
    }

    #[test]
    fn parse_adopt_accepts_absolute_origin_local_path() {
        let params = parse_adopt_params(&json!({
            "kind": "skill",
            "name": "demo",
            "source_path": ABS_PATH,
            "origin": {
                "kind": "local_path",
                "source_path": ABS_PATH
            }
        }))
        .unwrap();

        assert_eq!(params.source_path, PathBuf::from(ABS_PATH));
    }
}
