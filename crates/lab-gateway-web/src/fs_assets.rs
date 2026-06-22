//! Configured filesystem asset resolution with symlink-escape rejection.
//!
//! When a gateway is configured to serve assets from a directory on disk
//! (instead of, or in addition to, the embedded bundle), requests resolve
//! through [`resolve_fs_asset`]. Resolution canonicalizes both the configured
//! root and the resolved candidate and rejects any path that escapes the root —
//! this is the symlink-escape defense and must not be relaxed.

use std::path::{Component, Path, PathBuf};

use crate::response::AssetError;

/// Where assets come from for a given request.
///
/// The caller (a transport/product layer) owns the policy of which source to
/// use; this enum just carries the resolved choice into [`serve_asset`].
///
/// [`serve_asset`]: crate::response::serve_asset
#[derive(Debug, Clone)]
pub enum AssetSource {
    /// Serve from a configured filesystem directory (canonicalized per request,
    /// escapes rejected).
    Directory(PathBuf),
    /// Serve from the build-time embedded bundle.
    Embedded,
}

impl AssetSource {
    /// Serve from `dir` when a directory is configured, otherwise fall back to
    /// the embedded bundle. This mirrors the previous Labby precedence: a
    /// configured directory always wins; the embedded bundle is the no-config
    /// fallback.
    #[must_use]
    pub fn from_configured_dir_or_embedded(dir: Option<PathBuf>) -> Self {
        match dir {
            Some(dir) => Self::Directory(dir),
            None => Self::Embedded,
        }
    }
}

/// Strip a request path down to a safe relative path, dropping any leading
/// slashes and rejecting traversal (`..`), absolute, prefix, and root
/// components by collapsing to an empty path.
#[must_use]
pub fn sanitize_relative_path(path: &str) -> PathBuf {
    let trimmed = path.trim_start_matches('/');
    let mut out = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return PathBuf::new();
            }
        }
    }
    out
}

async fn resolve_candidate_path(base_dir: &Path, request_path: &str) -> PathBuf {
    let relative = sanitize_relative_path(request_path);
    let candidate = if relative.as_os_str().is_empty() {
        base_dir.join("index.html")
    } else {
        base_dir.join(relative)
    };

    match tokio::fs::metadata(&candidate).await {
        Ok(metadata) if metadata.is_file() => candidate,
        Ok(metadata) if metadata.is_dir() => candidate.join("index.html"),
        _ => base_dir.join("index.html"),
    }
}

/// Resolve `request_path` against the configured `base_dir`, returning the
/// canonical path of the file to serve.
///
/// Resolution order matches the previous Labby handler:
///
/// 1. Sanitize the path and map it to a candidate (file → itself, directory →
///    its `index.html`, missing → root `index.html` SPA fallback).
/// 2. Canonicalize the configured root and the resolved candidate.
/// 3. Reject (as [`AssetError::NotFound`]) any resolved path that does not stay
///    within the canonical root — this is the symlink-escape defense.
///
/// A canonicalization failure on either the root or the candidate also yields
/// [`AssetError::NotFound`].
pub async fn resolve_fs_asset(base_dir: &Path, request_path: &str) -> Result<PathBuf, AssetError> {
    let resolved = resolve_candidate_path(base_dir, request_path).await;

    let canonical_base = tokio::fs::canonicalize(base_dir).await.map_err(|error| {
        tracing::warn!(path = %base_dir.display(), error = %error, "failed to canonicalize web assets root");
        AssetError::NotFound
    })?;

    let canonical_resolved = tokio::fs::canonicalize(&resolved).await.map_err(|error| {
        tracing::warn!(path = %resolved.display(), error = %error, "failed to canonicalize web asset");
        AssetError::NotFound
    })?;

    if !canonical_resolved.starts_with(&canonical_base) {
        tracing::warn!(
            requested = %resolved.display(),
            resolved = %canonical_resolved.display(),
            base = %canonical_base.display(),
            "rejected web asset request that escaped the asset root"
        );
        return Err(AssetError::NotFound);
    }

    Ok(canonical_resolved)
}
