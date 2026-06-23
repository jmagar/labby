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
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use lab_runtime::error::ToolError;
use lab_runtime::gateway_config::{GatewayConfig, ResolvedPublicUrls};

/// Boxed future returned by the store's async env-write methods.
///
/// Persisting `config.toml` is pure blocking IO (see [`GatewayConfigStore::persist`]),
/// but the env-write methods also refresh the host's cached service clients,
/// which is async. Returning a boxed future keeps those methods dyn-compatible
/// (so the manager can hold `Arc<dyn GatewayConfigStore>`) without `#[async_trait]`.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Host-owned persistence + environment seam for the gateway manager.
///
/// All methods are synchronous: persistence is blocking file IO (the underlying
/// `write_gateway_config` uses `std::fs` + `fd_lock`), so there is nothing to
/// `await`. Keeping the trait sync also makes it dyn-compatible, so the manager
/// can hold an injected `Arc<dyn GatewayConfigStore>` without `#[async_trait]`
/// or boxed futures. Implemented by `lab` and injected at construction.
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

    /// Read raw `KEY=value` pairs from the `.env` file (best-effort).
    fn read_env_values(&self, path: &std::path::Path) -> BTreeMap<String, String>;
}

/// Default filesystem-backed [`GatewayConfigStore`].
///
/// Persists a bare [`GatewayConfig`] to `config.toml` via the gateway crate's
/// own foreign-key-preserving render path (gateway sections only) and writes
/// credentials to a sibling `.env` file. This is the store used by tests and by
/// any standalone caller that does not need the host's full `LabConfig`-backed
/// preservation of non-gateway sections.
///
/// Production Labby injects its own store (which keeps `LabConfig` and the
/// verbatim host render path) through `GatewayManager::from_config`.
pub struct FsGatewayConfigStore {
    config_path: PathBuf,
    env_path: PathBuf,
}

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
        let mut existing: BTreeMap<String, String> = self.read_env_values(&self.env_path);
        for (k, v) in pairs {
            existing.insert(k.clone(), v.clone());
        }
        if let Some(parent) = self.env_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::internal_message(format!("failed to create env dir: {e}"))
            })?;
        }
        // Values may contain spaces (e.g. `Bearer <token>`); double-quote them so
        // `dotenvy` round-trips the value back on read. The host store (`lab`)
        // owns the richer backup-first writer; this default is for tests/standalone.
        let body: String = existing
            .iter()
            .map(|(k, v)| format!("{k}={}\n", quote_env_value(v)))
            .collect();
        std::fs::write(&self.env_path, body)
            .map_err(|e| ToolError::internal_message(format!("failed to write env file: {e}")))
    }
}

/// Double-quote an env value when it contains whitespace or shell metacharacters
/// so `dotenvy` can read it back. Mirrors `lab`'s host-side `quote_env_value`.
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

    fn read_env_values(&self, path: &std::path::Path) -> BTreeMap<String, String> {
        dotenvy::from_path_iter(path)
            .ok()
            .map(|iter| iter.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }
}
