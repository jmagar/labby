use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use arc_swap::{ArcSwap, ArcSwapOption};
use tokio::sync::{Mutex, RwLock};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::config::{
    LabConfig, ProtectedMcpRouteConfig, ToolSearchConfig, UpstreamConfig, backup_env,
    env_is_up_to_date, write_env,
};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::error::ToolError;
use crate::dispatch::upstream::pool::{
    UpstreamCachedSummary, UpstreamPool, in_process_upstream_name,
};
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::oauth::upstream::encryption::EncryptionKey;
use crate::oauth::upstream::manager::UpstreamOauthManager;
use crate::registry::ToolRegistry;
use lab_apis::extract::types::ServiceCreds;

use super::SHARED_GATEWAY_OAUTH_SUBJECT;
use super::config::{
    default_gateway_bearer_env_name, insert_protected_mcp_route, insert_upstream,
    load_gateway_config, remove_protected_mcp_route, remove_upstream, update_protected_mcp_route,
    update_upstream, validate_bearer_token_env_name, validate_tool_search, write_gateway_config,
};
use super::config_mutation::{read_env_values, values_to_service_creds};
use super::index::{SearchHit, ToolIndex};
use super::params::GatewayUpdatePatch;
use super::projection::*;
use super::protected_routes::ProtectedRouteIndex;
pub use super::runtime::GatewayRuntimeHandle;
use super::runtime::runtime_origin_tag;
use super::service_catalog::service_meta;
use super::types::{
    CatalogChangeNotifier, GatewayCatalogDiff, GatewayRuntimeView, GatewayToolExposureRowView,
    GatewayView, McpClientConfigView, McpClientTransportType, ServiceConfigView,
    VirtualServerMcpPolicyView,
};
use super::view_models::ServerView;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayCatalogSnapshot {
    pub tools: BTreeSet<String>,
    pub resources: BTreeSet<String>,
    pub prompts: BTreeSet<String>,
}

pub fn diff_catalogs(
    before: &GatewayCatalogSnapshot,
    after: &GatewayCatalogSnapshot,
) -> GatewayCatalogDiff {
    GatewayCatalogDiff {
        tools_changed: before.tools != after.tools,
        resources_changed: before.resources != after.resources,
        prompts_changed: before.prompts != after.prompts,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct VirtualServerMigration {
    quarantined: Vec<String>,
}

impl VirtualServerMigration {
    fn changed(&self) -> bool {
        !self.quarantined.is_empty()
    }
}

/// Minimum wall-clock age between consecutive live reprobes on the search
/// hot path. Per-upstream; fresher indexes skip reprobe entirely.
const TOOL_SEARCH_REPROBE_TTL: Duration = Duration::from_secs(30);
const WARNING_UNKNOWN_SERVICE: &str = "unknown_service";

#[derive(Clone)]
struct GatewayToolIndexState {
    index: Arc<ArcSwapOption<ToolIndex>>,
    warming: Arc<AtomicBool>,
    /// Monotonically increases on every spawned rebuild. Tasks that finish
    /// with a stale generation are dropped instead of publishing, preventing
    /// a last-writer-wins race where an earlier rebuild clobbers a later one.
    generation: Arc<AtomicU64>,
    /// Handle for the most recent spawned rebuild, aborted when a new
    /// rebuild is scheduled so rapid config churn doesn't leak tasks.
    in_flight: Arc<StdMutex<Option<AbortHandle>>>,
    /// Timestamp of the last scheduled live reprobe attempt. Search-path
    /// refresh short-circuits when younger than `TOOL_SEARCH_REPROBE_TTL`,
    /// even if the previous attempt failed before rebuilding the index.
    last_reprobe_attempt_at: Arc<StdMutex<Option<Instant>>>,
}

impl Default for GatewayToolIndexState {
    fn default() -> Self {
        Self {
            index: Arc::new(ArcSwapOption::from(None)),
            warming: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            in_flight: Arc::new(StdMutex::new(None)),
            last_reprobe_attempt_at: Arc::new(StdMutex::new(None)),
        }
    }
}

/// Outcome of a `batch_add` call.
///
/// `views` contains one [`GatewayView`] for each spec that was successfully
/// inserted. `errors` contains `(name, error)` pairs for every spec that
/// failed validation or insertion.
#[derive(Debug, Default)]
pub struct BatchAddOutcome {
    pub views: Vec<GatewayView>,
    pub errors: Vec<(String, ToolError)>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayToolSearchResult {
    pub name: String,
    pub description: String,
    pub upstream: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Clone)]
pub struct GatewayManager {
    pub(super) path: PathBuf,
    pub(super) runtime: GatewayRuntimeHandle,
    pub(super) config: Arc<RwLock<LabConfig>>,
    pub(super) config_mutation: Arc<Mutex<()>>,
    service_clients: Option<SharedServiceClients>,
    notifier: Option<CatalogChangeNotifier>,
    pub(super) oauth_client_cache: Option<OauthClientCache>,
    pub(super) upstream_oauth_managers: Option<Arc<dashmap::DashMap<String, UpstreamOauthManager>>>,
    builtin_service_registry: Arc<ArcSwap<ToolRegistry>>,
    /// Resources needed to build transient OAuth managers for probed upstreams.
    pub(super) oauth_sqlite: Option<lab_auth::sqlite::SqliteStore>,
    pub(super) oauth_key: Option<EncryptionKey>,
    pub(super) oauth_redirect_uri: Option<Arc<String>>,
    tool_indexes: Arc<dashmap::DashMap<String, GatewayToolIndexState>>,
    protected_route_index: Arc<RwLock<ProtectedRouteIndex>>,
}

impl GatewayManager {
    pub fn new(path: PathBuf, runtime: GatewayRuntimeHandle) -> Self {
        Self {
            path,
            runtime,
            config: Arc::new(RwLock::new(LabConfig::default())),
            config_mutation: Arc::new(Mutex::new(())),
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
            tool_indexes: Arc::new(dashmap::DashMap::new()),
            protected_route_index: Arc::new(RwLock::new(ProtectedRouteIndex::default())),
        }
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

    fn registered_service_meta(
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
        // config.rs normalizes legacy tool_search before calling seed_config;
        // do not re-normalize here with false — that would incorrectly promote
        // legacy upstream config when the root [tool_search] is explicitly disabled.
        crate::config::set_process_tool_search_enabled(config.tool_search.enabled);
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&config.protected_mcp_routes);
        *self.config.write().await = config;
    }

    pub async fn resolve_protected_route(
        &self,
        host: &str,
        path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        self.protected_route_index.read().await.resolve(host, path)
    }

    pub async fn resolve_protected_route_metadata(
        &self,
        host: &str,
        path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        self.protected_route_index
            .read()
            .await
            .resolve_exact_metadata_path(host, path)
    }

    pub async fn current_pool(&self) -> Option<Arc<UpstreamPool>> {
        self.runtime.current_pool().await
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn oauth_client_cache(&self) -> Option<OauthClientCache> {
        self.oauth_client_cache.clone()
    }

    /// Probe `url` for OAuth support via RFC 8414 AS metadata discovery.
    ///
    /// On success, registers a transient `UpstreamOauthManager` (Dynamic strategy)
    /// keyed by the URL hostname so subsequent `begin_upstream_authorization` calls
    /// work without requiring a static config entry.
    /// Returns the upstream OAuth SQLite store, if configured.
    /// Returns the upstream OAuth callback redirect URI, if configured.
    ///
    /// Used by the `/.well-known/oauth-client` endpoint to build the client
    /// metadata document served to CIMD-supporting authorization servers.
    pub fn upstream_oauth_manager(&self, upstream: &str) -> Option<UpstreamOauthManager> {
        self.upstream_oauth_managers
            .as_ref()
            .and_then(|managers| managers.get(upstream).map(|entry| entry.clone()))
    }

    #[allow(dead_code)]
    pub fn evict_upstream_clients(&self, upstream: &str) {
        if let Some(cache) = &self.oauth_client_cache {
            cache.evict_upstream(upstream);
        }
    }

    /// Return the resolved canonical public URL pair for the app and MCP gateway.
    ///
    /// Merges env vars over config file over legacy `[auth].public_url` field.
    pub async fn public_urls(&self) -> crate::config::ResolvedPublicUrls {
        self.config.read().await.public_urls()
    }

    pub async fn get_service_config(&self, service: &str) -> Result<ServiceConfigView, ToolError> {
        let meta =
            self.registered_service_meta(service)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("unknown service `{service}`"),
                    param: "service".to_string(),
                })?;
        let values = read_env_values(&self.env_path())?;
        Ok(service_config_view(meta, &values))
    }

    pub async fn set_service_config(
        &self,
        service: &str,
        values: &BTreeMap<String, String>,
    ) -> Result<ServiceConfigView, ToolError> {
        let meta =
            self.registered_service_meta(service)
                .ok_or_else(|| ToolError::InvalidParam {
                    message: format!("unknown service `{service}`"),
                    param: "service".to_string(),
                })?;

        for field in values.keys() {
            let valid = meta
                .required_env
                .iter()
                .chain(meta.optional_env.iter())
                .any(|env| env.name == field);
            if !valid {
                return Err(ToolError::InvalidParam {
                    message: format!("field `{field}` is not valid for service `{service}`"),
                    param: "values".to_string(),
                });
            }
        }

        let _mutation_guard = self.config_mutation.lock().await;
        let creds = values_to_service_creds(service, values);
        let env_path = self.env_path();
        if !creds.is_empty() && !env_is_up_to_date(&env_path, &creds) {
            drop(backup_env(&env_path).map_err(|e| {
                ToolError::internal_message(format!("failed to back up env file: {e}"))
            })?);
            write_env(&env_path, &creds, true).map_err(|e| {
                ToolError::internal_message(format!("failed to write env file: {e}"))
            })?;
            if let Some(service_clients) = &self.service_clients {
                service_clients
                    .refresh_from_env_path(&env_path)
                    .await
                    .map_err(|e| {
                        ToolError::internal_message(format!(
                            "failed to refresh service clients: {e}"
                        ))
                    })?;
            }
        }

        let values = read_env_values(&env_path)?;
        Ok(service_config_view(meta, &values))
    }

    /// Return a snapshot of the current gateway config (read-only).
    pub async fn current_config(&self) -> LabConfig {
        self.config.read().await.clone()
    }

    pub async fn list(&self) -> Result<Vec<ServerView>, ToolError> {
        let (cfg_guard, pool) = tokio::join!(self.config.read(), self.runtime.current_pool(),);
        let cfg = cfg_guard.clone();
        drop(cfg_guard);
        let mut views = Vec::with_capacity(cfg.upstream.len() + cfg.virtual_servers.len());
        for upstream in &cfg.upstream {
            views.push(server_view_from_upstream(pool.as_deref(), upstream).await);
        }
        for virtual_server in &cfg.virtual_servers {
            let peer_name = in_process_upstream_name(&virtual_server.service);
            let summary = upstream_summary(pool.as_deref(), &peer_name).await;
            let last_error = operator_visible_upstream_error(match pool.as_deref() {
                Some(pool) => pool.upstream_last_error(&peer_name).await,
                None => None,
            });
            views.push(server_view_from_virtual_server(
                virtual_server,
                summary,
                last_error,
                None,
            ));
        }
        let unknown_service_count = degraded_server_warning_count(&views, WARNING_UNKNOWN_SERVICE);
        if unknown_service_count > 0 {
            tracing::warn!(
                action = "gateway.list",
                unknown_service_count,
                "gateway list returned degraded rows with unknown services"
            );
        }
        Ok(views)
    }

    pub async fn get_server(&self, id: &str) -> Result<ServerView, ToolError> {
        let (cfg_guard, pool) = tokio::join!(self.config.read(), self.runtime.current_pool(),);
        let cfg = cfg_guard.clone();
        drop(cfg_guard);

        if let Some(upstream) = cfg.upstream.iter().find(|upstream| upstream.name == id) {
            return Ok(server_view_from_upstream(pool.as_deref(), upstream).await);
        }

        let virtual_server = find_virtual_server(&cfg, id)?;
        let peer_name = in_process_upstream_name(&virtual_server.service);
        let summary = upstream_summary(pool.as_deref(), &peer_name).await;
        let last_error = operator_visible_upstream_error(match pool.as_deref() {
            Some(pool) => pool.upstream_last_error(&peer_name).await,
            None => None,
        });
        Ok(server_view_from_virtual_server(
            virtual_server,
            summary,
            last_error,
            None,
        ))
    }

    pub async fn get(&self, name: &str) -> Result<GatewayView, ToolError> {
        let cfg = self.config.read().await;
        let tool_search = cfg.tool_search.clone();
        let upstream = cfg
            .upstream
            .iter()
            .find(|u| u.name == name)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?
            .clone();
        drop(cfg);

        Ok(GatewayView {
            config: config_view(&upstream, &tool_search),
            runtime: runtime_view(
                self.runtime.current_pool().await.as_deref(),
                &upstream.name,
                None,
            )
            .await,
        })
    }

    pub async fn surface_enabled_for_service(&self, service: &str, surface: &str) -> bool {
        if self.registered_service_meta(service).is_none() {
            return true;
        }

        let cfg = self.config.read().await;
        let Some(virtual_server) = find_virtual_server_for_service(&cfg, service) else {
            return surface != "mcp";
        };

        if !virtual_server.enabled {
            return false;
        }

        match surface {
            "cli" => virtual_server.surfaces.cli,
            "api" => virtual_server.surfaces.api,
            "mcp" => virtual_server.surfaces.mcp,
            "webui" => virtual_server.surfaces.webui,
            _ => false,
        }
    }

    pub async fn allowed_mcp_actions_for_service(&self, service: &str) -> Option<Vec<String>> {
        if self.registered_service_meta(service).is_none() {
            return None;
        }

        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server_for_service(&cfg, service)?;
        if !virtual_server.enabled || !virtual_server.surfaces.mcp {
            return Some(Vec::new());
        }

        if let Some(policy) = &virtual_server.mcp_policy
            && !policy.allowed_actions.is_empty()
        {
            let mut allowed = vec!["help".to_string(), "schema".to_string()];
            allowed.extend(policy.allowed_actions.clone());
            return Some(allowed);
        }

        None
    }

    pub async fn mcp_action_allowed_for_service(&self, service: &str, action: &str) -> bool {
        if self.registered_service_meta(service).is_none() {
            return true;
        }

        if !self.surface_enabled_for_service(service, "mcp").await {
            return false;
        }

        if matches!(action, "help" | "schema") {
            return true;
        }

        let cfg = self.config.read().await;
        let Some(virtual_server) = find_virtual_server_for_service(&cfg, service) else {
            return false;
        };

        match &virtual_server.mcp_policy {
            Some(policy) if !policy.allowed_actions.is_empty() => policy
                .allowed_actions
                .iter()
                .any(|allowed| allowed == action),
            _ => true,
        }
    }

    pub async fn status(&self, name: Option<&str>) -> Result<Vec<GatewayRuntimeView>, ToolError> {
        let upstreams: Vec<UpstreamConfig> = self
            .config
            .read()
            .await
            .upstream
            .iter()
            .filter(|u| name.is_none_or(|needle| needle == u.name))
            .cloned()
            .collect();
        let pool = self.runtime.current_pool().await;
        let prompt_owners = match pool.as_deref() {
            Some(p) => Some(p.prompt_ownership_map(&[]).await),
            None => None,
        };
        let mut items = Vec::new();
        for upstream in &upstreams {
            items.push(runtime_view(pool.as_deref(), &upstream.name, prompt_owners.as_ref()).await);
        }
        Ok(items)
    }

    pub async fn service_for_virtual_server_id(&self, id: &str) -> Result<String, ToolError> {
        let cfg = self.config.read().await;
        Ok(find_virtual_server(&cfg, id)?.service.clone())
    }

    pub async fn test(
        &self,
        spec_or_name: Result<&UpstreamConfig, &str>,
        _allow_stdio: bool,
    ) -> Result<GatewayRuntimeView, ToolError> {
        let upstream = match spec_or_name {
            Ok(spec) => spec.clone(),
            Err(name) => {
                let cfg = self.config.read().await;
                cfg.upstream
                    .iter()
                    .find(|u| u.name == name)
                    .cloned()
                    .ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: format!("gateway `{name}` not found"),
                    })?
            }
        };

        let pool = match &self.oauth_client_cache {
            Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
            None => UpstreamPool::new(),
        };
        let registry = self.builtin_service_registry();
        pool.discover_all_for_subject_with_in_process_peers(
            &[upstream.clone()],
            SHARED_GATEWAY_OAUTH_SUBJECT,
            &registry,
        )
        .await;

        Ok(runtime_view(Some(&pool), &upstream.name, None).await)
    }

    pub async fn enable_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> {
        self.set_virtual_server_enabled(id, true).await
    }

    pub async fn disable_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> {
        self.set_virtual_server_enabled(id, false).await
    }

    pub async fn remove_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let index = cfg
            .virtual_servers
            .iter()
            .position(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            })?;
        let removed = cfg.virtual_servers.remove(index);
        let removed_view =
            server_view_from_virtual_server(&removed, UpstreamCachedSummary::default(), None, None);

        self.persist_config(cfg).await?;
        Ok(removed_view)
    }

    pub async fn list_quarantined_virtual_servers(&self) -> Result<Vec<ServerView>, ToolError> {
        let cfg = self.config.read().await;
        Ok(cfg
            .quarantined_virtual_servers
            .iter()
            .map(|virtual_server| {
                server_view_from_virtual_server(
                    virtual_server,
                    UpstreamCachedSummary::default(),
                    None,
                    None,
                )
            })
            .collect())
    }

    pub async fn restore_quarantined_virtual_server(
        &self,
        id: &str,
    ) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let index = cfg
            .quarantined_virtual_servers
            .iter()
            .position(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("quarantined virtual server `{id}` not found"),
            })?;
        let restored = cfg.quarantined_virtual_servers.remove(index);
        if self.registered_service_meta(&restored.service).is_none() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_service".to_string(),
                message: format!(
                    "service `{}` is not registered in this lab binary",
                    restored.service
                ),
            });
        }
        if cfg
            .virtual_servers
            .iter()
            .any(|server| server.id == restored.id)
        {
            return Err(ToolError::InvalidParam {
                message: format!("virtual server `{id}` already exists"),
                param: "id".to_string(),
            });
        }

        let restored_view = server_view_from_virtual_server(
            &restored,
            UpstreamCachedSummary::default(),
            None,
            None,
        );
        cfg.virtual_servers.push(restored);
        self.persist_config(cfg).await?;
        Ok(restored_view)
    }

    pub async fn set_virtual_server_surface(
        &self,
        id: &str,
        surface: &str,
        enabled: bool,
    ) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let virtual_server = cfg
            .virtual_servers
            .iter_mut()
            .find(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            })?;

        match surface {
            "cli" => virtual_server.surfaces.cli = enabled,
            "api" => virtual_server.surfaces.api = enabled,
            "mcp" => virtual_server.surfaces.mcp = enabled,
            "webui" => virtual_server.surfaces.webui = enabled,
            _ => {
                return Err(ToolError::InvalidParam {
                    message: format!("unknown surface `{surface}`"),
                    param: "surface".to_string(),
                });
            }
        }

        self.persist_config(cfg).await?;
        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server(&cfg, id)?;
        Ok(server_view_from_virtual_server(
            virtual_server,
            UpstreamCachedSummary::default(),
            None,
            None,
        ))
    }

    pub async fn get_virtual_server_mcp_policy(
        &self,
        id: &str,
    ) -> Result<VirtualServerMcpPolicyView, ToolError> {
        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server(&cfg, id)?;
        Ok(VirtualServerMcpPolicyView {
            allowed_actions: virtual_server
                .mcp_policy
                .as_ref()
                .map(|policy| policy.allowed_actions.clone())
                .unwrap_or_default(),
        })
    }

    pub async fn set_virtual_server_mcp_policy(
        &self,
        id: &str,
        allowed_actions: &[String],
    ) -> Result<VirtualServerMcpPolicyView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let virtual_server = cfg
            .virtual_servers
            .iter_mut()
            .find(|server| server.id == id)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            })?;

        virtual_server.mcp_policy = if allowed_actions.is_empty() {
            None
        } else {
            Some(crate::config::VirtualServerMcpPolicyConfig {
                allowed_actions: allowed_actions.to_vec(),
            })
        };

        self.persist_config(cfg).await?;
        Ok(VirtualServerMcpPolicyView {
            allowed_actions: allowed_actions.to_vec(),
        })
    }

    pub async fn add(
        &self,
        mut spec: UpstreamConfig,
        bearer_token_value: Option<String>,
        _allow_stdio: bool,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let started = Instant::now();
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        // Trim and validate bearer_token_env unconditionally so whitespace typos
        // are caught before they silently fail env-var lookup later.
        if let Some(ref env_name) = spec.bearer_token_env {
            let trimmed = env_name.trim().to_string();
            validate_bearer_token_env_name(&trimmed)?;
            spec.bearer_token_env = Some(trimmed);
        }

        if let Some(token_value) = bearer_token_value.as_deref().map(str::trim)
            && !token_value.is_empty()
        {
            let env_name =
                resolve_gateway_bearer_env_name(&spec.name, spec.bearer_token_env.as_deref())?;
            spec.bearer_token_env = Some(env_name.clone());
            insert_upstream(&mut cfg, spec.clone())?;
            self.persist_gateway_bearer_token(&env_name, token_value)
                .await?;
        } else {
            insert_upstream(&mut cfg, spec.clone())?;
        }

        // Log only after validation (inside insert_upstream) has passed so
        // spec.name is confirmed well-formed before it enters any log sink.
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.add",
            event = "install.start",
            phase = "start",
            gateway = %spec.name,
            target = ?redacted_gateway_target(&spec),
            "gateway reconcile"
        );
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.add",
            event = "install.finish",
            phase = "finish",
            gateway = %spec.name,
            target = ?redacted_gateway_target(&spec),
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        self.get(&spec.name).await
    }

    /// Add multiple upstream servers in a single config-persist + reload cycle.
    ///
    /// Each spec is validated and inserted individually. Specs that fail validation
    /// are collected into `errors`; specs that succeed populate `views`. If every
    /// spec fails the first error is returned as `Err`. Otherwise, a single
    /// `persist_config` + `reload_with_origin_unlocked` is issued for all successes.
    pub async fn batch_add(
        &self,
        specs: Vec<UpstreamConfig>,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<BatchAddOutcome, ToolError> {
        if specs.is_empty() {
            return Ok(BatchAddOutcome::default());
        }
        let started = std::time::Instant::now();
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        let mut added_names = Vec::new();
        let mut errors: Vec<(String, ToolError)> = Vec::new();
        for mut spec in specs {
            if let Some(ref env_name) = spec.bearer_token_env {
                let trimmed = env_name.trim().to_string();
                if let Err(e) = validate_bearer_token_env_name(&trimmed) {
                    errors.push((spec.name, e));
                    continue;
                }
                spec.bearer_token_env = Some(trimmed);
            }
            match insert_upstream(&mut cfg, spec.clone()) {
                Ok(()) => added_names.push(spec.name),
                Err(e) => errors.push((spec.name, e)),
            }
        }

        if added_names.is_empty() && !errors.is_empty() {
            // Every spec failed — return the first error to the caller.
            return Err(errors.remove(0).1);
        }

        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;

        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.import",
            event = "batch_install.finish",
            added = added_names.len(),
            skipped = errors.len(),
            tools_changed = diff.tools_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway batch reconcile"
        );

        let mut views = Vec::new();
        for name in &added_names {
            if let Ok(view) = self.get(name).await {
                views.push(view);
            }
        }
        Ok(BatchAddOutcome { views, errors })
    }

    pub async fn update(
        &self,
        name: &str,
        patch: GatewayUpdatePatch,
        bearer_token_value: Option<String>,
        _allow_stdio: bool,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let started = Instant::now();
        let mut patch = patch;
        let updated_name = patch.name.clone().unwrap_or_else(|| name.to_string());
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.update",
            event = "install.update.start",
            phase = "start",
            gateway = %name,
            new_gateway = %updated_name,
            "gateway reconcile"
        );
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();

        // Trim and validate bearer_token_env unconditionally so whitespace typos
        // are caught before they silently fail env-var lookup later.
        if let Some(Some(ref env_name)) = patch.bearer_token_env {
            let trimmed = env_name.trim().to_string();
            validate_bearer_token_env_name(&trimmed)?;
            patch.bearer_token_env = Some(Some(trimmed));
        }

        if let Some(token_value) = bearer_token_value.as_deref().map(str::trim)
            && !token_value.is_empty()
        {
            // Resolve env var name: prefer patch > existing config > error.
            // Auto-generation is intentionally not used here — callers must be
            // explicit so the stored env name is predictable and auditable.
            let env_name = if let Some(env) = patch
                .bearer_token_env
                .as_ref()
                .and_then(|value| value.as_deref())
            {
                env.to_string()
            } else if let Some(existing_env) = cfg
                .upstream
                .iter()
                .find(|u| u.name == name)
                .and_then(|u| u.bearer_token_env.as_deref())
            {
                existing_env.to_string()
            } else {
                return Err(ToolError::InvalidParam {
                    message: "bearer_token_env is required when providing bearer_token_value: \
                              set bearer_token_env in the patch or ensure the existing gateway \
                              already has one configured"
                        .to_string(),
                    param: "bearer_token_env".to_string(),
                });
            };
            patch.bearer_token_env = Some(Some(env_name.clone()));
            update_upstream(&mut cfg, name, patch)?;
            cfg.upstream
                .iter()
                .any(|u| u.name == updated_name)
                .then_some(())
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("gateway `{updated_name}` not found after update"),
                })?;
            self.persist_gateway_bearer_token(&env_name, token_value)
                .await?;
        } else {
            update_upstream(&mut cfg, name, patch)?;
            cfg.upstream
                .iter()
                .any(|u| u.name == updated_name)
                .then_some(())
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("gateway `{updated_name}` not found after update"),
                })?;
        }
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.update",
            event = "install.update.finish",
            phase = "finish",
            gateway = %name,
            new_gateway = %updated_name,
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        self.get(&updated_name).await
    }

    pub async fn remove(
        &self,
        name: &str,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayView, ToolError> {
        let started = Instant::now();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.remove",
            event = "remove.start",
            phase = "start",
            gateway = %name,
            "gateway reconcile"
        );
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let tool_search = cfg.tool_search.clone();
        let removed = remove_upstream(&mut cfg, name)?;
        self.persist_config(cfg).await?;
        let diff = self.reload_with_origin_unlocked(origin, owner).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.remove",
            event = "remove.finish",
            phase = "finish",
            gateway = %name,
            target = ?redacted_gateway_target(&removed),
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        Ok(GatewayView {
            config: config_view(&removed, &tool_search),
            runtime: GatewayRuntimeView {
                name: removed.name,
                ..GatewayRuntimeView::default()
            },
        })
    }

    pub async fn tool_search_config(&self) -> ToolSearchConfig {
        self.config.read().await.tool_search.clone()
    }

    pub async fn set_tool_search_config(
        &self,
        next: ToolSearchConfig,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<ToolSearchConfig, ToolError> {
        validate_tool_search(&next)?;
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        cfg.tool_search = next;
        self.persist_config(cfg).await?;
        self.reload_with_origin_unlocked(origin, owner).await?;
        Ok(self.tool_search_config().await)
    }

    pub async fn protected_route_list(&self) -> Vec<ProtectedMcpRouteConfig> {
        self.config.read().await.protected_mcp_routes.clone()
    }

    pub async fn protected_route_get(
        &self,
        name: &str,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        self.config
            .read()
            .await
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == name)
            .cloned()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("protected MCP route `{name}` not found"),
            })
    }

    pub async fn upstream_config(&self, name: &str) -> Option<UpstreamConfig> {
        self.config
            .read()
            .await
            .upstream
            .iter()
            .find(|upstream| upstream.name == name)
            .cloned()
    }

    pub async fn client_config(&self, name: &str) -> Result<McpClientConfigView, ToolError> {
        let upstream = self
            .upstream_config(name)
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("gateway `{name}` not found"),
            })?;

        if let Some(url) = upstream.url.clone() {
            return Ok(McpClientConfigView {
                name: upstream.name,
                r#type: McpClientTransportType::Http,
                url: Some(url),
                command: None,
                args: None,
                env: None,
            });
        }

        let Some(command) = upstream.command.clone() else {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_config".to_string(),
                message: format!("gateway `{name}` has neither url nor command configured"),
            });
        };

        Ok(McpClientConfigView {
            name: upstream.name,
            r#type: McpClientTransportType::Stdio,
            url: None,
            command: Some(command),
            args: (!upstream.args.is_empty()).then_some(upstream.args),
            env: None,
        })
    }

    pub async fn protected_route_add(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        self.persist_config(cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.add",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            enabled = route.enabled,
            scopes = ?route.scopes,
            "protected MCP route added"
        );
        Ok(route)
    }

    pub async fn protected_route_update(
        &self,
        name: &str,
        route: ProtectedMcpRouteConfig,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let route = update_protected_mcp_route(&mut cfg, name, route)?;
        self.persist_config(cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.update",
            route = %route.name,
            previous_name = %name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            enabled = route.enabled,
            scopes = ?route.scopes,
            "protected MCP route updated"
        );
        Ok(route)
    }

    pub async fn protected_route_remove(
        &self,
        name: &str,
    ) -> Result<ProtectedMcpRouteConfig, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let route = remove_protected_mcp_route(&mut cfg, name)?;
        self.persist_config(cfg).await?;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.remove",
            route = %route.name,
            public_host = %route.public_host,
            public_path = %route.public_path,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            "protected MCP route removed"
        );
        Ok(route)
    }

    pub async fn protected_route_test(
        &self,
        route: ProtectedMcpRouteConfig,
    ) -> Result<serde_json::Value, ToolError> {
        let mut cfg = LabConfig::default();
        let route = insert_protected_mcp_route(&mut cfg, route)?;
        let resource = route.public_resource();
        let metadata_url = format!(
            "https://{}/.well-known/oauth-protected-resource{}",
            route.public_host, route.public_path
        );
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.protected_route.test",
            route = %route.name,
            resource = %resource,
            metadata_url = %metadata_url,
            upstream = ?route.upstream,
            backend_url = %route.backend_url,
            backend_mcp_path = %route.backend_mcp_path,
            scopes = ?route.scopes,
            "protected MCP route validated"
        );
        Ok(serde_json::json!({
            "ok": true,
            "route": route,
            "resource": resource,
            "metadata_url": metadata_url,
        }))
    }

    pub async fn reload_with_origin(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        self.reload_with_origin_unlocked(origin, owner).await
    }

    async fn reload_with_origin_unlocked(
        &self,
        origin: Option<&str>,
        owner: Option<UpstreamRuntimeOwner>,
    ) -> Result<GatewayCatalogDiff, ToolError> {
        let started = Instant::now();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.start",
            phase = "config.load.start",
            "gateway reconcile"
        );
        let path = self.path.clone();
        let cfg = tokio::task::spawn_blocking(move || load_gateway_config(&path))
            .await
            .map_err(|e| ToolError::internal_message(format!("config read task failed: {e}")))??;
        let registry = self.builtin_service_registry();
        let (cfg, migration) = quarantine_unregistered_virtual_servers(cfg, &registry);
        if migration.changed() {
            tracing::warn!(
                action = "gateway.config.migrate",
                stale_virtual_server_count = migration.quarantined.len(),
                stale_virtual_servers = ?migration.quarantined,
                "quarantined virtual servers with unregistered backing services"
            );
            self.persist_config(cfg.clone()).await?;
        }
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.config.loaded",
            phase = "config.load.finish",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            quarantined_virtual_server_count = cfg.quarantined_virtual_servers.len(),
            "gateway reconcile"
        );
        if let Some(cache) = &self.oauth_client_cache {
            let existing = self.config.read().await.clone();
            for upstream in existing
                .upstream
                .iter()
                .filter(|upstream| upstream.oauth.is_some())
            {
                cache.evict_upstream(&upstream.name);
            }
        }
        // Reconcile the upstream_oauth_managers map from the new config.
        // Remove managers for OAuth upstreams no longer present; warn about
        // new OAuth upstreams that require a restart to get a manager.
        if let Some(managers) = &self.upstream_oauth_managers {
            let new_oauth_names: std::collections::HashSet<&str> = cfg
                .upstream
                .iter()
                .filter(|u| u.oauth.is_some())
                .map(|u| u.name.as_str())
                .collect();
            managers.retain(|name, _| new_oauth_names.contains(name.as_str()));
            for name in &new_oauth_names {
                if !managers.contains_key(*name) {
                    tracing::warn!(
                        upstream = name,
                        "new oauth upstream added via reload but no manager available; restart required"
                    );
                }
            }
        }
        let old_pool = self.runtime.current_pool().await;
        let before = snapshot_from_pool(old_pool.clone()).await;
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "health.schedule",
            operation = "health",
            phase = "pool.build.start",
            upstream_count = cfg.upstream.len(),
            "gateway reconcile"
        );
        crate::config::set_process_tool_search_enabled(cfg.tool_search.enabled);
        let fresh_pool = {
            let base_pool = match &self.oauth_client_cache {
                Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
                None => UpstreamPool::new(),
            };
            let pool = Arc::new(
                base_pool
                    .with_runtime_origin(runtime_origin_tag(origin))
                    .with_runtime_owner(owner),
            );
            let registry = self.builtin_service_registry();
            pool.discover_all_for_subject_with_in_process_peers(
                &cfg.upstream,
                SHARED_GATEWAY_OAUTH_SUBJECT,
                &registry,
            )
            .await;
            Some(pool)
        };
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "health.finish",
            operation = "health",
            phase = "pool.build.finish",
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        let after = snapshot_from_pool(fresh_pool.clone()).await;
        let old_pool_present = old_pool.is_some();
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "pool.swap",
            phase = "pool.swap",
            old_pool_present,
            "gateway reconcile"
        );
        self.runtime.swap(fresh_pool).await;
        if let Some(old_pool) = old_pool {
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "old_pool.drain.start",
                phase = "pool.drain.start",
                "gateway old upstream pool drain start"
            );
            old_pool.drain_for_swap("gateway.reload.swap").await;
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "gateway.reload",
                event = "old_pool.drain.finish",
                phase = "pool.drain.finish",
                "gateway old upstream pool drain finish"
            );
        }
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
        *self.config.write().await = cfg;
        let current_cfg = self.config.read().await.clone();
        let current_pool = self.runtime.current_pool().await;
        self.schedule_tool_search_rebuilds(&current_cfg, current_pool.clone());
        self.reconcile_runtime_state(&current_cfg, current_pool.as_deref())
            .await?;
        let diff = diff_catalogs(&before, &after);
        self.notify_catalog_changes(&diff);
        tracing::info!(
            surface = "dispatch",
            service = "gateway",
            action = "gateway.reload",
            event = "catalog.refresh.finish",
            phase = "finish",
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            before_tool_count = before.tools.len(),
            after_tool_count = after.tools.len(),
            before_resource_count = before.resources.len(),
            after_resource_count = after.resources.len(),
            before_prompt_count = before.prompts.len(),
            after_prompt_count = after.prompts.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "gateway reconcile"
        );
        Ok(diff)
    }

    pub async fn discovered_tools(
        &self,
        name: &str,
    ) -> Result<Vec<GatewayToolExposureRowView>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };

        Ok(pool
            .tool_exposure_rows(name)
            .await
            .into_iter()
            .map(|row| GatewayToolExposureRowView {
                name: row.name,
                description: row.description,
                exposed: row.exposed,
                matched_by: row.matched_by,
            })
            .collect())
    }

    pub async fn discovered_resources(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };
        let mut resources: Vec<String> = pool
            .list_upstream_resources()
            .await
            .into_iter()
            .filter_map(|resource| {
                resource
                    .uri
                    .strip_prefix(&format!("lab://upstream/{name}/"))
                    .map(ToOwned::to_owned)
            })
            .collect();
        resources.sort();
        Ok(resources)
    }

    pub async fn discovered_prompts(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let Some(pool) = self.runtime.current_pool().await else {
            return Ok(Vec::new());
        };

        let owners = pool.prompt_ownership_map(&[]).await;
        let mut prompts: Vec<String> = owners
            .into_iter()
            .filter(|(_, owner)| owner == name)
            .map(|(prompt_name, _)| prompt_name)
            .collect();
        prompts.sort();
        Ok(prompts)
    }

    pub async fn tool_search_enabled(&self) -> bool {
        self.config.read().await.tool_search.enabled
    }

    pub async fn tool_search_warming(&self) -> bool {
        self.tool_indexes
            .iter()
            .any(|entry| entry.value().warming.load(Ordering::Relaxed))
    }

    pub async fn search_tools(
        &self,
        query: &str,
        top_k: usize,
        include_schema: bool,
    ) -> Result<Vec<GatewayToolSearchResult>, ToolError> {
        if !self.config.read().await.tool_search.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "tool search is not enabled".to_string(),
            });
        }
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "query must not be empty".to_string(),
            });
        }
        if trimmed.len() > 500 {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "query exceeds max length 500".to_string(),
            });
        }

        let has_cached_index = self.has_cached_tool_search_index();
        self.refresh_tool_search_indexes_if_stale(!has_cached_index)
            .await;

        let requested = top_k.max(1).min(50);
        let mut hits: Vec<SearchHit> = self
            .tool_indexes
            .iter()
            .filter_map(|entry| entry.value().index.load_full())
            .flat_map(|index| index.search(trimmed, requested))
            .collect();

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.tool.name.cmp(&b.tool.name))
                .then_with(|| a.tool.upstream_name.cmp(&b.tool.upstream_name))
        });
        hits.truncate(requested);

        if hits.is_empty() && self.tool_search_warming().await {
            return Err(ToolError::Sdk {
                sdk_kind: "index_warming".to_string(),
                message: "tool index is being built, retry shortly".to_string(),
            });
        }

        Ok(hits
            .into_iter()
            .map(|hit| GatewayToolSearchResult {
                name: sanitize_tool_text(&hit.tool.name, 256),
                description: sanitize_tool_text(&hit.tool.description, 2048),
                upstream: hit.tool.upstream_name,
                score: hit.score,
                input_schema: if include_schema {
                    sanitize_schema(hit.tool.input_schema)
                } else {
                    None
                },
            })
            .collect())
    }

    pub async fn resolve_tool_execute(
        &self,
        name: &str,
    ) -> Result<(String, crate::dispatch::upstream::types::UpstreamTool), ToolError> {
        if !self.config.read().await.tool_search.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "tool search is not enabled; tool_execute requires tool_search mode"
                    .to_string(),
            });
        }
        let pool = self.current_pool().await.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: format!("tool `{name}` is not available"),
        })?;

        let matches = pool.find_tool_candidates(name).await;
        if matches.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{name}`"),
            });
        }
        if matches.len() > 1 {
            let valid = matches
                .iter()
                .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
                .collect::<Vec<_>>();
            return Err(ToolError::AmbiguousTool {
                message: format!("tool `{name}` matched multiple upstream tools"),
                valid,
            });
        }
        Ok(matches.into_iter().next().expect("checked len"))
    }

    fn has_cached_tool_search_index(&self) -> bool {
        self.tool_indexes
            .iter()
            .any(|entry| entry.value().index.load_full().is_some())
    }

    fn schedule_tool_search_rebuilds(&self, cfg: &LabConfig, pool: Option<Arc<UpstreamPool>>) {
        if !cfg.tool_search.enabled {
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "tool_search.rebuild",
                event = "disabled",
                "gateway tool index rebuild disabled"
            );
            for entry in self.tool_indexes.iter() {
                if let Ok(mut guard) = entry.in_flight.lock()
                    && let Some(handle) = guard.take()
                {
                    tracing::info!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "tool_search.rebuild",
                        event = "abort",
                        upstream = %entry.key(),
                        reason = "disabled",
                        "gateway tool index rebuild aborted"
                    );
                    handle.abort();
                }
            }
            self.tool_indexes.clear();
            return;
        }

        let enabled = cfg
            .upstream
            .iter()
            .map(|upstream| upstream.name.clone())
            .collect::<std::collections::HashSet<_>>();
        self.tool_indexes.retain(|name, state| {
            if !enabled.contains(name) {
                if let Ok(mut guard) = state.in_flight.lock()
                    && let Some(handle) = guard.take()
                {
                    tracing::info!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "tool_search.rebuild",
                        event = "abort",
                        upstream = %name,
                        reason = "upstream_removed",
                        "gateway tool index rebuild aborted"
                    );
                    handle.abort();
                }
            }
            enabled.contains(name)
        });

        let Some(pool) = pool else {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "tool_search.rebuild",
                event = "skipped",
                kind = "upstream_pool_empty",
                "gateway tool index rebuild skipped without pool"
            );
            self.tool_indexes.clear();
            return;
        };

        for upstream in &cfg.upstream {
            let state = self
                .tool_indexes
                .entry(upstream.name.clone())
                .or_default()
                .clone();
            // Abort the previous rebuild for this upstream before starting a
            // new one, and bump the generation so any in-flight older task
            // refuses to publish its result.
            if let Ok(mut guard) = state.in_flight.lock()
                && let Some(handle) = guard.take()
            {
                tracing::info!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "tool_search.rebuild",
                    event = "abort",
                    upstream = %upstream.name,
                    reason = "superseded",
                    "gateway tool index rebuild aborted"
                );
                handle.abort();
            }
            let my_generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
            let upstream = upstream.clone();
            let upstream_name = upstream.name.clone();
            let pool = pool.clone();
            let max_tools = cfg.tool_search.max_tools;
            state.warming.store(true, Ordering::Relaxed);
            let state_for_task = state.clone();
            tracing::info!(
                surface = "dispatch",
                service = "gateway",
                action = "tool_search.rebuild",
                event = "scheduled",
                upstream = %upstream.name,
                generation = my_generation,
                max_tools,
                "gateway tool index rebuild scheduled"
            );
            let handle = tokio::spawn(async move {
                let started = Instant::now();
                tracing::debug!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "tool_search.rebuild",
                    event = "start",
                    upstream = %upstream_name,
                    generation = my_generation,
                    "gateway tool index rebuild start"
                );
                let healthy_tools = pool.healthy_tools_for_upstream(&upstream_name).await;
                let tool_count = healthy_tools.len();
                let built = tokio::task::spawn_blocking(move || {
                    ToolIndex::build_from_tools(&upstream, healthy_tools, max_tools)
                })
                .await;
                if state_for_task.generation.load(Ordering::Relaxed) == my_generation
                    && let Ok(index) = built
                {
                    state_for_task.index.store(Some(Arc::new(index)));
                    tracing::info!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "tool_search.rebuild",
                        event = "finish",
                        upstream = %upstream_name,
                        generation = my_generation,
                        tool_count,
                        elapsed_ms = started.elapsed().as_millis(),
                        "gateway tool index rebuild finish"
                    );
                } else {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "gateway",
                        action = "tool_search.rebuild",
                        event = "skipped",
                        upstream = %upstream_name,
                        generation = my_generation,
                        kind = "stale_generation",
                        elapsed_ms = started.elapsed().as_millis(),
                        "gateway tool index rebuild skipped"
                    );
                }
                state_for_task.warming.store(false, Ordering::Relaxed);
            });
            if let Ok(mut guard) = state.in_flight.lock() {
                *guard = Some(handle.abort_handle());
            }
        }
    }

    /// Refresh per-upstream tool-search indexes on the search hot path.
    ///
    /// TTL-gated on `TOOL_SEARCH_REPROBE_TTL`: if the last attempted reprobe
    /// is younger than the TTL, skip the live probe and keep the cached
    /// index. Remaining stale upstreams are reprobed concurrently.
    async fn refresh_tool_search_indexes_if_stale(&self, wait_for_refresh: bool) {
        let cfg = self.config.read().await.clone();
        if !cfg.tool_search.enabled {
            return;
        }
        let Some(pool) = self.current_pool().await else {
            tracing::warn!(
                surface = "mcp",
                service = "gateway",
                action = "tool_search.reprobe",
                event = "skipped",
                operation = "health",
                kind = "upstream_pool_empty",
                "gateway tool index reprobe skipped without pool"
            );
            return;
        };

        let now = Instant::now();
        let max_tools = cfg.tool_search.max_tools;
        let mut pending = Vec::new();
        for upstream in cfg.upstream {
            if !upstream.enabled {
                tracing::debug!(
                    surface = "mcp",
                    service = "gateway",
                    action = "tool_search.reprobe",
                    event = "skipped",
                    operation = "health",
                    upstream = %upstream.name,
                    reason = "disabled",
                    "gateway tool index reprobe skipped"
                );
                continue;
            }
            let state = self
                .tool_indexes
                .entry(upstream.name.clone())
                .or_default()
                .clone();
            let fresh = state
                .last_reprobe_attempt_at
                .lock()
                .ok()
                .and_then(|guard| *guard)
                .is_some_and(|t| now.duration_since(t) < TOOL_SEARCH_REPROBE_TTL);
            if fresh {
                tracing::debug!(
                    surface = "mcp",
                    service = "gateway",
                    action = "tool_search.reprobe",
                    event = "skipped",
                    operation = "health",
                    upstream = %upstream.name,
                    reason = "fresh",
                    "gateway tool index reprobe skipped"
                );
                continue;
            }
            if let Ok(mut guard) = state.last_reprobe_attempt_at.lock() {
                *guard = Some(now);
            }
            state.warming.store(true, Ordering::Relaxed);
            tracing::info!(
                surface = "mcp",
                service = "gateway",
                action = "tool_search.reprobe",
                event = "scheduled",
                operation = "health",
                upstream = %upstream.name,
                "gateway tool index reprobe scheduled"
            );
            pending.push((upstream, state));
        }

        let tasks = pending.into_iter().map(|(upstream, state)| {
            let pool = Arc::clone(&pool);
            async move {
                let reprobe_started = Instant::now();
                tracing::debug!(
                    surface = "mcp",
                    service = "gateway",
                    action = "tool_search.reprobe",
                    event = "start",
                    operation = "health",
                    upstream = %upstream.name,
                    "gateway tool index reprobe start"
                );
                if let Err(err) = pool.reprobe_tools_for_upstream(&upstream).await {
                    tracing::warn!(
                        surface = "mcp",
                        service = "gateway",
                        action = "tool_search.reprobe",
                        event = "error",
                        operation = "health",
                        elapsed_ms = reprobe_started.elapsed().as_millis(),
                        kind = "upstream_reprobe_failed",
                        error = %err,
                        upstream = %upstream.name,
                        "gateway tool index reprobe failed"
                    );
                    state.warming.store(false, Ordering::Relaxed);
                    return;
                }
                let healthy_tools = pool.healthy_tools_for_upstream(&upstream.name).await;
                let upstream_clone = upstream.clone();
                let built = tokio::task::spawn_blocking(move || {
                    ToolIndex::build_from_tools(&upstream_clone, healthy_tools, max_tools)
                })
                .await;
                if let Ok(index) = built {
                    let should_publish = state.index.load_full().as_ref().is_none_or(|current| {
                        current.metadata.catalog_hash != index.metadata.catalog_hash
                    });
                    if should_publish {
                        state.index.store(Some(Arc::new(index)));
                    }
                    tracing::info!(
                        surface = "mcp",
                        service = "gateway",
                        action = "tool_search.reprobe",
                        event = "finish",
                        operation = "health",
                        elapsed_ms = reprobe_started.elapsed().as_millis(),
                        upstream = %upstream.name,
                        published = should_publish,
                        "gateway tool index reprobe finish"
                    );
                } else {
                    tracing::warn!(
                        surface = "mcp",
                        service = "gateway",
                        action = "tool_search.reprobe",
                        event = "error",
                        operation = "health",
                        elapsed_ms = reprobe_started.elapsed().as_millis(),
                        kind = "tool_index_build_failed",
                        upstream = %upstream.name,
                        "gateway tool index reprobe build failed"
                    );
                }
                state.warming.store(false, Ordering::Relaxed);
            }
        });

        if wait_for_refresh {
            futures::future::join_all(tasks).await;
        } else {
            for task in tasks {
                tokio::spawn(task);
            }
        }
    }

    #[cfg(test)]
    pub async fn replace_config_for_tests(&self, upstream: Vec<UpstreamConfig>) {
        self.seed_config(LabConfig {
            upstream,
            ..LabConfig::default()
        })
        .await;
    }

    fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff) {
        if !diff.tools_changed && !diff.resources_changed && !diff.prompts_changed {
            return;
        }

        if let Some(notifier) = &self.notifier {
            notifier.notify_catalog_changes(diff);
        }
    }

    async fn set_virtual_server_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<ServerView, ToolError> {
        let _mutation_guard = self.config_mutation.lock().await;
        let mut cfg = self.config.read().await.clone();
        let existing_index = cfg
            .virtual_servers
            .iter()
            .position(|server| server.id == id);
        let index = if let Some(index) = existing_index {
            index
        } else {
            let meta = self
                .registered_service_meta(id)
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("virtual server `{id}` not found"),
                })?;
            let values = read_env_values(&self.env_path())?;
            let configured = service_config_view(meta, &values).configured;
            if !configured {
                return Err(ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("virtual server `{id}` not found"),
                });
            }

            cfg.virtual_servers
                .push(crate::config::VirtualServerConfig {
                    id: id.to_string(),
                    service: id.to_string(),
                    enabled: false,
                    surfaces: crate::config::VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                });
            cfg.virtual_servers.len() - 1
        };

        let virtual_server = cfg
            .virtual_servers
            .get_mut(index)
            .expect("virtual server index should exist");
        if enabled
            && self
                .registered_service_meta(&virtual_server.service)
                .is_none()
        {
            return Err(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("virtual server `{id}` not found"),
            });
        }
        virtual_server.enabled = enabled;
        if enabled {
            virtual_server.surfaces.mcp = true;
        }

        self.persist_config(cfg).await?;
        let cfg = self.config.read().await;
        let virtual_server = find_virtual_server(&cfg, id)?;
        Ok(server_view_from_virtual_server(
            virtual_server,
            UpstreamCachedSummary::default(),
            None,
            None,
        ))
    }

    fn env_path(&self) -> PathBuf {
        #[cfg(test)]
        if let Some(parent) = self.path.parent() {
            // Tests isolate canonical service-config writes beside the temp
            // gateway config instead of touching the developer's ~/.lab/.env.
            return parent.join(".env");
        }
        crate::tui::services::lab_env_path()
    }

    async fn persist_gateway_bearer_token(
        &self,
        env_name: &str,
        token_value: &str,
    ) -> Result<(), ToolError> {
        validate_bearer_token_env_name(env_name)?;

        let auth_header = normalize_gateway_bearer_token(token_value);
        let env_path = self.env_path();
        let creds = [ServiceCreds {
            service: "gateway".to_string(),
            url: None,
            secret: Some(auth_header),
            env_field: env_name.to_string(),
            source_host: None,
            probe_host: None,
            runtime: None,
            url_verified: false,
        }];

        if !env_is_up_to_date(&env_path, &creds) {
            drop(backup_env(&env_path).map_err(|e| {
                ToolError::internal_message(format!("failed to back up env file: {e}"))
            })?);
            write_env(&env_path, &creds, true).map_err(|e| {
                ToolError::internal_message(format!("failed to write env file: {e}"))
            })?;
        }

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

        Ok(())
    }

    pub(super) async fn persist_config(&self, cfg: LabConfig) -> Result<(), ToolError> {
        let path = self.path.clone();
        let cfg_clone = cfg.clone();
        tracing::info!(
            action = "gateway.config.write",
            phase = "start",
            upstream_count = cfg.upstream.len(),
            virtual_server_count = cfg.virtual_servers.len(),
            "gateway reconcile"
        );
        tokio::task::spawn_blocking(move || write_gateway_config(&path, &cfg_clone))
            .await
            .map_err(|e| ToolError::internal_message(format!("config write task failed: {e}")))??;
        *self.protected_route_index.write().await =
            ProtectedRouteIndex::from_routes(&cfg.protected_mcp_routes);
        *self.config.write().await = cfg;
        tracing::info!(
            action = "gateway.config.write",
            phase = "finish",
            "gateway reconcile"
        );
        Ok(())
    }
}

fn resolve_gateway_bearer_env_name(
    gateway_name: &str,
    explicit_env_name: Option<&str>,
) -> Result<String, ToolError> {
    match explicit_env_name.map(str::trim) {
        Some(name) if !name.is_empty() => {
            validate_bearer_token_env_name(name)?;
            Ok(name.to_string())
        }
        _ => Ok(default_gateway_bearer_env_name(gateway_name)),
    }
}

fn normalize_gateway_bearer_token(token_value: &str) -> String {
    let trimmed = token_value.trim();
    if trimmed
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("bearer "))
    {
        format!("Bearer {}", &trimmed[7..])
    } else {
        format!("Bearer {trimmed}")
    }
}

fn find_virtual_server<'a>(
    cfg: &'a LabConfig,
    id: &str,
) -> Result<&'a crate::config::VirtualServerConfig, ToolError> {
    cfg.virtual_servers
        .iter()
        .find(|server| server.id == id)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("virtual server `{id}` not found"),
        })
}

fn find_virtual_server_for_service<'a>(
    cfg: &'a LabConfig,
    service: &str,
) -> Option<&'a crate::config::VirtualServerConfig> {
    cfg.virtual_servers
        .iter()
        .find(|server| server.service == service || server.id == service)
}

fn quarantine_unregistered_virtual_servers(
    mut cfg: LabConfig,
    registry: &ToolRegistry,
) -> (LabConfig, VirtualServerMigration) {
    let mut migration = VirtualServerMigration::default();
    let mut active = Vec::with_capacity(cfg.virtual_servers.len());

    for virtual_server in std::mem::take(&mut cfg.virtual_servers) {
        if registry.service(&virtual_server.service).is_some() {
            active.push(virtual_server);
            continue;
        }

        migration.quarantined.push(virtual_server.id.clone());
        let already_quarantined = cfg
            .quarantined_virtual_servers
            .iter()
            .any(|existing| existing.id == virtual_server.id);
        if !already_quarantined {
            cfg.quarantined_virtual_servers.push(virtual_server);
        }
    }

    cfg.virtual_servers = active;
    (cfg, migration)
}

async fn snapshot_from_pool(pool: Option<Arc<UpstreamPool>>) -> GatewayCatalogSnapshot {
    let Some(pool) = pool else {
        return GatewayCatalogSnapshot::default();
    };

    GatewayCatalogSnapshot {
        tools: pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|tool| tool.tool.name.to_string())
            .collect(),
        resources: pool
            .routable_upstream_names(
                crate::dispatch::upstream::types::UpstreamCapability::Resources,
            )
            .await
            .into_iter()
            .collect(),
        prompts: pool
            .routable_upstream_names(crate::dispatch::upstream::types::UpstreamCapability::Prompts)
            .await
            .into_iter()
            .collect(),
    }
}

#[cfg(test)]
use super::runtime::{process_matches_patterns, upstream_cleanup_patterns};

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::sync::Arc;

    use crate::config::{
        ProtectedMcpRouteConfig, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode,
        UpstreamOauthRegistration, VirtualServerConfig, VirtualServerSurfacesConfig,
    };
    use crate::oauth::upstream::cache::OauthClientCache;
    use crate::tui::events::ServiceHealth;
    use rmcp::transport::{AuthClient, AuthorizationManager};

    use super::*;

    async fn dummy_auth_client() -> Arc<AuthClient<reqwest::Client>> {
        let manager = AuthorizationManager::new("http://localhost")
            .await
            .expect("authorization manager");
        Arc::new(AuthClient::new(reqwest::Client::new(), manager))
    }

    fn fixture_stdio_upstream(name: &str) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: name.to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        }
    }

    fn fixture_http_upstream(name: &str) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: name.to_string(),
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        }
    }

    async fn tool_search_manager_with_pool(
        upstream: UpstreamConfig,
    ) -> (GatewayManager, Arc<UpstreamPool>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let runtime = GatewayRuntimeHandle::default();
        let pool = Arc::new(UpstreamPool::new());
        runtime.swap(Some(Arc::clone(&pool))).await;
        let manager = GatewayManager::new(path, runtime);
        manager
            .seed_config(LabConfig {
                tool_search: ToolSearchConfig {
                    enabled: true,
                    ..ToolSearchConfig::default()
                },
                upstream: vec![upstream],
                ..LabConfig::default()
            })
            .await;
        (manager, pool)
    }

    #[tokio::test]
    async fn tool_search_failed_reprobe_attempt_is_ttl_gated() {
        let upstream = fixture_http_upstream("downstream");
        let (manager, _pool) = tool_search_manager_with_pool(upstream).await;

        let _first_results = manager
            .search_tools("missing tool", 5, false)
            .await
            .expect("failed reprobe still returns current search result set");
        let state = manager
            .tool_indexes
            .get("downstream")
            .expect("tool index state")
            .clone();
        let first_attempt = *state
            .last_reprobe_attempt_at
            .lock()
            .expect("attempt timestamp lock");
        assert!(first_attempt.is_some(), "failed reprobe records attempt");

        let _second_results = manager
            .search_tools("missing tool", 5, false)
            .await
            .expect("fresh failed reprobe is TTL-gated");
        let second_attempt = *state
            .last_reprobe_attempt_at
            .lock()
            .expect("attempt timestamp lock");

        assert_eq!(
            first_attempt, second_attempt,
            "second search inside TTL must not schedule another reprobe"
        );
    }

    #[tokio::test]
    async fn tool_search_returns_cached_results_while_stale_refresh_runs() {
        let upstream = fixture_http_upstream("cached-upstream");
        let (manager, _pool) = tool_search_manager_with_pool(upstream.clone()).await;
        let schema = Arc::new(serde_json::Map::new());
        let upstream_name: Arc<str> = Arc::from(upstream.name.as_str());
        let tool = rmcp::model::Tool::new(
            "cached_lookup",
            "Cached marker search tool",
            Arc::clone(&schema),
        );
        let index = ToolIndex::build_from_tools(
            &upstream,
            vec![crate::dispatch::upstream::types::UpstreamTool {
                tool,
                input_schema: None,
                upstream_name,
            }],
            10,
        );
        let state = manager
            .tool_indexes
            .entry(upstream.name.clone())
            .or_default()
            .clone();
        state.index.store(Some(Arc::new(index)));

        let results = tokio::time::timeout(
            Duration::from_millis(100),
            manager.search_tools("cached marker", 5, false),
        )
        .await
        .expect("cached search must not wait for live stale refresh")
        .expect("cached search succeeds");

        assert!(
            results.iter().any(|result| result.name == "cached_lookup"),
            "cached index result should be returned"
        );
        assert!(
            state
                .last_reprobe_attempt_at
                .lock()
                .expect("attempt timestamp lock")
                .is_some(),
            "stale refresh should still be scheduled in the background"
        );
    }

    fn fixture_protected_route(name: &str) -> ProtectedMcpRouteConfig {
        ProtectedMcpRouteConfig {
            name: name.to_string(),
            enabled: true,
            public_host: "mcp.tootie.tv".to_string(),
            public_path: "/syslog".to_string(),
            upstream: None,
            backend_url: "http://100.88.16.79:3100".to_string(),
            backend_mcp_path: "/mcp".to_string(),
            scopes: vec!["mcp:read".to_string(), "mcp:write".to_string()],
            health_path: Some("/health".to_string()),
        }
    }

    #[tokio::test]
    async fn protected_route_add_updates_live_resolver_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .protected_route_add(fixture_protected_route("syslog"))
            .await
            .expect("add protected route");

        assert_eq!(
            manager
                .resolve_protected_route("mcp.tootie.tv", "/syslog")
                .await
                .expect("route should be live")
                .name,
            "syslog"
        );
        assert_eq!(
            manager
                .resolve_protected_route_metadata(
                    "mcp.tootie.tv",
                    "/.well-known/oauth-protected-resource/syslog",
                )
                .await
                .expect("metadata route should be live")
                .name,
            "syslog"
        );
    }

    #[test]
    fn catalog_diff_detects_removed_tool_provider() {
        let before = GatewayCatalogSnapshot {
            tools: ["fixture-http-echo".to_string()].into_iter().collect(),
            resources: BTreeSet::new(),
            prompts: BTreeSet::new(),
        };
        let after = GatewayCatalogSnapshot::default();

        let diff = diff_catalogs(&before, &after);
        assert!(diff.tools_changed);
        assert!(!diff.resources_changed);
        assert!(!diff.prompts_changed);
    }

    #[test]
    fn github_chat_cleanup_patterns_cover_uv_wrappers() {
        let upstream = UpstreamConfig {
            enabled: true,
            name: "github-chat".to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec!["github-chat-mcp".to_string()],
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        };

        let patterns = upstream_cleanup_patterns(&upstream, false);
        assert!(patterns.contains(&"github-chat-mcp".to_string()));
        assert!(patterns.contains(&"uvx github-chat-mcp".to_string()));
        assert!(patterns.contains(&"uv tool uvx github-chat-mcp".to_string()));
        assert!(patterns.contains(&"uv run github-chat-mcp".to_string()));
        assert!(patterns.contains(&"github-chat".to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_matcher_uses_joined_cmdline_text() {
        let patterns = vec!["uvx github-chat-mcp".to_string(), "github-chat".to_string()];
        assert!(process_matches_patterns(
            "uvx github-chat-mcp --transport stdio",
            &patterns,
        ));
        assert!(!process_matches_patterns(
            "python -m unrelated-service",
            &patterns,
        ));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cleanup_upstream_processes_kills_matching_github_chat_runtime() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
        let upstream_name = "github-chat-cleanup-manager";
        let runtime_arg = "github-chat-cleanup-manager-mcp";

        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: upstream_name.to_string(),
                url: None,
                bearer_token_env: None,
                command: Some("uvx".to_string()),
                args: vec![runtime_arg.to_string()],
                env: BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let mut command = Command::new("python3");
        command
            .args(["-c", "import time; time.sleep(60)", runtime_arg])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // The cleanup path kills process groups for child runtimes. Keep this
        // stand-in out of nextest's process group so the test process survives.
        command.process_group(0);
        let mut child = command.spawn().expect("spawn github chat stand-in");

        tokio::time::sleep(Duration::from_millis(150)).await;

        let _cleanup = manager
            .cleanup_upstream_processes(upstream_name, false, false)
            .await
            .expect("cleanup");

        for _ in 0..20 {
            if child.try_wait().expect("try_wait").is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        drop(child.kill());
        panic!("github-chat stand-in process was not terminated by cleanup");
    }

    #[tokio::test]
    async fn runtime_handle_starts_without_pool() {
        let handle = GatewayRuntimeHandle::default();
        assert!(handle.current_pool().await.is_none());
    }

    #[tokio::test]
    async fn runtime_handle_swaps_pool_atomically() {
        let handle = GatewayRuntimeHandle::default();
        let pool = Arc::new(UpstreamPool::new());

        handle.swap(Some(Arc::clone(&pool))).await;

        let current = handle.current_pool().await.expect("pool present");
        assert!(Arc::ptr_eq(&current, &pool));
    }

    #[tokio::test]
    async fn manager_get_preserves_bearer_token_env_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let gateway = manager.get("fixture-http").await.expect("gateway");
        assert_eq!(
            gateway.config.bearer_token_env.as_deref(),
            Some("FIXTURE_HTTP_TOKEN")
        );
    }

    #[tokio::test]
    async fn manager_get_redacts_sensitive_stdio_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .replace_config_for_tests(vec![UpstreamConfig {
                enabled: true,
                name: "fixture-stdio".to_string(),
                url: None,
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: Some("env".to_string()),
                args: vec![
                    "OPENAI_API_KEY=super-secret".to_string(),
                    "npx".to_string(),
                    "--access_token=abc123".to_string(),
                    "--api-key=super-secret".to_string(),
                ],
                env: BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            }])
            .await;

        let gateway = manager.get("fixture-stdio").await.expect("gateway");
        assert_eq!(gateway.config.command.as_deref(), Some("env"));
        assert_eq!(
            gateway.config.args,
            vec![
                "OPENAI_API_KEY=[redacted]".to_string(),
                "npx".to_string(),
                "--access_token=[redacted]".to_string(),
                "--api-key=[redacted]".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn server_view_redacts_sensitive_target_url_components() {
        let upstream = UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://user:pass@127.0.0.1:9001/callback?token=secret&mode=1".to_string()),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        };

        let view = server_view_from_upstream(None, &upstream).await;

        assert_eq!(
            view.config_summary.target.as_deref(),
            Some("http://127.0.0.1:9001/callback?token=[redacted]&mode=1")
        );
    }

    #[tokio::test]
    async fn server_view_redacts_invalid_target_urls() {
        let upstream = UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://user:pass@[::1".to_string()),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        };

        let view = server_view_from_upstream(None, &upstream).await;

        assert_eq!(
            view.config_summary.target.as_deref(),
            Some("[invalid-url-redacted]")
        );
    }

    #[tokio::test]
    async fn server_view_redacts_stdio_env_targets() {
        let upstream = UpstreamConfig {
            enabled: true,
            name: "fixture-stdio".to_string(),
            url: None,
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: Some("env".to_string()),
            args: vec![
                "OPENAI_API_KEY=super-secret".to_string(),
                "npx".to_string(),
                "--access_token=abc123".to_string(),
            ],
            env: BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        };

        let view = server_view_from_upstream(None, &upstream).await;

        assert_eq!(view.config_summary.target.as_deref(), Some("env"));
    }

    #[tokio::test]
    async fn configured_service_appears_in_list_before_virtual_server_enablement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: false,
                    surfaces: VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        let servers = manager.list().await.expect("list");
        let plex = servers
            .iter()
            .find(|server| server.id == "plex")
            .expect("plex server");
        assert!(plex.configured);
        assert!(!plex.enabled);
        assert_eq!(plex.source, "in_process");
    }

    #[tokio::test]
    async fn stale_virtual_server_with_unknown_service_does_not_break_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "mcpregistry".to_string(),
                    service: "mcpregistry".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        let servers = manager.list().await.expect("list should fail open");
        let stale = servers
            .iter()
            .find(|server| server.id == "mcpregistry")
            .expect("stale server row");

        assert!(!stale.connected);
        assert!(!stale.surfaces.mcp.connected);
        assert_eq!(stale.discovered_tool_count, 0);
        assert_eq!(
            stale.warnings.first().map(|warning| warning.code.as_str()),
            Some("unknown_service")
        );
    }

    #[tokio::test]
    async fn reload_quarantines_virtual_servers_for_unregistered_services() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_gateway_config(
            &path,
            &LabConfig {
                virtual_servers: vec![
                    VirtualServerConfig {
                        id: "plex".to_string(),
                        service: "plex".to_string(),
                        enabled: true,
                        surfaces: VirtualServerSurfacesConfig {
                            mcp: true,
                            ..VirtualServerSurfacesConfig::default()
                        },
                        mcp_policy: None,
                    },
                    VirtualServerConfig {
                        id: "stale-registry".to_string(),
                        service: "mcpregistry".to_string(),
                        enabled: true,
                        surfaces: VirtualServerSurfacesConfig {
                            mcp: true,
                            ..VirtualServerSurfacesConfig::default()
                        },
                        mcp_policy: None,
                    },
                ],
                ..LabConfig::default()
            },
        )
        .expect("write config");

        let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
        manager
            .reload_with_origin(None, None)
            .await
            .expect("reload");

        let listed = manager.list().await.expect("list");
        assert!(listed.iter().any(|server| server.id == "plex"));
        assert!(!listed.iter().any(|server| server.id == "stale-registry"));

        let migrated = load_gateway_config(&path).expect("load migrated config");
        assert_eq!(migrated.virtual_servers.len(), 1);
        assert_eq!(migrated.virtual_servers[0].id, "plex");
        assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
        assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-registry");
    }

    #[tokio::test]
    async fn reload_does_not_duplicate_existing_quarantined_virtual_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let stale = VirtualServerConfig {
            id: "stale-registry".to_string(),
            service: "mcpregistry".to_string(),
            enabled: true,
            surfaces: VirtualServerSurfacesConfig {
                mcp: true,
                ..VirtualServerSurfacesConfig::default()
            },
            mcp_policy: None,
        };
        write_gateway_config(
            &path,
            &LabConfig {
                virtual_servers: vec![stale.clone()],
                quarantined_virtual_servers: vec![stale],
                ..LabConfig::default()
            },
        )
        .expect("write config");

        let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
        manager
            .reload_with_origin(None, None)
            .await
            .expect("reload");

        let migrated = load_gateway_config(&path).expect("load migrated config");
        assert!(migrated.virtual_servers.is_empty());
        assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
        assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-registry");
    }

    #[test]
    fn quarantine_migration_is_noop_when_only_existing_quarantine_remains() {
        let stale = VirtualServerConfig {
            id: "stale-registry".to_string(),
            service: "mcpregistry".to_string(),
            enabled: true,
            surfaces: VirtualServerSurfacesConfig::default(),
            mcp_policy: None,
        };

        let registry = crate::registry::build_default_registry();
        let (migrated, migration) = quarantine_unregistered_virtual_servers(
            LabConfig {
                quarantined_virtual_servers: vec![stale],
                ..LabConfig::default()
            },
            &registry,
        );

        assert!(!migration.changed());
        assert!(migrated.virtual_servers.is_empty());
        assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
    }

    #[tokio::test]
    async fn service_config_get_redacts_secret_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let mut values = BTreeMap::new();
        values.insert("PLEX_URL".to_string(), "http://127.0.0.1:32400".to_string());
        values.insert("PLEX_TOKEN".to_string(), "super-secret".to_string());

        let config = manager
            .set_service_config("plex", &values)
            .await
            .expect("set service config");

        let token = config
            .fields
            .iter()
            .find(|field| field.name == "PLEX_TOKEN")
            .expect("token field");
        assert!(token.present);
        assert!(token.secret);
        assert_eq!(token.value_preview, None);
    }

    #[tokio::test]
    async fn service_config_get_treats_empty_values_as_not_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let mut values = BTreeMap::new();
        values.insert("OPENAI_API_KEY".to_string(), "token".to_string());
        values.insert("OPENAI_URL".to_string(), String::new());

        let config = manager
            .set_service_config("openai", &values)
            .await
            .expect("set service config");

        let url = config
            .fields
            .iter()
            .find(|field| field.name == "OPENAI_URL")
            .expect("url field");
        assert!(!url.present);
        assert_eq!(url.value_preview, None);
    }

    #[tokio::test]
    async fn service_config_get_marks_service_unconfigured_when_required_fields_are_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let mut values = BTreeMap::new();
        values.insert("PLEX_TOKEN".to_string(), "token".to_string());

        let config = manager
            .set_service_config("plex", &values)
            .await
            .expect("set service config");

        assert!(
            !config.configured,
            "plex should remain unconfigured until every required field is present"
        );
    }

    #[tokio::test]
    async fn service_config_get_marks_service_configured_when_required_fields_are_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let mut values = BTreeMap::new();
        values.insert("OPENAI_API_KEY".to_string(), "token".to_string());
        values.insert(
            "OPENAI_URL".to_string(),
            "https://api.openai.com/v1".to_string(),
        );

        let config = manager
            .set_service_config("openai", &values)
            .await
            .expect("set service config");

        assert!(config.configured);
    }

    #[tokio::test]
    async fn add_with_bearer_token_value_writes_env_and_references_generated_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let gateway = manager
            .add(
                UpstreamConfig {
                    enabled: true,
                    name: "github".to_string(),
                    url: Some("https://api.githubcopilot.com/mcp/".to_string()),
                    bearer_token_env: None,
                    command: None,
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                },
                Some("ghp_secret".to_string()),
                false,
                None,
                None,
            )
            .await
            .expect("add gateway");

        assert_eq!(
            gateway.config.bearer_token_env.as_deref(),
            Some("LAB_GW_GITHUB_AUTH_HEADER")
        );

        let values = read_env_values(&dir.path().join(".env")).expect("read env");
        assert_eq!(
            values.get("LAB_GW_GITHUB_AUTH_HEADER").map(String::as_str),
            Some("Bearer ghp_secret")
        );
    }

    #[tokio::test]
    async fn concurrent_gateway_adds_persist_both_gateways() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());

        let first = manager.clone();
        let second = manager.clone();
        let (first_result, second_result) = tokio::join!(
            first.add(fixture_stdio_upstream("alpha"), None, true, None, None),
            second.add(fixture_stdio_upstream("bravo"), None, true, None, None),
        );

        first_result.expect("add alpha");
        second_result.expect("add bravo");

        let persisted = load_gateway_config(&path).expect("load persisted config");
        let names = persisted
            .upstream
            .iter()
            .map(|upstream| upstream.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(names, BTreeSet::from(["alpha", "bravo"]));
    }

    #[tokio::test]
    async fn concurrent_root_and_virtual_server_mutations_both_persist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: false,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        let root = manager.clone();
        let virtual_server = manager.clone();
        let (root_result, virtual_result) = tokio::join!(
            root.set_tool_search_config(
                ToolSearchConfig {
                    enabled: true,
                    ..ToolSearchConfig::default()
                },
                None,
                None,
            ),
            virtual_server.set_virtual_server_surface("plex", "mcp", true),
        );

        root_result.expect("set root tool search config");
        virtual_result.expect("set virtual server surface");

        let persisted = load_gateway_config(&path).expect("load persisted config");
        assert!(persisted.tool_search.enabled);
        let plex = persisted
            .virtual_servers
            .iter()
            .find(|server| server.id == "plex")
            .expect("plex virtual server persisted");
        assert!(plex.surfaces.mcp);
    }

    #[tokio::test]
    async fn incomplete_service_does_not_appear_in_list_before_virtual_server_enablement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let mut values = BTreeMap::new();
        values.insert("PLEX_TOKEN".to_string(), "token".to_string());

        manager
            .set_service_config("plex", &values)
            .await
            .expect("set service config");

        let servers = manager.list().await.expect("list");
        assert!(
            servers.iter().all(|server| server.id != "plex"),
            "incomplete services should not appear in the gateway catalog"
        );
    }

    #[tokio::test]
    async fn disabling_virtual_server_preserves_configured_service_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig::default(),
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        let mut cfg = manager.config.read().await.clone();
        cfg.virtual_servers[0].enabled = false;
        manager.seed_config(cfg).await;

        let servers = manager.list().await.expect("list");
        let plex = servers
            .iter()
            .find(|server| server.id == "plex")
            .expect("plex server");
        assert!(plex.configured);
        assert!(!plex.enabled);
        assert_eq!(plex.config_summary.target.as_deref(), Some("plex"));
    }

    #[test]
    fn disabled_virtual_server_reports_disconnected_even_when_health_is_ok() {
        let view = server_view_from_virtual_server(
            &VirtualServerConfig {
                id: "plex".to_string(),
                service: "plex".to_string(),
                enabled: false,
                surfaces: VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            },
            UpstreamCachedSummary::default(),
            None,
            Some(&ServiceHealth {
                service: "plex".to_string(),
                reachable: true,
                auth_ok: true,
                latency_ms: Some(12),
                message: None,
            }),
        );

        assert!(!view.connected);
        assert!(!view.surfaces.mcp.connected);
    }

    #[test]
    fn healthy_informational_probe_messages_do_not_create_gateway_warnings() {
        let view = server_view_from_virtual_server(
            &VirtualServerConfig {
                id: "unraid".to_string(),
                service: "unraid".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            },
            UpstreamCachedSummary::default(),
            None,
            Some(&ServiceHealth {
                service: "unraid".to_string(),
                reachable: true,
                auth_ok: true,
                latency_ms: Some(12),
                message: Some("online".to_string()),
            }),
        );

        assert!(view.connected);
        assert!(view.warnings.is_empty());
    }

    #[tokio::test]
    async fn managed_services_are_hidden_on_surfaces_until_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        let mut values = BTreeMap::new();
        values.insert("PLEX_URL".to_string(), "http://127.0.0.1:32400".to_string());
        values.insert("PLEX_TOKEN".to_string(), "token".to_string());

        manager
            .set_service_config("plex", &values)
            .await
            .expect("set service config");

        assert!(!manager.surface_enabled_for_service("plex", "mcp").await);
        assert!(manager.surface_enabled_for_service("plex", "api").await);
        assert!(manager.surface_enabled_for_service("plex", "cli").await);
    }

    #[tokio::test]
    async fn enabled_virtual_server_only_exposes_enabled_surfaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        cli: false,
                        api: true,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        assert!(manager.surface_enabled_for_service("plex", "api").await);
        assert!(manager.surface_enabled_for_service("plex", "mcp").await);
        assert!(!manager.surface_enabled_for_service("plex", "cli").await);
    }

    #[test]
    fn enabled_virtual_server_reports_compiled_tool_counts() {
        let view = server_view_from_virtual_server(
            &VirtualServerConfig {
                id: "plex".to_string(),
                service: "plex".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: true,
                    api: true,
                    mcp: true,
                    webui: true,
                },
                mcp_policy: None,
            },
            UpstreamCachedSummary {
                discovered_tool_count: 5,
                exposed_tool_count: 5,
                discovered_resource_count: 0,
                exposed_resource_count: 0,
                discovered_prompt_count: 0,
                exposed_prompt_count: 0,
            },
            None,
            Some(&ServiceHealth {
                service: "plex".to_string(),
                reachable: true,
                auth_ok: true,
                latency_ms: Some(12),
                message: None,
            }),
        );

        assert!(view.discovered_tool_count > 0);
        assert_eq!(view.discovered_tool_count, view.exposed_tool_count);
        assert_eq!(view.discovered_resource_count, 0);
        assert_eq!(view.discovered_prompt_count, 0);
    }

    #[test]
    fn virtual_server_mcp_policy_reduces_exposed_tool_count() {
        let view = server_view_from_virtual_server(
            &VirtualServerConfig {
                id: "plex".to_string(),
                service: "plex".to_string(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    cli: true,
                    api: true,
                    mcp: true,
                    webui: true,
                },
                mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                    allowed_actions: vec!["server.info".to_string()],
                }),
            },
            UpstreamCachedSummary {
                discovered_tool_count: 5,
                exposed_tool_count: 3,
                discovered_resource_count: 0,
                exposed_resource_count: 0,
                discovered_prompt_count: 0,
                exposed_prompt_count: 0,
            },
            None,
            Some(&ServiceHealth {
                service: "plex".to_string(),
                reachable: true,
                auth_ok: true,
                latency_ms: Some(12),
                message: None,
            }),
        );

        assert!(view.discovered_tool_count > view.exposed_tool_count);
        assert_eq!(view.exposed_tool_count, 3);
    }

    #[tokio::test]
    async fn mcp_action_policy_restricts_actions_to_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: vec!["server.info".to_string()],
                    }),
                }],
                ..LabConfig::default()
            })
            .await;

        assert!(
            manager
                .mcp_action_allowed_for_service("plex", "server.info")
                .await
        );
        assert!(manager.mcp_action_allowed_for_service("plex", "help").await);
        assert!(
            !manager
                .mcp_action_allowed_for_service("plex", "sessions.list")
                .await
        );
    }

    #[tokio::test]
    async fn service_clients_refresh_after_service_config_update() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let shared_clients =
            SharedServiceClients::from_clients(crate::dispatch::clients::ServiceClients::default());
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default())
            .with_service_clients(shared_clients.clone());

        let mut values = BTreeMap::new();
        values.insert("PLEX_URL".to_string(), "http://127.0.0.1:32400".to_string());
        values.insert("PLEX_TOKEN".to_string(), "token".to_string());

        manager
            .set_service_config("plex", &values)
            .await
            .expect("set service config");

        assert_eq!(shared_clients.refresh_count(), 1);
    }

    #[tokio::test]
    async fn unrestricted_mcp_actions_return_none_when_no_policy_is_set() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager
            .seed_config(LabConfig {
                virtual_servers: vec![VirtualServerConfig {
                    id: "plex".to_string(),
                    service: "plex".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..LabConfig::default()
            })
            .await;

        assert_eq!(manager.allowed_mcp_actions_for_service("plex").await, None);
    }

    #[tokio::test]
    async fn synthetic_services_without_gateway_metadata_allow_mcp_actions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

        manager.seed_config(LabConfig::default()).await;

        assert!(
            manager
                .mcp_action_allowed_for_service("marketplace", "mcp.config")
                .await
        );
    }

    #[tokio::test]
    async fn runtime_view_includes_last_upstream_error() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("broken-upstream");
        let entry = crate::dispatch::upstream::types::UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            exposure_policy: crate::dispatch::upstream::types::ToolExposurePolicy::All,
            prompt_count: 0,
            resource_count: 0,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            resource_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            tool_unhealthy_since: Some(std::time::Instant::now()),
            prompt_unhealthy_since: Some(std::time::Instant::now()),
            resource_unhealthy_since: Some(std::time::Instant::now()),
            tool_last_error: Some("stdio handshake failed".to_string()),
            prompt_last_error: None,
            resource_last_error: None,
        };

        pool.insert_entry_for_tests("broken-upstream", entry).await;

        let runtime = runtime_view(Some(&pool), "broken-upstream", None).await;
        assert_eq!(
            runtime.last_error.as_deref(),
            Some("stdio handshake failed")
        );
    }

    #[tokio::test]
    async fn reload_evicts_removed_upstream_oauth_clients() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        write_gateway_config(
            &path,
            &LabConfig {
                upstream: vec![UpstreamConfig {
                    enabled: true,
                    name: "kept".to_string(),
                    url: Some("https://fixture.example.com:7001".to_string()),
                    bearer_token_env: None,
                    command: None,
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: None,
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                }],
                ..LabConfig::default()
            },
        )
        .expect("write config");

        let cache = OauthClientCache::new(Arc::new(dashmap::DashMap::new()));
        cache.insert_for_tests(
            "removed",
            "alice",
            "preregistered:client-a",
            dummy_auth_client().await,
        );

        let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
            .with_oauth_client_cache(cache.clone());
        manager
            .seed_config(LabConfig {
                upstream: vec![UpstreamConfig {
                    enabled: true,
                    name: "removed".to_string(),
                    url: Some("http://127.0.0.1:7000".to_string()),
                    bearer_token_env: None,
                    command: None,
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    proxy_resources: false,
                    proxy_prompts: false,
                    expose_tools: None,
                    expose_resources: None,
                    expose_prompts: None,
                    oauth: Some(UpstreamOauthConfig {
                        mode: UpstreamOauthMode::AuthorizationCodePkce,
                        registration: UpstreamOauthRegistration::Dynamic,
                        scopes: None,
                    }),
                    imported_from: None,
                    tool_search: ToolSearchConfig::default(),
                }],
                ..LabConfig::default()
            })
            .await;

        assert_eq!(cache.len(), 1);
        manager
            .reload_with_origin(None, None)
            .await
            .expect("reload");
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn runtime_view_preserves_non_benign_prompt_and_resource_errors() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("partial-upstream");
        let entry = crate::dispatch::upstream::types::UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            exposure_policy: crate::dispatch::upstream::types::ToolExposurePolicy::All,
            prompt_count: 3,
            resource_count: 2,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
            prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            resource_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            tool_unhealthy_since: None,
            prompt_unhealthy_since: Some(std::time::Instant::now()),
            resource_unhealthy_since: Some(std::time::Instant::now()),
            tool_last_error: None,
            prompt_last_error: Some("prompt listing unsupported".to_string()),
            resource_last_error: Some("resource listing unsupported".to_string()),
        };

        pool.insert_entry_for_tests("partial-upstream", entry).await;

        let runtime = runtime_view(Some(&pool), "partial-upstream", None).await;
        assert_eq!(
            runtime.last_error.as_deref(),
            Some("resource listing unsupported")
        );

        let server = server_view_from_upstream(
            Some(&pool),
            &UpstreamConfig {
                enabled: true,
                name: "partial-upstream".to_string(),
                url: Some("http://127.0.0.1:8080/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            },
        )
        .await;

        assert_eq!(server.warnings.len(), 1);
        assert_eq!(server.warnings[0].message, "resource listing unsupported");
    }

    #[tokio::test]
    async fn runtime_view_ignores_method_not_found_capability_errors() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("partial-upstream");
        let entry = crate::dispatch::upstream::types::UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            exposure_policy: crate::dispatch::upstream::types::ToolExposurePolicy::All,
            prompt_count: 1,
            resource_count: 1,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
            prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            resource_health: crate::dispatch::upstream::types::UpstreamHealth::Unhealthy {
                consecutive_failures: 1,
            },
            tool_unhealthy_since: None,
            prompt_unhealthy_since: Some(std::time::Instant::now()),
            resource_unhealthy_since: Some(std::time::Instant::now()),
            tool_last_error: None,
            prompt_last_error: Some(
                "failed to list prompts from upstream: Mcp error: -32601: Method not found"
                    .to_string(),
            ),
            resource_last_error: Some(
                "failed to list resources from upstream: Mcp error: -32601: Method not found"
                    .to_string(),
            ),
        };

        pool.insert_entry_for_tests("partial-upstream", entry).await;

        let runtime = runtime_view(Some(&pool), "partial-upstream", None).await;
        assert_eq!(runtime.last_error, None);

        let server = server_view_from_upstream(
            Some(&pool),
            &UpstreamConfig {
                enabled: true,
                name: "partial-upstream".to_string(),
                url: Some("http://127.0.0.1:8080/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                tool_search: ToolSearchConfig::default(),
            },
        )
        .await;

        assert!(server.warnings.is_empty());
    }

    #[tokio::test]
    async fn custom_gateway_connected_includes_resources_and_prompts() {
        let pool = UpstreamPool::new();
        let upstream = UpstreamConfig {
            enabled: true,
            name: "partial-upstream".to_string(),
            url: Some("http://127.0.0.1:9001/mcp".to_string()),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            imported_from: None,
            tool_search: ToolSearchConfig::default(),
        };
        let upstream_name: Arc<str> = Arc::from("partial-upstream");
        let entry = crate::dispatch::upstream::types::UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            exposure_policy: crate::dispatch::upstream::types::ToolExposurePolicy::All,
            prompt_count: 4,
            resource_count: 2,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
            prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
            resource_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        };

        pool.insert_entry_for_tests("partial-upstream", entry).await;

        let view = server_view_from_upstream(Some(&pool), &upstream).await;
        assert!(view.connected);
        assert!(view.warnings.is_empty());
        assert_eq!(view.exposed_resource_count, 2);
        assert_eq!(view.exposed_prompt_count, 4);
    }

    #[test]
    fn observability_source_covers_gateway_manager_reconcile_events() {
        let source = include_str!("manager.rs");
        for expected in [
            "event = \"install.start\"",
            "event = \"remove.finish\"",
            "event = \"catalog.refresh.finish\"",
            "before_tool_count",
            "after_tool_count",
            "event = \"old_pool.drain.start\"",
            "action = \"tool_search.rebuild\"",
            "action = \"tool_search.reprobe\"",
            "event = \"health.schedule\"",
            "operation = \"health\"",
            "kind = \"upstream_reprobe_failed\"",
        ] {
            assert!(
                source.contains(expected),
                "missing gateway manager observability field `{expected}`"
            );
        }
    }
}
