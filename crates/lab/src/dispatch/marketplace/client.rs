//! No external HTTP client — `marketplace` is a local-only synthetic service.
//!
//! This file satisfies the required dispatch service layout contract
//! (every migrated service must have `client.rs`). All work is local
//! filesystem I/O plus optional `tokio::process::Command` shell-out to
//! `claude plugin install/uninstall`.

#![allow(dead_code)]

use std::collections::HashSet;
use std::future::Future;
use std::io::{BufReader, Read};
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::env_non_empty;

/// Abstraction over a WebSocket-backed node RPC channel.
///
/// The concrete impl lives in `api/services/marketplace.rs` (a `NodeRpcPort`
/// that wraps `dispatch::node::send::send_rpc_to_node`). CLI/MCP surfaces
/// that have no direct WS session use `NoopNodeRpcPort`, which returns a
/// structured `not_connected` error for any call.
///
/// lab-zxx5.26: uses native `async fn in trait` (Rust 1.75+) per project
/// convention — no `#[async_trait]`, no hand-rolled `Pin<Box<dyn Future>>`.
pub trait NodeRpcPort: Send + Sync {
    fn send_rpc(
        &self,
        node_id: &str,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, ToolError>> + Send;
}

pub(super) struct NoopNodeRpcPort;

impl NodeRpcPort for NoopNodeRpcPort {
    async fn send_rpc(
        &self,
        node_id: &str,
        _method: &str,
        _params: Value,
    ) -> Result<Value, ToolError> {
        Err(ToolError::Sdk {
            sdk_kind: "not_connected".into(),
            message: format!(
                "node RPC to `{node_id}` is unavailable on this surface (use the HTTP API)"
            ),
        })
    }
}

#[cfg(test)]
static TEST_PLUGINS_ROOT_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);
#[cfg(test)]
static TEST_PLUGINS_ROOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn claude_plugins_root() -> Result<PathBuf, ToolError> {
    plugins_root()
}

pub(super) fn plugins_root() -> Result<PathBuf, ToolError> {
    #[cfg(test)]
    if let Some(path) = TEST_PLUGINS_ROOT_OVERRIDE.lock().unwrap().clone() {
        return Ok(path);
    }

    let home = env_non_empty("HOME").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "HOME env var not set".into(),
    })?;
    Ok(PathBuf::from(home).join(".claude").join("plugins"))
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DeployResult {
    pub ok: bool,
    pub changed: Vec<String>,
    pub skipped: Vec<String>,
    pub removed: Vec<String>,
    pub failed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DeployPreviewResult {
    pub changed: Vec<String>,
    pub skipped: Vec<String>,
    pub removed: Vec<String>,
    pub entries: Vec<DeployPreviewEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct DeployPreviewEntry {
    pub path: String,
    pub status: &'static str,
    #[serde(rename = "beforeContent", skip_serializing_if = "Option::is_none")]
    pub before_content: Option<String>,
    #[serde(rename = "afterContent", skip_serializing_if = "Option::is_none")]
    pub after_content: Option<String>,
}

#[derive(Debug, Default)]
struct SyncPreview {
    changed: Vec<String>,
    skipped: Vec<String>,
    removed: Vec<String>,
    entries: Vec<DeployPreviewEntry>,
}

#[derive(Debug)]
enum FileDiff {
    Unchanged,
    Changed {
        before_content: Option<String>,
        after_content: Option<String>,
    },
}

pub(super) fn sync_workspace_to_target(
    workspace: &Path,
    target: &Path,
) -> Result<DeployResult, ToolError> {
    let mut changed = Vec::new();
    let mut skipped = Vec::new();
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    sync_tree_to_target(
        workspace,
        target,
        workspace,
        &mut changed,
        &mut skipped,
        &mut removed,
        &mut failed,
    )?;
    Ok(DeployResult {
        ok: failed.is_empty(),
        changed,
        skipped,
        removed,
        failed,
        target: Some(target.to_string_lossy().into_owned()),
    })
}

pub(super) fn preview_workspace_sync(
    workspace: &Path,
    target: &Path,
) -> Result<DeployPreviewResult, ToolError> {
    let preview = preview_tree_sync(workspace, target, workspace)?;
    Ok(DeployPreviewResult {
        changed: preview.changed,
        skipped: preview.skipped,
        removed: preview.removed,
        entries: preview.entries,
        target: Some(target.to_string_lossy().into_owned()),
    })
}

fn sync_tree_to_target(
    workspace: &Path,
    target: &Path,
    current: &Path,
    changed: &mut Vec<String>,
    skipped: &mut Vec<String>,
    removed: &mut Vec<String>,
    failed: &mut Vec<String>,
) -> Result<(), ToolError> {
    let current_rel = current.strip_prefix(workspace).unwrap_or(current);
    let current_target = if current_rel.as_os_str().is_empty() {
        target.to_path_buf()
    } else {
        target.join(current_rel)
    };

    std::fs::create_dir_all(&current_target).map_err(io_internal)?;
    let rd = std::fs::read_dir(current).map_err(io_internal)?;
    let mut seen_names = HashSet::new();
    for entry in rd.flatten() {
        let source = entry.path();
        let file_name = entry.file_name();
        seen_names.insert(file_name.clone());
        let rel = source
            .strip_prefix(workspace)
            .unwrap_or(&source)
            .to_string_lossy()
            .into_owned();
        let dest = current_target.join(&file_name);
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(error) => {
                tracing::warn!(
                    service = "marketplace",
                    event = "sync.file_type_failed",
                    path = %source.display(),
                    error = %error,
                    "could not determine file type during sync; marking failed"
                );
                failed.push(rel);
                continue;
            }
        };
        if ft.is_symlink() {
            tracing::warn!(
                service = "marketplace",
                event = "sync.skipped",
                path = %source.display(),
                "skipping symlink during sync"
            );
            continue;
        }
        if ft.is_dir() {
            std::fs::create_dir_all(&dest).map_err(io_internal)?;
            sync_tree_to_target(
                workspace, target, &source, changed, skipped, removed, failed,
            )?;
            continue;
        }

        if files_match_by_metadata_or_content(&source, &dest)? {
            skipped.push(rel);
            continue;
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_internal)?;
        }
        match std::fs::copy(&source, &dest) {
            Ok(_) => changed.push(rel),
            Err(_) => failed.push(rel),
        }
    }

    let target_rd = std::fs::read_dir(&current_target).map_err(io_internal)?;
    for entry in target_rd.flatten() {
        let file_name = entry.file_name();
        if seen_names.contains(&file_name) {
            continue;
        }
        let stale_path = entry.path();
        let stale_rel = stale_path
            .strip_prefix(target)
            .unwrap_or(&stale_path)
            .to_string_lossy()
            .into_owned();
        let removal = if stale_path.is_dir() {
            std::fs::remove_dir_all(&stale_path)
        } else {
            std::fs::remove_file(&stale_path)
        };
        match removal {
            Ok(()) => removed.push(stale_rel),
            Err(_) => failed.push(stale_rel),
        }
    }

    Ok(())
}

fn preview_tree_sync(
    workspace: &Path,
    target: &Path,
    current: &Path,
) -> Result<SyncPreview, ToolError> {
    let current_rel = current.strip_prefix(workspace).unwrap_or(current);
    let current_target = if current_rel.as_os_str().is_empty() {
        target.to_path_buf()
    } else {
        target.join(current_rel)
    };

    let mut preview = SyncPreview::default();
    let rd = std::fs::read_dir(current).map_err(io_internal)?;
    let mut seen_names = HashSet::new();
    for entry in rd.flatten() {
        let source = entry.path();
        let file_name = entry.file_name();
        seen_names.insert(file_name.clone());
        let rel = source
            .strip_prefix(workspace)
            .unwrap_or(&source)
            .to_string_lossy()
            .into_owned();
        let dest = current_target.join(&file_name);
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(error) => {
                tracing::warn!(
                    service = "marketplace",
                    event = "preview.file_type_failed",
                    path = %source.display(),
                    error = %error,
                    rel = %rel,
                    "could not determine file type during preview; entry will be absent from preview"
                );
                continue;
            }
        };
        if ft.is_symlink() {
            tracing::warn!(
                service = "marketplace",
                event = "preview.skipped",
                path = %source.display(),
                "skipping symlink during preview"
            );
            continue;
        }
        if ft.is_dir() {
            let nested = preview_tree_sync(workspace, target, &source)?;
            preview.changed.extend(nested.changed);
            preview.skipped.extend(nested.skipped);
            preview.removed.extend(nested.removed);
            preview.entries.extend(nested.entries);
            continue;
        }

        match preview_file_diff(&source, &dest)? {
            FileDiff::Unchanged => preview.skipped.push(rel),
            FileDiff::Changed {
                before_content,
                after_content,
            } => {
                preview.changed.push(rel.clone());
                preview.entries.push(DeployPreviewEntry {
                    path: rel,
                    status: "changed",
                    before_content,
                    after_content,
                });
            }
        }
    }

    if let Ok(target_rd) = std::fs::read_dir(&current_target) {
        for entry in target_rd.flatten() {
            let file_name = entry.file_name();
            if seen_names.contains(&file_name) {
                continue;
            }
            let stale_path = entry.path();
            let stale_rel = stale_path
                .strip_prefix(target)
                .unwrap_or(&stale_path)
                .to_string_lossy()
                .into_owned();
            preview.removed.push(stale_rel);
            preview.entries.push(DeployPreviewEntry {
                path: stale_path
                    .strip_prefix(target)
                    .unwrap_or(&stale_path)
                    .to_string_lossy()
                    .into_owned(),
                status: "removed",
                before_content: read_text_if_present(&stale_path),
                after_content: None,
            });
        }
    }

    Ok(preview)
}

fn files_match_by_metadata_or_content(source: &Path, dest: &Path) -> Result<bool, ToolError> {
    let source_meta = std::fs::metadata(source).map_err(io_internal)?;
    let Ok(dest_meta) = std::fs::metadata(dest) else {
        return Ok(false);
    };
    if !metadata_size_matches(&source_meta, &dest_meta) {
        return Ok(false);
    }
    if metadata_fast_match(&source_meta, &dest_meta) {
        return Ok(true);
    }
    files_match_by_content(source, dest)
}

fn preview_file_diff(source: &Path, dest: &Path) -> Result<FileDiff, ToolError> {
    let source_meta = std::fs::metadata(source).map_err(io_internal)?;
    let Ok(dest_meta) = std::fs::metadata(dest) else {
        return Ok(FileDiff::Changed {
            before_content: None,
            after_content: read_text_if_present(source),
        });
    };

    if metadata_size_matches(&source_meta, &dest_meta)
        && metadata_fast_match(&source_meta, &dest_meta)
    {
        return Ok(FileDiff::Unchanged);
    }

    let source_bytes = std::fs::read(source).map_err(io_internal)?;
    let dest_bytes = std::fs::read(dest).map_err(io_internal)?;
    if source_bytes == dest_bytes {
        return Ok(FileDiff::Unchanged);
    }
    Ok(FileDiff::Changed {
        before_content: String::from_utf8(dest_bytes).ok(),
        after_content: String::from_utf8(source_bytes).ok(),
    })
}

fn metadata_size_matches(source_meta: &std::fs::Metadata, dest_meta: &std::fs::Metadata) -> bool {
    source_meta.len() == dest_meta.len()
}

fn metadata_fast_match(source_meta: &std::fs::Metadata, dest_meta: &std::fs::Metadata) -> bool {
    same_file_metadata(source_meta, dest_meta)
        || matching_modified_time(source_meta.modified(), dest_meta.modified())
}

#[cfg(unix)]
fn same_file_metadata(source_meta: &std::fs::Metadata, dest_meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    source_meta.dev() == dest_meta.dev() && source_meta.ino() == dest_meta.ino()
}

#[cfg(not(unix))]
fn same_file_metadata(_source_meta: &std::fs::Metadata, _dest_meta: &std::fs::Metadata) -> bool {
    false
}

fn matching_modified_time(
    source_modified: std::io::Result<SystemTime>,
    dest_modified: std::io::Result<SystemTime>,
) -> bool {
    matches!((source_modified, dest_modified), (Ok(source), Ok(dest)) if source == dest)
}

fn files_match_by_content(source: &Path, dest: &Path) -> Result<bool, ToolError> {
    #[cfg(test)]
    BYTE_COMPARE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let source_file = std::fs::File::open(source).map_err(io_internal)?;
    let dest_file = std::fs::File::open(dest).map_err(io_internal)?;
    let mut source_reader = BufReader::new(source_file);
    let mut dest_reader = BufReader::new(dest_file);
    let mut source_buf = [0_u8; 8192];
    let mut dest_buf = [0_u8; 8192];

    loop {
        let source_read = source_reader.read(&mut source_buf).map_err(io_internal)?;
        let dest_read = dest_reader.read(&mut dest_buf).map_err(io_internal)?;
        if source_read != dest_read {
            return Ok(false);
        }
        if source_read == 0 {
            return Ok(true);
        }
        if source_buf[..source_read] != dest_buf[..dest_read] {
            return Ok(false);
        }
    }
}

#[cfg(test)]
static BYTE_COMPARE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn read_text_if_present(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub(crate) use super::dispatch::walk_artifacts;

pub(crate) fn home_dir() -> Result<PathBuf, ToolError> {
    crate::config::home_dir().ok_or_else(|| io_internal("HOME env var not set"))
}

pub(crate) fn codex_config_path() -> Result<PathBuf, ToolError> {
    Ok(home_dir()?.join(".codex").join("config.toml"))
}

pub(crate) fn codex_cache_root() -> Result<PathBuf, ToolError> {
    Ok(home_dir()?.join(".codex").join("cache"))
}

pub(crate) fn io_internal(error: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: error.to_string(),
    }
}

#[cfg(test)]
pub(super) fn with_test_plugins_root<T>(home: &Path, run: impl FnOnce() -> T) -> T {
    // Recover from a poisoned serialization lock so that a panicking test
    // does not permanently block subsequent tests.
    let _guard = TEST_PLUGINS_ROOT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let plugins_root = home.join(".claude").join("plugins");
    let previous = {
        let mut slot = TEST_PLUGINS_ROOT_OVERRIDE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (*slot).replace(plugins_root)
    };

    // RAII guard: restore the override slot in `Drop` so that a panicking
    // `run()` closure never leaves stale global state.
    struct RestoreGuard(Option<PathBuf>);
    impl Drop for RestoreGuard {
        fn drop(&mut self) {
            *TEST_PLUGINS_ROOT_OVERRIDE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = self.0.take();
        }
    }
    let _restore = RestoreGuard(previous);

    run()
}

#[cfg(test)]
pub(super) fn test_plugins_home_override() -> Option<PathBuf> {
    TEST_PLUGINS_ROOT_OVERRIDE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|plugins| {
            plugins
                .parent()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use tempfile::tempdir;

    #[test]
    fn preview_sync_uses_metadata_fast_path_without_byte_compare_when_files_are_same() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let source = workspace.join("plugin.json");
        let dest = target.join("plugin.json");
        std::fs::write(&source, r#"{"name":"demo"}"#).unwrap();
        std::fs::hard_link(&source, &dest).unwrap();

        BYTE_COMPARE_CALLS.store(0, Ordering::SeqCst);
        let preview = preview_workspace_sync(&workspace, &target).unwrap();

        assert_eq!(preview.skipped, vec!["plugin.json"]);
        assert!(preview.changed.is_empty());
        assert_eq!(BYTE_COMPARE_CALLS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn deploy_sync_uses_metadata_fast_path_without_byte_compare_when_files_are_same() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let source = workspace.join("plugin.json");
        let dest = target.join("plugin.json");
        std::fs::write(&source, r#"{"name":"demo"}"#).unwrap();
        std::fs::hard_link(&source, &dest).unwrap();

        BYTE_COMPARE_CALLS.store(0, Ordering::SeqCst);
        let result = sync_workspace_to_target(&workspace, &target).unwrap();

        assert_eq!(result.skipped, vec!["plugin.json"]);
        assert!(result.changed.is_empty());
        assert_eq!(BYTE_COMPARE_CALLS.load(Ordering::SeqCst), 0);
    }
}
