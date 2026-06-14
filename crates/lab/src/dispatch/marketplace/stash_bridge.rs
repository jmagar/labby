use std::path::{Path, PathBuf};

use lab_apis::stash::{MarketplaceOrigin, StashComponentKind, StashOrigin};
use serde::Serialize;
use serde_json::Value;

use crate::dispatch::error::ToolError;

pub(super) fn component_name_for_fork(plugin_id: &str, artifact_path: Option<&str>) -> String {
    let raw = match artifact_path {
        Some(path) => format!("{plugin_id}-{path}"),
        None => plugin_id.to_string(),
    };
    let mut out = String::with_capacity(raw.len());
    let mut last_was_dash = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_was_dash {
                out.push(mapped);
            }
            last_was_dash = true;
        } else {
            out.push(mapped);
            last_was_dash = false;
        }
    }
    out.trim_matches('-').chars().take(128).collect()
}

pub(super) fn kind_for_artifact_path(artifact_path: Option<&str>) -> StashComponentKind {
    let Some(path) = artifact_path else {
        return StashComponentKind::Plugin;
    };
    let first = path.split('/').next().unwrap_or(path);
    match first {
        "agents" => StashComponentKind::Agent,
        "commands" => StashComponentKind::Command,
        "hooks" => StashComponentKind::Hook,
        "monitors" => StashComponentKind::Monitor,
        "output-styles" | "output_styles" => StashComponentKind::OutputStyle,
        "themes" => StashComponentKind::Theme,
        "bin" => StashComponentKind::BinFile,
        "settings.json" => StashComponentKind::Settings,
        path if path.ends_with(".mcp.json") => StashComponentKind::McpConfig,
        path if path.ends_with(".lsp.json") => StashComponentKind::LspConfig,
        "skills" => StashComponentKind::Skill,
        _ => StashComponentKind::Skill,
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ForkResult {
    pub plugin_id: String,
    pub component_id: String,
    pub revision_id: String,
    pub stash_workspace: String,
    pub forked_artifacts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForkResponse {
    pub forks: Vec<ForkResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ForkedPluginStatus {
    pub plugin_id: String,
    pub component_id: String,
    pub stash_workspace: String,
    pub forked_artifacts: Vec<String>,
    pub status: String,
}

pub(super) fn fork_source_path(
    plugin_id: &str,
    artifact_path: Option<&str>,
) -> Result<PathBuf, ToolError> {
    let (_marketplace_root, source) =
        crate::dispatch::marketplace::update::source_paths_for_bridge(plugin_id)?;
    match artifact_path {
        Some(path) => {
            crate::dispatch::marketplace::stash_meta::validate_rel_path(path)?;
            let candidate = source.join(path);
            if !candidate.exists() {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("artifact `{path}` not found in `{plugin_id}`"),
                });
            }
            Ok(candidate)
        }
        None => Ok(source),
    }
}

pub(super) fn fork_state_dir(component_id: &str) -> Result<PathBuf, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    Ok(root.join("marketplace").join(component_id))
}

pub(super) async fn fork_artifacts(
    plugin_id: &str,
    artifacts: Option<Vec<String>>,
) -> Result<Value, ToolError> {
    let artifact_paths = artifacts.unwrap_or_else(|| vec![String::new()]);
    let source_version =
        crate::dispatch::marketplace::update::upstream_version_for_bridge(plugin_id).ok();
    let source_fingerprint =
        crate::dispatch::marketplace::update::source_fingerprint_for_bridge(plugin_id).ok();
    let mut forks = Vec::with_capacity(artifact_paths.len());
    let mut warnings = Vec::new();
    for artifact in artifact_paths {
        let artifact_path = if artifact.is_empty() {
            None
        } else {
            Some(artifact.as_str())
        };
        if let Some(existing) = existing_fork(plugin_id, artifact_path)? {
            warnings.push(format!("fork already exists for {plugin_id}:{artifact}"));
            forks.push(existing);
            continue;
        }
        let source_path = fork_source_path(plugin_id, artifact_path)?;
        let name = component_name_for_fork(plugin_id, artifact_path);
        let kind = kind_for_artifact_path(artifact_path);
        let origin = StashOrigin::Marketplace(MarketplaceOrigin {
            plugin_id: plugin_id.to_string(),
            artifact_path: artifact_path.map(ToString::to_string),
            source_version: source_version.clone(),
            source_fingerprint: source_fingerprint.clone(),
        });
        let root = crate::dispatch::stash::client::require_stash_root()?.clone();
        let store = crate::dispatch::stash::store::StashStore::new(root);
        store.ensure_dirs().map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("stash store init: {error}"),
        })?;
        let adopt = crate::dispatch::stash::service::adopt_component_from_path(
            &store,
            stash_kind_param(kind),
            &name,
            Some(&format!("Fork of {plugin_id}")),
            &source_path,
            origin,
            Some(&format!("Fork from {plugin_id}")),
        )
        .await?;
        seed_base_snapshot(&store, &adopt.component.id, &source_path, artifact_path)?;
        forks.push(ForkResult {
            plugin_id: plugin_id.to_string(),
            component_id: adopt.component.id.clone(),
            revision_id: adopt.revision.id.clone(),
            stash_workspace: adopt.component.workspace_root.display().to_string(),
            forked_artifacts: artifact_path
                .map(|path| vec![path.to_string()])
                .unwrap_or_default(),
        });
    }
    crate::dispatch::helpers::to_json(ForkResponse { forks, warnings })
}

pub(super) async fn list_forks(plugin_id: Option<String>) -> Result<Value, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    store.ensure_dirs().map_err(|error| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("stash store init: {error}"),
    })?;
    let mut rows = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if plugin_id.as_ref().is_some_and(|id| id != &origin.plugin_id) {
            continue;
        }
        rows.push(ForkedPluginStatus {
            plugin_id: origin.plugin_id,
            component_id: component.id,
            stash_workspace: component.workspace_root.display().to_string(),
            forked_artifacts: origin.artifact_path.into_iter().collect(),
            status: "unknown".to_string(),
        });
    }
    crate::dispatch::helpers::to_json(rows)
}

#[derive(Debug, Clone)]
pub(super) struct MarketplaceForkComponent {
    pub plugin_id: String,
    pub component_id: String,
    pub artifact_path: Option<String>,
    pub workspace_root: PathBuf,
    pub workspace_dir: PathBuf,
    pub state_dir: PathBuf,
    pub base_revision_id: Option<String>,
    pub upstream_version: String,
}

pub(super) fn fork_component_for_plugin(
    plugin_id: &str,
) -> Result<MarketplaceForkComponent, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id == plugin_id {
            return Ok(MarketplaceForkComponent {
                plugin_id: origin.plugin_id,
                component_id: component.id.clone(),
                artifact_path: origin.artifact_path,
                workspace_root: component.workspace_root.clone(),
                workspace_dir: store.workspace_dir(&component.id),
                state_dir: fork_state_dir(&component.id)?,
                base_revision_id: component.head_revision_id.clone(),
                upstream_version: origin
                    .source_version
                    .unwrap_or_else(|| "unknown".to_string()),
            });
        }
    }
    Err(ToolError::Sdk {
        sdk_kind: "not_found".into(),
        message: format!("no stash fork found for `{plugin_id}`"),
    })
}

fn stash_kind_param(kind: StashComponentKind) -> &'static str {
    match kind {
        StashComponentKind::Skill => "skill",
        StashComponentKind::Agent => "agent",
        StashComponentKind::Command => "command",
        StashComponentKind::Channel => "channel",
        StashComponentKind::Monitor => "monitor",
        StashComponentKind::Hook => "hook",
        StashComponentKind::OutputStyle => "output_style",
        StashComponentKind::Theme => "theme",
        StashComponentKind::Settings => "settings",
        StashComponentKind::McpConfig => "mcp_config",
        StashComponentKind::LspConfig => "lsp_config",
        StashComponentKind::Script => "script",
        StashComponentKind::BinFile => "bin_file",
        StashComponentKind::Plugin => "plugin",
    }
}

fn existing_fork(
    plugin_id: &str,
    artifact_path: Option<&str>,
) -> Result<Option<ForkResult>, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id == plugin_id && origin.artifact_path.as_deref() == artifact_path {
            return Ok(Some(ForkResult {
                plugin_id: origin.plugin_id,
                component_id: component.id,
                revision_id: component.head_revision_id.unwrap_or_default(),
                stash_workspace: component.workspace_root.display().to_string(),
                forked_artifacts: artifact_path
                    .map(|path| vec![path.to_string()])
                    .unwrap_or_default(),
            }));
        }
    }
    Ok(None)
}

fn seed_base_snapshot(
    store: &crate::dispatch::stash::store::StashStore,
    component_id: &str,
    source_path: &Path,
    artifact_path: Option<&str>,
) -> Result<(), ToolError> {
    let state_dir = fork_state_dir(component_id)?;
    let base = state_dir.join("base");
    match artifact_path {
        Some(path) => {
            let dest = base.join(path);
            crate::dispatch::path_safety::reject_symlink(source_path)?;
            if source_path.is_dir() {
                copy_tree_to_base(source_path, &dest, source_path)?;
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(crate::dispatch::marketplace::client::io_internal)?;
                }
                std::fs::copy(source_path, dest)
                    .map_err(crate::dispatch::marketplace::client::io_internal)?;
            }
        }
        None => copy_tree_to_base(source_path, &base, source_path)?,
    }
    let _ = store;
    Ok(())
}

fn copy_tree_to_base(root: &Path, dest_root: &Path, current: &Path) -> Result<(), ToolError> {
    for entry in
        std::fs::read_dir(current).map_err(crate::dispatch::marketplace::client::io_internal)?
    {
        let entry = entry.map_err(crate::dispatch::marketplace::client::io_internal)?;
        let file_type = entry
            .file_type()
            .map_err(crate::dispatch::marketplace::client::io_internal)?;
        if file_type.is_symlink() {
            return Err(ToolError::Sdk {
                sdk_kind: "symlink_rejected".into(),
                message: format!(
                    "symlink `{}` rejected while seeding base snapshot",
                    entry.path().display()
                ),
            });
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_err(crate::dispatch::marketplace::client::io_internal)?
            .to_path_buf();
        let dest = dest_root.join(rel);
        if file_type.is_dir() {
            std::fs::create_dir_all(&dest)
                .map_err(crate::dispatch::marketplace::client::io_internal)?;
            copy_tree_to_base(root, dest_root, &entry.path())?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(crate::dispatch::marketplace::client::io_internal)?;
            }
            std::fs::copy(entry.path(), dest)
                .map_err(crate::dispatch::marketplace::client::io_internal)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_component_name_sanitizes_plugin_and_artifact() {
        assert_eq!(
            component_name_for_fork("demo@labby", Some("skills/demo/SKILL.md")),
            "demo-labby-skills-demo-skill-md"
        );
    }

    #[test]
    fn kind_for_artifact_path_maps_plugin_layout_to_stash_kind() {
        assert_eq!(
            kind_for_artifact_path(Some("skills/demo")),
            StashComponentKind::Skill
        );
        assert_eq!(
            kind_for_artifact_path(Some("agents/demo.md")),
            StashComponentKind::Agent
        );
        assert_eq!(
            kind_for_artifact_path(Some("commands/demo.md")),
            StashComponentKind::Command
        );
        assert_eq!(
            kind_for_artifact_path(Some("settings.json")),
            StashComponentKind::Settings
        );
        assert_eq!(kind_for_artifact_path(None), StashComponentKind::Plugin);
    }
}
