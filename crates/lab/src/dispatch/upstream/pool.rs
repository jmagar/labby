//! `UpstreamPool` — manages connections to upstream MCP servers.
//!
//! Connects to configured upstreams via HTTP (`StreamableHttpClientTransport`)
//! or stdio (child process), discovers their tools, and caches schemas.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use rmcp::RoleClient;
use rmcp::model::{GetPromptRequestParams, GetPromptResult, Prompt};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::UpstreamConfig;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::registry::RegisteredService;

use super::types::{
    UpstreamCapability, UpstreamEntry, UpstreamRuntimeMetadata, UpstreamRuntimeOwner,
};

mod capability;
mod connect;
mod connect_stdio;
mod connection;
mod discover;
mod ensure;
mod entries;
mod health;
mod helpers;
mod lifecycle;
mod logging;
mod probe;
mod registration;
mod resources_list;
mod resources_read;
#[cfg(test)]
mod testsupport;
mod tools;
mod tools_call;
mod validate;

pub(crate) use helpers::redact_resource_uri_for_logging;
pub use helpers::{UpstreamCachedSummary, in_process_upstream_name};
// Leaf helpers used unqualified throughout the residual pool module and its
// descendants. Glob-importing the child's `pub(super)` items keeps existing
// call sites unchanged while the bodies live in the child modules.
use connect::*;
use discover::*;
use helpers::*;
use logging::*;

/// Upstream connection pool — holds live connections and discovered tool catalogs.
#[derive(Clone)]
pub struct UpstreamPool {
    /// Discovered upstream state, keyed by upstream name.
    catalog: Arc<RwLock<HashMap<String, UpstreamEntry>>>,
    /// Live client connections, keyed by upstream name.
    /// Each is an `Arc<Peer<RoleClient>>` that can `call_tool` / `list_tools`.
    connections: Arc<RwLock<HashMap<String, UpstreamConnection>>>,
    /// Names of upstreams that have `proxy_resources=true`.
    resource_upstreams: Arc<RwLock<Vec<String>>>,
    /// Per-upstream OAuth managers, keyed by upstream name.
    /// `None` when the server was started without OAuth support.
    oauth_client_cache: Option<OauthClientCache>,
    /// Background reprobe task cancellation tokens, keyed by upstream name.
    probe_tasks: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Per-upstream lazy connection gates to prevent duplicate cold starts.
    lazy_connect_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    /// Request/session identity stamped onto spawned stdio upstreams.
    runtime_origin: Option<String>,
    /// Structured owner metadata stamped onto spawned stdio upstreams.
    runtime_owner: Option<UpstreamRuntimeOwner>,
    /// Maximum time to wait for an upstream tool/resource/prompt response.
    request_timeout: Duration,
}

/// A live connection to an upstream MCP server.
struct UpstreamConnection {
    /// The running client service handle — kept alive to maintain the connection.
    _client_service: rmcp::service::RunningService<RoleClient, ()>,
    /// Background task holding an in-process server alive when applicable.
    _server_task: Option<tokio::task::JoinHandle<()>>,
    /// The peer handle for making requests.
    peer: rmcp::service::Peer<RoleClient>,
    /// Runtime metadata for process-backed upstreams.
    runtime: UpstreamRuntimeMetadata,
}

struct InProcessRegistration {
    connection: Option<UpstreamConnection>,
    tools: Vec<rmcp::model::Tool>,
    entry_name: Arc<str>,
    upstream_name: String,
}

type InProcessConnector = Arc<
    dyn Fn(RegisteredService) -> BoxFuture<'static, anyhow::Result<InProcessRegistration>>
        + Send
        + Sync,
>;

#[cfg(test)]
type TestUpstreamConnector = Arc<
    dyn Fn(
            UpstreamConfig,
        ) -> BoxFuture<
            'static,
            anyhow::Result<(Option<UpstreamConnection>, Vec<rmcp::model::Tool>)>,
        > + Send
        + Sync,
>;

impl UpstreamPool {
    /// Create a new empty pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            catalog: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            resource_upstreams: Arc::new(RwLock::new(Vec::new())),
            oauth_client_cache: None,
            probe_tasks: Arc::new(RwLock::new(HashMap::new())),
            lazy_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            runtime_origin: None,
            runtime_owner: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Attach the per-`(upstream, subject)` OAuth client cache so the pool can
    /// authenticate OAuth-protected upstreams.
    ///
    /// Must be called before `discover_all` for OAuth upstreams to connect successfully.
    #[must_use]
    pub fn with_oauth_client_cache(mut self, cache: OauthClientCache) -> Self {
        self.oauth_client_cache = Some(cache);
        self
    }

    #[must_use]
    pub fn with_runtime_origin(mut self, origin: Option<String>) -> Self {
        self.runtime_origin = origin;
        self
    }

    #[must_use]
    pub fn with_runtime_owner(mut self, owner: Option<UpstreamRuntimeOwner>) -> Self {
        self.runtime_owner = owner;
        self
    }

    #[cfg(test)]
    pub(super) fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Fetch prompts from all healthy upstreams and merge them, returning both the
    /// deduplicated prompt list and the ownership map (prompt_name -> upstream_name).
    ///
    /// This is the single RPC pass shared by all prompt-related queries.
    async fn collect_upstream_prompts(
        &self,
        builtin_names: &[&str],
    ) -> (Vec<Prompt>, HashMap<String, String>) {
        let peers = routable_upstream_peers(self, UpstreamCapability::Prompts).await;

        // Issue RPCs in parallel. merge_upstream_prompts sorts internally,
        // so completion order does not affect the final result.
        let mut futures = FuturesUnordered::new();
        for (name, peer) in peers {
            futures.push(async move {
                let result = peer.list_prompts(None).await;
                (name, result)
            });
        }

        let mut upstream_prompts = Vec::new();
        let mut prompt_name_updates: HashMap<String, Vec<String>> = HashMap::new();
        while let Some((name, result)) = futures.next().await {
            match result {
                Ok(result) => {
                    self.record_success_for(&name, UpstreamCapability::Prompts)
                        .await;
                    prompt_name_updates.insert(name.clone(), Vec::new());
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = result.prompts.len();
                        }
                    }
                    upstream_prompts.push((name, result.prompts));
                }
                Err(e) => {
                    self.record_failure_for(
                        &name,
                        UpstreamCapability::Prompts,
                        format!("failed to list prompts from upstream: {e}"),
                    )
                    .await;
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = 0;
                        }
                    }
                    tracing::warn!(
                        upstream = %name,
                        error = %e,
                        "failed to list prompts from upstream"
                    );
                }
            }
        }

        let (prompts, owners) = merge_upstream_prompts(builtin_names, upstream_prompts);
        if !prompt_name_updates.is_empty() {
            for prompt in &prompts {
                if let Some(upstream_name) = owners.get(prompt.name.as_str())
                    && let Some(names) = prompt_name_updates.get_mut(upstream_name)
                {
                    names.push(prompt.name.to_string());
                }
            }
            let mut catalog = self.catalog.write().await;
            for (upstream_name, names) in prompt_name_updates {
                if let Some(entry) = catalog.get_mut(&upstream_name) {
                    entry.prompt_names = names;
                }
            }
        }

        (prompts, owners)
    }

    /// List prompts from all healthy upstreams, filtering built-in and cross-upstream collisions.
    pub async fn list_upstream_prompts(&self, builtin_names: &[&str]) -> Vec<Prompt> {
        let (prompts, _) = self.collect_upstream_prompts(builtin_names).await;
        prompts
    }

    /// Return cached prompt names from all upstreams, excluding any that clash with builtins.
    pub async fn cached_upstream_prompt_names(&self, builtins: &[&str]) -> Vec<String> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .flat_map(|entry| entry.prompt_names.iter().cloned())
            .filter(|name| !builtins.contains(&name.as_str()))
            .collect()
    }

    async fn cached_prompt_owner(
        &self,
        prompt_name: &str,
        require_routable: bool,
    ) -> Option<String> {
        let catalog = self.catalog.read().await;
        let mut entries = catalog.values().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        entries.into_iter().find_map(|entry| {
            if require_routable && !entry.prompt_health.is_routable() {
                return None;
            }
            entry
                .prompt_names
                .iter()
                .any(|name| name == prompt_name)
                .then(|| entry.name.to_string())
        })
    }

    pub async fn subject_scoped_prompts(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        builtin_names: &[&str],
    ) -> Vec<Prompt> {
        let mut futures = FuturesUnordered::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let oauth_client_cache = oauth_client_cache.clone();
            futures.push(async move {
                let result = connect_upstream(
                    &config,
                    Some(subject.as_str()),
                    oauth_client_cache.as_ref(),
                    None,
                    None,
                )
                .await
                .map(|(conn, _)| conn);
                (config.name.clone(), result)
            });
        }

        let mut upstream_prompts = Vec::new();
        while let Some((name, result)) = futures.next().await {
            let Ok(conn) = result else {
                continue;
            };
            match conn.peer.list_prompts(None).await {
                Ok(result) => upstream_prompts.push((name, result.prompts)),
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream prompt discovery failed"
                    );
                }
            }
        }

        let (prompts, _) = merge_upstream_prompts(builtin_names, upstream_prompts);
        prompts
    }

    /// Build prompt ownership map: prompt_name -> upstream_name.
    ///
    /// Makes M RPCs (one per healthy upstream), not M*N. Use this when you need
    /// to look up ownership for multiple prompts.
    pub async fn prompt_ownership_map(&self, builtin_names: &[&str]) -> HashMap<String, String> {
        let (_, owners) = self.collect_upstream_prompts(builtin_names).await;
        owners
    }

    /// Resolve which upstream owns a given prompt name.
    ///
    /// Prefer `prompt_ownership_map()` when resolving ownership for multiple
    /// prompts to avoid an N+1 RPC pattern.
    pub async fn find_prompt_owner(&self, prompt_name: &str) -> Option<String> {
        if let Some(owner) = self.cached_prompt_owner(prompt_name, true).await {
            return Some(owner);
        }

        let (_, owners) = self.collect_upstream_prompts(&[]).await;
        if let Some(owner) = owners.get(prompt_name) {
            return Some(owner.clone());
        }

        self.cached_prompt_owner(prompt_name, false).await
    }

    pub async fn subject_scoped_prompt_owner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        prompt_name: &str,
    ) -> Option<String> {
        let mut futures = FuturesUnordered::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let oauth_client_cache = oauth_client_cache.clone();
            let target_prompt = prompt_name.to_string();
            futures.push(async move {
                let result = connect_upstream(
                    &config,
                    Some(subject.as_str()),
                    oauth_client_cache.as_ref(),
                    None,
                    None,
                )
                .await
                .map(|(conn, _)| conn);
                (config.name.clone(), target_prompt, result)
            });
        }

        while let Some((name, target_prompt, result)) = futures.next().await {
            let Ok(conn) = result else {
                continue;
            };
            if let Ok(result) = conn.peer.list_prompts(None).await
                && result
                    .prompts
                    .iter()
                    .any(|prompt| prompt.name == target_prompt)
            {
                return Some(name);
            }
        }
        None
    }

    /// Proxy a get-prompt request to a specific upstream.
    pub async fn get_prompt(
        &self,
        upstream_name: &str,
        params: GetPromptRequestParams,
    ) -> Option<Result<GetPromptResult, String>> {
        let start = Instant::now();
        let prompt_name = params.name.to_string();
        let event = UpstreamRequestLog::prompt(upstream_name, &prompt_name, false);
        let peer = self
            .acquire_peer(upstream_name, UpstreamCapability::Prompts, "prompt.get")
            .await?;

        log_upstream_request_start(event);

        match tokio::time::timeout(self.request_timeout, peer.get_prompt(params)).await {
            Ok(Ok(result)) => {
                self.record_success_for(upstream_name, UpstreamCapability::Prompts)
                    .await;
                log_upstream_request_finish(event, start.elapsed().as_millis(), None);
                Some(Ok(result))
            }
            Ok(Err(e)) => {
                self.record_failure_for(
                    upstream_name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt get failed: {e}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_error",
                    Some(&e),
                    None,
                    None,
                );
                Some(Err(format!("upstream prompt get failed: {e}")))
            }
            Err(_) => {
                let message = format!(
                    "upstream prompt get timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                self.record_failure_for(
                    upstream_name,
                    UpstreamCapability::Prompts,
                    message.clone(),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "timeout",
                    None,
                    None,
                    None,
                );
                Some(Err(message))
            }
        }
    }

    pub async fn subject_scoped_get_prompt(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        params: GetPromptRequestParams,
    ) -> Result<GetPromptResult, String> {
        let start = Instant::now();
        let prompt_name = params.name.to_string();
        let event = UpstreamRequestLog::prompt(&config.name, &prompt_name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let (conn, _) = match connect_upstream(
            config,
            Some(subject),
            self.oauth_client_cache.as_ref(),
            None,
            None,
        )
        .await
        {
            Ok(conn) => conn,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt connect failed: {error}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_connect_error",
                    Some(&error),
                    None,
                    None,
                );
                return Err(error.to_string());
            }
        };
        match tokio::time::timeout(self.request_timeout, conn.peer.get_prompt(params)).await {
            Ok(Ok(result)) => {
                self.record_success_for(&config.name, UpstreamCapability::Prompts)
                    .await;
                log_upstream_request_finish(event, start.elapsed().as_millis(), None);
                Ok(result)
            }
            Ok(Err(error)) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Prompts,
                    format!("upstream prompt get failed: {error}"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "upstream_error",
                    Some(&error),
                    None,
                    None,
                );
                Err(format!("upstream prompt get failed: {error}"))
            }
            Err(_) => {
                let message = format!(
                    "upstream prompt get timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                self.record_failure_for(&config.name, UpstreamCapability::Prompts, message.clone())
                    .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "timeout",
                    None,
                    None,
                    None,
                );
                Err(message)
            }
        }
    }
}

impl Default for UpstreamPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::types;
    use super::testsupport::*;
    use super::*;
    use crate::mcp::logging::logging_level_rank;
    use crate::mcp::server::LabMcpServer;
    use crate::registry::ToolRegistry;
    use rmcp::model::LoggingLevel;
    use std::sync::atomic::{AtomicU8, Ordering};

    #[test]
    fn merge_upstream_prompts_is_deterministic() {
        let left = Prompt::new("shared", Some("left"), None);
        let right = Prompt::new("shared", Some("right"), None);
        let left_only = Prompt::new("left-only", Some("left-only"), None);
        let right_only = Prompt::new("right-only", Some("right-only"), None);

        let (prompts, owners) = merge_upstream_prompts(
            &["builtin"],
            vec![
                ("zeta".into(), vec![right.clone(), right_only]),
                ("alpha".into(), vec![left.clone(), left_only]),
            ],
        );

        let names: Vec<_> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        assert_eq!(names, vec!["shared", "left-only", "right-only"]);
        assert_eq!(owners.get("shared").map(String::as_str), Some("alpha"));
        assert_eq!(owners.get("left-only").map(String::as_str), Some("alpha"));
        assert_eq!(owners.get("right-only").map(String::as_str), Some("zeta"));
    }

    #[tokio::test]
    async fn successful_prompt_listing_populates_snapshot_cache() {
        let pool = static_catalog_pool("static").await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        let prompt_names: Vec<&str> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        assert_eq!(
            prompt_names,
            vec!["upstream.prompt.one", "upstream.prompt.two"]
        );
        assert_eq!(
            pool.cached_upstream_prompt_names(&[]).await,
            vec![
                "upstream.prompt.one".to_string(),
                "upstream.prompt.two".to_string()
            ]
        );

        let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
        runtime.swap(Some(Arc::clone(&pool))).await;
        let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ));
        let server = LabMcpServer {
            registry: Arc::new(ToolRegistry::new()),
            gateway_manager: Some(manager),
            node_role: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Info))),
        };

        let snapshot = server.snapshot_catalog().await;
        assert!(snapshot.prompts.contains("upstream.prompt.one"));
        assert!(snapshot.prompts.contains("upstream.prompt.two"));
    }

    #[tokio::test]
    async fn prompt_owner_lookup_uses_cache_without_listing_upstreams() {
        let server = StaticCatalogServer::default();
        let list_prompts_count = Arc::clone(&server.list_prompts_count);
        let get_prompt_count = Arc::clone(&server.get_prompt_count);
        let pool = static_catalog_pool_with_server("static", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 2);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        let owner = pool.find_prompt_owner("upstream.prompt.one").await;
        assert_eq!(owner.as_deref(), Some("static"));
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        let result = pool
            .get_prompt("static", GetPromptRequestParams::new("upstream.prompt.one"))
            .await
            .expect("upstream remains connected")
            .expect("prompt get succeeds");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(get_prompt_count.load(Ordering::SeqCst), 1);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prompt_owner_lookup_falls_back_to_stale_cache_after_listing_miss() {
        let server = StaticCatalogServer::default();
        let list_prompts_count = Arc::clone(&server.list_prompts_count);
        let fail_list_prompts = Arc::clone(&server.fail_list_prompts);
        let pool = static_catalog_pool_with_server("static", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 2);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        for _ in 0..types::CIRCUIT_BREAKER_THRESHOLD {
            pool.record_failure_for(
                "static",
                UpstreamCapability::Prompts,
                "prompt listing failed for test",
            )
            .await;
        }
        fail_list_prompts.store(true, Ordering::SeqCst);

        let owner = pool.find_prompt_owner("upstream.prompt.one").await;
        assert_eq!(owner.as_deref(), Some("static"));
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            pool.cached_upstream_prompt_names(&[]).await,
            vec![
                "upstream.prompt.one".to_string(),
                "upstream.prompt.two".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn get_prompt_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .get_prompt("slow", GetPromptRequestParams::new("slow.prompt"))
            .await
            .expect("upstream is connected")
            .expect_err("slow prompt get should time out");

        assert!(result.contains("timed out"));
    }
}
