//! Configuration, workspace, and durable-store path resolution.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::LabConfig;

pub fn toml_candidates() -> Result<Vec<PathBuf>> {
    Ok(toml_candidates_for(lab_home_override()?, home_dir()))
}

fn toml_candidates_for(lab_home: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    if let Some(lab_home) = lab_home {
        return vec![lab_home.join("config.toml")];
    }
    home.map(|home| vec![home.join(".labby/config.toml")])
        .unwrap_or_default()
}

fn lab_home_override() -> Result<Option<PathBuf>> {
    std::env::var_os("LABBY_HOME")
        .filter(|root| !root.is_empty())
        .map(crate::installation::InstallationPaths::from_root)
        .transpose()
        .map(|paths| paths.map(|paths| paths.root().to_path_buf()))
        .map_err(Into::into)
}

fn lab_home_dir() -> Result<PathBuf> {
    Ok(crate::installation::InstallationPaths::resolve()?
        .root()
        .to_path_buf())
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[must_use]
pub fn workspace_root_for_home(config: &LabConfig, home: &Path) -> PathBuf {
    config
        .workspace
        .root
        .as_deref()
        .map(|root| expand_home_path(root, home))
        .unwrap_or_else(|| home.join(".labby/workspace"))
}

pub fn workspace_root_path(config: &LabConfig) -> Result<PathBuf> {
    if let Some(root) = config.workspace.root.as_deref() {
        let home = home_dir().ok_or_else(|| anyhow::anyhow!("HOME env var not set"))?;
        return Ok(expand_home_path(root, &home));
    }
    Ok(lab_home_dir()?.join("workspace"))
}

pub(crate) fn file_stash_root_path(config: &LabConfig) -> Result<PathBuf> {
    if let Some(root) = config.file_stash.root.as_deref() {
        let home = home_dir().ok_or_else(|| anyhow::anyhow!("HOME env var not set"))?;
        return Ok(expand_home_path(root, &home));
    }
    Ok(lab_home_dir()?.join("file-stash"))
}

fn expand_home_path(path: &Path, home: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path.to_path_buf()
}

pub fn dotenv_path() -> Result<PathBuf> {
    Ok(lab_home_dir()?.join(".env"))
}

pub(super) fn dotenv_candidates() -> Result<Vec<PathBuf>> {
    Ok(vec![
        crate::installation::InstallationPaths::resolve()?.dotenv(),
    ])
}

#[cfg(test)]
fn dotenv_candidates_for(
    lab_home: Option<PathBuf>,
    home: Option<PathBuf>,
    _cwd: PathBuf,
) -> Vec<PathBuf> {
    if let Some(lab_home) = lab_home {
        return vec![lab_home.join(".env")];
    }
    home.map(|home| vec![home.join(".labby/.env")])
        .unwrap_or_default()
}

pub fn config_toml_path() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = super::TEST_CONFIG_TOML_PATH
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("test config path lock")
        .clone()
    {
        return Ok(path);
    }
    let candidates = toml_candidates()?;
    Ok(candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or(lab_home_dir()?.join("config.toml")))
}

fn labby_db(name: &str) -> Result<PathBuf> {
    Ok(lab_home_dir()?.join(name))
}

#[cfg(test)]
fn access_db_path_for(lab_home: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf> {
    access_db_path_from_roots(lab_home, home)
}

#[allow(dead_code)]
pub(crate) fn access_db_path() -> Result<PathBuf> {
    Ok(crate::installation::InstallationPaths::resolve()?.access_db())
}

#[cfg(test)]
fn access_db_path_from_roots(lab_home: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf> {
    let state_root = lab_home
        .or_else(|| home.map(|home| home.join(".labby")))
        .ok_or_else(|| anyhow::anyhow!("neither LABBY_HOME nor HOME is set"))?;
    if !state_root.is_absolute() {
        anyhow::bail!(
            "Labby state root must be absolute: {}",
            state_root.display()
        );
    }
    Ok(state_root.join("access.db"))
}

pub fn usage_db_path() -> Result<PathBuf> {
    labby_db("usage.db")
}
pub fn codemode_journal_db_path() -> Result<PathBuf> {
    labby_db("codemode_journal.db")
}
pub fn codemode_journal_enabled() -> bool {
    std::env::var("LABBY_CODE_MODE_JOURNAL_DISABLED")
        .ok()
        .as_deref()
        != Some("1")
}
pub fn usage_telemetry_enabled() -> bool {
    resolve_usage_telemetry_enabled(
        std::env::var("LABBY_GATEWAY_USAGE_DISABLED")
            .ok()
            .as_deref(),
    )
}
pub(super) fn resolve_usage_telemetry_enabled(raw: Option<&str>) -> bool {
    raw != Some("1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_labby_home_replaces_user_config_candidates() {
        assert_eq!(
            toml_candidates_for(Some("/srv/labby".into()), Some("/home/operator".into())),
            vec![PathBuf::from("/srv/labby/config.toml")]
        );
    }

    #[test]
    fn default_candidates_use_the_canonical_installation_root_only() {
        assert_eq!(
            toml_candidates_for(None, Some("/home/operator".into())),
            vec![PathBuf::from("/home/operator/.labby/config.toml")]
        );
    }

    #[test]
    fn explicit_labby_home_excludes_current_directory_dotenv() {
        assert_eq!(
            dotenv_candidates_for(
                Some("/srv/labby".into()),
                Some("/home/operator".into()),
                "/worktree".into(),
            ),
            vec![PathBuf::from("/srv/labby/.env")]
        );
        assert_eq!(
            dotenv_candidates_for(None, Some("/home/operator".into()), "/worktree".into()),
            vec![PathBuf::from("/home/operator/.labby/.env")]
        );
    }

    #[test]
    fn access_database_uses_explicit_labby_home_and_canonical_filename() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            access_db_path_for(Some(root.join("labby")), Some(root.join("operator"))).unwrap(),
            root.join("labby/access.db")
        );
    }

    #[test]
    fn access_database_uses_the_default_labby_state_root() {
        let dir = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            access_db_path_for(None, Some(home.clone())).unwrap(),
            home.join(".labby/access.db")
        );
    }

    #[test]
    fn access_database_rejects_missing_or_relative_state_roots() {
        assert!(access_db_path_for(None, None).is_err());
        assert!(
            access_db_path_for(Some("relative/labby".into()), Some("/home/operator".into()))
                .is_err()
        );
        assert!(access_db_path_for(None, Some("relative/home".into())).is_err());
    }
}
