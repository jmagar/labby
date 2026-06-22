//! Resolve the executable used to spawn `labby internal code-mode-runner`.

use std::path::{Path, PathBuf};

use crate::error::ToolError;

pub(super) fn resolve_runner_exe() -> Result<PathBuf, ToolError> {
    let current = std::env::current_exe().map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to locate current executable for Code Mode runner: {err}"),
    })?;
    let override_exe = std::env::var_os("LAB_CODE_MODE_RUNNER_EXE").map(PathBuf::from);
    resolve_runner_exe_from(current, override_exe)
}

pub(super) fn resolve_runner_exe_from(
    current_exe: PathBuf,
    override_exe: Option<PathBuf>,
) -> Result<PathBuf, ToolError> {
    if let Some(path) = override_exe {
        let path = validate_operator_override(path)?;
        tracing::warn!(
            runner_exe = %path.display(),
            "using LAB_CODE_MODE_RUNNER_EXE override for Code Mode runner"
        );
        return Ok(path);
    }

    if is_usable_exe(&current_exe) {
        return Ok(current_exe);
    }

    Err(ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!(
            "Code Mode runner executable is stale or unavailable: `{}`; restart labby.service or set LAB_CODE_MODE_RUNNER_EXE to a validated labby binary",
            current_exe.display()
        ),
    })
}

fn validate_operator_override(path: PathBuf) -> Result<PathBuf, ToolError> {
    if !path.is_absolute() {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "LAB_CODE_MODE_RUNNER_EXE must be an absolute path".to_string(),
        });
    }
    let canonical = std::fs::canonicalize(&path).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!(
            "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but it cannot be resolved: {err}",
            path.display()
        ),
    })?;
    if !is_usable_exe(&canonical) {
        return Err(ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!(
                "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but that file is not executable",
                canonical.display()
            ),
        });
    }
    reject_untrusted_permissions(&canonical)?;
    Ok(canonical)
}

fn is_usable_exe(path: &Path) -> bool {
    if path.to_string_lossy().ends_with(" (deleted)") {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn reject_untrusted_permissions(path: &Path) -> Result<(), ToolError> {
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to inspect `{}`: {err}", path.display()),
        })?;
        if meta.mode() & 0o022 != 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!(
                    "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but the file is group/world writable",
                    path.display()
                ),
            });
        }
        let current_uid = nix::unistd::Uid::current().as_raw();
        if meta.uid() != current_uid && meta.uid() != 0 {
            return Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!(
                    "LAB_CODE_MODE_RUNNER_EXE points at `{}`, but the file is not owned by the current user or root",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn uses_current_exe_when_it_is_usable() {
        let temp = tempfile::tempdir().unwrap();
        let current = temp.path().join("labby");
        std::fs::write(&current, b"binary").unwrap();
        #[cfg(unix)]
        make_executable(&current);

        let resolved = resolve_runner_exe_from(current.clone(), None).unwrap();

        assert_eq!(resolved, current);
    }

    #[test]
    fn deleted_current_exe_without_override_reports_restart_guidance() {
        let err = resolve_runner_exe_from(PathBuf::from("/usr/local/bin/labby (deleted)"), None)
            .unwrap_err();

        assert_eq!(err.kind(), "internal_error");
        assert!(err.to_string().contains("restart labby.service"));
    }

    #[test]
    fn override_must_be_absolute() {
        let err = resolve_runner_exe_from(
            PathBuf::from("/usr/local/bin/labby"),
            Some(PathBuf::from("target/debug/labby")),
        )
        .unwrap_err();

        assert_eq!(err.kind(), "invalid_param");
        assert!(err.to_string().contains("absolute path"));
    }

    #[test]
    fn override_must_point_to_usable_file() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing-labby");

        let err = resolve_runner_exe_from(PathBuf::from("/usr/local/bin/labby"), Some(missing))
            .unwrap_err();

        assert_eq!(err.kind(), "internal_error");
        assert!(err.to_string().contains("LAB_CODE_MODE_RUNNER_EXE"));
        assert!(err.to_string().contains("missing-labby"));
    }

    #[test]
    fn explicit_override_is_used_after_validation() {
        let temp = tempfile::tempdir().unwrap();
        let override_path = temp.path().join("labby");
        std::fs::write(&override_path, b"binary").unwrap();
        #[cfg(unix)]
        make_executable(&override_path);

        let resolved = resolve_runner_exe_from(
            PathBuf::from("/usr/local/bin/labby (deleted)"),
            Some(override_path.clone()),
        )
        .unwrap();

        assert_eq!(resolved, std::fs::canonicalize(override_path).unwrap());
    }
}
