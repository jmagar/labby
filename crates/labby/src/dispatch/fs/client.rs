//! Client / configuration for the workspace filesystem browser service.
//!
//! Responsibilities:
//!
//! - Resolve + canonicalize the configured Lab workspace root from
//!   `config.toml` at startup.
//! - Cache the canonical root in a process-global `OnceLock` so MCP dispatch
//!   (which has no `AppState` handle) can reach it without re-canonicalizing
//!   per-request.
//! - Build the credential deny-list `GlobSet` once and cache it (Phase 2).
//! - Host the Phase-3 preview constants: server byte cap, safe-MIME
//!   whitelist, and inline/attachment classification.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::config::LabConfig;
use crate::dispatch::error::ToolError;

#[cfg(feature = "fs")]
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

pub(crate) fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "workspace_not_configured".to_string(),
        message: "workspace.root does not point at an existing directory".to_string(),
    }
}

/// Resolve the configured workspace root, canonicalize it, and verify it
/// exists and is a directory.
///
/// Called once at startup (from `cli::serve`). Keep pure — no logging, no
/// side effects. The returned path is what callers feed into
/// `AppState::with_workspace_root`.
pub fn resolve_workspace_root(config: &LabConfig) -> std::io::Result<PathBuf> {
    let root = crate::config::workspace_root_path(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
    canonicalize_workspace_dir(root)
}

fn canonicalize_workspace_dir(path: PathBuf) -> std::io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("workspace.root must be absolute; got {}", path.display()),
        ));
    }
    if path.exists() && !std::fs::metadata(&path)?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("workspace.root is not a directory: {}", path.display()),
        ));
    }
    std::fs::create_dir_all(&path)?;
    let canonical = std::fs::canonicalize(&path)?;
    let meta = std::fs::metadata(&canonical)?;
    if !meta.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("workspace.root is not a directory: {}", canonical.display()),
        ));
    }
    Ok(canonical)
}

static WORKSPACE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Return the canonical workspace root, or a structured error if config is
/// invalid. First call canonicalizes; subsequent calls
/// return the cached value.
pub fn require_workspace_root() -> Result<&'static PathBuf, ToolError> {
    let cached = WORKSPACE_ROOT.get_or_init(|| {
        let cfg = crate::config::load_toml(&crate::config::toml_candidates()).ok()?;
        resolve_workspace_root(&cfg).ok()
    });
    cached.as_ref().ok_or_else(not_configured_error)
}

#[cfg(feature = "fs")]
const DENY_PATTERNS: &[&str] = &[
    ".env",
    "**/.env",
    ".env.*",
    "**/.env.*",
    "*.pem",
    "**/*.pem",
    "*.key",
    "**/*.key",
    "*.pfx",
    "**/*.pfx",
    "*.p12",
    "**/*.p12",
    ".git/config",
    "**/.git/config",
    ".git/credentials",
    "**/.git/credentials",
    ".ssh/**",
    "**/.ssh/**",
    "id_rsa*",
    "**/id_rsa*",
    "id_ecdsa*",
    "**/id_ecdsa*",
    "id_ed25519*",
    "**/id_ed25519*",
    "authorized_keys",
    "**/authorized_keys",
    "known_hosts",
    "**/known_hosts",
    ".aws/credentials",
    "**/.aws/credentials",
    ".aws/config",
    "**/.aws/config",
    ".gnupg/**",
    "**/.gnupg/**",
    ".netrc",
    "**/.netrc",
    ".pgpass",
    "**/.pgpass",
    ".docker/config.json",
    "**/.docker/config.json",
    "credentials",
    "**/credentials",
    "credentials.json",
    "**/credentials.json",
];

#[cfg(feature = "fs")]
#[allow(clippy::panic, clippy::expect_used)]
fn build_deny_globset() -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in DENY_PATTERNS {
        builder.add(
            GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .unwrap_or_else(|e| panic!("invalid deny-list glob {pattern}: {e}")),
        );
    }
    builder
        .build()
        .expect("deny-list globset build should not fail")
}

#[cfg(feature = "fs")]
static DENY_GLOBSET: OnceLock<GlobSet> = OnceLock::new();

/// Return the cached credential deny-list `GlobSet`.
#[cfg(feature = "fs")]
pub fn deny_globset() -> &'static GlobSet {
    DENY_GLOBSET.get_or_init(build_deny_globset)
}

/// Server-enforced hard cap on `fs.preview` response size. Caller-supplied
/// `max_bytes` is clamped to this value regardless of how large the client
/// asks for.
#[cfg(feature = "fs")]
pub const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

/// Classify a preview response's `Content-Type`.
///
/// Only a narrow image whitelist gets the matching MIME. Everything else —
/// including SVG (active XSS vector), HTML, scripts, and unknown types — is
/// served as `application/octet-stream`.
#[cfg(feature = "fs")]
#[must_use]
pub fn safe_content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Whether [`safe_content_type`] returned a whitelist hit.
#[cfg(feature = "fs")]
#[must_use]
pub fn is_inline_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn canonicalize_existing_dir_accepts_absolute_existing_dir() {
        let tmp = tempdir().expect("tempdir");
        let resolved = canonicalize_workspace_dir(tmp.path().to_path_buf()).expect("ok");
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }

    #[test]
    fn canonicalize_existing_dir_rejects_relative() {
        let err = canonicalize_workspace_dir(PathBuf::from("relative/path")).expect_err("err");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn canonicalize_existing_dir_creates_missing_directory() {
        let tmp = tempdir().expect("tempdir");
        let missing = tmp.path().join("created");
        let resolved = canonicalize_workspace_dir(missing.clone()).expect("ok");
        assert_eq!(resolved, std::fs::canonicalize(missing).expect("canonical"));
    }

    #[test]
    fn canonicalize_existing_dir_rejects_file_target() {
        let tmp = tempdir().expect("tempdir");
        let file = tmp.path().join("a-file");
        std::fs::write(&file, b"hi").expect("write");
        let err = canonicalize_workspace_dir(file).expect_err("err");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn deny_globset_suppresses_common_secrets() {
        let set = deny_globset();
        assert!(set.is_match(".env"));
        assert!(set.is_match("secrets/.env"));
        assert!(set.is_match(".ssh/id_rsa"));
        assert!(set.is_match("subdir/.aws/credentials"));
        assert!(set.is_match("server.pem"));
        assert!(set.is_match("nested/dir/cert.key"));
        assert!(set.is_match(".git/credentials"));
        assert!(set.is_match("project/.git/config"));
        assert!(set.is_match(".docker/config.json"));
        assert!(set.is_match("home/.netrc"));
        assert!(!set.is_match("README.md"));
        assert!(!set.is_match("src/main.rs"));
        assert!(!set.is_match("envoy.yaml"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn deny_globset_is_case_insensitive() {
        // Case-insensitive filesystems (macOS APFS/HFS+ default, Windows NTFS
        // default) treat `.ENV` and `.env` as the same file. Deny-list matching
        // must therefore be case-insensitive — otherwise a user (or attacker)
        // can read credential files via their uppercased names.
        let set = deny_globset();
        assert!(set.is_match(".ENV"));
        assert!(set.is_match(".Env"));
        assert!(set.is_match("server.PEM"));
        assert!(set.is_match("Id_Rsa"));
        assert!(set.is_match(".SSH/id_rsa"));
        assert!(set.is_match("Credentials.JSON"));
        // Negative cases must still be respected even with case-insensitive matching.
        assert!(!set.is_match("README.md"));
        assert!(!set.is_match("envoy.yaml"));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn safe_content_type_whitelists_images_only() {
        use std::path::Path;
        assert_eq!(safe_content_type(Path::new("a.png")), "image/png");
        assert_eq!(safe_content_type(Path::new("b.JPG")), "image/jpeg");
        assert_eq!(safe_content_type(Path::new("c.jpeg")), "image/jpeg");
        assert_eq!(safe_content_type(Path::new("d.gif")), "image/gif");
        assert_eq!(safe_content_type(Path::new("e.webp")), "image/webp");
        assert_eq!(
            safe_content_type(Path::new("evil.svg")),
            "application/octet-stream"
        );
        assert_eq!(
            safe_content_type(Path::new("index.html")),
            "application/octet-stream"
        );
        assert_eq!(
            safe_content_type(Path::new("script.js")),
            "application/octet-stream"
        );
        assert_eq!(
            safe_content_type(Path::new("no-ext")),
            "application/octet-stream"
        );
    }

    #[cfg(feature = "fs")]
    #[test]
    fn is_inline_mime_matches_whitelist() {
        assert!(is_inline_mime("image/png"));
        assert!(is_inline_mime("image/jpeg"));
        assert!(is_inline_mime("image/gif"));
        assert!(is_inline_mime("image/webp"));
        assert!(!is_inline_mime("image/svg+xml"));
        assert!(!is_inline_mime("application/octet-stream"));
        assert!(!is_inline_mime("text/html"));
    }
}
