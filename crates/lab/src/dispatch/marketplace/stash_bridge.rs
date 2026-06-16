//! Marketplace ↔ stash fork bridge.
//!
//! This module is the sanctioned cross-service seam between the marketplace
//! service and the stash store (the `(marketplace, stash)` edge is allow-listed
//! in `crates/lab/tests/architecture_orchestrator.rs`). It forks plugin/artifact
//! content into the stash service as first-class `StashOrigin::Marketplace`
//! components and seeds an upstream `base` snapshot used later for three-way
//! merge.
//!
//! Fork lifecycle: `adopt_component_from_path` (stash service) → normalize the
//! component workspace → seed the `<stash_root>/marketplace/<component_id>/base`
//! snapshot → read the component back. All blocking store/FS work runs under
//! `spawn_blocking`; the origin lock (`acquire_origin_lock`) serializes
//! concurrent fork/update operations on the same plugin.
//!
//! Durable fork state lives as the component record plus that sidecar — NOT in
//! the legacy `.stash.json` workspace layout (see `update.rs::collect_forks` for
//! the read-only legacy discovery path retained for back-compat).

use std::path::{Path, PathBuf};

use lab_apis::stash::{MarketplaceOrigin, StashComponentKind, StashOrigin};
use serde::Serialize;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::client::join_err;

/// Run a best-effort cleanup step, logging at WARN on failure instead of
/// silently swallowing the error. Used on rollback/cleanup paths where the
/// primary error is already being propagated but an orphaned component or state
/// directory would otherwise be invisible.
fn best_effort<E: std::fmt::Display>(label: &str, result: Result<(), E>) {
    if let Err(error) = result {
        tracing::warn!(error = %error, "{label}");
    }
}

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
    /// The stash revision recorded for this fork. `None` when an existing fork
    /// has no head revision yet — serialized as JSON `null` rather than an empty
    /// string so callers can distinguish "no revision" from a real id.
    pub revision_id: Option<String>,
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
            crate::dispatch::marketplace::params::validate_rel_path(path, "artifact_path")?;
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

/// RAII guard holding an OS advisory lock that serializes fork/update
/// operations on a single plugin origin. Held while the file descriptor is open
/// and released by the kernel on close — including on process crash — so a
/// killed operation cannot permanently wedge the fork on `conflict` (unlike a
/// `create_new` sentinel file, which leaks).
#[derive(Debug)]
struct OriginLock {
    _file: std::fs::File,
}

fn acquire_origin_lock(
    plugin_id: &str,
    artifact_path: Option<&str>,
) -> Result<OriginLock, ToolError> {
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let lock_dir = root.join("marketplace").join(".locks");
    std::fs::create_dir_all(&lock_dir)
        .map_err(crate::dispatch::marketplace::client::io_internal)?;
    let key = component_name_for_fork(plugin_id, artifact_path);
    let path = lock_dir.join(format!("{key}.lock"));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(crate::dispatch::marketplace::client::io_internal)?;
    // `File::try_lock` (stable since Rust 1.89): non-blocking advisory lock,
    // auto-released by the kernel on fd close (incl. crash). `Err(WouldBlock)`
    // means another holder has it.
    match file.try_lock() {
        Ok(()) => Ok(OriginLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => Err(ToolError::Sdk {
            sdk_kind: "conflict".into(),
            message: format!("fork for `{plugin_id}` is locked by another operation"),
        }),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(crate::dispatch::marketplace::client::io_internal(error))
        }
    }
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
    let root = tokio::task::spawn_blocking(|| {
        let root = crate::dispatch::stash::client::require_stash_root()?.clone();
        let store = crate::dispatch::stash::store::StashStore::new(root.clone());
        store.ensure_dirs().map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("stash store init: {error}"),
        })?;
        Ok::<_, ToolError>(root)
    })
    .await
    .map_err(join_err)??;
    let store = crate::dispatch::stash::store::StashStore::new(root);
    let mut forks = Vec::with_capacity(artifact_paths.len());
    let mut warnings = Vec::new();
    let mut created_component_ids = Vec::new();
    for artifact in artifact_paths {
        let artifact_path = if artifact.is_empty() {
            None
        } else {
            Some(artifact.as_str())
        };
        let preflight = {
            let plugin_id = plugin_id.to_string();
            let artifact_path = artifact_path.map(ToString::to_string);
            tokio::task::spawn_blocking(move || {
                let lock = acquire_origin_lock(&plugin_id, artifact_path.as_deref())?;
                let existing = existing_fork(&plugin_id, artifact_path.as_deref())?;
                let source_path = if existing.is_some() {
                    None
                } else {
                    Some(fork_source_path(&plugin_id, artifact_path.as_deref())?)
                };
                Ok::<_, ToolError>((lock, existing, source_path))
            })
            .await
            .map_err(join_err)
        };
        let (_lock, existing, source_path) = match preflight {
            Ok(Ok(preflight)) => preflight,
            Ok(Err(error)) => {
                cleanup_created_forks(&store, &created_component_ids);
                return Err(error);
            }
            Err(error) => {
                cleanup_created_forks(&store, &created_component_ids);
                return Err(error);
            }
        };
        if let Some(existing) = existing {
            warnings.push(format!("fork already exists for {plugin_id}:{artifact}"));
            forks.push(existing);
            continue;
        }
        let Some(source_path) = source_path else {
            cleanup_created_forks(&store, &created_component_ids);
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("fork preflight returned no source path for `{plugin_id}`"),
            });
        };
        let name = component_name_for_fork(plugin_id, artifact_path);
        let kind = kind_for_artifact_path(artifact_path);
        let origin = StashOrigin::Marketplace(MarketplaceOrigin {
            plugin_id: plugin_id.to_string(),
            artifact_path: artifact_path.map(ToString::to_string),
            source_version: source_version.clone(),
            source_fingerprint: source_fingerprint.clone(),
        });
        let adopt = match crate::dispatch::stash::service::adopt_component_from_path(
            &store,
            stash_kind_param(kind),
            &name,
            Some(&format!("Fork of {plugin_id}")),
            &source_path,
            origin,
            Some(&format!("Fork from {plugin_id}")),
        )
        .await
        {
            Ok(adopt) => adopt,
            Err(error) => {
                cleanup_created_forks(&store, &created_component_ids);
                return Err(error);
            }
        };
        let setup = async {
            let revision = normalize_marketplace_workspace(
                &store,
                &adopt.component.id,
                source_path.clone(),
                artifact_path.map(ToString::to_string),
                &format!("Fork from {plugin_id}"),
            )
            .await?
            .unwrap_or(adopt.revision.clone());
            let component_id = adopt.component.id.clone();
            let source_path = source_path.clone();
            let artifact_path = artifact_path.map(ToString::to_string);
            tokio::task::spawn_blocking(move || {
                seed_base_snapshot(&component_id, &source_path, artifact_path.as_deref())
            })
            .await
            .map_err(join_err)??;
            Ok::<_, ToolError>(revision)
        }
        .await;
        let revision = match setup {
            Ok(revision) => revision,
            Err(error) => {
                best_effort(
                    "failed to delete component during fork rollback",
                    store.delete_component(&adopt.component.id),
                );
                if let Ok(state) = fork_state_dir(&adopt.component.id)
                    && state.exists()
                {
                    best_effort(
                        "failed to remove fork state dir during rollback",
                        std::fs::remove_dir_all(state),
                    );
                }
                cleanup_created_forks(&store, &created_component_ids);
                return Err(error);
            }
        };
        let component_id = adopt.component.id.clone();
        let store_for_blocking = store.clone();
        let component = match tokio::task::spawn_blocking(move || {
            store_for_blocking
                .read_component(&component_id)
                .and_then(|component| {
                    component.ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "not_found".into(),
                        message: format!(
                            "component `{component_id}` missing after marketplace fork"
                        ),
                    })
                })
        })
        .await
        .map_err(join_err)
        {
            Ok(Ok(component)) => component,
            Ok(Err(error)) | Err(error) => {
                best_effort(
                    "failed to delete component during fork rollback",
                    store.delete_component(&adopt.component.id),
                );
                cleanup_created_forks(&store, &created_component_ids);
                return Err(error);
            }
        };
        created_component_ids.push(component.id.clone());
        tracing::info!(
            plugin_id,
            component_id = %component.id,
            artifact = artifact_path.unwrap_or("<plugin>"),
            "marketplace fork created as stash component"
        );
        forks.push(ForkResult {
            plugin_id: plugin_id.to_string(),
            component_id: component.id.clone(),
            revision_id: Some(revision.id.clone()),
            stash_workspace: component.workspace_root.display().to_string(),
            forked_artifacts: artifact_path
                .map(|path| vec![path.to_string()])
                .unwrap_or_default(),
        });
    }
    crate::dispatch::helpers::to_json(ForkResponse { forks, warnings })
}

fn cleanup_created_forks(
    store: &crate::dispatch::stash::store::StashStore,
    component_ids: &[String],
) {
    for component_id in component_ids.iter().rev() {
        best_effort(
            "failed to delete component during fork cleanup",
            store.delete_component(component_id),
        );
        if let Ok(state) = fork_state_dir(component_id)
            && state.exists()
        {
            best_effort(
                "failed to remove fork state dir during cleanup",
                std::fs::remove_dir_all(state),
            );
        }
    }
}

pub(super) async fn list_forks(plugin_id: Option<String>) -> Result<Value, ToolError> {
    tokio::task::spawn_blocking(move || list_forks_blocking(plugin_id))
        .await
        .map_err(join_err)?
}

fn list_forks_blocking(plugin_id: Option<String>) -> Result<Value, ToolError> {
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
        let status = fork_drift_status(
            &component.id,
            &component.workspace_root,
            origin.artifact_path.as_deref(),
        );
        rows.push(ForkedPluginStatus {
            plugin_id: origin.plugin_id,
            component_id: component.id,
            stash_workspace: component.workspace_root.display().to_string(),
            forked_artifacts: origin.artifact_path.into_iter().collect(),
            status,
        });
    }
    crate::dispatch::helpers::to_json(rows)
}

/// Compute the drift status of a fork by comparing its `base` snapshot (the last
/// forked/applied upstream content) against the live stash workspace.
///
/// Returns `"clean"` (workspace matches base), `"dirty"` (user has diverged from
/// base), `"base_missing"` (no base snapshot to compare against), or `"unknown"`
/// (the comparison could not be completed — e.g. an I/O error).
fn fork_drift_status(
    component_id: &str,
    workspace_root: &Path,
    artifact_path: Option<&str>,
) -> String {
    let Ok(state_dir) = fork_state_dir(component_id) else {
        return "unknown".to_string();
    };
    let base_root = state_dir.join("base");
    if !base_root.exists() {
        return "base_missing".to_string();
    }
    // `workspace_root` already resolves to the live artifact (file-shaped
    // component) or the plugin tree (directory-shaped), so it is the work side
    // for both. Only the base side is namespaced by `artifact_path`, since the
    // base snapshot mirrors the source layout under `base/`.
    //
    // `artifact_path` is read back from the persisted stash component record, so
    // re-validate it before joining — a tampered/restored record must not turn
    // this into an arbitrary-file-read oracle via `base_root.join("../..")`.
    let (base_path, work_path) = match artifact_path {
        Some(path) => {
            if crate::dispatch::marketplace::params::validate_rel_path(path, "artifact_path")
                .is_err()
            {
                tracing::warn!(
                    component_id,
                    "stored artifact_path failed re-validation; reporting drift as unknown"
                );
                return "unknown".to_string();
            }
            (base_root.join(path), workspace_root.to_path_buf())
        }
        None => (base_root, workspace_root.to_path_buf()),
    };
    match paths_differ(&base_path, &work_path) {
        Ok(true) => "dirty".to_string(),
        Ok(false) => "clean".to_string(),
        // The comparison genuinely could not complete (e.g. an unreadable stash
        // workspace). Surface "unknown" to the caller but log so the I/O failure
        // is diagnosable rather than silently indistinguishable from a derive
        // failure above.
        Err(error) => {
            tracing::warn!(
                component_id,
                error = %error,
                "fork drift comparison failed; reporting status as unknown"
            );
            "unknown".to_string()
        }
    }
}

/// Whether `base` and `work` differ in structure or content. A `work` file that
/// does not exist counts as a difference; any other I/O error is propagated.
///
/// Symlinks encountered while walking a directory are ignored — the fork flow
/// never seeds symlinks into `base`, so this only affects a workspace symlink the
/// user added, which is left out of the (advisory) drift comparison.
fn paths_differ(base: &Path, work: &Path) -> std::io::Result<bool> {
    let meta = std::fs::symlink_metadata(base)?;
    // Resolve the work side once. A missing work counterpart, or one whose shape
    // (file vs directory) does not match base, is structural drift.
    let work_meta = match std::fs::symlink_metadata(work) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if meta.is_dir() {
        if !work_meta.is_dir() {
            return Ok(true);
        }
        let mut base_files = std::collections::BTreeMap::new();
        collect_rel_files(base, base, 0, &mut base_files)?;
        let mut work_files = std::collections::BTreeMap::new();
        collect_rel_files(work, work, 0, &mut work_files)?;
        if !base_files.keys().eq(work_files.keys()) {
            return Ok(true);
        }
        for (rel, base_abs) in &base_files {
            let Some(work_abs) = work_files.get(rel) else {
                return Ok(true);
            };
            if bytes_differ(base_abs, work_abs)? {
                return Ok(true);
            }
        }
        Ok(false)
    } else {
        // base is a file: a non-file work counterpart is drift.
        if !work_meta.is_file() {
            return Ok(true);
        }
        bytes_differ(base, work)
    }
}

/// Per-file cap for the drift byte-compare. Files larger than this are treated
/// as equal rather than read into memory — drift status is advisory and not
/// worth an OOM risk on a fork that adopted a large blob.
const MAX_DRIFT_COMPARE_BYTES: u64 = 16 * 1024 * 1024;

/// Whether two regular files differ in content. Short-circuits on a size
/// mismatch (no read) and skips the byte read entirely for oversized files.
fn bytes_differ(a: &Path, b: &Path) -> std::io::Result<bool> {
    let len_a = std::fs::metadata(a)?.len();
    let len_b = std::fs::metadata(b)?.len();
    if len_a != len_b {
        return Ok(true);
    }
    if len_a > MAX_DRIFT_COMPARE_BYTES {
        return Ok(false);
    }
    Ok(std::fs::read(a)? != std::fs::read(b)?)
}

/// Bound on directory recursion depth for the drift walk — defends against a
/// pathologically deep workspace tree causing a stack overflow.
const MAX_DRIFT_WALK_DEPTH: usize = 64;

/// Recursively collect regular files under `dir`, keyed by their path relative
/// to `root`. Skips symlinks; propagates I/O errors; stops descending past
/// [`MAX_DRIFT_WALK_DEPTH`].
fn collect_rel_files(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut std::collections::BTreeMap<PathBuf, PathBuf>,
) -> std::io::Result<()> {
    if depth >= MAX_DRIFT_WALK_DEPTH {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_rel_files(root, &path, depth + 1, out)?;
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.insert(rel.to_path_buf(), path.clone());
        }
    }
    Ok(())
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
    let mut matches = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id == plugin_id {
            matches.push(MarketplaceForkComponent {
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
    if matches.len() > 1 {
        return Err(ToolError::Sdk {
            sdk_kind: "conflict".into(),
            message: format!(
                "multiple stash forks found for `{plugin_id}`; use artifact-specific update actions"
            ),
        });
    }
    if let Some(component) = matches.into_iter().next() {
        return Ok(component);
    }
    Err(ToolError::Sdk {
        sdk_kind: "not_found".into(),
        message: format!("no stash fork found for `{plugin_id}`"),
    })
}

#[derive(Debug, Serialize)]
pub(super) struct UnforkResult {
    pub plugin_id: String,
    pub removed_component_ids: Vec<String>,
}

pub(super) async fn unfork(
    plugin_id: &str,
    artifacts: Option<Vec<String>>,
) -> Result<Value, ToolError> {
    let plugin_id = plugin_id.to_string();
    tokio::task::spawn_blocking(move || unfork_blocking(&plugin_id, artifacts))
        .await
        .map_err(join_err)?
}

fn unfork_blocking(plugin_id: &str, artifacts: Option<Vec<String>>) -> Result<Value, ToolError> {
    // Serialize destructive lifecycle mutations on this plugin against each
    // other (unfork vs reset vs a plugin-level fork) via the plugin-scoped
    // origin lock. (Per-artifact fork ops use a different key; full unification
    // across all four operations is a larger follow-up.)
    let _lock = acquire_origin_lock(plugin_id, None)?;
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    let mut removed = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id != plugin_id {
            continue;
        }
        if let Some(filter) = &artifacts {
            let Some(path) = origin.artifact_path.as_ref() else {
                continue;
            };
            if !filter.iter().any(|candidate| candidate == path) {
                continue;
            }
        }
        let state = fork_state_dir(&component.id)?;
        if state.exists() {
            std::fs::remove_dir_all(&state)
                .map_err(crate::dispatch::marketplace::client::io_internal)?;
        }
        store.delete_component(&component.id)?;
        removed.push(component.id);
    }
    crate::dispatch::helpers::to_json(UnforkResult {
        plugin_id: plugin_id.to_string(),
        removed_component_ids: removed,
    })
}

#[derive(Debug, Serialize)]
pub(super) struct ResetResult {
    pub plugin_id: String,
    pub reset_artifacts: Vec<String>,
    pub revision_ids: Vec<String>,
}

struct ResetWork {
    store: crate::dispatch::stash::store::StashStore,
    component_ids: Vec<String>,
    reset_artifacts: Vec<String>,
}

pub(super) async fn reset(
    plugin_id: &str,
    artifacts: Option<Vec<String>>,
) -> Result<Value, ToolError> {
    let plugin_id = plugin_id.to_string();
    let work = tokio::task::spawn_blocking({
        let plugin_id = plugin_id.clone();
        move || reset_workspaces_blocking(&plugin_id, artifacts)
    })
    .await
    .map_err(join_err)??;
    let mut revision_ids = Vec::new();
    for component_id in &work.component_ids {
        let revision = crate::dispatch::stash::revision::save_revision(
            &work.store,
            component_id,
            Some("Reset to marketplace base"),
        )
        .await?;
        revision_ids.push(revision.id);
    }
    crate::dispatch::helpers::to_json(ResetResult {
        plugin_id,
        reset_artifacts: work.reset_artifacts,
        revision_ids,
    })
}

fn reset_workspaces_blocking(
    plugin_id: &str,
    artifacts: Option<Vec<String>>,
) -> Result<ResetWork, ToolError> {
    // Plugin-scoped origin lock — serializes reset against unfork and a
    // plugin-level fork (see `unfork_blocking`).
    let _lock = acquire_origin_lock(plugin_id, None)?;
    let root = crate::dispatch::stash::client::require_stash_root()?.clone();
    let store = crate::dispatch::stash::store::StashStore::new(root);
    let mut reset_artifacts = Vec::new();
    let mut selected = Vec::new();
    for component in store.list_components()? {
        let Some(StashOrigin::Marketplace(origin)) = component.origin_meta.clone() else {
            continue;
        };
        if origin.plugin_id != plugin_id {
            continue;
        }
        if let Some(filter) = &artifacts {
            match origin.artifact_path.as_ref() {
                Some(path) if filter.iter().any(|candidate| candidate == path) => {}
                Some(_) | None => continue,
            }
        }
        selected.push((component, origin));
    }
    if selected.is_empty() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: match &artifacts {
                Some(paths) => format!(
                    "no stash forks found for `{plugin_id}` matching artifact(s): {}",
                    paths.join(", ")
                ),
                None => format!("no stash forks found for `{plugin_id}`"),
            },
        });
    }
    let mut component_ids = Vec::with_capacity(selected.len());
    for (component, origin) in selected {
        let workspace = store.workspace_dir(&component.id);
        let base = fork_state_dir(&component.id)?.join("base");
        let paths: Vec<&str> = match &artifacts {
            Some(paths) => paths
                .iter()
                .filter_map(|path| {
                    (origin.artifact_path.as_deref() == Some(path.as_str()))
                        .then_some(path.as_str())
                })
                .collect(),
            None => origin.artifact_path.as_deref().into_iter().collect(),
        };
        if paths.is_empty() {
            reset_artifacts.extend(replace_workspace_from_base(&base, &workspace)?);
        } else {
            for rel in paths {
                crate::dispatch::marketplace::params::validate_rel_path(rel, "artifact_path")?;
                reset_artifacts.extend(reset_one_path_from_base(&base, &workspace, rel)?);
            }
        }
        component_ids.push(component.id);
    }
    Ok(ResetWork {
        store,
        component_ids,
        reset_artifacts,
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
                revision_id: component.head_revision_id,
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
    component_id: &str,
    source_path: &Path,
    artifact_path: Option<&str>,
) -> Result<(), ToolError> {
    let state_dir = fork_state_dir(component_id)?;
    let base = state_dir.join("base");
    match artifact_path {
        Some(path) => {
            let dest = base.join(path);
            copy_artifact_source(source_path, &dest)?;
        }
        None => copy_tree_to_base(source_path, &base, source_path)?,
    }
    Ok(())
}

async fn normalize_marketplace_workspace(
    store: &crate::dispatch::stash::store::StashStore,
    component_id: &str,
    source_path: PathBuf,
    artifact_path: Option<String>,
    save_label: &str,
) -> Result<Option<lab_apis::stash::StashRevision>, ToolError> {
    let Some(path) = artifact_path else {
        return Ok(None);
    };
    let store_for_blocking = store.clone();
    let component_id_for_blocking = component_id.to_string();
    tokio::task::spawn_blocking(move || {
        normalize_marketplace_workspace_blocking(
            &store_for_blocking,
            &component_id_for_blocking,
            &source_path,
            &path,
        )
    })
    .await
    .map_err(join_err)??;
    let revision =
        crate::dispatch::stash::revision::save_revision(store, component_id, Some(save_label))
            .await?;
    Ok(Some(revision))
}

fn normalize_marketplace_workspace_blocking(
    store: &crate::dispatch::stash::store::StashStore,
    component_id: &str,
    source_path: &Path,
    artifact_path: &str,
) -> Result<(), ToolError> {
    crate::dispatch::marketplace::params::validate_rel_path(artifact_path, "artifact_path")?;
    let workspace = store.workspace_dir(component_id);
    let temp_workspace = workspace.with_file_name(format!(
        ".{component_id}.marketplace-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    ));
    if temp_workspace.exists() {
        std::fs::remove_dir_all(&temp_workspace)
            .map_err(crate::dispatch::marketplace::client::io_internal)?;
    }
    let target = temp_workspace.join(artifact_path);
    copy_artifact_source(source_path, &target)?;

    store.with_component_lock(component_id, || {
        let mut component = store
            .read_component(component_id)?
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("component `{component_id}` missing after marketplace adopt"),
            })?;
        replace_path_atomically(&temp_workspace, &workspace)?;
        component.workspace_root = workspace.join(artifact_path);
        component.updated_at = jiff::Timestamp::now().to_string();
        store.write_component(&component)
    })
}

fn copy_artifact_source(source_path: &Path, target: &Path) -> Result<(), ToolError> {
    crate::dispatch::path_safety::reject_symlink(source_path)?;
    if source_path.is_dir() {
        std::fs::create_dir_all(target)
            .map_err(crate::dispatch::marketplace::client::io_internal)?;
        copy_tree_to_base(source_path, target, source_path)
    } else {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(crate::dispatch::marketplace::client::io_internal)?;
        }
        std::fs::copy(source_path, target)
            .map(|_| ())
            .map_err(crate::dispatch::marketplace::client::io_internal)
    }
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
        // Defense-in-depth: reject a symlinked ancestor in the destination tree
        // so a pre-seeded symlink under the base dir cannot redirect the copy
        // outside the snapshot (mirrors stash `service::copy_dir_to`).
        crate::dispatch::path_safety::reject_existing_symlink_ancestors(dest_root, &dest)?;
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

fn replace_workspace_from_base(base: &Path, workspace: &Path) -> Result<Vec<String>, ToolError> {
    if !base.exists() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("base snapshot `{}` is missing", base.display()),
        });
    }
    let temp = sibling_temp_path(workspace, "reset");
    if temp.exists() {
        remove_existing_path(&temp)?;
    }
    std::fs::create_dir_all(&temp).map_err(crate::dispatch::marketplace::client::io_internal)?;
    copy_tree_to_base(base, &temp, base)?;
    let paths = relative_file_paths(&temp, &temp)?;
    crate::dispatch::path_safety::reject_existing_symlinks_in_path(workspace)?;
    replace_path_atomically(&temp, workspace)?;
    Ok(paths)
}

fn reset_one_path_from_base(
    base: &Path,
    workspace: &Path,
    rel: &str,
) -> Result<Vec<String>, ToolError> {
    let source = base.join(rel);
    let target = workspace.join(rel);
    if !source.exists() {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("base snapshot `{rel}` is missing"),
        });
    }
    crate::dispatch::path_safety::reject_symlink(&source)?;
    crate::dispatch::path_safety::reject_existing_symlink_ancestors(workspace, &target)?;
    let temp = sibling_temp_path(&target, "reset");
    if temp.exists() {
        remove_existing_path(&temp)?;
    }
    if source.is_dir() {
        std::fs::create_dir_all(&temp)
            .map_err(crate::dispatch::marketplace::client::io_internal)?;
        copy_tree_to_base(&source, &temp, &source)?;
        let paths = relative_file_paths(base, &source)?;
        replace_path_atomically(&temp, &target)?;
        Ok(paths)
    } else {
        if let Some(parent) = temp.parent() {
            std::fs::create_dir_all(parent)
                .map_err(crate::dispatch::marketplace::client::io_internal)?;
        }
        std::fs::copy(&source, &temp).map_err(crate::dispatch::marketplace::client::io_internal)?;
        replace_path_atomically(&temp, &target)?;
        Ok(vec![rel.to_string()])
    }
}

fn sibling_temp_path(path: &Path, label: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("path");
    path.with_file_name(format!(
        ".{name}.{label}-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    ))
}

// Deliberately NOT `dispatch::fs_atomic` (which writes file *bytes*): this swaps
// a whole staged path — file or directory tree — over `live`, using a
// backup-then-rename so a mid-swap failure can restore the previous content.
// `rename` over a directory is atomic on Linux but not portable, hence the
// explicit backup. Do not "unify" this with the byte-write helper.
fn replace_path_atomically(staged: &Path, live: &Path) -> Result<(), ToolError> {
    let backup = sibling_temp_path(live, "backup");
    let had_live = live.exists();
    if had_live {
        std::fs::rename(live, &backup)
            .map_err(crate::dispatch::marketplace::client::io_internal)?;
    }
    if let Err(error) =
        std::fs::rename(staged, live).map_err(crate::dispatch::marketplace::client::io_internal)
    {
        if had_live {
            best_effort(
                "failed to restore backup after atomic replace failure",
                std::fs::rename(&backup, live),
            );
        }
        return Err(error);
    }
    if had_live {
        remove_existing_path(&backup)?;
    }
    Ok(())
}

fn remove_existing_path(path: &Path) -> Result<(), ToolError> {
    if path.is_dir() {
        std::fs::remove_dir_all(path).map_err(crate::dispatch::marketplace::client::io_internal)
    } else {
        std::fs::remove_file(path).map_err(crate::dispatch::marketplace::client::io_internal)
    }
}

fn relative_file_paths(root: &Path, current: &Path) -> Result<Vec<String>, ToolError> {
    let mut out = Vec::new();
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
                    "symlink `{}` rejected while listing reset paths",
                    entry.path().display()
                ),
            });
        }
        if file_type.is_dir() {
            out.extend(relative_file_paths(root, &entry.path())?);
        } else {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(crate::dispatch::marketplace::client::io_internal)?
                .to_path_buf();
            out.push(crate::dispatch::path_safety::rel_to_unix_string(&relative));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lab_apis::stash::{StashComponent, StashWorkspaceShape};
    use serde_json::Value;
    use tempfile::tempdir;

    #[test]
    fn paths_differ_detects_clean_dirty_and_missing() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("base");
        let work = dir.path().join("work");
        std::fs::create_dir_all(base.join("agents")).unwrap();
        std::fs::create_dir_all(work.join("agents")).unwrap();
        std::fs::write(base.join("agents/a.md"), "hello").unwrap();
        std::fs::write(work.join("agents/a.md"), "hello").unwrap();
        // identical trees → no difference (clean)
        assert!(!paths_differ(&base, &work).unwrap());
        // edit the workspace copy → difference (dirty)
        std::fs::write(work.join("agents/a.md"), "changed").unwrap();
        assert!(paths_differ(&base, &work).unwrap());
        // remove the workspace file → difference
        std::fs::remove_file(work.join("agents/a.md")).unwrap();
        assert!(paths_differ(&base, &work).unwrap());
    }

    #[test]
    fn paths_differ_single_file_missing_work_counts_as_diff() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("base.txt");
        std::fs::write(&base, "x").unwrap();
        // work file does not exist → differ
        assert!(paths_differ(&base, &dir.path().join("missing.txt")).unwrap());
    }

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

    #[test]
    fn replace_workspace_from_base_resets_whole_plugin_tree() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("base");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(base.join("skills/demo")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(base.join("skills/demo/SKILL.md"), "# base\n").unwrap();
        std::fs::write(workspace.join("extra.md"), "user-only\n").unwrap();

        let reset = replace_workspace_from_base(&base, &workspace).unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.join("skills/demo/SKILL.md")).unwrap(),
            "# base\n"
        );
        assert!(!workspace.join("extra.md").exists());
        assert_eq!(reset, vec!["skills/demo/SKILL.md"]);
    }

    #[test]
    fn reset_one_path_from_base_handles_directory_artifacts() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("base");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(base.join("skills/demo")).unwrap();
        std::fs::create_dir_all(workspace.join("skills/demo")).unwrap();
        std::fs::write(base.join("skills/demo/SKILL.md"), "# base\n").unwrap();
        std::fs::write(workspace.join("skills/demo/extra.md"), "user-only\n").unwrap();

        let reset = reset_one_path_from_base(&base, &workspace, "skills/demo").unwrap();

        assert_eq!(
            std::fs::read_to_string(workspace.join("skills/demo/SKILL.md")).unwrap(),
            "# base\n"
        );
        assert!(!workspace.join("skills/demo/extra.md").exists());
        assert_eq!(reset, vec!["skills/demo/SKILL.md"]);
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn seed_component(
        root: &Path,
        component_id: &str,
        artifact_path: &str,
        base_content: &str,
        local_content: &str,
    ) {
        let store = crate::dispatch::stash::store::StashStore::new(root.to_path_buf());
        store.ensure_dirs().unwrap();
        let workspace = store.workspace_dir(component_id);
        write_file(&workspace.join(artifact_path), local_content);
        write_file(
            &root
                .join("marketplace")
                .join(component_id)
                .join("base")
                .join(artifact_path),
            base_content,
        );
        let now = "2026-06-14T00:00:00Z".to_string();
        store
            .write_component(&StashComponent {
                id: component_id.to_string(),
                kind: StashComponentKind::Skill,
                name: component_id.to_string(),
                label: None,
                head_revision_id: None,
                origin: None,
                origin_meta: Some(StashOrigin::Marketplace(MarketplaceOrigin {
                    plugin_id: "demo@labby".to_string(),
                    artifact_path: Some(artifact_path.to_string()),
                    source_version: Some("1.0.0".to_string()),
                    source_fingerprint: None,
                })),
                workspace_root: workspace.join(artifact_path),
                workspace_shape: StashWorkspaceShape::Directory,
                unix_mode: None,
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
    }

    #[test]
    fn artifact_list_reports_real_drift_status_per_fork() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("stash");
        seed_component(&root, "comp-clean", "skills/d/SKILL.md", "x\n", "x\n");
        seed_component(&root, "comp-dirty", "commands/d.md", "base\n", "edited\n");

        let rows = crate::dispatch::stash::client::with_test_stash_root(root.clone(), || {
            list_forks_blocking(None)
        })
        .unwrap();
        let status_of = |id: &str| -> String {
            rows.as_array()
                .unwrap()
                .iter()
                .find(|r| r["component_id"] == id)
                .unwrap()["status"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(status_of("comp-clean"), "clean");
        assert_eq!(status_of("comp-dirty"), "dirty");

        // Removing the base snapshot surfaces the base_missing branch.
        std::fs::remove_dir_all(root.join("marketplace/comp-clean/base")).unwrap();
        let rows2 = crate::dispatch::stash::client::with_test_stash_root(root, || {
            list_forks_blocking(None)
        })
        .unwrap();
        let status2 = rows2
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["component_id"] == "comp-clean")
            .unwrap()["status"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(status2, "base_missing");
    }

    #[test]
    fn acquire_origin_lock_conflicts_then_releases() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("stash");
        crate::dispatch::stash::client::with_test_stash_root(root, || {
            let held = acquire_origin_lock("demo@labby", None).unwrap();
            // Second acquire on the same origin key must surface `conflict`.
            assert_eq!(
                acquire_origin_lock("demo@labby", None).unwrap_err().kind(),
                "conflict"
            );
            // Releasing the guard (closing the fd) frees the advisory lock.
            drop(held);
            assert!(acquire_origin_lock("demo@labby", None).is_ok());
        });
    }

    #[test]
    fn fork_result_revision_id_serializes_none_as_null() {
        let json = serde_json::to_value(ForkResult {
            plugin_id: "p".into(),
            component_id: "c".into(),
            revision_id: None,
            stash_workspace: "/ws".into(),
            forked_artifacts: vec![],
        })
        .unwrap();
        assert!(json.get("revision_id").is_some(), "field must be present");
        assert!(json["revision_id"].is_null(), "None must serialize as null");

        let some = serde_json::to_value(ForkResult {
            plugin_id: "p".into(),
            component_id: "c".into(),
            revision_id: Some("rev-1".into()),
            stash_workspace: "/ws".into(),
            forked_artifacts: vec![],
        })
        .unwrap();
        assert_eq!(some["revision_id"], "rev-1");
    }

    #[test]
    fn reset_with_artifacts_targets_only_matching_artifact_forks() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("stash");
        seed_component(
            &root,
            "comp-skill",
            "skills/demo/SKILL.md",
            "skill=base\n",
            "skill=edited\n",
        );
        seed_component(
            &root,
            "comp-command",
            "commands/demo.md",
            "command=base\n",
            "command=edited\n",
        );

        let result = crate::dispatch::stash::client::with_test_stash_root(root.clone(), || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    reset("demo@labby", Some(vec!["skills/demo/SKILL.md".to_string()])).await
                })
        })
        .unwrap();

        assert_eq!(result["plugin_id"], "demo@labby");
        assert_eq!(
            result["reset_artifacts"],
            Value::from(vec!["skills/demo/SKILL.md"])
        );
        assert_eq!(
            std::fs::read_to_string(root.join("workspaces/comp-skill/skills/demo/SKILL.md"))
                .unwrap(),
            "skill=base\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("workspaces/comp-command/commands/demo.md")).unwrap(),
            "command=edited\n"
        );
        let command = crate::dispatch::stash::store::StashStore::new(root)
            .read_component("comp-command")
            .unwrap()
            .unwrap();
        assert!(
            command.head_revision_id.is_none(),
            "unselected fork must not receive a reset revision"
        );
    }

    #[test]
    fn reset_with_unknown_artifact_fails_before_mutating_any_fork() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("stash");
        seed_component(
            &root,
            "comp-skill",
            "skills/demo/SKILL.md",
            "skill=base\n",
            "skill=edited\n",
        );

        let err = crate::dispatch::stash::client::with_test_stash_root(root.clone(), || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    reset("demo@labby", Some(vec!["commands/demo.md".to_string()])).await
                })
        })
        .unwrap_err();

        assert_eq!(err.kind(), "not_found");
        assert_eq!(
            std::fs::read_to_string(root.join("workspaces/comp-skill/skills/demo/SKILL.md"))
                .unwrap(),
            "skill=edited\n"
        );
    }
}
