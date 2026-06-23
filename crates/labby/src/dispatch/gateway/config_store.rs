//! Host-owned [`GatewayConfigStore`] implementation for `lab`.
//!
//! `lab-gateway` owns the in-memory [`GatewayConfig`] and all runtime behavior,
//! but it must not own the host's full [`LabConfig`], the `config.toml` render
//! path (with its foreign-key-preservation invariant), or the `.env` credential
//! helpers — those are shared with non-gateway Labby code and stay here.
//!
//! [`LabConfigStore`] is injected into [`GatewayManager`] at construction. It
//! holds the live `Arc<RwLock<LabConfig>>`, writes the gateway-owned sections
//! back into it on `persist`, and renders the full `LabConfig` through the
//! verbatim `toml_edit` merge path (`write_gateway_config`) that preserves
//! foreign top-level keys byte-for-byte. Env writes go through the host's real
//! backup-first / atomic `write_env` helpers and refresh any cached service
//! clients.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use labby_gateway::gateway::config_store::{GatewayConfigStore, StoreFuture};
use labby_runtime::gateway_config::{GatewayConfig, ResolvedPublicUrls};

use crate::config::{EnvCredential, LabConfig, backup_env, env_is_up_to_date, home_dir, write_env};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::error::ToolError;

// `load_gateway_config` is consumed by the gateway API integration tests;
// `write_gateway_config` is used by `LabConfigStore::persist`. Allow the
// bin-target unused-import lint for the test-only re-export.
#[allow(unused_imports)]
pub use host_config::{load_gateway_config, write_gateway_config};

/// Host-owned [`GatewayConfigStore`] backed by the live [`LabConfig`].
pub struct LabConfigStore {
    /// Live config the manager's gateway sections are persisted back into.
    config: Arc<RwLock<LabConfig>>,
    /// Path to the owned `config.toml`.
    config_path: PathBuf,
    /// Cached service clients to refresh after a credential write.
    service_clients: Option<SharedServiceClients>,
}

impl LabConfigStore {
    /// Build a store over the live `config` and the owned `config_path`.
    #[must_use]
    pub fn new(config: Arc<RwLock<LabConfig>>, config_path: PathBuf) -> Self {
        Self {
            config,
            config_path,
            service_clients: None,
        }
    }

    /// Attach cached service clients to refresh after credential writes.
    #[must_use]
    pub fn with_service_clients(mut self, clients: SharedServiceClients) -> Self {
        self.service_clients = Some(clients);
        self
    }

    fn resolved_env_path(&self) -> PathBuf {
        home_dir()
            .map(|h| h.join(".lab").join(".env"))
            .unwrap_or_else(|| PathBuf::from(".env"))
    }

    /// Backup-first atomic write of `creds`, then refresh cached clients.
    async fn write_creds_and_refresh(&self, creds: Vec<EnvCredential>) -> Result<(), ToolError> {
        let env_path = self.resolved_env_path();
        if !creds.is_empty() && !env_is_up_to_date(&env_path, &creds) {
            let env_path_for_write = env_path.clone();
            tokio::task::spawn_blocking(move || -> Result<(), ToolError> {
                drop(backup_env(&env_path_for_write).map_err(|e| {
                    ToolError::internal_message(format!("failed to back up env file: {e}"))
                })?);
                write_env(&env_path_for_write, &creds, true).map_err(|e| {
                    ToolError::internal_message(format!("failed to write env file: {e}"))
                })?;
                Ok(())
            })
            .await
            .map_err(|e| ToolError::internal_message(format!("env write task failed: {e}")))??;

            if let Some(service_clients) = &self.service_clients {
                service_clients
                    .refresh_from_env_path(&env_path)
                    .await
                    .map_err(|e| {
                        ToolError::internal_message(format!(
                            "failed to refresh service clients from {}: {e}",
                            env_path.display()
                        ))
                    })?;
            }
        }
        Ok(())
    }
}

impl GatewayConfigStore for LabConfigStore {
    fn public_urls(&self) -> ResolvedPublicUrls {
        // Read the live LabConfig synchronously. `public_urls` reads `auth`,
        // `public_urls`, and env vars the gateway does not model.
        match self.config.read() {
            Ok(guard) => guard.public_urls(),
            Err(poisoned) => poisoned.into_inner().public_urls(),
        }
    }

    fn set_process_code_mode_enabled(&self, enabled: bool) {
        crate::config::set_process_code_mode_enabled(enabled);
    }

    fn env_path(&self) -> PathBuf {
        self.resolved_env_path()
    }

    fn persist(&self, cfg: &GatewayConfig) -> Result<(), ToolError> {
        // Apply the gateway-owned sections into the live LabConfig, then render
        // the FULL LabConfig through the foreign-key-preserving toml_edit path.
        let snapshot = {
            let mut guard = self
                .config
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.apply_gateway_config(cfg);
            guard.clone()
        };
        write_gateway_config(&self.config_path, &snapshot)
    }

    fn persist_gateway_bearer_token<'a>(
        &'a self,
        env_name: &'a str,
        token_value: &'a str,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        // The manager already validated the env name and normalized the header.
        Box::pin(async move {
            let creds = vec![EnvCredential {
                service: "gateway".to_string(),
                url: None,
                secret: Some(token_value.to_string()),
                env_field: env_name.to_string(),
            }];
            self.write_creds_and_refresh(creds).await
        })
    }

    fn persist_service_env<'a>(
        &'a self,
        service: &'a str,
        values: &'a BTreeMap<String, String>,
    ) -> StoreFuture<'a, Result<(), ToolError>> {
        Box::pin(async move {
            let creds = values_to_service_creds(service, values);
            self.write_creds_and_refresh(creds).await
        })
    }

    fn read_env_values(&self, path: &Path) -> BTreeMap<String, String> {
        dotenvy::from_path_iter(path)
            .ok()
            .map(|iter| iter.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }
}

/// Map a service's `{FIELD: value}` set to host [`EnvCredential`]s. A
/// `{SERVICE}_URL` field is treated as the service URL; everything else is a
/// secret credential.
fn values_to_service_creds(service: &str, values: &BTreeMap<String, String>) -> Vec<EnvCredential> {
    let url_field = format!("{}_URL", service.to_uppercase());
    values
        .iter()
        .map(|(field, value)| {
            let url = (field == &url_field).then(|| value.clone());
            let secret = if url.is_some() {
                None
            } else {
                Some(value.clone())
            };
            EnvCredential {
                service: service.to_string(),
                url,
                secret,
                env_field: field.clone(),
            }
        })
        .collect()
}

/// Host-owned `config.toml` render path: serialize the full [`LabConfig`] and
/// merge it into the existing document so foreign top-level keys (sections
/// `LabConfig` does not model) survive byte-for-byte.
mod host_config {
    use std::fs::OpenOptions;
    use std::io::Write as _;
    use std::path::Path;

    use anyhow::Context as _;
    use fd_lock::RwLock;
    use tempfile::NamedTempFile;

    use crate::config::LabConfig;
    use crate::dispatch::error::ToolError;

    /// Top-level keys `LabConfig` models. On render these are removed from the
    /// existing document and rewritten from the struct; every other (foreign)
    /// key is preserved. `[deploy]`/`[device]`/etc. are intentionally rewritten
    /// from the struct (their comments/formatting are not preserved by design).
    const KNOWN_LAB_CONFIG_KEYS: &[&str] = &[
        "mcp",
        "log",
        "local_logs",
        "api",
        "web",
        "workspace",
        "mcpregistry",
        "oauth",
        "device",
        "node",
        "admin",
        "services",
        "auth",
        "code_mode",
        "upstream_request_timeout_ms",
        "upstream_relay_timeout_ms",
        "upstream",
        "upstream_import_tombstones",
        "upstream_pending",
        "protected_mcp_routes",
        "virtual_servers",
        "quarantined_virtual_servers",
        "gateway",
        "deploy",
        "public_urls",
    ];

    fn lock_path(path: &Path) -> std::path::PathBuf {
        let mut p = path.to_path_buf();
        let name = p
            .file_name()
            .map(|n| format!("{}.lock", n.to_string_lossy()))
            .unwrap_or_else(|| "config.toml.lock".to_string());
        p.set_file_name(name);
        p
    }

    /// Load the gateway-relevant config from `path` as a full [`LabConfig`].
    ///
    /// Consumed by the gateway API integration tests (in the lib test target);
    /// allow dead_code so the bin-target build, which does not compile those
    /// tests, stays lint-clean.
    #[allow(dead_code)]
    pub fn load_gateway_config(path: &Path) -> Result<LabConfig, ToolError> {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let mut cfg = toml::from_str::<LabConfig>(&raw).map_err(|e| ToolError::Sdk {
                    sdk_kind: "internal_error".to_string(),
                    message: format!("failed to parse {}: {e}", path.display()),
                })?;
                cfg.normalize_protected_mcp_routes()
                    .map_err(|e| ToolError::Sdk {
                        sdk_kind: "internal_error".to_string(),
                        message: format!("invalid config {}: {e}", path.display()),
                    })?;
                Ok(cfg)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LabConfig::default()),
            Err(e) => Err(ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to read {}: {e}", path.display()),
            }),
        }
    }

    /// Render `cfg` into the existing document (preserving foreign keys) and
    /// atomically replace the file at `path`.
    pub fn write_gateway_config(path: &Path, cfg: &LabConfig) -> Result<(), ToolError> {
        cfg.validate().map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("invalid config: {e}"),
        })?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to create {}: {e}", parent.display()),
            })?;
        }

        let lock_path = lock_path(path);
        let lock_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open {}", lock_path.display()))
            .map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: e.to_string(),
            })?;
        let mut lock = RwLock::new(lock_file);
        let _guard = lock.try_write().map_err(|_| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("gateway config is locked: {}", lock_path.display()),
        })?;

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let raw = render_gateway_config(path, cfg)?;

        let mut tmp = NamedTempFile::new_in(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create temp file in {}: {e}", parent.display()),
        })?;
        tmp.write_all(raw.as_bytes()).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to write temp gateway config: {e}"),
        })?;
        tmp.as_file().sync_all().map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to sync temp gateway config: {e}"),
        })?;
        tmp.persist(path).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to persist {}: {}", path.display(), e.error),
        })?;

        Ok(())
    }

    fn render_gateway_config(path: &Path, cfg: &LabConfig) -> Result<String, ToolError> {
        let serialized = toml::to_string(cfg).map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to serialize gateway config: {e}"),
        })?;
        let desired = serialized
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: format!("failed to parse serialized gateway config: {e}"),
            })?;

        let Ok(existing_raw) = std::fs::read_to_string(path) else {
            return Ok(serialized);
        };
        let Ok(mut existing) = existing_raw.parse::<toml_edit::DocumentMut>() else {
            return Ok(serialized);
        };

        // Remove the keys we model so they are rewritten from the struct, then
        // overlay the desired document. Foreign top-level keys are untouched.
        for key in KNOWN_LAB_CONFIG_KEYS {
            existing.as_table_mut().remove(key);
        }
        for (key, item) in desired.as_table() {
            existing[key] = item.clone();
        }

        Ok(existing.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trust invariant: persisting a gateway mutation through the host store must
    /// preserve a FOREIGN top-level section (one `LabConfig` does not model),
    /// including its operator comment and formatting, byte-for-byte.
    ///
    /// `[deploy]`/`[device]` are intentionally NOT covered here: they are in
    /// `KNOWN_LAB_CONFIG_KEYS` and are rewritten from the struct by design.
    #[test]
    fn persist_preserves_foreign_top_level_section_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");

        // A foreign section LabConfig does not model + a gateway section the
        // manager owns.
        let initial = "\
[experimental_external_tool]
# operator comment must survive
foo = 1

[gateway]

[[upstream]]
name = \"alpha\"
enabled = true
url = \"https://alpha.example.com/mcp\"
";
        std::fs::write(&path, initial).expect("write initial config");

        // Load the full LabConfig and seed the store with it.
        let loaded = load_gateway_config(&path).expect("load config");
        let store = LabConfigStore::new(Arc::new(RwLock::new(loaded.clone())), path.clone());

        // Mutate a gateway-owned upstream and persist through the host store.
        let mut gw = loaded.to_gateway_config();
        gw.upstream[0].enabled = false;
        store.persist(&gw).expect("persist gateway mutation");

        let rendered = std::fs::read_to_string(&path).expect("read persisted config");

        // The gateway mutation landed.
        let reloaded = load_gateway_config(&path).expect("reload config");
        assert!(
            !reloaded.upstream[0].enabled,
            "gateway upstream mutation must persist"
        );

        // The foreign section's comment + formatting survived byte-for-byte.
        assert!(
            rendered.contains("[experimental_external_tool]"),
            "foreign section header must survive, got:\n{rendered}"
        );
        assert!(
            rendered.contains("# operator comment must survive"),
            "foreign section operator comment must survive byte-for-byte, got:\n{rendered}"
        );
        assert!(
            rendered.contains("foo = 1"),
            "foreign section value must survive, got:\n{rendered}"
        );
    }
}
