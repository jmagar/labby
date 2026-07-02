//! Construction surface for [`GatewayManager`]: `new()`, the `with_*` builder
//! chain, the `from_config` factory, and small accessors.

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tokio::sync::{Mutex, RwLock};

use labby_auth::upstream::cache::OauthClientCache;
use labby_auth::upstream::encryption::EncryptionKey;
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::GatewayConfig;

use crate::gateway::code_mode::{CodeModeHistory, CodeModeSourceStore};
use crate::gateway::config::{normalize_config, validate_config};
#[cfg(any(test, feature = "testkit"))]
use crate::gateway::config_store::FsGatewayConfigStore;
use crate::gateway::config_store::GatewayConfigStore;
use crate::gateway::protected_routes::ProtectedRouteIndex;
use crate::gateway::service_registry::{EmptyServiceRegistry, GatewayServiceRegistry};
use crate::gateway::types::CatalogChangeNotifier;
use crate::upstream::pool::{InProcessConnector, UpstreamPool};

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
    /// Host-owned persistence + environment seam.
    pub store: Arc<dyn GatewayConfigStore>,
    /// Pre-built builtin-service registry (host-injected).
    pub registry: Arc<dyn GatewayServiceRegistry>,
    /// Optional in-process MCP connector.  Required on the `serve` path;
    /// optional for one-shot CLI commands that don't need in-process peers.
    pub in_process_connector: Option<InProcessConnector>,
    /// Optional upstream OAuth runtime.  `None` when OAuth is not configured.
    pub oauth: Option<GatewayOauthConfig>,
}

/// OAuth components needed by the manager, bundled to avoid partial-move issues.
pub struct GatewayOauthConfig {
    pub managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    pub cache: OauthClientCache,
    pub sqlite: labby_auth::sqlite::SqliteStore,
    pub key: EncryptionKey,
    pub redirect_uri: String,
}

impl GatewayManager {
    /// Assemble a `GatewayManager` from a `GatewayManagerConfig` (A-H3).
    ///
    /// Collapses the duplicated builder chains in `cli/gateway.rs`,
    /// `cli/serve.rs`, and test harnesses into one call site.
    pub fn from_config(
        cfg: GatewayManagerConfig,
        runtime: GatewayRuntimeHandle,
    ) -> Result<Self, ToolError> {
        let mut manager = Self::try_with_store(cfg.config_path, runtime, cfg.store)?
            .with_builtin_service_registry(cfg.registry);

        if let Some(connector) = cfg.in_process_connector {
            manager = manager.with_in_process_connector(connector);
        }
        if let Some(oauth) = cfg.oauth {
            manager = manager
                .with_upstream_oauth_managers(oauth.managers)
                .with_oauth_client_cache(oauth.cache)
                .with_oauth_resources(oauth.sqlite, oauth.key, oauth.redirect_uri);
        }
        Ok(manager)
    }
}

impl GatewayManager {
    /// Construct a manager with the testkit filesystem-backed config store.
    ///
    /// Production callers use [`Self::from_config`] or [`Self::with_store`] so
    /// the host owns config rendering and credential persistence.
    #[cfg(any(test, feature = "testkit"))]
    pub fn new(path: PathBuf, runtime: GatewayRuntimeHandle) -> Self {
        let store = Arc::new(FsGatewayConfigStore::new(path.clone()));
        Self::with_store(path, runtime, store)
    }

    /// Construct a manager with an explicit host-owned config store.
    pub fn with_store(
        path: PathBuf,
        runtime: GatewayRuntimeHandle,
        store: Arc<dyn GatewayConfigStore>,
    ) -> Self {
        Self::try_with_store(path, runtime, store)
            .expect("current executable must resolve for Code Mode runner pool")
    }

    /// Construct a manager with an explicit host-owned config store, surfacing
    /// runner bootstrap failures for production constructors.
    pub fn try_with_store(
        path: PathBuf,
        runtime: GatewayRuntimeHandle,
        store: Arc<dyn GatewayConfigStore>,
    ) -> Result<Self, ToolError> {
        let registry: Arc<dyn GatewayServiceRegistry> = Arc::new(EmptyServiceRegistry);
        Ok(Self {
            path,
            store,
            runtime,
            config: Arc::new(RwLock::new(GatewayConfig::default())),
            config_mutation: Arc::new(Mutex::new(())),
            lazy_pool_init: Arc::new(Mutex::new(())),
            notifier: None,
            oauth_client_cache: None,
            upstream_oauth_managers: None,
            builtin_service_registry: Arc::new(ArcSwap::from_pointee(registry)),
            oauth_sqlite: None,
            oauth_key: None,
            oauth_redirect_uri: None,
            protected_route_index: Arc::new(RwLock::new(ProtectedRouteIndex::default())),
            code_mode_history: Arc::new(Mutex::new(CodeModeHistory::default())),
            code_mode_source_store: Arc::new(Mutex::new(CodeModeSourceStore::default())),
            in_process_connector: None,
            code_mode_refresh_deadline: Arc::new(Mutex::new(None)),
            code_mode_refresh_inflight: Arc::new(Mutex::new(())),
            code_mode_catalog_render_cache: Arc::new(Mutex::new(None)),
            code_mode_snippet_metadata_cache: Arc::new(Mutex::new(None)),
            code_mode_runner_pool: Arc::new(crate::gateway::code_mode::RunnerPool::from_env()?),
        })
    }

    /// Override the `.env` path used by config persistence helpers (test only).
    ///
    /// Rebuilds the default filesystem store so writes land beside the temp
    /// `config.toml` instead of `~/.lab/.env`.
    #[cfg(any(test, feature = "testkit"))]
    #[must_use]
    pub fn with_env_path(mut self, path: PathBuf) -> Self {
        self.store = Arc::new(FsGatewayConfigStore::new(self.path.clone()).with_env_path(path));
        self
    }

    /// Attach a connector for in-process (built-in) service peers.
    ///
    /// The connector is propagated to every `UpstreamPool` the manager creates
    /// so built-in lab services are accessible as in-process MCP peers.
    #[must_use]
    pub fn with_in_process_connector(mut self, connector: InProcessConnector) -> Self {
        self.in_process_connector = Some(connector);
        self
    }

    #[must_use]
    pub fn with_builtin_service_registry(
        mut self,
        registry: Arc<dyn GatewayServiceRegistry>,
    ) -> Self {
        self.builtin_service_registry = Arc::new(ArcSwap::from_pointee(registry));
        self
    }

    pub fn set_builtin_service_registry(&self, registry: Arc<dyn GatewayServiceRegistry>) {
        self.builtin_service_registry.store(Arc::new(registry));
    }

    pub(crate) fn builtin_service_registry(&self) -> Arc<dyn GatewayServiceRegistry> {
        Arc::clone(&*self.builtin_service_registry.load())
    }

    pub(super) fn registered_service_meta(
        &self,
        service: &str,
    ) -> Option<&'static labby_primitives::plugin::PluginMeta> {
        self.builtin_service_registry().service_meta(service)
    }

    #[must_use]
    pub fn with_oauth_resources(
        mut self,
        sqlite: labby_auth::sqlite::SqliteStore,
        key: EncryptionKey,
        redirect_uri: String,
    ) -> Self {
        self.oauth_sqlite = Some(sqlite);
        self.oauth_key = Some(key);
        self.oauth_redirect_uri = Some(Arc::new(redirect_uri));
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

    pub async fn try_seed_config(&self, mut config: GatewayConfig) -> Result<(), ToolError> {
        // config.rs normalizes legacy code_mode before calling seed_config;
        // do not re-normalize here with false — that would incorrectly promote
        // legacy upstream config when the root [code_mode] is explicitly disabled.
        normalize_config(&mut config)?;
        validate_config(&config)?;
        self.seed_config_unchecked(config).await;
        Ok(())
    }

    pub async fn seed_config(&self, mut config: GatewayConfig) {
        normalize_config(&mut config).expect("gateway seed config should normalize");
        validate_config(&config).expect("gateway seed config should validate");
        self.seed_config_unchecked(config).await;
    }

    #[doc(hidden)]
    pub async fn seed_config_unchecked_for_tests(&self, config: GatewayConfig) {
        self.seed_config_unchecked(config).await;
    }

    async fn seed_config_unchecked(&self, config: GatewayConfig) {
        self.store
            .set_process_code_mode_enabled(config.code_mode.enabled);
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&config.protected_mcp_routes);
        *self.config.write().await = config;
        // Cold-connect for the codemode surface is handled lazily by the
        // code_mode path (`ensure_search_runtime_ready`) on first call, so
        // seed_config does not eagerly connect upstreams here. This keeps startup
        // cheap and non-blocking.
    }

    pub async fn current_pool(&self) -> Option<Arc<UpstreamPool>> {
        self.runtime.current_pool().await
    }

    /// Build a base [`UpstreamPool`] wired with the manager's OAuth client
    /// cache (when present), the given upstream request timeout, and the
    /// (longer) relay timeout used by the elicitation-relay path.
    ///
    /// Collapses the pool-construction skeleton previously duplicated across
    /// `pool_lifecycle`, `views`, `code_mode_runtime`, and `oauth_lifecycle`.
    pub(crate) fn new_base_pool(
        &self,
        request_timeout: std::time::Duration,
        relay_timeout: std::time::Duration,
    ) -> UpstreamPool {
        match &self.oauth_client_cache {
            Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
            None => UpstreamPool::new(),
        }
        .with_request_timeout(request_timeout)
        .with_relay_timeout(relay_timeout)
    }

    #[doc(hidden)]
    pub async fn replace_config_for_tests(
        &self,
        upstream: Vec<labby_runtime::gateway_config::UpstreamConfig>,
    ) {
        self.seed_config_unchecked_for_tests(GatewayConfig {
            upstream,
            ..GatewayConfig::default()
        })
        .await;
    }
}
