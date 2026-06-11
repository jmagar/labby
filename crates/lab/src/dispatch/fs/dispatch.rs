//! Top-level dispatch for the `fs` workspace filesystem browser service.
//!
//! Exposes built-in `help` / `schema` and the `fs.list` action. All I/O
//! happens inside a `spawn_blocking` to keep the async executor responsive
//! under pressure from the synchronous `walkdir` walker.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::{action_schema, help_payload, require_str};

use super::catalog::ACTIONS;

#[cfg(feature = "fs")]
use super::client::{MAX_PREVIEW_BYTES, deny_globset, require_workspace_root, safe_content_type};
#[cfg(feature = "fs")]
use super::params::{parse_list, parse_preview};

/// Maximum number of entries returned in a single `fs.list` response.
/// Extra entries are suppressed and `truncated: true` is set.
#[cfg(feature = "fs")]
pub const LIST_CAP: usize = 10_000;

/// Top-level MCP/CLI dispatch. HTTP handlers that hold the canonical
/// workspace root in `AppState` should call [`dispatch_with_root`] to
/// avoid a second env lookup per request.
///
/// # Note on `surface`
///
/// `surface` is hardcoded to `"mcp"` here — the shared dispatch layer does
/// not yet thread surface context. Same limitation as the `lab_admin`
/// dispatcher.
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    let start = std::time::Instant::now();
    let result = dispatch_inner(action, params).await;
    let elapsed_ms = start.elapsed().as_millis();

    match &result {
        Ok(_) => tracing::info!(
            surface = "mcp",
            service = "fs",
            action,
            elapsed_ms,
            "dispatch ok"
        ),
        Err(err) => {
            if err.is_internal() {
                tracing::error!(
                    surface = "mcp",
                    service = "fs",
                    action,
                    elapsed_ms,
                    kind = err.kind(),
                    "dispatch error"
                );
            } else {
                tracing::warn!(
                    surface = "mcp",
                    service = "fs",
                    action,
                    elapsed_ms,
                    kind = err.kind(),
                    "dispatch error"
                );
            }
        }
    }
    result
}

/// Handle catalog-discovery actions and HTTP-only refusals that must fire
/// regardless of workspace configuration. Returns `Some(_)` when the action
/// was handled, `None` to delegate to the service-specific path.
fn handle_builtin(action: &str, params: &Value) -> Option<Result<Value, ToolError>> {
    match action {
        "help" => Some(Ok(help_payload("fs", ACTIONS))),
        "schema" => Some(match require_str(params, "action") {
            Ok(a) => action_schema(ACTIONS, a),
            Err(e) => Err(e),
        }),
        "fs.preview" => Some(Err(ToolError::Sdk {
            sdk_kind: "http_only".to_string(),
            message: "fs.preview is not available on the MCP surface; use GET /v1/fs/preview"
                .to_string(),
        })),
        _ => None,
    }
}

async fn dispatch_inner(action: &str, params: Value) -> Result<Value, ToolError> {
    if let Some(result) = handle_builtin(action, &params) {
        return result;
    }

    #[cfg(feature = "fs")]
    {
        let root = require_workspace_root()?;
        dispatch_with_root(root, action, params).await
    }

    #[cfg(not(feature = "fs"))]
    {
        let _ = params;
        Err(ToolError::UnknownAction {
            message: format!("unknown action `fs.{action}`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        })
    }
}

/// Single dispatch body. `dispatch()` resolves the workspace root from
/// `config.toml` (or returns `workspace_not_configured`) and
/// delegates here; HTTP handlers pass the canonical root from `AppState`.
#[cfg(feature = "fs")]
pub async fn dispatch_with_root(
    root: &Path,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    // Defense-in-depth: direct HTTP callers of dispatch_with_root still get
    // a valid catalog payload / http_only rejection without re-implementing
    // the built-in logic.
    if let Some(result) = handle_builtin(action, &params) {
        return result;
    }
    match action {
        "fs.list" => list_action(root, params).await,
        unknown => Err(ToolError::UnknownAction {
            message: format!("unknown action `fs.{unknown}`"),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
            hint: None,
        }),
    }
}

/// Execute `fs.list` — enumerate the immediate children of a workspace path.
///
/// Steps, in order:
/// 1. Parse + NFKC-normalize the requested relative path.
/// 2. Join to the canonical workspace root, canonicalize the join target,
///    and assert it still lives beneath the root (symlink-escape guard).
/// 3. Walk one level deep with `follow_links=false`, using `symlink_metadata`
///    so dangling symlinks surface as `accessible: false` rather than erroring.
/// 4. Suppress every entry whose NFKC-normalized relative path matches the
///    credential deny-list — these do not appear in the response.
/// 5. Cap at [`LIST_CAP`] entries; set `truncated` when the walker had more
///    to offer.
#[cfg(feature = "fs")]
async fn list_action(root: &Path, params: Value) -> Result<Value, ToolError> {
    let parsed = parse_list(&params)?;
    let target = target_within_root(root, &parsed.relative).await?;

    // canonical root containment already enforced by `target_within_root`,
    // so the blocking walker only needs the validated target + rel prefix.
    let _ = root;
    let rel_str = parsed.rel_str.clone();

    let listing = tokio::task::spawn_blocking(move || list_directory(&target, &rel_str))
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("spawn_blocking join error: {e}"),
        })??;

    serde_json::to_value(listing).map_err(|e| ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message: e.to_string(),
    })
}

/// Join `relative` to `root`, canonicalize, and assert the target remains
/// beneath the root. Returns the canonical target path.
#[cfg(feature = "fs")]
async fn target_within_root(root: &Path, relative: &Path) -> Result<PathBuf, ToolError> {
    let joined = if relative.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let canonical = tokio::fs::canonicalize(&joined)
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("path does not exist: `{}`", relative.display()),
            },
            std::io::ErrorKind::PermissionDenied => ToolError::Sdk {
                sdk_kind: "permission_denied".into(),
                message: "permission denied".into(),
            },
            _ => ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: e.to_string(),
            },
        })?;
    if !canonical.starts_with(root) {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".into(),
            message: "path escapes workspace root".into(),
        });
    }
    let meta = tokio::fs::symlink_metadata(&canonical)
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: e.to_string(),
        })?;
    if !meta.is_dir() {
        return Err(ToolError::InvalidParam {
            message: "path does not name a directory".into(),
            param: "path".into(),
        });
    }
    Ok(canonical)
}

#[cfg(feature = "fs")]
#[derive(Debug, serde::Serialize)]
pub(crate) struct Entry {
    pub name: String,
    /// Workspace-relative path of the entry (forward-slash joined).
    pub path: String,
    /// `"file"`, `"dir"`, `"symlink"`, or `"other"`.
    pub kind: &'static str,
    /// Size in bytes. `None` for directories, symlinks, and entries whose
    /// metadata is unreadable (`accessible: false`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// RFC-3339 modified timestamp. `None` when unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// `false` when the entry could not be stat'd (e.g. dangling symlink).
    pub accessible: bool,
}

#[cfg(feature = "fs")]
#[derive(Debug, serde::Serialize)]
pub(crate) struct ListResponse {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Synchronous walker — enumerate one level of `target` under `root`,
/// suppressing deny-list matches and capping at [`LIST_CAP`]. Called from
/// `spawn_blocking`.
#[cfg(feature = "fs")]
fn list_directory(target: &Path, rel_prefix: &str) -> Result<ListResponse, ToolError> {
    use unicode_normalization::UnicodeNormalization;

    let deny = deny_globset();
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;

    // walkdir with min_depth=1 + max_depth=1 = children only. follow_links
    // defaults to false.
    let walker = walkdir::WalkDir::new(target)
        .min_depth(1)
        .max_depth(1)
        .follow_links(false)
        .sort_by_file_name();

    for dent in walker.into_iter() {
        let Ok(dent) = dent else {
            // Skip unreadable entries silently — a directory listing should
            // not fail because one child is inaccessible.
            continue;
        };

        let name = match dent.file_name().to_str() {
            Some(s) => s.to_owned(),
            None => continue, // non-UTF-8 name — skip
        };

        // Build the workspace-relative path for this entry.
        let rel_path = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{rel_prefix}/{name}")
        };
        // ASCII fast-path: NFKC is a no-op for ASCII (>99% of paths in
        // practice), so skip the per-entry allocation. Non-ASCII paths
        // still normalize so deny-list patterns match Unicode-equivalent
        // forms.
        let denied = if rel_path.is_ascii() {
            deny.is_match(&rel_path)
        } else {
            let normalized: String = rel_path.nfkc().collect();
            deny.is_match(&normalized)
        };
        if denied {
            continue;
        }

        if entries.len() >= LIST_CAP {
            truncated = true;
            break;
        }

        // dent.metadata() reuses walkdir's stat instead of issuing a
        // second lstat on dent.path(). With follow_links=false (set
        // above), this returns the link's own metadata — same semantics
        // as std::fs::symlink_metadata. Saves ~10k syscalls for a full
        // LIST_CAP listing (~5-15ms warm, 30-80ms cold).
        let meta = dent.metadata();
        let (kind, size, accessible, modified) = match meta {
            Ok(m) => {
                let file_type = m.file_type();
                let k = if file_type.is_symlink() {
                    "symlink"
                } else if file_type.is_dir() {
                    "dir"
                } else if file_type.is_file() {
                    "file"
                } else {
                    "other"
                };
                let size = if file_type.is_file() {
                    Some(m.len())
                } else {
                    None
                };
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| jiff::Timestamp::try_from(t).ok())
                    .map(|ts| ts.to_string());
                // A symlink itself is readable even if its target is not —
                // but we report the link kind; accessibility of the target
                // is signaled by whether canonicalize would succeed, which
                // callers can test via a subsequent list(target).
                (k, size, true, modified)
            }
            Err(_) => ("symlink", None, false, None),
        };

        entries.push(Entry {
            name,
            path: rel_path,
            kind,
            size,
            modified,
            accessible,
        });
    }

    Ok(ListResponse { entries, truncated })
}

/// A prepared preview: an opened read-only file descriptor (no symlinks
/// followed across the workspace root) plus the metadata the HTTP adapter
/// needs to build a response.
#[cfg(feature = "fs")]
#[derive(Debug)]
pub struct Preview {
    /// Opened file, async-ready via `tokio::fs::File::from_std`.
    pub file: tokio::fs::File,
    /// Safe Content-Type from the extension whitelist, or
    /// `application/octet-stream`. Callers decide inline vs attachment by
    /// passing this through [`super::client::is_inline_mime`].
    pub content_type: &'static str,
    /// Upper bound on bytes to stream — already clamped to the server cap.
    pub max_bytes: u64,
    /// Basename for `Content-Disposition: attachment; filename="…"`,
    /// stripped of path separators and control characters.
    pub disposition_filename: String,
}

/// Open a workspace file for preview.
///
/// This is the shared entry point called by `api::services::fs`. MCP and
/// CLI intentionally do NOT reach this function — see
/// `crate::mcp::services::fs` for the rationale.
///
/// On success, returns an opened [`tokio::fs::File`] plus metadata. On
/// failure, returns a structured [`ToolError`] whose `kind()` maps cleanly
/// to an HTTP status.
///
/// Error kinds used:
/// - `missing_param` / `invalid_param` — param shape problems
/// - `not_found` — file does not exist or deny-list suppression
///   (deny-list intentionally aliases to `not_found` to avoid revealing
///   the existence of a suppressed secret)
/// - `permission_denied` — path escapes workspace root / symlink refusal
/// - `invalid_param` — path names a directory, not a regular file
#[cfg(feature = "fs")]
pub async fn open_for_preview(root: &Path, params: Value) -> Result<Preview, ToolError> {
    let parsed = parse_preview(&params)?;

    // Deny-list: intentionally surfaces as `not_found` so the caller
    // cannot distinguish "file does not exist" from "file exists but is
    // suppressed" — returning the latter would be an exfiltration oracle.
    if deny_globset().is_match(&parsed.rel_str) {
        // Intentionally omit the `path` field: naming the matched credential
        // file in structured logs turns the audit event itself into an
        // exfiltration oracle (the deny-list aliases to `not_found` for the
        // same reason). `kind = "deny_list"` is sufficient to count denials;
        // operators who need to investigate can correlate by timestamp +
        // `request_id` at the request layer.
        tracing::info!(
            surface = "api",
            service = "fs",
            action = "fs.preview",
            kind = "deny_list",
            "preview rejected by credential deny-list"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("path not found: `{}`", parsed.rel_str),
        });
    }

    let root_owned = root.to_path_buf();
    let rel_owned = parsed.relative.clone();
    let rel_str = parsed.rel_str.clone();

    // Open in a blocking context — rustix::openat2 is a syscall and
    // std::fs::File::open on the fallback path is also blocking.
    let opened = tokio::task::spawn_blocking(move || open_no_follow(&root_owned, &rel_owned))
        .await
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("spawn_blocking join error: {e}"),
        })??;

    let meta = opened.metadata().map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: e.to_string(),
    })?;
    if !meta.is_file() {
        return Err(ToolError::InvalidParam {
            message: "path does not name a regular file".into(),
            param: "path".into(),
        });
    }

    let caller_cap = parsed.max_bytes.unwrap_or(MAX_PREVIEW_BYTES);
    let effective_cap = caller_cap.min(MAX_PREVIEW_BYTES);

    let ext_path = Path::new(&rel_str);
    let content_type = safe_content_type(ext_path);

    let disposition_filename = parsed
        .relative
        .file_name()
        .and_then(|n| n.to_str())
        .map(sanitize_filename)
        .unwrap_or_else(|| "file".to_string());

    Ok(Preview {
        file: tokio::fs::File::from_std(opened),
        content_type,
        max_bytes: effective_cap,
        disposition_filename,
    })
}

/// Open `rel` as a read-only regular file under `root` without following
/// symlinks at any point in the resolution.
///
/// - On Linux, uses `rustix::fs::openat2` with `RESOLVE_BENEATH |
///   RESOLVE_NO_SYMLINKS` so the kernel enforces containment atomically
///   (closes the TOCTOU window between validate and open).
/// - On non-Linux, falls back to canonicalize + `starts_with` + final
///   `symlink_metadata` refusal. The fallback is documented as weaker but
///   still rejects the common escape patterns.
#[cfg(all(feature = "fs", target_os = "linux"))]
fn open_no_follow(root: &Path, rel: &Path) -> Result<std::fs::File, ToolError> {
    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};

    let root_file = std::fs::File::open(root).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("open workspace root: {e}"),
    })?;

    let fd_result = openat2(
        &root_file,
        rel,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    );

    match fd_result {
        Ok(owned) => Ok(std::fs::File::from(owned)),
        Err(errno) => match errno {
            rustix::io::Errno::LOOP | rustix::io::Errno::XDEV => Err(ToolError::Sdk {
                sdk_kind: "permission_denied".into(),
                message: "symlink or cross-device resolution refused".into(),
            }),
            rustix::io::Errno::NOENT => Err(ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("path not found: `{}`", rel.display()),
            }),
            rustix::io::Errno::ACCESS | rustix::io::Errno::PERM => Err(ToolError::Sdk {
                sdk_kind: "permission_denied".into(),
                message: "permission denied".into(),
            }),
            rustix::io::Errno::NOSYS => {
                // Kernel predates openat2 (pre-5.6): use the slower fallback.
                open_no_follow_fallback(root, rel)
            }
            other => Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: format!("openat2 failed: {other}"),
            }),
        },
    }
}

#[cfg(all(feature = "fs", not(target_os = "linux")))]
fn open_no_follow(root: &Path, rel: &Path) -> Result<std::fs::File, ToolError> {
    open_no_follow_fallback(root, rel)
}

/// Portable fallback used when `openat2` returns `ENOSYS` (pre-5.6 Linux) or
/// when running on a non-Linux Unix target (macOS, FreeBSD, etc.).
///
/// # Security model (lab-f1t2.34)
///
/// On Unix targets this replaces the old `canonicalize + starts_with + open`
/// chain with a per-component `openat(O_NOFOLLOW)` walk that gives the same
/// atomicity guarantee as `openat2(RESOLVE_NO_SYMLINKS)`:
///
/// - Opening each directory component with `O_NOFOLLOW | O_DIRECTORY` ensures
///   no symlink is followed at any step of the resolution.
/// - The final file open uses `O_RDONLY | O_NOFOLLOW | O_CLOEXEC`; if any
///   component has been swapped to a symlink between steps the kernel rejects
///   the open with `ELOOP` / `permission_denied`.
/// - This closes the TOCTOU race where a stat-then-canonicalize chain could
///   be defeated by swapping a regular file for an in-root symlink between
///   the two calls.
///
/// On non-Unix targets (Windows) the old `canonicalize + starts_with +
/// symlink_metadata` chain is retained as a best-effort guard; the Windows
/// gap is tracked separately and does not worsen the existing posture.
///
/// Callers on Linux 5.6+ never reach this function — they take the
/// `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` path in `open_no_follow`.
#[cfg(feature = "fs")]
fn open_no_follow_fallback(root: &Path, rel: &Path) -> Result<std::fs::File, ToolError> {
    #[cfg(unix)]
    {
        open_no_follow_unix(root, rel)
    }
    #[cfg(not(unix))]
    {
        open_no_follow_windows_fallback(root, rel)
    }
}

/// Per-component `openat(O_NOFOLLOW)` walk for Unix targets without `openat2`.
///
/// Each directory intermediate is opened with `O_DIRECTORY | O_NOFOLLOW`; the
/// final file is opened with `O_RDONLY | O_NOFOLLOW`. This makes the check
/// atomic at each step, eliminating the TOCTOU window present in the old
/// `stat-then-canonicalize` chain.
#[cfg(all(feature = "fs", unix))]
fn open_no_follow_unix(root: &Path, rel: &Path) -> Result<std::fs::File, ToolError> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::os::unix::io::AsFd;

    // Open the workspace root directory.
    let root_dir = std::fs::File::open(root).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: format!("open workspace root: {e}"),
    })?;

    let components: Vec<_> = rel.components().collect();
    let n = components.len();
    if n == 0 {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("path not found: `{}`", rel.display()),
        });
    }

    // Walk directory components with O_DIRECTORY | O_NOFOLLOW.
    // The last component is the target file — handled separately below.
    let mut current_dir = root_dir;
    for component in &components[..n - 1] {
        let name = component.as_os_str();
        let result = openat(
            current_dir.as_fd(),
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        );
        current_dir = match result {
            Ok(fd) => std::fs::File::from(fd),
            Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) => {
                return Err(ToolError::Sdk {
                    sdk_kind: "permission_denied".into(),
                    message: "symlinks are not followed for previews".into(),
                });
            }
            Err(rustix::io::Errno::NOENT) => {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("path not found: `{}`", rel.display()),
                });
            }
            Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {
                return Err(ToolError::Sdk {
                    sdk_kind: "permission_denied".into(),
                    message: "permission denied".into(),
                });
            }
            Err(other) => {
                return Err(ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: format!("openat dir component failed: {other}"),
                });
            }
        };
    }

    // Open the final component as a regular file with O_NOFOLLOW.
    let file_name = components[n - 1].as_os_str();
    let file_result = openat(
        current_dir.as_fd(),
        file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    );

    match file_result {
        Ok(fd) => {
            let file = std::fs::File::from(fd);
            // Verify it is a regular file (not a directory opened as O_RDONLY).
            let meta = file.metadata().map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: e.to_string(),
            })?;
            if !meta.is_file() {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: format!("path not found: `{}`", rel.display()),
                });
            }
            // Re-validate the canonical relative path against the deny-list.
            // The input `rel` was screened in `open_for_preview`, but the
            // canonical form may differ on case-insensitive filesystems.
            if let Ok(canonical) = std::fs::canonicalize(root.join(rel)) {
                if let Ok(canonical_rel) = canonical.strip_prefix(root) {
                    let s = canonical_rel.to_string_lossy();
                    if deny_globset().is_match(s.as_ref()) {
                        return Err(ToolError::Sdk {
                            sdk_kind: "not_found".into(),
                            message: format!("path not found: `{}`", rel.display()),
                        });
                    }
                }
            }
            Ok(file)
        }
        Err(rustix::io::Errno::LOOP) => Err(ToolError::Sdk {
            sdk_kind: "permission_denied".into(),
            message: "symlinks are not followed for previews".into(),
        }),
        Err(rustix::io::Errno::NOENT) => Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("path not found: `{}`", rel.display()),
        }),
        Err(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => Err(ToolError::Sdk {
            sdk_kind: "permission_denied".into(),
            message: "permission denied".into(),
        }),
        Err(rustix::io::Errno::ISDIR) => Err(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("path not found: `{}`", rel.display()),
        }),
        Err(other) => Err(ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("openat file failed: {other}"),
        }),
    }
}

/// Best-effort fallback for non-Unix targets (Windows).
///
/// Uses `canonicalize + starts_with + symlink_metadata` — the same chain
/// that was previously used everywhere. The TOCTOU window on Windows is
/// tracked separately (see lab-f1t2 follow-up for Windows NtCreateFile).
#[cfg(all(feature = "fs", not(unix)))]
fn open_no_follow_windows_fallback(root: &Path, rel: &Path) -> Result<std::fs::File, ToolError> {
    let mut check = root.to_path_buf();
    for component in rel.components() {
        check.push(component);
        let m = std::fs::symlink_metadata(&check).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("path not found: `{}`", rel.display()),
            },
            std::io::ErrorKind::PermissionDenied => ToolError::Sdk {
                sdk_kind: "permission_denied".into(),
                message: "permission denied".into(),
            },
            _ => ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: e.to_string(),
            },
        })?;
        if m.file_type().is_symlink() {
            return Err(ToolError::Sdk {
                sdk_kind: "permission_denied".into(),
                message: "symlinks are not followed for previews".into(),
            });
        }
    }

    let joined = root.join(rel);
    let canonical = std::fs::canonicalize(&joined).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("path not found: `{}`", rel.display()),
        },
        std::io::ErrorKind::PermissionDenied => ToolError::Sdk {
            sdk_kind: "permission_denied".into(),
            message: "permission denied".into(),
        },
        _ => ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: e.to_string(),
        },
    })?;
    if !canonical.starts_with(root) {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".into(),
            message: "path escapes workspace root".into(),
        });
    }
    if let Ok(canonical_rel) = canonical.strip_prefix(root) {
        let canonical_rel_str = canonical_rel.to_string_lossy();
        if deny_globset().is_match(canonical_rel_str.as_ref()) {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".into(),
                message: format!("path not found: `{}`", rel.display()),
            });
        }
    }
    let meta = std::fs::symlink_metadata(&canonical).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: e.to_string(),
    })?;
    if meta.file_type().is_symlink() {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".into(),
            message: "symlinks are not followed for previews".into(),
        });
    }
    std::fs::File::open(&canonical).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: e.to_string(),
    })
}

/// Strip path separators and control characters from a basename so it can
/// safely appear inside `Content-Disposition: attachment; filename="…"`.
/// Quotes and backslashes are escaped per RFC 6266 quoted-string rules.
#[cfg(feature = "fs")]
fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            '/' | '\\' => out.push('_'),
            c if c.is_control() => out.push('_'),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    if out.is_empty() {
        "file".to_string()
    } else {
        out
    }
}

#[cfg(all(test, feature = "fs"))]
mod tests {
    use super::*;
    use serde_json::json;
    // Unix-only: the symlink-behavior tests below are `#[cfg(unix)]`; the import
    // is gated to match so Windows test builds don't pull in `std::os::unix`.
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;
    use tempfile::tempdir;

    fn tree_layout() -> tempfile::TempDir {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::write(root.join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir_all(root.join("secrets")).unwrap();
        std::fs::write(root.join("secrets/.env"), "INNER=1").unwrap();
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/id_rsa"), "key").unwrap();
        std::fs::write(root.join("README.md"), "hi").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
        tmp
    }

    #[tokio::test]
    async fn list_root_suppresses_top_level_dotenv() {
        let tmp = tree_layout();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let value = dispatch_with_root(&root, "fs.list", json!({}))
            .await
            .unwrap();
        let names: Vec<String> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&".env".to_string()), "{names:?}");
        assert!(names.contains(&"README.md".to_string()));
        assert!(names.contains(&"src".to_string()));
        // .ssh directory itself — the deny-list targets `.ssh/**`, not `.ssh`
        // alone. We accept that a hidden dir name shows up but children are
        // unreachable via list because listing `.ssh` below also suppresses
        // them.
    }

    #[tokio::test]
    async fn list_subdir_suppresses_nested_dotenv() {
        let tmp = tree_layout();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let value = dispatch_with_root(&root, "fs.list", json!({"path": "secrets"}))
            .await
            .unwrap();
        let names: Vec<String> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&".env".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn list_ssh_dir_suppresses_id_rsa() {
        let tmp = tree_layout();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let value = dispatch_with_root(&root, "fs.list", json!({"path": ".ssh"}))
            .await
            .unwrap();
        let names: Vec<String> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&"id_rsa".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn list_rejects_path_escape() {
        let tmp = tree_layout();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = dispatch_with_root(&root, "fs.list", json!({"path": "../etc"}))
            .await
            .expect_err("err");
        assert!(matches!(err, ToolError::InvalidParam { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn list_rejects_unknown_action() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = dispatch_with_root(&root, "fs.nuke", json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::UnknownAction { .. }), "{err:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_symlink_escape_blocked_by_starts_with() {
        // Symlink pointing outside root must not leak targeted contents.
        let tmp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top").unwrap();
        unix_fs::symlink(outside.path(), tmp.path().join("escape")).unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = dispatch_with_root(&root, "fs.list", json!({"path": "escape"}))
            .await
            .expect_err("err");
        assert!(
            matches!(&err, ToolError::Sdk { sdk_kind, .. } if sdk_kind == "permission_denied"),
            "expected permission_denied; got {err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn list_dangling_symlink_reports_symlink_kind() {
        let tmp = tempdir().unwrap();
        unix_fs::symlink("/nonexistent/does/not/exist", tmp.path().join("broken")).unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let value = dispatch_with_root(&root, "fs.list", json!({}))
            .await
            .unwrap();
        let entries = value["entries"].as_array().unwrap();
        let broken = entries
            .iter()
            .find(|e| e["name"] == "broken")
            .expect("broken in listing");
        assert_eq!(broken["kind"], "symlink");
        assert_eq!(broken["accessible"], true);
    }

    #[tokio::test]
    async fn list_truncates_large_directory() {
        // Exercise the cap logic at a smaller scale — full 10k takes long
        // under test. We verify truncated=false for a small dir; the cap
        // is unit-tested via list_directory directly below.
        let tmp = tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(tmp.path().join(format!("f{i}")), "").unwrap();
        }
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let value = dispatch_with_root(&root, "fs.list", json!({}))
            .await
            .unwrap();
        assert_eq!(value["truncated"], false);
        assert_eq!(value["entries"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn list_directory_caps_at_list_cap() {
        // Directly test the cap logic with a mock directory containing
        // LIST_CAP + 2 entries.
        let tmp = tempdir().unwrap();
        let total = LIST_CAP + 2;
        for i in 0..total {
            std::fs::write(tmp.path().join(format!("f{i:05}")), "").unwrap();
        }
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let listing = list_directory(&root, "").expect("ok");
        assert_eq!(listing.entries.len(), LIST_CAP);
        assert!(listing.truncated);
    }

    #[tokio::test]
    async fn help_lists_fs_list() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let value = dispatch_with_root(&root, "help", json!({})).await.unwrap();
        let names: Vec<String> = value["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"fs.list".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn dispatch_rejects_fs_preview_via_mcp() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = dispatch_with_root(&root, "fs.preview", json!({"path": "foo"}))
            .await
            .expect_err("err");
        assert!(
            matches!(&err, ToolError::Sdk { sdk_kind, .. } if sdk_kind == "http_only"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn preview_rejects_missing_path() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = open_for_preview(&root, json!({})).await.expect_err("err");
        assert!(matches!(err, ToolError::MissingParam { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn preview_rejects_directory_target() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = open_for_preview(&root, json!({"path": "sub"}))
            .await
            .expect_err("err");
        // On Linux, openat2 opens a dir fd without failing; our metadata
        // check surfaces the error as invalid_param. Portably, this may
        // also come back as invalid_param via the fallback.
        assert!(matches!(err, ToolError::InvalidParam { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn preview_denylist_returns_not_found() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join(".env"), "SECRET=1").unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = open_for_preview(&root, json!({"path": ".env"}))
            .await
            .expect_err("err");
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "not_found"),
            other => panic!("expected Sdk not_found; got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn preview_symlink_refused() {
        let tmp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "hi").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            tmp.path().join("link-out"),
        )
        .unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = open_for_preview(&root, json!({"path": "link-out"}))
            .await
            .expect_err("err");
        // Linux openat2 with NO_SYMLINKS -> ELOOP -> permission_denied.
        // Fallback -> symlink_metadata is_symlink() -> permission_denied.
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "permission_denied"),
            other => panic!("expected Sdk permission_denied; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn preview_path_escape_refused() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = open_for_preview(&root, json!({"path": "../etc/passwd"}))
            .await
            .expect_err("err");
        // `..` is rejected at param-validation time, before we ever call openat2.
        assert!(matches!(err, ToolError::InvalidParam { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn preview_caps_max_bytes_at_server_limit() {
        let tmp = tempdir().unwrap();
        let big = tmp.path().join("big.bin");
        // Allocate a >2 MiB file quickly by writing sparsely.
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_PREVIEW_BYTES + 8192).unwrap();
        drop(f);
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        // caller asks for 100 MiB; server clamps to MAX_PREVIEW_BYTES
        let preview = open_for_preview(
            &root,
            json!({"path": "big.bin", "max_bytes": 100 * 1024 * 1024u64}),
        )
        .await
        .expect("ok");
        assert_eq!(preview.max_bytes, MAX_PREVIEW_BYTES);
    }

    #[tokio::test]
    async fn preview_svg_downgrades_to_octet_stream() {
        let tmp = tempdir().unwrap();
        std::fs::write(
            tmp.path().join("evil.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        )
        .unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let preview = open_for_preview(&root, json!({"path": "evil.svg"}))
            .await
            .expect("ok");
        assert_eq!(preview.content_type, "application/octet-stream");
        assert!(!super::super::client::is_inline_mime(preview.content_type));
    }

    #[tokio::test]
    async fn preview_png_keeps_image_mime() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("thumb.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let preview = open_for_preview(&root, json!({"path": "thumb.png"}))
            .await
            .expect("ok");
        assert_eq!(preview.content_type, "image/png");
        assert!(super::super::client::is_inline_mime(preview.content_type));
    }

    #[tokio::test]
    async fn preview_sanitizes_filename() {
        // The basename is extracted from the validated relative path so
        // raw separators never reach disposition_filename. Verify the
        // sanitizer still handles control chars defensively.
        let cleaned = sanitize_filename("weird\x01\"name/x.txt");
        assert!(!cleaned.contains('/'));
        assert!(!cleaned.contains('\x01'));
        assert!(cleaned.contains("\\\""));
    }

    /// A workspace-internal symlink whose target is a deny-listed file
    /// (`readme.txt -> .env`) must be refused. Pre-fix, the fallback would
    /// canonicalize through the link, find `.env` is still inside root, and
    /// then `symlink_metadata(canonical=.env)` would report a regular file —
    /// returning the secret to the caller. Exercising
    /// `open_no_follow_fallback` directly covers the non-Linux / pre-5.6
    /// path that doesn't get the kernel-enforced NO_SYMLINKS behavior.
    #[cfg(unix)]
    #[test]
    fn preview_fallback_rejects_symlink_to_denied_inside_root() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::write(root.join(".env"), "SECRET=topsecret").unwrap();
        unix_fs::symlink(root.join(".env"), root.join("readme.txt")).unwrap();

        let err = open_no_follow_fallback(&root, Path::new("readme.txt"))
            .expect_err("symlink to .env must be refused");
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "permission_denied"),
            other => panic!("expected Sdk permission_denied; got {other:?}"),
        }
    }

    /// help must remain reachable through the canonical `dispatch()` entry
    /// point regardless of whether `workspace.root` is configured. The
    /// fix moves help/schema in front of `require_workspace_root()` so the
    /// catalog stays discoverable on any env state — see bead lab-f1t2.24.
    ///
    /// Note: `require_workspace_root()` reads a process-global `OnceLock`,
    /// so this test does not control its state. The fix makes
    /// `dispatch("help", _)` short-circuit BEFORE that lookup, so the test
    /// is deterministic regardless of OnceLock seeding.
    #[tokio::test]
    async fn dispatch_help_works_without_workspace_configured() {
        let v = dispatch("help", json!({}))
            .await
            .expect("help ok without workspace");
        let names: Vec<String> = v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"fs.list".to_string()), "{names:?}");
    }

    /// schema must likewise remain reachable without a configured
    /// workspace — same rationale as the help test above.
    #[tokio::test]
    async fn dispatch_schema_works_without_workspace_configured() {
        let v = dispatch("schema", json!({"action": "fs.list"}))
            .await
            .expect("schema ok without workspace");
        assert_eq!(v["action"].as_str(), Some("fs.list"));
    }

    /// An intermediate symlink in the path (`sub -> .ssh`) must also be
    /// refused, even when the final basename (`id_rsa`) is itself a regular
    /// file. The component walk catches the symlink at the prefix before
    /// any canonicalize step has a chance to silently follow it.
    #[cfg(unix)]
    #[test]
    fn preview_fallback_rejects_intermediate_symlink() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/id_rsa"), "PRIVATE KEY").unwrap();
        unix_fs::symlink(root.join(".ssh"), root.join("sub")).unwrap();

        let err = open_no_follow_fallback(&root, Path::new("sub/id_rsa"))
            .expect_err("intermediate symlink must be refused");
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "permission_denied"),
            other => panic!("expected Sdk permission_denied; got {other:?}"),
        }
    }

    /// lab-f1t2.34: open_no_follow_unix (the O_NOFOLLOW per-component fallback)
    /// must reject a symlink at the final path component — including the case
    /// where the link points to a denied file that is lexically inside the root.
    ///
    /// This exercises the `open_no_follow_unix` path directly so that the
    /// O_NOFOLLOW behaviour is verified independently of the Linux openat2 path.
    #[cfg(unix)]
    #[test]
    fn open_no_follow_unix_rejects_symlink_at_final_component() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::write(root.join(".env"), "SECRET=topsecret").unwrap();
        // `notes.txt` is a symlink to `.env` inside the root.
        unix_fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();

        let err = open_no_follow_unix(&root, Path::new("notes.txt"))
            .expect_err("final-component symlink must be refused by openat O_NOFOLLOW");
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(
                sdk_kind, "permission_denied",
                "expected permission_denied, got: {sdk_kind}"
            ),
            other => panic!("expected Sdk permission_denied; got {other:?}"),
        }
    }

    /// lab-f1t2.34: open_no_follow_unix must accept a legitimate regular file
    /// (no symlinks anywhere in the path) — the happy path must still work.
    #[cfg(unix)]
    #[test]
    fn open_no_follow_unix_accepts_regular_file() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::write(root.join("readme.txt"), "hello world").unwrap();

        let _file = open_no_follow_unix(&root, Path::new("readme.txt"))
            .expect("regular file must be openable via openat O_NOFOLLOW");
    }

    /// lab-f1t2.34: open_no_follow_unix must reject an intermediate directory
    /// component that is a symlink — same semantics as the fallback.
    #[cfg(unix)]
    #[test]
    fn open_no_follow_unix_rejects_intermediate_symlink_dir() {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::write(root.join(".ssh/id_rsa"), "PRIVATE KEY").unwrap();
        unix_fs::symlink(root.join(".ssh"), root.join("sub")).unwrap();

        let err = open_no_follow_unix(&root, Path::new("sub/id_rsa"))
            .expect_err("intermediate symlink dir must be refused");
        match err {
            ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "permission_denied"),
            other => panic!("expected Sdk permission_denied; got {other:?}"),
        }
    }
}
