//! Host-owned persistence and environment seam for [`GatewayManager`].
//!
//! `lab-gateway` owns the gateway's in-memory [`GatewayConfig`] and all runtime
//! behavior, but it must NOT own the host's full `LabConfig`, the `config.toml`
//! render path (with its foreign-key preservation invariant), or the `.env`
//! credential file helpers — those are shared with non-gateway Labby code and
//! stay in the `lab` binary.
//!
//! The manager reaches those host concerns exclusively through this trait. The
//! host (`lab`) implements it over its live `Arc<RwLock<LabConfig>>` + the
//! existing `write_gateway_config`/`render_gateway_config` toml_edit logic,
//! reused verbatim. The manager mutates its in-memory `GatewayConfig` and then
//! calls [`GatewayConfigStore::persist`] to write it back through the host.
//!
//! **Consistency invariant.** The gateway-owned config sections (`upstream`,
//! `virtual_servers`, `code_mode`, …) are only ever mutated through
//! `GatewayManager`, which always persists through this store. The host's
//! `LabConfig` and the manager's `GatewayConfig` therefore stay in sync for
//! those sections; non-gateway sections and foreign top-level keys are never
//! touched by the manager.

use std::collections::BTreeMap;
#[cfg(any(test, feature = "testkit"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "testkit"))]
use std::fs;
use std::future::Future;
#[cfg(any(test, feature = "testkit"))]
use std::io::Write as _;
#[cfg(any(test, feature = "testkit"))]
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
#[cfg(any(test, feature = "testkit"))]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(any(test, feature = "testkit"))]
use std::time::{SystemTime, UNIX_EPOCH};

use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{GatewayConfig, ResolvedPublicUrls};
#[cfg(any(test, feature = "testkit"))]
use tempfile::NamedTempFile;

#[cfg(any(test, feature = "testkit"))]
const ENV_BACKUP_RETAIN: usize = 10;
#[cfg(any(test, feature = "testkit"))]
static ENV_BACKUP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Boxed future returned by the store's async env-write methods.
///
/// Persisting `config.toml` is pure blocking IO (see [`GatewayConfigStore::persist`]),
/// but the env-write methods also refresh the host's cached service clients,
/// which is async. Returning a boxed future keeps those methods dyn-compatible
/// (so the manager can hold `Arc<dyn GatewayConfigStore>`) without `#[async_trait]`.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Host-owned persistence + environment seam for the gateway manager.
///
/// Config persistence is synchronous blocking file IO (the underlying
/// `write_gateway_config` uses `std::fs` + `fd_lock`), while credential writes
/// return boxed futures because host implementations may refresh async service
/// clients after touching `.env`. This keeps the manager's injected
/// `Arc<dyn GatewayConfigStore>` dyn-compatible without `#[async_trait]`.
/// Implemented by `lab` and injected at construction.
pub trait GatewayConfigStore: Send + Sync {
    /// Resolve the canonical public URL pair (env over config over legacy
    /// `[auth].public_url`). Host-owned because it reads `LabConfig` sections
    /// (`auth`, `public_urls`) the gateway does not model.
    fn public_urls(&self) -> ResolvedPublicUrls;

    /// Apply a side effect when the process-wide Code Mode flag changes. The
    /// host owns the global atomic shared with non-gateway code.
    fn set_process_code_mode_enabled(&self, enabled: bool);

    /// The canonical `.env` path used for credential persistence. `None` means
    /// "use the host default" (`~/.lab/.env`); tests inject an override.
    fn env_path(&self) -> PathBuf;

    /// Persist the gateway-owned config sections back to `config.toml`.
    ///
    /// The host writes `cfg` into its live `LabConfig`, renders via the existing
    /// foreign-key-preserving toml_edit path, and atomically replaces the file.
    fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError>;

    /// Idempotently write the gateway HTTP bearer token to the `.env` file
    /// (backup-first) and refresh any cached service clients. `token_value` is
    /// the already-normalized header value the store writes verbatim.
    fn persist_gateway_bearer_token<'a>(
        &'a self,
        env_name: &'a str,
        token_value: &'a str,
    ) -> StoreFuture<'a, Result<(), ToolError>>;

    /// Idempotently write a registered service's credential env vars and refresh
    /// cached service clients. `values` maps env field name → value.
    fn persist_service_env<'a>(
        &'a self,
        service: &'a str,
        values: &'a BTreeMap<String, String>,
    ) -> StoreFuture<'a, Result<(), ToolError>>;
}

/// Testkit filesystem-backed [`GatewayConfigStore`].
///
/// Persists a bare [`GatewayConfig`] to `config.toml` via the gateway crate's
/// own foreign-key-preserving render path (gateway sections only) and writes
/// credentials to a sibling `.env` file. This is the store used by tests and
/// testkit callers that do not need the host's full `LabConfig`-backed
/// preservation of non-gateway sections.
///
/// Production hosts inject their own store (which keeps `LabConfig` and the
/// verbatim host render path) through `GatewayManager::from_config` or
/// `GatewayManager::with_store`.
#[cfg(any(test, feature = "testkit"))]
pub struct FsGatewayConfigStore {
    config_path: PathBuf,
    env_path: PathBuf,
}

#[cfg(any(test, feature = "testkit"))]
impl FsGatewayConfigStore {
    /// Build a store for `config_path`, deriving the `.env` path as a sibling
    /// `.env` file (or `~/.lab/.env` when `config_path` has no parent).
    #[must_use]
    pub fn new(config_path: PathBuf) -> Self {
        let env_path = config_path
            .parent()
            .map(|p| p.join(".env"))
            .unwrap_or_else(|| PathBuf::from(".env"));
        Self {
            config_path,
            env_path,
        }
    }

    /// Override the `.env` path (used by tests writing beside a temp config).
    #[must_use]
    pub fn with_env_path(mut self, env_path: PathBuf) -> Self {
        self.env_path = env_path;
        self
    }

    fn write_env_pairs(&self, pairs: &[(String, String)]) -> Result<(), ToolError> {
        merge_env_pairs(&self.env_path, pairs)
    }
}

#[cfg(any(test, feature = "testkit"))]
fn merge_env_pairs(path: &Path, pairs: &[(String, String)]) -> Result<(), ToolError> {
    if pairs.is_empty() {
        return Ok(());
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|e| ToolError::internal_message(format!("failed to create env dir: {e}")))?;
    reject_env_symlink(path)?;

    let existing_raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(ToolError::internal_message(format!(
                "failed to read env file {}: {err}",
                path.display()
            )));
        }
    };

    let existing_lines = existing_raw.lines().collect::<Vec<_>>();
    let existing_values = parse_env_values(&existing_lines);
    let requested = dedupe_pairs(pairs);
    let mut overrides = HashMap::new();
    let mut new_keys = Vec::new();

    for (key, value) in &requested {
        match existing_values.get(key) {
            Some(existing) if existing == value => {}
            Some(_) => {
                overrides.insert(key.clone(), value.clone());
            }
            None => {
                new_keys.push((key.clone(), value.clone()));
            }
        }
    }

    if overrides.is_empty() && new_keys.is_empty() {
        return Ok(());
    }

    if path.exists() {
        create_env_backup(path)?;
    }

    let mut out_lines = Vec::with_capacity(existing_lines.len() + new_keys.len() + 1);
    for line in &existing_lines {
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((key, _)) = trimmed.split_once('=')
        {
            let key = key.trim();
            if let Some(value) = overrides.get(key) {
                out_lines.push(format!("{key}={}", quote_env_value(value)));
                continue;
            }
        }
        out_lines.push((*line).to_string());
    }

    if !new_keys.is_empty() {
        if !out_lines.last().is_none_or(|line| line.trim().is_empty()) {
            out_lines.push(String::new());
        }
        for (key, value) in new_keys {
            out_lines.push(format!("{key}={}", quote_env_value(&value)));
        }
    }

    write_env_lines_atomically(path, parent, &out_lines)
}

#[cfg(any(test, feature = "testkit"))]
fn dedupe_pairs(pairs: &[(String, String)]) -> Vec<(String, String)> {
    let mut deduped: Vec<(String, String)> = Vec::new();
    for (key, value) in pairs {
        if let Some((_, existing_value)) = deduped.iter_mut().find(|(existing, _)| existing == key)
        {
            *existing_value = value.clone();
        } else {
            deduped.push((key.clone(), value.clone()));
        }
    }
    deduped
}

#[cfg(any(test, feature = "testkit"))]
fn parse_env_values(lines: &[&str]) -> HashMap<String, String> {
    lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            trimmed
                .split_once('=')
                .map(|(key, value)| (key.trim().to_string(), unquote_env_value(value.trim())))
        })
        .collect()
}

#[cfg(any(test, feature = "testkit"))]
fn write_env_lines_atomically(
    path: &Path,
    parent: &Path,
    lines: &[String],
) -> Result<(), ToolError> {
    let mut tmp = NamedTempFile::new_in(parent).map_err(|e| {
        ToolError::internal_message(format!(
            "failed to create temp env file in {}: {e}",
            parent.display()
        ))
    })?;
    for line in lines {
        writeln!(tmp, "{line}")
            .map_err(|e| ToolError::internal_message(format!("failed to write env file: {e}")))?;
    }
    tmp.as_file()
        .sync_all()
        .map_err(|e| ToolError::internal_message(format!("failed to sync env file: {e}")))?;
    tmp.persist(path).map_err(|e| {
        ToolError::internal_message(format!("failed to persist {}: {}", path.display(), e.error))
    })?;
    restrict_secret_file_permissions(path)
}

#[cfg(any(test, feature = "testkit"))]
fn create_env_backup(path: &Path) -> Result<PathBuf, ToolError> {
    reject_env_symlink(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(".env");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let counter = ENV_BACKUP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let backup = parent.join(format!("{file_name}.bak.{millis}.{counter}"));
    fs::copy(path, &backup).map_err(|e| {
        ToolError::internal_message(format!(
            "failed to back up {} to {}: {e}",
            path.display(),
            backup.display()
        ))
    })?;
    restrict_secret_file_permissions(&backup)?;
    prune_env_backups(parent, file_name)?;
    Ok(backup)
}

#[cfg(any(test, feature = "testkit"))]
fn reject_env_symlink(path: &Path) -> Result<(), ToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!(
                "refusing to write env file through symlink: {}",
                path.display()
            ),
        }),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(ToolError::internal_message(format!(
            "failed to inspect env file {}: {err}",
            path.display()
        ))),
    }
}

#[cfg(any(test, feature = "testkit"))]
fn prune_env_backups(parent: &Path, file_name: &str) -> Result<(), ToolError> {
    let prefix = format!("{file_name}.bak.");
    let mut backups = Vec::new();
    let entries = fs::read_dir(parent)
        .map_err(|e| ToolError::internal_message(format!("failed to read env backup dir: {e}")))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| ToolError::internal_message(format!("failed to read env backup: {e}")))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            backups.push(entry.path());
        }
    }
    backups.sort();
    let remove_count = backups.len().saturating_sub(ENV_BACKUP_RETAIN);
    for backup in backups.into_iter().take(remove_count) {
        fs::remove_file(&backup).map_err(|e| {
            ToolError::internal_message(format!(
                "failed to prune env backup {}: {e}",
                backup.display()
            ))
        })?;
    }
    Ok(())
}

/// Double-quote an env value when it contains whitespace or shell metacharacters
/// so `dotenvy` can read it back.
#[cfg(any(test, feature = "testkit"))]
fn quote_env_value(v: &str) -> String {
    let needs_quotes = v
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | '#' | '$' | '\\' | '"' | '\'' | '`'));
    if needs_quotes {
        let escaped = v.replace('\\', r"\\").replace('"', r#"\""#);
        format!("\"{escaped}\"")
    } else {
        v.to_owned()
    }
}

#[cfg(any(test, feature = "testkit"))]
fn unquote_env_value(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .map_or_else(
            || value.to_string(),
            |inner| inner.replace(r#"\""#, "\"").replace(r"\\", r"\"),
        )
}

#[cfg(any(test, feature = "testkit"))]
fn restrict_secret_file_permissions(path: &Path) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| {
            ToolError::internal_message(format!("failed to chmod {}: {e}", path.display()))
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(any(test, feature = "testkit"))]
impl GatewayConfigStore for FsGatewayConfigStore {
    fn public_urls(&self) -> ResolvedPublicUrls {
        ResolvedPublicUrls::default()
    }

    fn set_process_code_mode_enabled(&self, _enabled: bool) {}

    fn env_path(&self) -> PathBuf {
        self.env_path.clone()
    }

    fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError> {
        super::config::write_gateway_config(&self.config_path, cfg)
    }

    fn persist_gateway_bearer_token<'a>(
        &'a self,
        env_name: &'a str,
        token_value: &'a str,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        // The manager normalizes the header before calling; write it verbatim.
        Box::pin(
            async move { self.write_env_pairs(&[(env_name.to_string(), token_value.to_string())]) },
        )
    }

    fn persist_service_env<'a>(
        &'a self,
        _service: &'a str,
        values: &'a BTreeMap<String, String>,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let pairs: Vec<(String, String)> =
                values.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            self.write_env_pairs(&pairs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup_count(dir: &Path) -> usize {
        fs::read_dir(dir)
            .expect("read temp dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".env.bak."))
            .count()
    }

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn fs_store_env_merge_preserves_comments_and_creates_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        fs::write(&path, "# operator note\nOTHER=keep\nTOKEN=old\n").expect("write env");

        merge_env_pairs(
            &path,
            &[
                ("TOKEN".to_string(), "new value".to_string()),
                ("ADDED".to_string(), "abc123".to_string()),
            ],
        )
        .expect("merge env");

        let rendered = fs::read_to_string(&path).expect("read env");
        assert!(rendered.contains("# operator note"));
        assert!(rendered.contains("OTHER=keep"));
        assert!(rendered.contains("TOKEN=\"new value\""));
        assert!(rendered.contains("ADDED=abc123"));
        assert_eq!(backup_count(dir.path()), 1);
    }

    #[test]
    fn fs_store_env_merge_idempotent_write_skips_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        fs::write(&path, "TOKEN=\"new value\"\n").expect("write env");

        merge_env_pairs(&path, &[("TOKEN".to_string(), "new value".to_string())])
            .expect("merge env");

        assert_eq!(backup_count(dir.path()), 0);
        assert_eq!(
            fs::read_to_string(&path).expect("read env"),
            "TOKEN=\"new value\"\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fs_store_env_merge_restricts_env_and_backup_permissions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        fs::write(&path, "TOKEN=old\n").expect("write env");

        merge_env_pairs(&path, &[("TOKEN".to_string(), "new".to_string())]).expect("merge env");

        assert_eq!(file_mode(&path), 0o600);
        let backup = fs::read_dir(dir.path())
            .expect("read temp dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".env.bak."))
            })
            .expect("backup exists");
        assert_eq!(file_mode(&backup), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn fs_store_env_merge_refuses_symlink_target() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.env");
        let link = dir.path().join(".env");
        fs::write(&target, "TOKEN=old\n").expect("write target");
        unix_fs::symlink(&target, &link).expect("symlink env");

        let err = merge_env_pairs(&link, &[("TOKEN".to_string(), "new".to_string())])
            .expect_err("env symlink must be refused");

        assert_eq!(err.kind(), "invalid_param");
        assert_eq!(
            fs::read_to_string(&target).expect("read target"),
            "TOKEN=old\n"
        );
    }
}
