//! Surface-neutral asset response shaping.
//!
//! This module returns data (`AssetResponse`) and errors (`AssetError`), never
//! transport-specific responses. Callers (axum handlers, etc.) translate
//! `AssetResponse` into their own response type and choose how to handle
//! `AssetError`. Content-type and cache-control are derived here so every
//! consuming surface ships identical headers.

use std::path::Path;

use crate::assets;
use crate::fs_assets::{self, AssetSource};

/// A resolved asset, ready for a transport layer to turn into a response.
///
/// `bytes` is the full asset body. Surfaces serving a `HEAD` request should send
/// the headers but omit the body themselves; this type always carries the body
/// so the same value works for `GET` and `HEAD`.
#[derive(Debug, Clone)]
pub struct AssetResponse {
    /// The asset body.
    pub bytes: Vec<u8>,
    /// `Content-Type` header value (static, includes charset where relevant).
    pub content_type: &'static str,
    /// `Cache-Control` header value (`no-store` for `index.html`, immutable
    /// long-lived caching otherwise).
    pub cache_control: &'static str,
}

/// Why an asset could not be served.
///
/// `NotFound` covers every "serve a 404" case the previous Labby handler mapped
/// to `StatusCode::NOT_FOUND`: missing embedded bundle, sanitized-away paths,
/// symlink escapes outside the configured root, canonicalization failures, and
/// read failures. The transport layer decides the concrete status code.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    /// No asset matched the request and no SPA fallback was available.
    #[error("asset not found")]
    NotFound,
}

/// `Content-Type` for a resolved asset path, by extension.
#[must_use]
pub fn guess_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("map") => "application/json",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// `Cache-Control` for a resolved asset path. The SPA entry point must never be
/// cached so deploys take effect immediately; every other (content-hashed)
/// asset is cached aggressively.
#[must_use]
pub fn cache_control_for(path: &Path) -> &'static str {
    if path.file_name().and_then(|name| name.to_str()) == Some("index.html") {
        "no-store"
    } else {
        "public, max-age=31536000, immutable"
    }
}

fn response_for(bytes: Vec<u8>, path: &Path) -> AssetResponse {
    AssetResponse {
        content_type: guess_content_type(path),
        cache_control: cache_control_for(path),
        bytes,
    }
}

/// Resolve `request_path` against `source` and return the asset to serve.
///
/// Behavior mirrors the previous Labby handler exactly:
///
/// - For a configured filesystem directory: sanitize the path, resolve it
///   (file → itself, directory → its `index.html`, missing → root `index.html`
///   SPA fallback), canonicalize both the root and the resolved path, reject any
///   path that escapes the canonical root, then read the file. Any failure in
///   that chain yields [`AssetError::NotFound`].
/// - For the embedded bundle: sanitize the path and look up the exact key, then
///   `<path>/index.html`, then the root `index.html` SPA fallback. A missing
///   bundle yields [`AssetError::NotFound`].
pub async fn serve_asset(
    request_path: &str,
    source: &AssetSource,
) -> Result<AssetResponse, AssetError> {
    match source {
        AssetSource::Directory(base_dir) => {
            let resolved = fs_assets::resolve_fs_asset(base_dir, request_path).await?;
            let bytes = tokio::fs::read(&resolved).await.map_err(|error| {
                tracing::warn!(path = %resolved.display(), error = %error, "failed to serve web asset");
                AssetError::NotFound
            })?;
            Ok(response_for(bytes, &resolved))
        }
        AssetSource::Embedded => {
            let (bytes, path) =
                assets::resolve_embedded_asset(request_path).ok_or(AssetError::NotFound)?;
            Ok(response_for(bytes.to_vec(), &path))
        }
    }
}
