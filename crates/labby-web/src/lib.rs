//! Gateway admin static asset embedding, resolution, and header helpers.
//!
//! This crate owns **only** asset lookup: the build-time embedded bundle, the
//! configured-filesystem resolution (with symlink-escape rejection), and the
//! surface-neutral content-type / cache-control headers. It returns data, not
//! transport responses — axum routing, node-role policy, auth policy, and SPA
//! fallback ordering live in the consuming product/daemon crates.
//!
//! # Example
//!
//! ```no_run
//! # async fn demo() -> Result<(), labby_web::AssetError> {
//! use labby_web::{serve_asset, AssetSource};
//!
//! // Configured directory wins; otherwise fall back to the embedded bundle.
//! let source = AssetSource::from_configured_dir_or_embedded(None);
//! let asset = serve_asset("/", &source).await?;
//! assert_eq!(asset.content_type, "text/html; charset=utf-8");
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

pub mod assets;
pub mod fs_assets;
pub mod response;

pub use assets::{
    embedded_asset, embedded_asset_paths, embedded_assets_available, resolve_embedded_asset,
};
pub use fs_assets::{AssetSource, resolve_fs_asset, sanitize_relative_path};
pub use response::{AssetError, AssetResponse, cache_control_for, guess_content_type, serve_asset};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn filesystem_assets_serve_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();

        let source = AssetSource::Directory(dir.path().to_path_buf());
        let asset = serve_asset("/gateways/", &source).await.unwrap();
        assert_eq!(asset.content_type, "text/html; charset=utf-8");
        assert_eq!(asset.cache_control, "no-store");
        assert_eq!(asset.bytes, b"<html><body>Labby</body></html>");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn filesystem_assets_reject_symlink_escape() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("index.html"),
            "<html><body>Labby</body></html>",
        )
        .unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top-secret").unwrap();
        unix_fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("secret.txt"),
        )
        .unwrap();

        let source = AssetSource::Directory(dir.path().to_path_buf());
        let result = serve_asset("/secret.txt", &source).await;
        assert!(matches!(result, Err(AssetError::NotFound)));
    }

    #[tokio::test]
    async fn filesystem_missing_asset_falls_back_to_index() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "spa").unwrap();

        let source = AssetSource::Directory(dir.path().to_path_buf());
        let asset = serve_asset("/does/not/exist", &source).await.unwrap();
        assert_eq!(asset.bytes, b"spa");
        assert_eq!(asset.cache_control, "no-store");
    }

    #[tokio::test]
    async fn filesystem_missing_file_like_asset_is_not_spa_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "spa").unwrap();

        let source = AssetSource::Directory(dir.path().to_path_buf());
        let result = serve_asset("/install.sh", &source).await;

        assert!(matches!(result, Err(AssetError::NotFound)));
    }

    #[tokio::test]
    async fn filesystem_shell_script_uses_shell_content_type() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("install.sh"), "#!/bin/sh\n").unwrap();

        let source = AssetSource::Directory(dir.path().to_path_buf());
        let asset = serve_asset("/install.sh", &source).await.unwrap();

        assert_eq!(asset.content_type, "text/x-shellscript; charset=utf-8");
        assert_eq!(asset.cache_control, "no-store");
    }

    #[tokio::test]
    async fn filesystem_missing_root_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        // No index.html written, so even the SPA fallback fails to canonicalize.
        let source = AssetSource::Directory(dir.path().to_path_buf());
        let result = serve_asset("/", &source).await;
        assert!(matches!(result, Err(AssetError::NotFound)));
    }

    #[tokio::test]
    async fn embedded_assets_serve_when_present() {
        // In a fresh clone `apps/gateway-admin/out` is empty, which is a valid
        // backend-only state — skip rather than fail spuriously.
        if !embedded_assets_available() {
            eprintln!(
                "skipping: apps/gateway-admin/out/index.html missing — \
                 run `pnpm --filter gateway-admin build` to populate"
            );
            return;
        }
        let source = AssetSource::Embedded;
        let asset = serve_asset("/", &source).await.unwrap();
        assert_eq!(asset.cache_control, "no-store");
        assert!(asset.content_type.contains("text/html"));
    }

    #[tokio::test]
    async fn embedded_install_script_is_shell_and_not_immutable_when_present() {
        if !embedded_assets_available() {
            eprintln!(
                "skipping: apps/gateway-admin/out/install.sh missing — \
                 run `pnpm --filter gateway-admin build` to populate"
            );
            return;
        }
        let source = AssetSource::Embedded;
        let asset = serve_asset("/install.sh", &source).await.unwrap();
        assert_eq!(asset.content_type, "text/x-shellscript; charset=utf-8");
        assert_eq!(asset.cache_control, "no-store");
        assert!(asset.bytes.starts_with(b"#!/bin/sh"));
    }

    #[tokio::test]
    async fn embedded_missing_file_like_asset_is_not_spa_fallback() {
        if !embedded_assets_available() {
            return;
        }
        let source = AssetSource::Embedded;
        let result = serve_asset("/missing.js", &source).await;
        assert!(matches!(result, Err(AssetError::NotFound)));
    }

    #[tokio::test]
    async fn embedded_missing_bundle_is_not_found() {
        if embedded_assets_available() {
            // Bundle present in this build; the "missing" assertion does not apply.
            return;
        }
        let source = AssetSource::Embedded;
        let result = serve_asset("/", &source).await;
        assert!(matches!(result, Err(AssetError::NotFound)));
    }

    #[test]
    fn sanitize_rejects_traversal() {
        assert_eq!(sanitize_relative_path("/../etc/passwd"), PathBuf::new());
        assert_eq!(sanitize_relative_path("/a/b"), PathBuf::from("a/b"));
        assert_eq!(sanitize_relative_path("/"), PathBuf::new());
    }

    #[test]
    fn content_type_and_cache_by_extension() {
        assert_eq!(
            guess_content_type(std::path::Path::new("app.js")),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            cache_control_for(std::path::Path::new("static/app.js")),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            cache_control_for(std::path::Path::new("index.html")),
            "no-store"
        );
        assert_eq!(
            cache_control_for(std::path::Path::new("install.sh")),
            "no-store"
        );
    }
}
