//! Local-only Incus helpers for host-side Labby gateway bootstrap.
//!
//! These helpers are intentionally CLI-only. They are not in the setup action
//! catalog and must not be exposed through MCP, HTTP, or Code Mode.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_yaml::Value;

use crate::dispatch::error::ToolError;

const INCUS_BOOTSTRAP_SCRIPT: &str = include_str!("../../../../../scripts/incus-bootstrap.sh");
const INSTALL_SCRIPT: &str = include_str!("../../../../../scripts/install.sh");
const GATEWAY_PROFILE_YAML: &str =
    include_str!("../../../../../config/incus/labby-gateway-profile.yaml");
const BACKUP_CONFIG_YAML: &str = include_str!("../../../../../config/incus/labby-backup.yaml");

const SUPPORTED_BACKUP_KEYS: &[&str] = &[
    "snapshots.schedule",
    "snapshots.expiry",
    "snapshots.pattern",
    "snapshots.schedule.stopped",
];

#[derive(Debug, Deserialize)]
struct IncusConfigDocument {
    config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigApplyOutcome {
    pub container: String,
    pub dry_run: bool,
    pub applied: Vec<BackupConfigEntry>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapOptions {
    pub name: Option<String>,
    pub image: Option<String>,
    pub profile_name: Option<String>,
    pub backup_config: Option<PathBuf>,
    pub no_backup_config: bool,
    pub runtime_profile_name: Option<String>,
    pub storage_driver: Option<String>,
    pub storage_pool: Option<String>,
    pub storage_source: Option<String>,
    pub version: Option<String>,
    pub local_binary: Option<PathBuf>,
    pub skip_install: bool,
    pub dry_run: bool,
    pub tailscale_ssh: bool,
    pub allow_source_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapArtifacts {
    pub root: PathBuf,
    pub bootstrap_script: PathBuf,
    pub install_script: PathBuf,
    pub profile_file: PathBuf,
    pub backup_config_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
}

pub(crate) fn parse_backup_config(path: &Path) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ToolError::Sdk {
        message: format!("failed to read Incus backup config {}: {e}", path.display()),
        sdk_kind: "incus_backup_config_read_failed".into(),
    })?;
    parse_backup_config_str(&raw)
}

pub(crate) fn parse_backup_config_str(raw: &str) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let doc: IncusConfigDocument = serde_yaml::from_str(raw).map_err(|e| ToolError::Sdk {
        message: format!("invalid Incus backup YAML: {e}"),
        sdk_kind: "incus_backup_config_invalid_yaml".into(),
    })?;

    let mut entries = Vec::new();
    for (key, value) in doc.config {
        validate_backup_key(&key)?;
        entries.push(BackupConfigEntry {
            key,
            value: scalar_to_string(value)?,
        });
    }
    if entries.is_empty() {
        return Err(ToolError::Sdk {
            message: "Incus backup config must contain at least one supported config key".into(),
            sdk_kind: "incus_backup_config_empty".into(),
        });
    }
    Ok(entries)
}

pub(crate) fn apply_backup_config(
    container: &str,
    path: &Path,
    dry_run: bool,
) -> Result<BackupConfigApplyOutcome, ToolError> {
    if container.trim().is_empty() {
        return Err(ToolError::MissingParam {
            message: "missing required parameter `container`".into(),
            param: "container".into(),
        });
    }
    let entries = parse_backup_config(path)?;
    if !dry_run {
        for entry in &entries {
            let status = Command::new("incus")
                .arg("config")
                .arg("set")
                .arg(container)
                .arg(&entry.key)
                .arg(&entry.value)
                .status()
                .map_err(|e| ToolError::Sdk {
                    message: format!("failed to run incus config set: {e}"),
                    sdk_kind: "incus_config_set_failed".into(),
                })?;
            if !status.success() {
                return Err(ToolError::Sdk {
                    message: format!(
                        "incus config set failed for {} on container {}",
                        entry.key, container
                    ),
                    sdk_kind: "incus_config_set_failed".into(),
                });
            }
        }
    }
    Ok(BackupConfigApplyOutcome {
        container: container.to_string(),
        dry_run,
        applied: entries,
    })
}

pub(crate) fn materialize_bootstrap_artifacts(
    root: &Path,
) -> Result<IncusBootstrapArtifacts, ToolError> {
    let scripts_dir = root.join("scripts");
    let config_dir = root.join("config").join("incus");
    std::fs::create_dir_all(&scripts_dir).map_err(|e| ToolError::Sdk {
        message: format!("failed to create {}: {e}", scripts_dir.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    std::fs::create_dir_all(&config_dir).map_err(|e| ToolError::Sdk {
        message: format!("failed to create {}: {e}", config_dir.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;

    let bootstrap_script = scripts_dir.join("incus-bootstrap.sh");
    let install_script = scripts_dir.join("install.sh");
    let profile_file = config_dir.join("labby-gateway-profile.yaml");
    let backup_config_file = config_dir.join("labby-backup.yaml");

    write_materialized_file(&bootstrap_script, INCUS_BOOTSTRAP_SCRIPT, 0o755)?;
    write_materialized_file(&install_script, INSTALL_SCRIPT, 0o755)?;
    write_materialized_file(&profile_file, GATEWAY_PROFILE_YAML, 0o644)?;
    write_materialized_file(&backup_config_file, BACKUP_CONFIG_YAML, 0o644)?;

    Ok(IncusBootstrapArtifacts {
        root: root.to_path_buf(),
        bootstrap_script,
        install_script,
        profile_file,
        backup_config_file,
    })
}

pub(crate) fn bootstrap_command(
    artifacts: &IncusBootstrapArtifacts,
    options: &IncusBootstrapOptions,
) -> Result<IncusBootstrapCommand, ToolError> {
    let mut args = vec![artifacts.bootstrap_script.as_os_str().to_os_string()];
    if options.no_backup_config && options.backup_config.is_some() {
        return Err(ToolError::Sdk {
            message: "--backup-config cannot be combined with --no-backup-config".into(),
            sdk_kind: "incus_bootstrap_invalid_options".into(),
        });
    }
    push_option(&mut args, "--name", options.name.as_deref());
    push_option(&mut args, "--image", options.image.as_deref());
    push_option(&mut args, "--profile-name", options.profile_name.as_deref());
    push_path_option(&mut args, "--profile-file", &artifacts.profile_file);
    if options.no_backup_config {
        push_flag(&mut args, "--no-backup-config", true);
    } else {
        let backup_config = options
            .backup_config
            .clone()
            .or_else(backup_config_from_env)
            .as_ref()
            .map(|path| absolutize_user_path(path))
            .transpose()?
            .unwrap_or_else(|| artifacts.backup_config_file.clone());
        push_path_option(&mut args, "--backup-config", &backup_config);
    }
    push_option(
        &mut args,
        "--runtime-profile-name",
        options.runtime_profile_name.as_deref(),
    );
    push_option(
        &mut args,
        "--storage-driver",
        options.storage_driver.as_deref(),
    );
    push_option(&mut args, "--storage-pool", options.storage_pool.as_deref());
    push_option(
        &mut args,
        "--storage-source",
        options.storage_source.as_deref(),
    );
    push_option(&mut args, "--version", options.version.as_deref());
    if let Some(local_binary) = &options.local_binary {
        push_path_option(
            &mut args,
            "--local-binary",
            &absolutize_user_path(local_binary)?,
        );
    }
    push_flag(&mut args, "--skip-install", options.skip_install);
    push_flag(&mut args, "--dry-run", options.dry_run);
    push_flag(&mut args, "--tailscale-ssh", options.tailscale_ssh);
    push_flag(
        &mut args,
        "--allow-source-fallback",
        options.allow_source_fallback,
    );

    Ok(IncusBootstrapCommand {
        program: OsString::from("sh"),
        args,
        current_dir: artifacts.root.clone(),
    })
}

pub(crate) fn run_incus_bootstrap(options: IncusBootstrapOptions) -> Result<(), ToolError> {
    let tempdir = tempfile::tempdir().map_err(|e| ToolError::Sdk {
        message: format!("failed to create Incus bootstrap tempdir: {e}"),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    let artifacts = materialize_bootstrap_artifacts(tempdir.path())?;
    let command = bootstrap_command(&artifacts, &options)?;
    let status = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.current_dir)
        .status()
        .map_err(|e| ToolError::Sdk {
            message: format!("failed to run Incus bootstrap: {e}"),
            sdk_kind: "incus_bootstrap_failed".into(),
        })?;
    if !status.success() {
        return Err(ToolError::Sdk {
            message: format!("Incus bootstrap failed with status {status}"),
            sdk_kind: "incus_bootstrap_failed".into(),
        });
    }
    Ok(())
}

fn write_materialized_file(path: &Path, content: &str, mode: u32) -> Result<(), ToolError> {
    std::fs::write(path, content).map_err(|e| ToolError::Sdk {
        message: format!("failed to write {}: {e}", path.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
            ToolError::Sdk {
                message: format!("failed to chmod {}: {e}", path.display()),
                sdk_kind: "incus_bootstrap_materialize_failed".into(),
            }
        })?;
    }
    let _ = mode;
    Ok(())
}

fn push_option(args: &mut Vec<OsString>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(OsString::from(flag));
        args.push(OsString::from(value));
    }
}

fn push_path_option(args: &mut Vec<OsString>, flag: &str, value: &Path) {
    args.push(OsString::from(flag));
    args.push(value.as_os_str().to_os_string());
}

fn push_flag(args: &mut Vec<OsString>, flag: &str, enabled: bool) {
    if enabled {
        args.push(OsString::from(flag));
    }
}

fn backup_config_from_env() -> Option<PathBuf> {
    std::env::var_os("LABBY_INCUS_BACKUP_CONFIG")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn absolutize_user_path(path: &Path) -> Result<PathBuf, ToolError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| ToolError::Sdk {
        message: format!("failed to resolve current directory: {e}"),
        sdk_kind: "incus_bootstrap_path_resolve_failed".into(),
    })?;
    Ok(cwd.join(path))
}

fn validate_backup_key(key: &str) -> Result<(), ToolError> {
    if SUPPORTED_BACKUP_KEYS.contains(&key) {
        return Ok(());
    }
    Err(ToolError::Sdk {
        message: format!("unsupported Incus backup config key: {key}"),
        sdk_kind: "incus_backup_config_unsupported_key".into(),
    })
}

fn scalar_to_string(value: Value) -> Result<String, ToolError> {
    match value {
        Value::String(value) => Ok(value),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null | Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => {
            Err(ToolError::Sdk {
                message: "Incus backup config values must be scalar strings, booleans, or numbers"
                    .into(),
                sdk_kind: "incus_backup_config_non_scalar".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn parses_supported_snapshot_keys() {
        let entries = parse_backup_config_str(
            r#"
config:
  snapshots.schedule: "@daily"
  snapshots.expiry: "14d"
  snapshots.pattern: "labby-{{ creation_date|date:'2006-01-02_15-04-05' }}"
  snapshots.schedule.stopped: false
"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 4);
        assert!(
            entries.iter().any(|entry| {
                entry.key == "snapshots.schedule.stopped" && entry.value == "false"
            })
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse_backup_config_str(
            r#"
config:
  security.privileged: true
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_unsupported_key");
    }

    #[test]
    fn rejects_non_scalar_values() {
        let err = parse_backup_config_str(
            r#"
config:
  snapshots.schedule:
    nested: nope
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_non_scalar");
    }

    #[test]
    fn materializes_embedded_bootstrap_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();

        assert!(artifacts.bootstrap_script.exists());
        assert!(artifacts.install_script.exists());
        assert!(artifacts.profile_file.exists());
        assert!(artifacts.backup_config_file.exists());

        let bootstrap = std::fs::read_to_string(&artifacts.bootstrap_script).unwrap();
        assert!(bootstrap.contains("incus-bootstrap.sh"));
        assert!(bootstrap.contains("labby setup --provision --yes"));

        let profile = std::fs::read_to_string(&artifacts.profile_file).unwrap();
        assert!(profile.contains("security.privileged: \"false\""));
    }

    #[test]
    fn builds_bootstrap_command_from_embedded_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            version: Some("v1.2.3".to_string()),
            dry_run: true,
            storage_driver: Some("dir".to_string()),
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();
        let args = command.args;

        assert_eq!(command.program, OsStr::new("sh"));
        assert_eq!(args[0], artifacts.bootstrap_script.as_os_str());
        assert!(has_arg_pair(&args, "--version", OsStr::new("v1.2.3")));
        assert!(has_arg_pair(
            &args,
            "--profile-file",
            artifacts.profile_file.as_os_str()
        ));
        assert!(args.windows(2).any(|pair| pair
            == [
                OsStr::new("--backup-config"),
                artifacts.backup_config_file.as_os_str()
            ]));
        assert!(has_arg_pair(&args, "--storage-driver", OsStr::new("dir")));
        assert!(args.iter().any(|arg| arg == OsStr::new("--dry-run")));
    }

    #[test]
    fn resolves_user_paths_before_switching_to_temp_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            backup_config: Some(PathBuf::from("my-backup.yaml")),
            local_binary: Some(PathBuf::from("target/debug/labby")),
            dry_run: true,
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();
        let args = command.args;
        let cwd = std::env::current_dir().unwrap();

        assert!(has_arg_pair(
            &args,
            "--backup-config",
            cwd.join("my-backup.yaml").as_os_str()
        ));
        assert!(has_arg_pair(
            &args,
            "--local-binary",
            cwd.join("target/debug/labby").as_os_str()
        ));
    }

    #[test]
    fn rejects_conflicting_backup_config_options() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            backup_config: Some(PathBuf::from("my-backup.yaml")),
            no_backup_config: true,
            ..IncusBootstrapOptions::default()
        };

        let err = bootstrap_command(&artifacts, &options).unwrap_err();
        assert_eq!(err.kind(), "incus_bootstrap_invalid_options");
    }

    fn has_arg_pair(args: &[OsString], flag: &str, value: &OsStr) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == OsStr::new(flag) && pair[1] == value)
    }
}
