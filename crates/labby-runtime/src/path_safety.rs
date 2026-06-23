//! Surface-neutral path-safety helpers for dispatch modules that operate on the
//! local filesystem.
//!
//! These helpers now live in `lab-runtime` so the standalone gateway crates can
//! share them. They are re-exported from `crate::dispatch::path_safety` (and the
//! lexical `reject_path_traversal` guard from `crate::dispatch::helpers`) so the
//! existing import paths keep working.
//!
//! # Contents
//!
//! - `reject_path_traversal` — lexical `..`/absolute-component guard. Previously
//!   lived in `dispatch/helpers.rs`; moved here because it is pure and is needed
//!   by this module's tests, which `lab-runtime` cannot reach across crates.
//! - `reject_symlink` — consolidated from `dispatch/marketplace/stash_meta.rs`
//!   where it was a private function.  Stash dispatch (and future modules that
//!   walk user-supplied paths) import from here instead of duplicating the
//!   logic.
//! - `canonicalize_and_reject_read_path` / `canonicalize_and_reject_write_path`
//!   — separate policy entry points for local filesystem reads and writes.
//! - `reject_existing_symlink_ancestors` — rejects writes whose existing
//!   destination root/parents contain symlinks before the final file is opened.
//!
//! # Intentionally omitted
//!
//! - `ensure_target_within_write_root`: the only existing implementation lives
//!   in `node/install.rs` and is async + anyhow-based (not `ToolError`-based).
//!   Wave-3 `stash/store.rs` will add a synchronous version when it has
//!   concrete callers.

use std::path::{Component, Path, PathBuf};

use crate::error::ToolError;

// ── Lexical traversal guard ───────────────────────────────────────────────────

/// Reject any path input that contains a `Component::ParentDir` (`..`) segment.
///
/// This is a **lexical** check only. Callers that join the input against a
/// trusted root MUST additionally `canonicalize` + `starts_with(root)` after
/// writing to protect against symlinks escaping the jail (TOCTOU-weak, but
/// strictly better than skipping). Windows UNC / absolute paths are rejected
/// upstream by callers via `Path::is_absolute`.
///
/// Emits the canonical `path_traversal` kind (see `docs/dev/ERRORS.md`) so this
/// lexical guard and the canonicalizing guards in this module report the same
/// path-escape threat under one stable kind rather than splitting it across
/// `path_traversal` and `invalid_param`.
pub fn reject_path_traversal(rel_path: &str) -> Result<(), ToolError> {
    for component in Path::new(rel_path).components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(ToolError::Sdk {
                sdk_kind: "path_traversal".to_string(),
                message: format!(
                    "path traversal rejected: `{rel_path}` must be a relative path with only normal components"
                ),
            });
        }
    }
    Ok(())
}

// ── System-path denylist ──────────────────────────────────────────────────────

/// Sensitive paths that stash must never read from or write to, regardless of
/// operator configuration. Checked after canonicalization so that symlinks and
/// `..` traversals cannot bypass the list.
///
/// This intentionally does not include broad user/workspace roots like `/home`,
/// `/tmp`, or `/workspace`: stash is meant to import and export operator-owned
/// files from those locations. Symlink-aware destination checks protect write
/// containment separately.
pub const SENSITIVE_READ_PATH_DENYLIST: &[&str] = &[
    // Core FHS system directories
    "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/dev", "/proc", "/sys",
    // Variable / runtime data — often writable, always sensitive
    "/var", "/run",  // Privileged user homes
    "/root", // Optional / mounted software and common privileged container mounts
    "/opt", "/srv", "/app", "/data", "/config", "/mnt", "/media", "/storage",
];

/// Sensitive paths that stash must never write to. Kept separate from the read
/// denylist so future read/write policy differences are explicit.
pub const SENSITIVE_WRITE_PATH_DENYLIST: &[&str] = SENSITIVE_READ_PATH_DENYLIST;

/// Canonicalize a local read source and reject sensitive roots.
pub fn canonicalize_and_reject_read_path(path: &Path) -> Result<PathBuf, ToolError> {
    let canonical = canonicalize_verifiable_path(path)?;
    reject_path_against_denylist(&canonical, path, SENSITIVE_READ_PATH_DENYLIST)?;
    Ok(canonical)
}

/// Canonicalize a local write destination and reject sensitive roots.
pub fn canonicalize_and_reject_write_path(path: &Path) -> Result<PathBuf, ToolError> {
    let canonical = canonicalize_verifiable_path(path)?;
    reject_path_against_denylist(&canonical, path, SENSITIVE_WRITE_PATH_DENYLIST)?;
    Ok(canonical)
}

fn canonicalize_verifiable_path(path: &Path) -> Result<PathBuf, ToolError> {
    // Canonicalize the path if it exists; otherwise canonicalize the nearest
    // existing ancestor and rejoin the remaining components.
    if path.exists() {
        std::fs::canonicalize(path).map_err(|e| ToolError::Sdk {
            sdk_kind: "path_traversal".into(),
            message: format!(
                "cannot verify path `{}` is safe: canonicalize failed: {e}",
                path.display()
            ),
        })
    } else if let Some(parent) = path.parent() {
        if parent == Path::new("") || !parent.exists() {
            // Cannot canonicalize — fail closed.
            return Err(ToolError::Sdk {
                sdk_kind: "path_traversal".into(),
                message: format!(
                    "cannot verify path `{}` is safe: parent directory does not exist",
                    path.display()
                ),
            });
        }
        let canonical_parent = std::fs::canonicalize(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "path_traversal".into(),
            message: format!(
                "cannot verify path `{}` is safe: canonicalize parent failed: {e}",
                path.display()
            ),
        })?;
        let file_name = path.file_name().unwrap_or_default();
        Ok(canonical_parent.join(file_name))
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "path_traversal".into(),
            message: format!(
                "cannot verify path `{}` is safe: no parent directory",
                path.display()
            ),
        })
    }
}

fn reject_path_against_denylist(
    canonical: &Path,
    original: &Path,
    denylist: &[&str],
) -> Result<(), ToolError> {
    let canonical_str = canonical.to_string_lossy();
    for &system in denylist {
        if canonical_str == system || canonical_str.starts_with(&format!("{system}/")) {
            return Err(ToolError::Sdk {
                sdk_kind: "path_traversal".into(),
                message: format!(
                    "path `{}` resolves to a sensitive system path (`{}`) and is not allowed",
                    original.display(),
                    system
                ),
            });
        }
    }
    Ok(())
}

/// Reject a path that exists on disk as a symlink.
///
/// This is a **lstat-based** check — it does not follow the link. Callers that
/// need a post-canonicalize within-root guarantee must perform that check
/// separately (the TOCTOU window between `reject_symlink` and the actual I/O
/// operation is narrow but non-zero; treat it as defence-in-depth, not as the
/// sole guard).
///
/// Returns `ToolError::Sdk { sdk_kind: "not_found" }` when the path does not
/// exist, and `ToolError::internal_message` when the path *is* a symlink.
/// Returns `Ok(())` for regular files and directories.
pub fn reject_symlink(path: &Path) -> Result<(), ToolError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: "path is missing".into(),
            });
        }
        Err(error) => {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("lstat failed: {error}"),
            });
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(ToolError::Sdk {
            sdk_kind: "symlink_rejected".into(),
            message: format!("refusing to operate on symlinked path `{}`", path.display()),
        });
    }
    Ok(())
}

/// Reject a write target when the destination root, any existing parent between
/// `write_root` and `target`, or the existing target itself is a symlink.
///
/// Call this immediately before creating directories or writing files. It
/// closes the gap where a lexical containment check passes but an existing
/// symlinked parent redirects the actual write outside the intended root.
pub fn reject_existing_symlink_ancestors(
    write_root: &Path,
    target: &Path,
) -> Result<(), ToolError> {
    let root = normalize_lexical(write_root);
    let target = normalize_lexical(target);
    if !target.starts_with(&root) {
        return Err(ToolError::Sdk {
            sdk_kind: "path_traversal".into(),
            message: format!(
                "target path `{}` escapes write root `{}`",
                target.display(),
                root.display()
            ),
        });
    }

    let mut current = root.clone();
    reject_existing_symlink(&current)?;

    let Ok(relative) = target.strip_prefix(&root) else {
        return Err(ToolError::Sdk {
            sdk_kind: "path_traversal".into(),
            message: format!(
                "target path `{}` escapes write root `{}`",
                target.display(),
                root.display()
            ),
        });
    };

    for component in relative.components() {
        current.push(component.as_os_str());
        reject_existing_symlink(&current)?;
    }

    Ok(())
}

/// Reject a path if any existing component in the path is a symlink.
///
/// Use this before `create_dir_all(path)` when the path itself may not exist
/// yet; checking the target against itself would otherwise miss a symlinked
/// existing parent.
pub fn reject_existing_symlinks_in_path(path: &Path) -> Result<(), ToolError> {
    let path = normalize_lexical(path);
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        reject_existing_symlink(&current)?;
    }
    Ok(())
}

fn reject_existing_symlink(path: &Path) -> Result<(), ToolError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::Sdk {
            sdk_kind: "symlink_rejected".into(),
            message: format!(
                "refusing to write through symlinked path `{}`",
                path.display()
            ),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("lstat `{}` failed: {error}", path.display()),
        }),
    }
}

/// Render a relative path with `/` separators on every platform so the logical
/// artifact keys stashes and updates emit are stable across Unix and Windows.
///
/// `Path::to_string_lossy()` uses the platform separator (`\` on Windows), which
/// would make the same artifact key differ by OS and break equality with the
/// forward-slash keys callers and snapshots expect. Iterating `components()`
/// only rewrites the real `MAIN_SEPARATOR`; a literal backslash inside a Unix
/// filename stays intact because it is part of a single `Normal` component.
pub fn rel_to_unix_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn reject_symlink_accepts_regular_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("regular.txt");
        std::fs::write(&file, b"hi").unwrap();
        assert!(reject_symlink(&file).is_ok());
    }

    #[test]
    fn rel_to_unix_string_joins_components_with_forward_slash() {
        // Multi-component relative path renders with `/` on every platform —
        // this is what keeps artifact keys stable across the Windows runner and
        // Linux. Build via `join` so the input uses the platform separator.
        let path = Path::new("skills").join("demo").join("SKILL.md");
        assert_eq!(rel_to_unix_string(&path), "skills/demo/SKILL.md");
    }

    #[cfg(unix)]
    #[test]
    fn rel_to_unix_string_preserves_backslash_in_unix_filename() {
        // On Unix a `\` is an ordinary filename byte, not a separator, so it must
        // survive as part of a single component rather than being rewritten.
        assert_eq!(
            rel_to_unix_string(Path::new(r"weird\name.txt")),
            r"weird\name.txt"
        );
    }

    #[test]
    fn reject_symlink_accepts_directory() {
        let dir = tempdir().unwrap();
        assert!(reject_symlink(dir.path()).is_ok());
    }

    #[test]
    fn reject_symlink_rejects_missing_path() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("missing");
        let err = reject_symlink(&missing).unwrap_err();
        assert_eq!(err.kind(), "not_found");
    }

    #[cfg(unix)]
    #[test]
    fn reject_symlink_rejects_symlink() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"hi").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = reject_symlink(&link).unwrap_err();
        assert_eq!(err.kind(), "symlink_rejected");
    }

    #[test]
    fn reject_path_traversal_rejects_dotdot() {
        let err = reject_path_traversal("../escape").unwrap_err();
        // Canonical path-escape kind, shared with this module's
        // `canonicalize_and_reject_*` guards (see docs/dev/ERRORS.md).
        assert_eq!(err.kind(), "path_traversal");
    }

    #[test]
    fn reject_path_traversal_accepts_relative_normal() {
        assert!(reject_path_traversal("sub/path.txt").is_ok());
    }

    // Unix-only: asserts unix-absolute operator paths (`/workspace`, `/home`,
    // `/tmp`) are allowed. These are unix filesystem locations; the denylist
    // logic itself is cross-platform.
    #[cfg(unix)]
    #[test]
    fn system_path_check_allows_operator_workspace_paths() {
        assert!(canonicalize_and_reject_write_path(Path::new("/workspace")).is_ok());
        assert!(canonicalize_and_reject_write_path(Path::new("/home/stash-src")).is_ok());
        assert!(canonicalize_and_reject_write_path(Path::new("/tmp/stash-src")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_ancestor_check_rejects_redirected_parent() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let root = dir.path().join("out");
        std::fs::create_dir_all(&root).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        let err = reject_existing_symlink_ancestors(&root, &link.join("file.txt")).unwrap_err();
        assert_eq!(err.kind(), "symlink_rejected");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_path_check_rejects_redirected_parent_before_root_exists() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        let err = reject_existing_symlinks_in_path(&link.join("out")).unwrap_err();
        assert_eq!(err.kind(), "symlink_rejected");
    }
}
