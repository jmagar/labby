//! Param parsing + path validation for the `fs` service.
//!
//! The validation surface is intentionally strict: callers supply a single
//! workspace-relative path, and we reject anything that could escape the
//! jail (absolute paths, Windows UNC prefixes, `..`, embedded NUL, paths
//! longer than the platform maximum). The returned `PathBuf` is safe to
//! join against the canonical workspace root — but callers still need to
//! canonicalize the joined path and re-assert `starts_with(root)` to close
//! the TOCTOU / symlink-escape holes.

use std::path::{Component, PathBuf};

use serde_json::Value;

use crate::dispatch::error::ToolError;

#[cfg(feature = "fs")]
use unicode_normalization::UnicodeNormalization;

/// Maximum length (in bytes) of a workspace-relative path.
pub const MAX_PATH_BYTES: usize = 4096;

/// Extracted params for `fs.list`.
#[derive(Debug, Clone)]
pub struct FsListParams {
    /// NFKC-normalized, validated workspace-relative path. Empty means
    /// "workspace root".
    pub relative: PathBuf,
    /// NFKC-normalized path as a forward-slash string, used as the deny-list
    /// match input. Empty when `relative` is empty.
    pub rel_str: String,
}

/// Extracted params for `fs.preview`.
#[derive(Debug, Clone)]
pub struct FsPreviewParams {
    /// NFKC-normalized, validated workspace-relative path. Empty means the
    /// workspace root itself — rejected at parse time because preview
    /// requires a file target.
    pub relative: PathBuf,
    /// Same rel path rendered as a forward-slash string for deny-list
    /// matching and audit logs.
    pub rel_str: String,
    /// Caller-requested byte cap; the dispatch layer always clamps this
    /// against the server-enforced [`crate::dispatch::fs::client::MAX_PREVIEW_BYTES`].
    pub max_bytes: Option<u64>,
}

/// Parse + validate params for `fs.preview`.
#[cfg(feature = "fs")]
pub fn parse_preview(params: &Value) -> Result<FsPreviewParams, ToolError> {
    let raw = match params.get("path") {
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        Some(Value::String(_) | Value::Null) | None => {
            return Err(ToolError::MissingParam {
                message: "missing required parameter `path`".into(),
                param: "path".into(),
            });
        }
        Some(_) => {
            return Err(ToolError::InvalidParam {
                message: "parameter `path` must be a string".into(),
                param: "path".into(),
            });
        }
    };
    let relative = validate_workspace_rel_path(raw)?;
    if relative.as_os_str().is_empty() {
        return Err(ToolError::InvalidParam {
            message: "parameter `path` must name a file, not the workspace root".into(),
            param: "path".into(),
        });
    }
    let rel_str = components_to_slash_string(&relative);

    let max_bytes = match params.get("max_bytes") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| ToolError::InvalidParam {
                message: "parameter `max_bytes` must be a non-negative integer".into(),
                param: "max_bytes".into(),
            })?;
            Some(n)
        }
    };

    Ok(FsPreviewParams {
        relative,
        rel_str,
        max_bytes,
    })
}

/// Parse and validate params for `fs.list`.
#[cfg(feature = "fs")]
pub fn parse_list(params: &Value) -> Result<FsListParams, ToolError> {
    let raw = match params.get("path") {
        None | Some(Value::Null) => "",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => {
            return Err(ToolError::InvalidParam {
                message: "parameter `path` must be a string".into(),
                param: "path".into(),
            });
        }
    };
    let relative = validate_workspace_rel_path(raw)?;
    let rel_str = components_to_slash_string(&relative);
    Ok(FsListParams { relative, rel_str })
}

/// Render a validated workspace-relative `PathBuf` as a forward-slash
/// string, used both as deny-list match input and audit-log path. Only
/// `Component::Normal` segments are emitted — `validate_workspace_rel_path`
/// already strips `CurDir` and rejects everything else, so the join is
/// platform-independent.
#[cfg(feature = "fs")]
fn components_to_slash_string(rel: &std::path::Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(n) => n.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Validate and NFKC-normalize a workspace-relative path.
///
/// Rejects:
/// - NUL bytes anywhere in the input
/// - Inputs longer than [`MAX_PATH_BYTES`]
/// - Absolute paths (`Component::RootDir`) and Windows UNC/drive prefixes
///   (`Component::Prefix`)
/// - `..` (`Component::ParentDir`) anywhere in the path
///
/// Returns the NFKC-normalized path as a `PathBuf` containing only
/// `Component::Normal` segments. `Component::CurDir` entries are stripped.
#[cfg(feature = "fs")]
pub fn validate_workspace_rel_path(raw: &str) -> Result<PathBuf, ToolError> {
    if raw.as_bytes().contains(&0) {
        return Err(ToolError::InvalidParam {
            message: "path contains NUL byte".into(),
            param: "path".into(),
        });
    }
    if raw.len() > MAX_PATH_BYTES {
        return Err(ToolError::InvalidParam {
            message: format!("path exceeds {MAX_PATH_BYTES} bytes"),
            param: "path".into(),
        });
    }

    // NFKC-normalize before checking components so callers can't sneak
    // Unicode-equivalent variants of `.` or `/` past the deny-list.
    let normalized: String = raw.nfkc().collect();

    let mut out = PathBuf::new();
    for component in std::path::Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => {
                let Some(s) = part.to_str() else {
                    return Err(ToolError::InvalidParam {
                        message: "path contains non-UTF-8 segment".into(),
                        param: "path".into(),
                    });
                };
                // Defense-in-depth: reject segments that normalize to `..`
                // (e.g. fullwidth `．．`) before they get pushed.
                if s == ".." {
                    return Err(ToolError::InvalidParam {
                        message: format!("path traversal rejected: `{raw}` contains `..`"),
                        param: "path".into(),
                    });
                }
                out.push(s);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(ToolError::InvalidParam {
                    message: format!("path traversal rejected: `{raw}` contains `..`"),
                    param: "path".into(),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::InvalidParam {
                    message: format!("absolute or prefixed paths are not allowed: `{raw}`"),
                    param: "path".into(),
                });
            }
        }
    }
    Ok(out)
}

#[cfg(all(test, feature = "fs"))]
mod tests {
    use super::*;

    fn reject(input: &str) {
        let err =
            validate_workspace_rel_path(input).expect_err(&format!("should reject {input:?}"));
        assert!(matches!(err, ToolError::InvalidParam { .. }), "{err:?}");
    }

    #[test]
    fn rejects_parent_dir() {
        reject("..");
        reject("../etc");
        reject("foo/../bar");
    }

    #[test]
    fn rejects_absolute() {
        reject("/etc/passwd");
    }

    #[test]
    fn rejects_windows_unc() {
        // `\\server\share` is interpreted as a Windows UNC prefix on
        // Windows, but on Unix each `\` is just a normal character in a
        // single segment — there is no portable way to force `Prefix`
        // recognition. We still reject backslashes-in-name from NFKC?
        // No — fall through test: `\\server\share` on Unix parses as a
        // single `Normal("\\\\server\\share")` segment, which is allowed.
        // On Windows it would be a Prefix — rejected.
        if cfg!(windows) {
            reject("\\\\server\\share");
        }
    }

    #[test]
    fn rejects_nul() {
        reject("foo\0bar");
    }

    #[test]
    fn rejects_overlong() {
        let big = "a".repeat(MAX_PATH_BYTES + 1);
        reject(&big);
    }

    #[test]
    fn rejects_nfkc_fullwidth_dot() {
        // U+FF0E FULLWIDTH FULL STOP — NFKC-normalizes to `.`. Two of
        // them make `..`, which must be rejected after normalization.
        reject("\u{FF0E}\u{FF0E}");
        reject("foo/\u{FF0E}\u{FF0E}");
    }

    #[test]
    fn accepts_empty() {
        let p = validate_workspace_rel_path("").expect("ok");
        assert!(p.as_os_str().is_empty());
    }

    #[test]
    fn accepts_nested_normal() {
        let p = validate_workspace_rel_path("src/main.rs").expect("ok");
        assert_eq!(p, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn strips_curdir() {
        let p = validate_workspace_rel_path("./foo/./bar").expect("ok");
        assert_eq!(p, PathBuf::from("foo/bar"));
    }

    #[test]
    fn parse_list_accepts_missing_path() {
        let params = serde_json::json!({});
        let parsed = parse_list(&params).expect("ok");
        assert!(parsed.relative.as_os_str().is_empty());
        assert_eq!(parsed.rel_str, "");
    }

    #[test]
    fn parse_list_rejects_non_string_path() {
        let params = serde_json::json!({ "path": 42 });
        let err = parse_list(&params).expect_err("err");
        assert!(matches!(err, ToolError::InvalidParam { .. }), "{err:?}");
    }

    #[test]
    fn parse_list_builds_forward_slash_rel_str() {
        let params = serde_json::json!({ "path": "a/b/c" });
        let parsed = parse_list(&params).expect("ok");
        assert_eq!(parsed.rel_str, "a/b/c");
    }
}
