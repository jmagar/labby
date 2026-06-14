//! Export / handoff flow for the stash service.
//!
//! Reads a component's head revision and materializes its files to an output
//! directory, with optional credential-guard and execute-bit restoration for
//! `BinFile` components.

use std::path::{Path, PathBuf};

use futures::stream::{self, StreamExt};
use lab_apis::stash::types::{StashComponentKind, StashExportOptions, StashWorkspaceShape};

use crate::dispatch::error::ToolError;
use crate::dispatch::path_safety::{
    reject_existing_symlink_ancestors, reject_existing_symlinks_in_path,
};
use crate::dispatch::stash::store::StashStore;

// ── Path containment helper ───────────────────────────────────────────────────

/// Ensure `target` is a child of `write_root` using a **lexical** check only.
///
/// Does **not** create any directories — callers must create directories only
/// during the actual write phase to avoid leaving partial directory trees when
/// a later validation step fails.
///
/// The check normalises both paths by stripping `..` components via
/// `Path::components` so it works even when the paths do not yet exist on disk.
///
/// Returns `ToolError::Sdk { sdk_kind: "path_traversal" }` when `target`
/// escapes `write_root`.
fn ensure_target_within_write_root(write_root: &Path, target: &Path) -> Result<(), ToolError> {
    // Normalise a path lexically: resolve `.` and `..` without touching the
    // filesystem.  This is safe for containment checks on paths that do not
    // yet exist.
    fn normalize(p: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for comp in p.components() {
            match comp {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                c => out.push(c),
            }
        }
        out
    }

    let norm_root = normalize(write_root);
    let norm_target = normalize(target);

    if !norm_target.starts_with(&norm_root) {
        return Err(ToolError::Sdk {
            sdk_kind: "path_traversal".into(),
            message: format!(
                "output path `{}` escapes write root `{}`",
                norm_target.display(),
                norm_root.display()
            ),
        });
    }
    Ok(())
}

// ── Output types ──────────────────────────────────────────────────────────────

/// Result of a successful export operation.
#[derive(Debug)]
pub struct ExportResult {
    /// Absolute path to the export root directory.
    pub output_root: PathBuf,
    /// Revision ID that was exported.
    pub revision_id: String,
    /// Number of files written.
    pub file_count: usize,
}

// ── Read one revision file ────────────────────────────────────────────────────

/// Read a single revision file from the store.
///
/// Returns `(relative_path, bytes)`.
async fn read_revision_file(
    files_dir: PathBuf,
    rel: PathBuf,
) -> Result<(PathBuf, Vec<u8>), ToolError> {
    let abs = files_dir.join(&rel);
    tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&abs).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("read `{}`: {e}", abs.display()),
        })?;
        Ok::<_, ToolError>((rel, bytes))
    })
    .await
    .map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("spawn_blocking panicked: {e}"),
    })?
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Export a component's head revision to `output_path`.
///
/// Files are first materialized into a staged sibling directory and then moved
/// into place. With `force=true`, an existing output path is renamed to a
/// backup and restored if the staged replacement cannot be installed.
///
/// # Arguments
/// * `store` — the stash store to read from
/// * `component_id` — component to export
/// * `output_path` — destination directory (created if absent)
/// * `options` — export behaviour flags (`include_secrets`, `force`)
///
/// # Errors
/// Returns `ToolError::Sdk` with `sdk_kind`:
/// * `not_found` — component or head revision not found
/// * `secrets_export_not_allowed` — kind is `Settings` or `McpConfig` and
///   `options.include_secrets == false`
/// * `export_target_not_empty` — output directory already has content and
///   `options.force == false`
/// * `path_traversal` — an output file path escapes `output_path`
/// * `symlink_rejected` — a symlink found during reading
/// * `internal_error` — I/O failures
pub async fn export_component(
    store: &StashStore,
    component_id: &str,
    output_path: &Path,
    options: StashExportOptions,
) -> Result<ExportResult, ToolError> {
    // 1. Load component.
    let component = store
        .read_component(component_id)?
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("component `{component_id}` not found"),
        })?;

    // 2. Credential guard — check BEFORE reading any files.
    let needs_secrets_guard = matches!(
        component.kind,
        StashComponentKind::Settings | StashComponentKind::McpConfig
    );
    if needs_secrets_guard && !options.include_secrets {
        return Err(ToolError::Sdk {
            sdk_kind: "secrets_export_not_allowed".into(),
            message: format!(
                "component kind `{:?}` may contain credentials; set include_secrets = true to export",
                component.kind
            ),
        });
    }

    // 3. Load head revision.
    let rev_id = component
        .head_revision_id
        .as_deref()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("component `{component_id}` has no head revision"),
        })?;
    let revision = store
        .read_revision_meta(rev_id)?
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("revision `{rev_id}` not found"),
        })?;

    // 4. Check output_path: non-empty directory + !force → error.
    let output_path = output_path.to_path_buf();
    if output_path.exists() && !options.force {
        if !output_path.is_dir() {
            return Err(ToolError::Sdk {
                sdk_kind: "export_target_not_empty".into(),
                message: format!(
                    "output path `{}` already exists and is not a directory; set force = true to overwrite",
                    output_path.display()
                ),
            });
        }
        let mut rd = std::fs::read_dir(&output_path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("read_dir `{}`: {e}", output_path.display()),
        })?;
        if rd.next().is_some() {
            return Err(ToolError::Sdk {
                sdk_kind: "export_target_not_empty".into(),
                message: format!(
                    "output directory `{}` is not empty; set force = true to overwrite",
                    output_path.display()
                ),
            });
        }
    }

    // 5 & 6 & 7 & 8: Collect revision file list, read concurrently, write.
    let files_dir = store.revision_files_path(&revision.id);
    let is_file_shaped = component.workspace_shape == StashWorkspaceShape::File;

    // Collect relative paths from the revision files directory.
    let rel_paths: Vec<PathBuf> = {
        let files_dir_ref = files_dir.clone();
        tokio::task::spawn_blocking(move || collect_rel_paths(&files_dir_ref))
            .await
            .map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("spawn_blocking panicked: {e}"),
            })??
    };

    // 5. Path containment: validate every output file path.
    // We do a blocking check before spawning reads.
    {
        let output_path_ref = output_path.clone();
        let rel_paths_ref = rel_paths.clone();
        tokio::task::spawn_blocking(move || {
            for rel in &rel_paths_ref {
                let target = if is_file_shaped {
                    // Single-file: materialize to output_root/<filename>
                    output_path_ref.join(rel.file_name().unwrap_or(rel.as_os_str()))
                } else {
                    output_path_ref.join(rel)
                };
                ensure_target_within_write_root(&output_path_ref, &target)?;
            }
            Ok::<_, ToolError>(())
        })
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("spawn_blocking panicked: {e}"),
        })??;
    }

    // 6. Read files concurrently via buffer_unordered(8).
    let file_reads: Vec<(PathBuf, Vec<u8>)> = stream::iter(rel_paths.iter().cloned())
        .map(|rel| {
            let fd = files_dir.clone();
            async move { read_revision_file(fd, rel).await }
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let file_count = file_reads.len();
    #[cfg(unix)]
    let unix_mode = revision.unix_mode;
    let output_path_clone = output_path.clone();
    let rev_id_clone = revision.id.clone();
    let force = options.force;

    // 7 & 8. Write files to a staged sibling directory, then swap it into place.
    tokio::task::spawn_blocking(move || {
        reject_existing_symlinks_in_path(&output_path_clone)?;
        let stage_root = export_stage_path(&output_path_clone)?;
        remove_path_if_exists(&stage_root)?;
        reject_existing_symlinks_in_path(&stage_root)?;
        std::fs::create_dir_all(&stage_root).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("create_dir_all `{}`: {e}", stage_root.display()),
        })?;

        for (rel, bytes) in &file_reads {
            let dst = if is_file_shaped {
                // Single-file workspaces: materialize to output_root/<filename>
                stage_root.join(rel.file_name().unwrap_or(rel.as_os_str()))
            } else {
                stage_root.join(rel)
            };

            reject_existing_symlink_ancestors(&stage_root, &dst)?;

            // Create parent directory.
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: format!("create_dir_all `{}`: {e}", parent.display()),
                })?;
            }

            // Write file.
            std::fs::write(&dst, bytes).map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("write `{}`: {e}", dst.display()),
            })?;

            // 8. BinFile: restore execute bits, NEVER raw stored bits.
            //    Unconditionally strip setuid/setgid/sticky.
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                use std::os::unix::fs::PermissionsExt;
                let safe_mode = mode & 0o0755;
                let perms = std::fs::Permissions::from_mode(safe_mode);
                std::fs::set_permissions(&dst, perms).map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: format!("set_permissions `{}`: {e}", dst.display()),
                })?;
            }
        }

        commit_export_stage(&stage_root, &output_path_clone, force)?;
        Ok::<_, ToolError>(())
    })
    .await
    .map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("spawn_blocking panicked: {e}"),
    })??;

    Ok(ExportResult {
        output_root: output_path,
        revision_id: rev_id_clone,
        file_count,
    })
}

fn export_stage_path(output_path: &Path) -> Result<PathBuf, ToolError> {
    let Some(parent) = output_path.parent() else {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("output path `{}` has no parent", output_path.display()),
        });
    };
    let name = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("export");
    Ok(parent.join(format!(
        ".{name}.export-stage-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    )))
}

fn export_backup_path(output_path: &Path) -> Result<PathBuf, ToolError> {
    let Some(parent) = output_path.parent() else {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("output path `{}` has no parent", output_path.display()),
        });
    };
    let name = output_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("export");
    Ok(parent.join(format!(
        ".{name}.export-backup-{}",
        ulid::Ulid::new().to_string().to_lowercase()
    )))
}

fn commit_export_stage(
    stage_root: &Path,
    output_path: &Path,
    force: bool,
) -> Result<(), ToolError> {
    if output_path.exists() {
        if !force {
            remove_path_if_exists(output_path)?;
            rename_path(stage_root, output_path)?;
            return Ok(());
        }

        let backup = export_backup_path(output_path)?;
        remove_path_if_exists(&backup)?;
        rename_path(output_path, &backup)?;
        match rename_path(stage_root, output_path) {
            Ok(()) => {
                remove_path_if_exists(&backup)?;
                Ok(())
            }
            Err(err) => {
                drop(rename_path(&backup, output_path));
                Err(err)
            }
        }
    } else {
        rename_path(stage_root, output_path)
    }
}

fn rename_path(src: &Path, dst: &Path) -> Result<(), ToolError> {
    std::fs::rename(src, dst).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("rename `{}` → `{}`: {e}", src.display(), dst.display()),
    })
}

fn remove_path_if_exists(path: &Path) -> Result<(), ToolError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("symlink_metadata `{}`: {e}", path.display()),
            });
        }
    };
    if meta.file_type().is_dir() {
        std::fs::remove_dir_all(path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("remove_dir_all `{}`: {e}", path.display()),
        })
    } else {
        std::fs::remove_file(path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("remove_file `{}`: {e}", path.display()),
        })
    }
}

/// Collect relative paths of all files under `files_dir` (non-recursive walk).
///
/// For single-file-shaped revisions the directory contains exactly one entry.
/// For directory-shaped revisions it may contain a subtree.
fn collect_rel_paths(files_dir: &Path) -> Result<Vec<PathBuf>, ToolError> {
    if !files_dir.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    collect_rel_paths_recursive(files_dir, files_dir, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect_rel_paths_recursive(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), ToolError> {
    let read_dir = std::fs::read_dir(dir).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("read_dir `{}`: {e}", dir.display()),
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("read_dir entry: {e}"),
        })?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("symlink_metadata `{}`: {e}", path.display()),
        })?;
        if meta.file_type().is_symlink() {
            return Err(ToolError::Sdk {
                sdk_kind: "symlink_rejected".into(),
                message: format!("symlink at `{}`", path.display()),
            });
        }
        if meta.is_dir() {
            collect_rel_paths_recursive(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: format!("strip_prefix: {e}"),
                })?
                .to_path_buf();
            out.push(rel);
        }
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use lab_apis::stash::types::{
        StashComponent, StashComponentKind, StashRevision, StashWorkspaceShape,
    };
    use tempfile::tempdir;

    fn make_store() -> (StashStore, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let store = StashStore::new(dir.path().to_path_buf());
        store.ensure_dirs().expect("ensure_dirs");
        (store, dir)
    }

    /// Write a component + revision to the store directly.
    fn setup_dir_component_with_revision(
        store: &StashStore,
        comp_id: &str,
        rev_id: &str,
        files: &[(&str, &[u8])],
    ) {
        // Write workspace files.
        let ws_dir = store.workspace_dir(comp_id);
        std::fs::create_dir_all(&ws_dir).unwrap();
        for (name, content) in files {
            std::fs::write(ws_dir.join(name), content).unwrap();
        }

        // Write revision snapshot files.
        let rev_files_dir = store.revision_files_path(rev_id);
        std::fs::create_dir_all(&rev_files_dir).unwrap();
        for (name, content) in files {
            std::fs::write(rev_files_dir.join(name), content).unwrap();
        }

        // Write revision meta.
        let rev = StashRevision {
            id: rev_id.to_string(),
            component_id: comp_id.to_string(),
            label: None,
            content_digest: "abc".to_string(),
            created_at: "2026-04-26T12:00:00Z".to_string(),
            file_count: files.len(),
            unix_mode: None,
        };
        store.write_revision_meta(&rev).unwrap();

        // Write component pointing to revision.
        let comp = StashComponent {
            id: comp_id.to_string(),
            kind: StashComponentKind::Skill,
            name: "test".to_string(),
            label: None,
            head_revision_id: Some(rev_id.to_string()),
            origin: None,
            origin_meta: None,
            workspace_root: ws_dir,
            workspace_shape: StashWorkspaceShape::Directory,
            unix_mode: None,
            created_at: "2026-04-26T12:00:00Z".to_string(),
            updated_at: "2026-04-26T12:00:00Z".to_string(),
        };
        store.write_component(&comp).unwrap();
    }

    fn setup_file_component_with_revision(
        store: &StashStore,
        comp_id: &str,
        rev_id: &str,
        filename: &str,
        content: &[u8],
        kind: StashComponentKind,
    ) {
        // Write workspace file.
        let ws_dir = store.workspace_dir(comp_id);
        std::fs::create_dir_all(&ws_dir).unwrap();
        let ws_file = ws_dir.join(filename);
        std::fs::write(&ws_file, content).unwrap();

        // Write revision snapshot.
        let rev_files_dir = store.revision_files_path(rev_id);
        std::fs::create_dir_all(&rev_files_dir).unwrap();
        std::fs::write(rev_files_dir.join(filename), content).unwrap();

        let rev = StashRevision {
            id: rev_id.to_string(),
            component_id: comp_id.to_string(),
            label: None,
            content_digest: "abc".to_string(),
            created_at: "2026-04-26T12:00:00Z".to_string(),
            file_count: 1,
            unix_mode: None,
        };
        store.write_revision_meta(&rev).unwrap();

        let comp = StashComponent {
            id: comp_id.to_string(),
            kind,
            name: "test".to_string(),
            label: None,
            head_revision_id: Some(rev_id.to_string()),
            origin: None,
            origin_meta: None,
            workspace_root: ws_file,
            workspace_shape: StashWorkspaceShape::File,
            unix_mode: None,
            created_at: "2026-04-26T12:00:00Z".to_string(),
            updated_at: "2026-04-26T12:00:00Z".to_string(),
        };
        store.write_component(&comp).unwrap();
    }

    #[tokio::test]
    async fn export_dir_component_success() {
        let (store, _dir) = make_store();
        setup_dir_component_with_revision(
            &store,
            "comp-01",
            "rev-01",
            &[("main.ts", b"export {}"), ("SKILL.md", b"# Skill")],
        );

        let output = tempdir().unwrap();
        let result = export_component(
            &store,
            "comp-01",
            output.path(),
            StashExportOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.file_count, 2);
        assert_eq!(result.revision_id, "rev-01");
        assert!(output.path().join("main.ts").exists());
        assert!(output.path().join("SKILL.md").exists());
    }

    #[tokio::test]
    async fn export_settings_blocked_without_secrets_flag() {
        let (store, _dir) = make_store();
        setup_file_component_with_revision(
            &store,
            "comp-02",
            "rev-02",
            "settings.json",
            b"{}",
            StashComponentKind::Settings,
        );

        let output = tempdir().unwrap();
        let err = export_component(
            &store,
            "comp-02",
            output.path(),
            StashExportOptions {
                include_secrets: false,
                force: false,
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "secrets_export_not_allowed");
    }

    #[tokio::test]
    async fn export_settings_allowed_with_secrets_flag() {
        let (store, _dir) = make_store();
        setup_file_component_with_revision(
            &store,
            "comp-03",
            "rev-03",
            "settings.json",
            b"{\"key\":\"val\"}",
            StashComponentKind::Settings,
        );

        let output = tempdir().unwrap();
        let result = export_component(
            &store,
            "comp-03",
            output.path(),
            StashExportOptions {
                include_secrets: true,
                force: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.file_count, 1);
        assert!(output.path().join("settings.json").exists());
    }

    #[tokio::test]
    async fn export_fails_non_empty_output_without_force() {
        let (store, _dir) = make_store();
        setup_dir_component_with_revision(
            &store,
            "comp-04",
            "rev-04",
            &[("main.ts", b"export {}")],
        );

        let output = tempdir().unwrap();
        // Pre-create a file in the output dir.
        std::fs::write(output.path().join("existing.txt"), b"x").unwrap();

        let err = export_component(
            &store,
            "comp-04",
            output.path(),
            StashExportOptions::default(),
        )
        .await
        .unwrap_err();

        assert_eq!(err.kind(), "export_target_not_empty");
    }

    #[tokio::test]
    async fn export_force_replaces_existing_output_from_stage() {
        let (store, _dir) = make_store();
        setup_dir_component_with_revision(
            &store,
            "comp-04b",
            "rev-04b",
            &[("main.ts", b"export const fresh = true;")],
        );

        let output = tempdir().unwrap();
        std::fs::write(output.path().join("stale.txt"), b"old").unwrap();

        let result = export_component(
            &store,
            "comp-04b",
            output.path(),
            StashExportOptions {
                include_secrets: false,
                force: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(result.file_count, 1);
        assert!(output.path().join("main.ts").is_file());
        assert!(
            !output.path().join("stale.txt").exists(),
            "forced staged export must replace stale output"
        );
    }

    #[tokio::test]
    async fn export_not_found_component() {
        let (store, _dir) = make_store();
        let output = tempdir().unwrap();
        let err = export_component(
            &store,
            "nonexistent",
            output.path(),
            StashExportOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "not_found");
    }

    #[tokio::test]
    async fn export_no_head_revision_error() {
        let (store, _dir) = make_store();
        let ws_dir = store.workspace_dir("comp-05");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let comp = StashComponent {
            id: "comp-05".to_string(),
            kind: StashComponentKind::Skill,
            name: "no-rev".to_string(),
            label: None,
            head_revision_id: None, // no head revision
            origin: None,
            origin_meta: None,
            workspace_root: ws_dir,
            workspace_shape: StashWorkspaceShape::Directory,
            unix_mode: None,
            created_at: "2026-04-26T12:00:00Z".to_string(),
            updated_at: "2026-04-26T12:00:00Z".to_string(),
        };
        store.write_component(&comp).unwrap();

        let output = tempdir().unwrap();
        let err = export_component(
            &store,
            "comp-05",
            output.path(),
            StashExportOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), "not_found");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn export_stage_replaces_old_symlinked_output_without_following_it() {
        let (store, _dir) = make_store();
        let comp_id = "comp-06";
        let rev_id = "rev-06";
        let ws_dir = store.workspace_dir(comp_id);
        std::fs::create_dir_all(&ws_dir).unwrap();

        let rev_files_dir = store.revision_files_path(rev_id);
        std::fs::create_dir_all(rev_files_dir.join("link")).unwrap();
        std::fs::write(rev_files_dir.join("link").join("payload.txt"), b"secret").unwrap();

        let rev = StashRevision {
            id: rev_id.to_string(),
            component_id: comp_id.to_string(),
            label: None,
            content_digest: "abc".to_string(),
            created_at: "2026-04-26T12:00:00Z".to_string(),
            file_count: 1,
            unix_mode: None,
        };
        store.write_revision_meta(&rev).unwrap();

        let comp = StashComponent {
            id: comp_id.to_string(),
            kind: StashComponentKind::Skill,
            name: "test".to_string(),
            label: None,
            head_revision_id: Some(rev_id.to_string()),
            origin: None,
            origin_meta: None,
            workspace_root: ws_dir,
            workspace_shape: StashWorkspaceShape::Directory,
            unix_mode: None,
            created_at: "2026-04-26T12:00:00Z".to_string(),
            updated_at: "2026-04-26T12:00:00Z".to_string(),
        };
        store.write_component(&comp).unwrap();

        let output = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), output.path().join("link")).unwrap();

        let result = export_component(
            &store,
            comp_id,
            output.path(),
            StashExportOptions {
                include_secrets: false,
                force: true,
            },
        )
        .await
        .expect("staged export should replace old output without following symlinks");

        assert_eq!(result.file_count, 1);
        assert!(output.path().join("link").join("payload.txt").is_file());
        assert!(!outside.path().join("payload.txt").exists());
    }
}
