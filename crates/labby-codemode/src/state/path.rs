use std::path::{Component, Path};

use crate::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct VirtualPath(String);

impl VirtualPath {
    pub(crate) fn parse(raw: &str) -> Result<Self, ToolError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "/" {
            return Err(ToolError::InvalidParam {
                message: "state path must name a file or directory inside the workspace"
                    .to_string(),
                param: "path".to_string(),
            });
        }

        let normalized = trimmed.replace('\\', "/");
        if has_windows_drive_prefix(&normalized) {
            return Err(path_traversal(raw));
        }

        let stripped = normalized.trim_start_matches('/');
        let mut parts = Vec::new();
        for component in Path::new(stripped).components() {
            match component {
                Component::Normal(value) => {
                    let part = value.to_string_lossy();
                    if !part.is_empty() {
                        parts.push(part.to_string());
                    }
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(path_traversal(raw));
                }
            }
        }

        if parts.is_empty() {
            return Err(ToolError::InvalidParam {
                message: "state path must name a file or directory inside the workspace"
                    .to_string(),
                param: "path".to_string(),
            });
        }

        let value = parts.join("/");
        reject_credential_like_path(&value)?;
        Ok(Self(value))
    }

    pub(crate) fn parse_read_scope(raw: &str) -> Result<Self, ToolError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == "/" {
            return Ok(Self(String::new()));
        }
        Self::parse(raw)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn path_traversal(raw: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "path_traversal".to_string(),
        message: format!("state path `{raw}` escapes the workspace"),
    }
}

fn reject_credential_like_path(path: &str) -> Result<(), ToolError> {
    let lower = path.to_ascii_lowercase();
    if lower
        .split('/')
        .any(|segment| segment == ".git" || segment == ".labby-state")
    {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".to_string(),
            message: "state path is denied because it targets provider metadata".to_string(),
        });
    }

    let denied = [
        ".env",
        ".ssh/",
        ".aws/",
        ".config/gcloud/",
        ".netrc",
        "id_rsa",
        "id_ed25519",
    ];
    if denied
        .iter()
        .any(|needle| lower == *needle || lower.contains(needle))
    {
        return Err(ToolError::Sdk {
            sdk_kind: "permission_denied".to_string(),
            message: "state path is denied because it looks credential-related".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_path_accepts_rooted_and_relative_paths() {
        assert_eq!(
            VirtualPath::parse("/src/app.rs").unwrap().as_str(),
            "src/app.rs"
        );
        assert_eq!(
            VirtualPath::parse("src/app.rs").unwrap().as_str(),
            "src/app.rs"
        );
    }

    #[test]
    fn virtual_path_rejects_escape_and_host_paths() {
        for raw in ["../secret", "/../secret", "C:/Users/x", "C:relative", "/"] {
            assert!(VirtualPath::parse(raw).is_err(), "{raw} should fail");
        }
    }

    #[test]
    fn virtual_path_read_scope_accepts_workspace_root() {
        for raw in ["", ".", "/"] {
            assert_eq!(VirtualPath::parse_read_scope(raw).unwrap().as_str(), "");
        }
    }

    #[test]
    fn virtual_path_normalizes_windows_separators() {
        assert_eq!(
            VirtualPath::parse("src\\\\app.rs").unwrap().as_str(),
            "src/app.rs"
        );
    }

    #[test]
    fn virtual_path_rejects_credential_like_paths() {
        for raw in [
            ".env",
            "src/.env",
            ".ssh/id_ed25519",
            ".git",
            ".git/config",
            ".git/HEAD",
            "src/.git/config",
            "src/.git/hooks/pre-commit",
            ".labby-state/plans/abc.json",
        ] {
            let err = VirtualPath::parse(raw).expect_err("credential path should fail");
            assert_eq!(err.kind(), "permission_denied");
        }
    }

    #[test]
    fn virtual_path_allows_git_substrings_outside_reserved_segments() {
        assert_eq!(
            VirtualPath::parse("docs/foo.gitkeep").unwrap().as_str(),
            "docs/foo.gitkeep"
        );
    }
}
