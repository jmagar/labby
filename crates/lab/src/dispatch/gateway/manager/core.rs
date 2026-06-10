//! Construction surface for [`GatewayManager`]: `new()`, the `with_*` builder
//! chain, the `from_config` factory, and small accessors.

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, RwLock};

use crate::config::LabConfig;
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::gateway::code_mode::CodeModeHistory;
use crate::dispatch::gateway::protected_routes::ProtectedRouteIndex;
use crate::dispatch::gateway::service_catalog::service_meta;
use crate::dispatch::gateway::types::CatalogChangeNotifier;
use crate::dispatch::upstream::pool::{InProcessConnector, UpstreamPool};
use crate::oauth::upstream::cache::OauthClientCache;
use crate::oauth::upstream::encryption::EncryptionKey;
use crate::oauth::upstream::manager::UpstreamOauthManager;
use crate::registry::ToolRegistry;

use super::{GatewayManager, GatewayRuntimeHandle};

// ── Gateway manager factory (A-H3) ────────────────────────────────────────────

/// All inputs needed to assemble a `GatewayManager` without repeating the
/// `new().with_*()...seed_config()` builder chain at every call site.
///
/// Used by `GatewayManager::from_config`.  Callers that need MCP peer
/// notifications should call `manager.set_notifier(...)` right after
/// `from_config`, before the first `seed_config` call.
pub struct GatewayManagerConfig {
    /// Path to the `config.toml` the manager owns.
    pub config_path: PathBuf,
    /// Pre-built builtin-service registry.
    pub registry: ToolRegistry,
    /// Shared service clients (feature-gated, env-loaded at startup).
    pub service_clients: SharedServiceClients,
    /// Optional in-process MCP connector.  Required on the `serve` path;
    /// optional for one-shot CLI commands that don't need in-process peers.
    ///
    /// `pub(crate)` because `InProcessConnector` is itself crate-private; the
    /// field cannot be more visible than its type.
    pub(crate) in_process_connector: Option<InProcessConnector>,
    /// Optional upstream OAuth runtime.  `None` when OAuth is not configured.
    pub oauth: Option<GatewayOauthConfig>,
}

/// OAuth components needed by the manager, bundled to avoid partial-move issues.
pub struct GatewayOauthConfig {
    pub managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    pub cache: OauthClientCache,
    pub sqlite: lab_auth::sqlite::SqliteStore,
    pub key: EncryptionKey,
    pub redirect_uri: String,
}

impl GatewayManager {
    /// Assemble a `GatewayManager` from a `GatewayManagerConfig` (A-H3).
    ///
    /// Collapses the duplicated builder chains in `cli/gateway.rs`,
    /// `cli/serve.rs`, and test harnesses into one call site.
    pub fn from_config(cfg: GatewayManagerConfig, runtime: GatewayRuntimeHandle) -> Self {
        let mut manager = Self::new(cfg.config_path, runtime)
            .with_builtin_service_registry(cfg.registry)
            .with_service_clients(cfg.service_clients);

        if let Some(connector) = cfg.in_process_connector {
            manager = manager.with_in_process_connector(connector);
        }
        if let Some(oauth) = cfg.oauth {
            manager = manager
                .with_upstream_oauth_managers(oauth.managers)
                .with_oauth_client_cache(oauth.cache)
                .with_oauth_resources(oauth.sqlite, oauth.key, oauth.redirect_uri);
        }
        manager
    }
}

impl GatewayManager {
    pub fn new(path: PathBuf, runtime: GatewayRuntimeHandle) -> Self {
        Self {
            path,
            env_path_override: None,
            runtime,
            config: Arc::new(RwLock::new(LabConfig::default())),
            config_mutation: Arc::new(Mutex::new(())),
            lazy_pool_init: Arc::new(Mutex::new(())),
            service_clients: None,
            notifier: None,
            oauth_client_cache: None,
            upstream_oauth_managers: None,
            builtin_service_registry: Arc::new(ArcSwap::from_pointee(
                crate::registry::build_default_registry(),
            )),
            oauth_sqlite: None,
            oauth_key: None,
            oauth_redirect_uri: None,
            protected_route_index: Arc::new(RwLock::new(ProtectedRouteIndex::default())),
            code_mode_history: Arc::new(Mutex::new(CodeModeHistory::default())),
            in_process_connector: None,
            code_mode_refresh_deadline: Arc::new(Mutex::new(None)),
            code_mode_refresh_inflight: Arc::new(Mutex::new(())),
            code_mode_catalog_render_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the `.env` path used by config persistence helpers (test only).
    ///
    /// In production the path is always derived from the home directory.  Tests
    /// call this to redirect writes beside the temp `config.toml` instead of
    /// touching the developer's `~/.lab/.env`.
    #[cfg(test)]
    #[must_use]
    pub fn with_env_path(mut self, path: PathBuf) -> Self {
        self.env_path_override = Some(path);
        self
    }

    /// Attach a connector for in-process (built-in) service peers.
    ///
    /// The connector is propagated to every `UpstreamPool` the manager creates
    /// so built-in lab services are accessible as in-process MCP peers.
    #[must_use]
    pub(crate) fn with_in_process_connector(mut self, connector: InProcessConnector) -> Self {
        self.in_process_connector = Some(connector);
        self
    }

    #[must_use]
    pub fn with_builtin_service_registry(mut self, registry: ToolRegistry) -> Self {
        self.builtin_service_registry = Arc::new(ArcSwap::from_pointee(registry));
        self
    }

    pub fn set_builtin_service_registry(&self, registry: ToolRegistry) {
        self.builtin_service_registry.store(Arc::new(registry));
    }

    pub(crate) fn builtin_service_registry(&self) -> Arc<ToolRegistry> {
        self.builtin_service_registry.load_full()
    }

    pub(super) fn registered_service_meta(
        &self,
        service: &str,
    ) -> Option<&'static lab_apis::core::PluginMeta> {
        self.builtin_service_registry()
            .service(service)
            .and_then(|entry| service_meta(entry.name))
    }

    #[must_use]
    pub fn with_oauth_resources(
        mut self,
        sqlite: lab_auth::sqlite::SqliteStore,
        key: EncryptionKey,
        redirect_uri: String,
    ) -> Self {
        self.oauth_sqlite = Some(sqlite);
        self.oauth_key = Some(key);
        self.oauth_redirect_uri = Some(Arc::new(redirect_uri));
        self
    }

    #[must_use]
    pub fn with_service_clients(mut self, service_clients: SharedServiceClients) -> Self {
        self.service_clients = Some(service_clients);
        self
    }

    #[must_use]
    pub fn with_oauth_client_cache(mut self, cache: OauthClientCache) -> Self {
        self.oauth_client_cache = Some(cache);
        self
    }

    #[must_use]
    pub fn with_upstream_oauth_managers(
        mut self,
        managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    ) -> Self {
        self.upstream_oauth_managers = Some(managers);
        self
    }

    /// Attach a catalog-change notifier (e.g. the MCP peer notifier).
    ///
    /// Must be called before any operations that trigger catalog changes
    /// (add, update, remove, reload) if the caller wants notifications.
    pub fn set_notifier(&mut self, notifier: CatalogChangeNotifier) {
        self.notifier = Some(notifier);
    }

    pub async fn seed_config(&self, config: LabConfig) {
        // config.rs normalizes legacy code_mode before calling seed_config;
        // do not re-normalize here with false — that would incorrectly promote
        // legacy upstream config when the root [code_mode] is explicitly disabled.

        crate::config::set_process_code_mode_enabled(config.code_mode.enabled);
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&config.protected_mcp_routes);
        *self.config.write().await = config;
        // Cold-connect for the search/execute surface is handled lazily by the
        // code_mode path (`ensure_search_runtime_ready`) on first call, so
        // seed_config does not eagerly connect upstreams here. This keeps startup
        // cheap and non-blocking.
    }

    pub async fn current_pool(&self) -> Option<Arc<UpstreamPool>> {
        self.runtime.current_pool().await
    }

    /// Build a base [`UpstreamPool`] wired with the manager's OAuth client
    /// cache (when present) and the given upstream request timeout.
    ///
    /// Collapses the pool-construction skeleton previously duplicated across
    /// `pool_lifecycle`, `views`, `code_mode_runtime`, and `oauth_lifecycle`.
    pub(crate) fn new_base_pool(&self, request_timeout: std::time::Duration) -> UpstreamPool {
        match &self.oauth_client_cache {
            Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
            None => UpstreamPool::new(),
        }
        .with_request_timeout(request_timeout)
    }

    #[cfg(test)]
    pub async fn replace_config_for_tests(&self, upstream: Vec<crate::config::UpstreamConfig>) {
        self.seed_config(LabConfig {
            upstream,
            ..LabConfig::default()
        })
        .await;
    }
}
