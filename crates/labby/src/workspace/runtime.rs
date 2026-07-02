use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct WorkspaceRuntime {
    workspace_root: Result<PathBuf, String>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceRuntimeConfig {
    pub root: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceRuntimeBuilder {
    config: WorkspaceRuntimeConfig,
}

impl WorkspaceRuntimeBuilder {
    #[must_use]
    pub fn new(config: WorkspaceRuntimeConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn build(self) -> WorkspaceRuntime {
        WorkspaceRuntime {
            workspace_root: resolve_workspace_root(&self.config).map_err(|error| error.to_string()),
        }
    }
}

impl WorkspaceRuntime {
    #[must_use]
    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref().ok()
    }

    #[must_use]
    pub fn workspace_root_error(&self) -> Option<&str> {
        self.workspace_root.as_ref().err().map(String::as_str)
    }

    #[must_use]
    pub const fn should_mount_http_routes(
        web_ui_auth_disabled: bool,
        api_auth_configured: bool,
    ) -> bool {
        !web_ui_auth_disabled || api_auth_configured
    }
}

fn resolve_workspace_root(config: &WorkspaceRuntimeConfig) -> std::io::Result<PathBuf> {
    let root = match &config.root {
        Some(root) => expand_home_path(root, config.home.as_deref())?,
        None => {
            let home = config.home.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "HOME is not set and workspace.root is not configured",
                )
            })?;
            home.join(".labby").join("stash")
        }
    };
    canonicalize_workspace_dir(root)
}

fn expand_home_path(path: &Path, home: Option<&Path>) -> std::io::Result<PathBuf> {
    let raw = path.as_os_str().to_string_lossy();
    if raw == "~" {
        return home.map(Path::to_path_buf).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME is not set and workspace.root uses home expansion",
            )
        });
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return home.map(|home| home.join(rest)).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME is not set and workspace.root uses home expansion",
            )
        });
    }
    Ok(path.to_path_buf())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_resolves_configured_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkspaceRuntimeConfig {
            root: Some(temp.path().to_path_buf()),
            home: None,
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path()).expect("canonical")
        );
        assert!(runtime.workspace_root_error().is_none());
    }

    #[test]
    fn builder_keeps_invalid_workspace_root_unset() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("not-a-dir");
        std::fs::write(&file, b"not a directory").expect("write");
        let config = WorkspaceRuntimeConfig {
            root: Some(file),
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert!(runtime.workspace_root().is_none());
        assert!(runtime.workspace_root_error().is_some());
    }

    #[test]
    fn builder_uses_home_default_when_root_is_unset() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkspaceRuntimeConfig {
            root: None,
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path().join(".labby").join("stash")).expect("canonical")
        );
    }

    #[test]
    fn builder_expands_tilde_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkspaceRuntimeConfig {
            root: Some(PathBuf::from("~/stash")),
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path().join("stash")).expect("canonical")
        );
    }

    #[test]
    fn builder_expands_windows_style_tilde_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkspaceRuntimeConfig {
            root: Some(PathBuf::from("~\\stash")),
            home: Some(temp.path().to_path_buf()),
        };

        let runtime = WorkspaceRuntimeBuilder::new(config).build();

        assert_eq!(
            runtime.workspace_root().expect("workspace root"),
            std::fs::canonicalize(temp.path().join("stash")).expect("canonical")
        );
    }

    #[test]
    fn mount_policy_refuses_disabled_auth_without_api_auth() {
        assert!(!WorkspaceRuntime::should_mount_http_routes(true, false));
    }

    #[test]
    fn mount_policy_allows_disabled_auth_when_api_auth_is_configured() {
        assert!(WorkspaceRuntime::should_mount_http_routes(true, true));
    }
}
