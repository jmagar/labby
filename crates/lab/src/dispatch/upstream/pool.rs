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
use rmcp::model::{
    AnnotateAble, CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult,
    Prompt, RawResource, ReadResourceResult, Resource,
};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::UpstreamConfig;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::registry::{RegisteredService, ToolRegistry};

use super::transport::websocket::{jitter_delay, reprobe_backoff};
use super::types;
use super::types::{
    UpstreamCapability, UpstreamEntry, UpstreamHealth, UpstreamRuntimeMetadata,
    UpstreamRuntimeOwner, UpstreamTool, UpstreamToolExposureRow,
};

mod capability;
mod connect;
mod connect_stdio;
mod connection;
mod discover;
mod entries;
mod health;
mod helpers;
mod lifecycle;
mod logging;
#[cfg(test)]
mod testsupport;
mod validate;

pub(crate) use helpers::redact_resource_uri_for_logging;
pub use helpers::{UpstreamCachedSummary, in_process_upstream_name};
// Leaf helpers used unqualified throughout the residual pool module and its
// descendants. Glob-importing the child's `pub(super)` items keeps existing
// call sites unchanged while the bodies live in the child modules.
use connect::*;
use connect_stdio::*;
use discover::*;
use entries::*;
use helpers::*;
use logging::*;
use validate::*;

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

    fn ensure_probe_task(&self, config: UpstreamConfig) {
        if config.oauth.is_some() {
            return;
        }

        let pool = self.clone();
        tokio::spawn(async move {
            let mut tasks = pool.probe_tasks.write().await;
            if tasks.contains_key(&config.name) {
                return;
            }
            let cancel = CancellationToken::new();
            tasks.insert(config.name.clone(), cancel.clone());
            drop(tasks);
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "scheduled",
                operation = "health",
                upstream = %config.name,
                transport = upstream_transport(&config),
                "upstream reprobe scheduled"
            );

            let mut attempt = 0_u32;
            loop {
                let base = reprobe_backoff(attempt);
                let sleep_for = if attempt == 0 {
                    types::REPROBE_INTERVAL
                } else {
                    jitter_delay(base, stable_jitter_seed(&config.name, attempt))
                };
                tracing::debug!(
                    surface = "dispatch",
                    service = "upstream.pool",
                    action = "upstream.reprobe",
                    event = "sleep",
                    operation = "health",
                    upstream = %config.name,
                    transport = upstream_transport(&config),
                    attempt,
                    sleep_ms = sleep_for.as_millis(),
                    "upstream reprobe sleep scheduled"
                );
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "cancelled",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            "upstream reprobe cancelled"
                        );
                        break;
                    },
                    _ = tokio::time::sleep(sleep_for) => {}
                }

                let reprobe_started = Instant::now();
                match pool.reprobe_upstream(&config).await {
                    Ok(true) => {
                        tracing::info!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "finish",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            changed = true,
                            "upstream reprobe succeeded"
                        );
                        attempt = 0;
                    }
                    Ok(false) => {
                        tracing::debug!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "finish",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            changed = false,
                            "upstream reprobe skipped"
                        );
                    }
                    Err(error) => {
                        let kind = classify_upstream_error(&error.to_string());
                        attempt = attempt.saturating_add(1);
                        if auth_error_should_backoff_aggressively(kind) {
                            attempt = attempt.max(AUTH_FAILURE_REPROBE_ATTEMPT_FLOOR);
                        }
                        tracing::warn!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "error",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            kind,
                            error = %error,
                            "upstream reprobe failed"
                        );
                    }
                }
            }
        });
    }

    async fn reprobe_upstream(&self, config: &UpstreamConfig) -> anyhow::Result<bool> {
        let started = Instant::now();
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "start",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(config),
            "upstream reprobe start"
        );
        let existing_peer = {
            let connections = self.connections.read().await;
            connections
                .get(&config.name)
                .map(|connection| connection.peer.clone())
        };

        if let Some(peer) = existing_peer {
            match tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_all_tools()).await {
                Ok(Ok(tools)) => {
                    self.replace_catalog_tools(config, tools).await;
                    self.record_success_for(&config.name, UpstreamCapability::Tools)
                        .await;
                    tracing::info!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.finish",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream heartbeat succeeded"
                    );
                    return Ok(true);
                }
                Ok(Err(error)) => {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        format!("upstream heartbeat failed: {error}"),
                    )
                    .await;
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.error",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        kind = "upstream_heartbeat_failed",
                        error = %error,
                        "upstream heartbeat failed"
                    );
                }
                Err(_) => {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        "upstream heartbeat timed out",
                    )
                    .await;
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.error",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        kind = "timeout",
                        timeout_secs = DISCOVERY_TIMEOUT.as_secs(),
                        "upstream heartbeat timed out"
                    );
                }
            }
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "empty",
                operation = "health",
                upstream = %config.name,
                transport = upstream_transport(config),
                elapsed_ms = started.elapsed().as_millis(),
                kind = "upstream_not_connected",
                "upstream reprobe found no existing connection"
            );
        }

        let stale_connection = {
            let mut connections = self.connections.write().await;
            connections.remove(&config.name)
        };
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.reprobe.reconnect")
                .await;
        }

        let (conn, tools) = connect_upstream(
            config,
            None,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
        )
        .await?;
        {
            let mut connections = self.connections.write().await;
            connections.insert(config.name.clone(), conn);
        }
        self.replace_catalog_tools(config, tools).await;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "reconnect.finish",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(config),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream reprobe reconnect succeeded"
        );
        Ok(true)
    }

    /// Seed the upstream catalog from config without starting any upstream runtime.
    pub async fn seed_lazy_upstreams(&self, configs: &[UpstreamConfig]) {
        let mut catalog = self.catalog.write().await;
        let mut resource_names = Vec::new();
        let mut processed_names = std::collections::HashSet::new();

        for config in configs {
            if !config.enabled {
                continue;
            }
            if !processed_names.insert(&config.name) {
                continue;
            }
            if !upstream_name_is_uri_safe(&config.name) {
                tracing::warn!(
                    upstream = %config.name,
                    "upstream name contains URI-unsafe characters (/, ?, #) — skipping"
                );
                continue;
            }
            if let Err(msg) = validate_upstream_config(config) {
                tracing::warn!(
                    upstream = %config.name,
                    "skipping upstream: {msg}"
                );
                continue;
            }

            catalog
                .entry(config.name.clone())
                .or_insert_with(|| lazy_upstream_entry(config, Arc::from(config.name.as_str())));

            if config.proxy_resources {
                resource_names.push(config.name.clone());
            }
        }

        resource_names.sort_unstable();
        resource_names.dedup();
        *self.resource_upstreams.write().await = resource_names;
    }

    async fn ensure_lazy_upstream_entry(&self, config: &UpstreamConfig) {
        if !config.enabled {
            return;
        }
        if !upstream_name_is_uri_safe(&config.name) {
            tracing::warn!(
                upstream = %config.name,
                "upstream name contains URI-unsafe characters (/, ?, #) — skipping"
            );
            return;
        }
        if let Err(msg) = validate_upstream_config(config) {
            tracing::warn!(
                upstream = %config.name,
                "skipping upstream: {msg}"
            );
            return;
        }
        self.catalog
            .write()
            .await
            .entry(config.name.clone())
            .or_insert_with(|| lazy_upstream_entry(config, Arc::from(config.name.as_str())));
        if config.proxy_resources {
            let mut resource_upstreams = self.resource_upstreams.write().await;
            if !resource_upstreams.iter().any(|name| name == &config.name) {
                resource_upstreams.push(config.name.clone());
                resource_upstreams.sort_unstable();
            }
        }
    }

    /// Ensure one upstream has discovered tools, connecting it lazily when needed.
    pub async fn ensure_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        runtime_owner: Option<&UpstreamRuntimeOwner>,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }

        let connect_lock = self.lazy_connect_lock(&config.name).await;
        let _connect_guard = connect_lock.lock().await;
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }

        self.ensure_lazy_upstream_entry(config).await;
        let stale_connection = {
            let mut connections = self.connections.write().await;
            connections.remove(&config.name)
        };
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.lazy.ensure.before_connect")
                .await;
        }

        let started = Instant::now();
        let subject = config.oauth.as_ref().and(oauth_subject);
        let runtime_owner = runtime_owner.or(self.runtime_owner.as_ref());
        let connect_result = tokio::time::timeout(
            DISCOVERY_TIMEOUT,
            connect_upstream(
                config,
                subject,
                self.oauth_client_cache.as_ref(),
                self.runtime_origin.as_deref(),
                runtime_owner,
            ),
        )
        .await;
        let (conn, tools) = match connect_result {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("lazy upstream connect failed: {error}"),
                )
                .await;
                return Err(error);
            }
            Err(_) => {
                let error = anyhow::anyhow!(
                    "lazy upstream connect timed out after {}s waiting for {} MCP list_tools response from {}",
                    DISCOVERY_TIMEOUT.as_secs(),
                    upstream_transport(config),
                    upstream_target_redacted(config)
                );
                self.record_failure_for(&config.name, UpstreamCapability::Tools, error.to_string())
                    .await;
                return Err(error);
            }
        };
        let tool_count = tools.len();
        self.connections
            .write()
            .await
            .insert(config.name.clone(), conn);
        self.replace_catalog_tools(config, tools).await;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.lazy.ensure",
            event = "finish",
            operation = "connection.acquire",
            upstream = %config.name,
            tool_count,
            elapsed_ms = started.elapsed().as_millis(),
            "lazy upstream tools connected"
        );
        Ok(true)
    }

    #[cfg(test)]
    async fn ensure_tools_for_upstream_with_connector(
        &self,
        config: &UpstreamConfig,
        _oauth_subject: Option<&str>,
        connector: TestUpstreamConnector,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }

        let connect_lock = self.lazy_connect_lock(&config.name).await;
        let _connect_guard = connect_lock.lock().await;
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }

        self.ensure_lazy_upstream_entry(config).await;
        let stale_connection = {
            let mut connections = self.connections.write().await;
            connections.remove(&config.name)
        };
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.lazy.ensure.before_connect")
                .await;
        }

        let (connection, tools) = connector(config.clone()).await?;
        if let Some(connection) = connection {
            self.connections
                .write()
                .await
                .insert(config.name.clone(), connection);
        }
        self.replace_catalog_tools(config, tools).await;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        Ok(true)
    }

    #[cfg(test)]
    pub async fn install_test_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
        tools: Vec<rmcp::model::Tool>,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }
        self.ensure_lazy_upstream_entry(config).await;
        self.replace_catalog_tools(config, tools).await;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        Ok(true)
    }

    async fn lazy_connect_lock(&self, upstream_name: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self
            .lazy_connect_locks
            .read()
            .await
            .get(upstream_name)
            .cloned()
        {
            return lock;
        }
        let mut locks = self.lazy_connect_locks.write().await;
        locks
            .entry(upstream_name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn reprobe_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "skipped",
                operation = "health",
                upstream = %config.name,
                reason = "disabled",
                "upstream reprobe skipped"
            );
            return Ok(false);
        }
        self.reprobe_upstream(config).await
    }

    async fn replace_catalog_tools(&self, config: &UpstreamConfig, tools: Vec<rmcp::model::Tool>) {
        let exposure_policy = resolve_exposure_policy(&config.name, config.expose_tools.clone());
        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let tools = tools
            .into_iter()
            .map(|tool| cached_upstream_tool(tool, &upstream_name))
            .collect::<HashMap<_, _>>();

        let mut catalog = self.catalog.write().await;
        if let Some(entry) = catalog.get_mut(&config.name) {
            entry.tools = tools;
            entry.exposure_policy = exposure_policy;
        }
    }

    pub async fn register_in_process_service_peers(&self, registry: &ToolRegistry) {
        let services: Vec<RegisteredService> = registry
            .services()
            .iter()
            .filter(|service| !service.actions.is_empty())
            .cloned()
            .collect();
        self.register_in_process_service_list(services).await;
    }

    async fn register_in_process_service_list(&self, services: Vec<RegisteredService>) {
        let connector: InProcessConnector = Arc::new(|service| {
            Box::pin(async move {
                let upstream_name = in_process_upstream_name(service.name);
                let entry_name: Arc<str> = Arc::from(upstream_name.as_str());
                let (conn, tools) = connect_in_process_service_peer(&service).await?;
                Ok(InProcessRegistration {
                    connection: Some(conn),
                    tools,
                    entry_name,
                    upstream_name,
                })
            })
        });
        self.register_in_process_service_list_with_connector(services, connector)
            .await;
    }

    async fn register_in_process_service_list_with_connector(
        &self,
        services: Vec<RegisteredService>,
        connector: InProcessConnector,
    ) {
        let mut in_process_resource_names = Vec::new();
        let mut futures = FuturesUnordered::new();
        let mut failed_count = 0usize;
        let mut timeout_count = 0usize;

        for service in services {
            let upstream_name = in_process_upstream_name(service.name);
            tracing::info!(
                upstream = %upstream_name,
                service = service.name,
                timeout_secs = IN_PROCESS_DISCOVERY_TIMEOUT.as_secs(),
                "starting in-process peer registration"
            );
            let connector = Arc::clone(&connector);
            futures.push(async move {
                let service_name = service.name;
                let result =
                    tokio::time::timeout(IN_PROCESS_DISCOVERY_TIMEOUT, connector(service)).await;
                (service_name, upstream_name, result)
            });
        }

        while let Some((service_name, upstream_name, result)) = futures.next().await {
            match result {
                Ok(Ok(registration)) => {
                    let mut tool_map = HashMap::new();
                    let tool_count = registration.tools.len();
                    for tool in registration.tools {
                        tool_map.insert(
                            tool.name.to_string(),
                            cached_upstream_tool(tool, &registration.entry_name).1,
                        );
                    }

                    self.catalog.write().await.insert(
                        registration.upstream_name.clone(),
                        healthy_in_process_entry(Arc::clone(&registration.entry_name), tool_map),
                    );
                    if let Some(conn) = registration.connection {
                        self.connections
                            .write()
                            .await
                            .insert(registration.upstream_name.clone(), conn);
                    }
                    in_process_resource_names.push(registration.upstream_name.clone());
                    tracing::info!(
                        upstream = %registration.entry_name,
                        service = service_name,
                        tool_count,
                        resource_count = 0,
                        prompt_count = 0,
                        "in-process peer registration succeeded"
                    );
                }
                Ok(Err(error)) => {
                    failed_count += 1;
                    let error_message =
                        format!("failed to register in-process service peer: {error}");
                    tracing::warn!(
                        upstream = %upstream_name,
                        service = service_name,
                        error = %error_message,
                        "in-process peer registration failed"
                    );
                    let mut catalog = self.catalog.write().await;
                    let name: Arc<str> = Arc::from(upstream_name.as_str());
                    let entry = catalog
                        .remove(&upstream_name)
                        .map(|existing| {
                            failed_in_process_entry_from_existing(existing, error_message.clone())
                        })
                        .unwrap_or_else(|| failed_in_process_entry(name, error_message));
                    catalog.insert(upstream_name, entry);
                }
                Err(_) => {
                    failed_count += 1;
                    timeout_count += 1;
                    let error_message = format!(
                        "in-process peer registration timed out after {}s",
                        IN_PROCESS_DISCOVERY_TIMEOUT.as_secs()
                    );
                    tracing::warn!(
                        upstream = %upstream_name,
                        service = service_name,
                        timeout_secs = IN_PROCESS_DISCOVERY_TIMEOUT.as_secs(),
                        error = %error_message,
                        "in-process peer registration timed out"
                    );
                    let mut catalog = self.catalog.write().await;
                    let name: Arc<str> = Arc::from(upstream_name.as_str());
                    let entry = catalog
                        .remove(&upstream_name)
                        .map(|existing| {
                            failed_in_process_entry_from_existing(existing, error_message.clone())
                        })
                        .unwrap_or_else(|| failed_in_process_entry(name, error_message));
                    catalog.insert(upstream_name, entry);
                }
            }
        }

        if !in_process_resource_names.is_empty() {
            let mut resource_upstreams = self.resource_upstreams.write().await;
            resource_upstreams.extend(in_process_resource_names);
            resource_upstreams.sort_unstable();
            resource_upstreams.dedup();
        }

        if failed_count > 0 {
            tracing::warn!(
                failed_count,
                timeout_count,
                "in-process peer registration completed with degraded services"
            );
        }
    }

    /// Get all healthy upstream tools.
    pub async fn healthy_tools(&self) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .filter(|entry| entry.tool_health.is_routable())
            .flat_map(|entry| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
            .collect()
    }

    pub async fn healthy_tools_for_upstream(&self, upstream: &str) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        catalog
            .get(upstream)
            .into_iter()
            .filter(|entry| entry.tool_health.is_routable())
            .flat_map(|entry| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
            .collect()
    }

    async fn has_healthy_tools_for_upstream(&self, upstream: &str) -> bool {
        let catalog = self.catalog.read().await;
        catalog.get(upstream).is_some_and(|entry| {
            entry.tool_health.is_routable()
                && entry
                    .tools
                    .values()
                    .any(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
        })
    }

    pub async fn find_tool_candidates(&self, tool_name: &str) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !entry.tool_health.is_routable() {
                continue;
            }
            if let Some(tool) = entry.tools.get(tool_name)
                && entry.exposure_policy.matches(tool.tool.name.as_ref())
            {
                matches.push((upstream_name.clone(), tool.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches
    }

    pub async fn subject_scoped_tools(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
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
                .await;
                (config.name.clone(), result)
            });
        }

        let mut discovered = Vec::new();
        while let Some((name, result)) = futures.next().await {
            match result {
                Ok((_conn, tools)) => discovered.push((name, tools)),
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream tool discovery failed"
                    );
                }
            }
        }
        discovered
    }

    pub async fn subject_scoped_call_tool(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        params: CallToolRequestParams,
    ) -> Result<CallToolResult, String> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        let event = UpstreamRequestLog::tool(&config.name, &tool_name, true)
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
                    UpstreamCapability::Tools,
                    format!("upstream connect failed: {error}"),
                )
                .await;
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(
                    event,
                    elapsed_ms,
                    "upstream_connect_error",
                    Some(&error),
                    None,
                    None,
                );
                return Err(error.to_string());
            }
        };
        match tokio::time::timeout(self.request_timeout, conn.peer.call_tool(params)).await {
            Ok(Ok(result)) => {
                let response_size = estimate_response_size(&result);
                let max_bytes = max_response_bytes();
                if response_size > max_bytes {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        format!("response too large: {response_size} bytes"),
                    )
                    .await;
                    let elapsed_ms = start.elapsed().as_millis();
                    log_upstream_request_error(
                        event,
                        elapsed_ms,
                        "response_too_large",
                        None,
                        Some(response_size),
                        Some(max_bytes),
                    );
                    return Err(format!(
                        "upstream response too large ({response_size} bytes, max {max_bytes})"
                    ));
                }
                self.record_success_for(&config.name, UpstreamCapability::Tools)
                    .await;
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_finish(event, elapsed_ms, Some(response_size));
                Ok(result)
            }
            Ok(Err(error)) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("upstream call failed: {error}"),
                )
                .await;
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(
                    event,
                    elapsed_ms,
                    "upstream_error",
                    Some(&error),
                    None,
                    None,
                );
                Err(format!("upstream call failed: {error}"))
            }
            Err(_) => {
                let message = format!(
                    "upstream call timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                self.record_failure_for(&config.name, UpstreamCapability::Tools, message.clone())
                    .await;
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(event, elapsed_ms, "timeout", None, None, None);
                Err(message)
            }
        }
    }

    /// Return the names of upstreams currently routable for a capability.
    pub async fn routable_upstream_names(&self, capability: UpstreamCapability) -> Vec<String> {
        let catalog = self.catalog.read().await;
        let mut names: Vec<String> = match capability {
            UpstreamCapability::Resources => {
                let resource_names = self.resource_upstreams.read().await;
                resource_names
                    .iter()
                    .filter(|name| {
                        catalog
                            .get(*name)
                            .is_some_and(|entry| entry.health_for(capability).is_routable())
                    })
                    .cloned()
                    .collect()
            }
            UpstreamCapability::Tools | UpstreamCapability::Prompts => catalog
                .iter()
                .filter(|(_, entry)| entry.health_for(capability).is_routable())
                .map(|(name, _)| name.clone())
                .collect(),
        };
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Look up which upstream owns a given tool name.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn find_tool(&self, tool_name: &str) -> Option<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .filter(|entry| entry.tool_health.is_routable())
            .find_map(|entry| {
                entry.tools.get(tool_name).and_then(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool_name)
                        .then(|| (entry.name.to_string(), tool.clone()))
                })
            })
    }

    /// Get the cached schema for a specific upstream tool.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn tool_schema(&self, tool_name: &str) -> Option<Value> {
        let catalog = self.catalog.read().await;
        catalog.values().find_map(|entry| {
            entry.tools.get(tool_name).and_then(|tool| {
                entry
                    .exposure_policy
                    .matches(tool_name)
                    .then(|| tool.input_schema.clone())
                    .flatten()
            })
        })
    }

    /// Return all discovered tools for one upstream, including hidden tools and exposure metadata.
    pub async fn tool_exposure_rows(&self, upstream_name: &str) -> Vec<UpstreamToolExposureRow> {
        let catalog = self.catalog.read().await;
        let Some(entry) = catalog.get(upstream_name) else {
            return Vec::new();
        };

        let mut rows: Vec<UpstreamToolExposureRow> = entry
            .tools
            .values()
            .map(|tool| {
                let matched_by = entry.exposure_policy.matched_by(tool.tool.name.as_ref());
                UpstreamToolExposureRow {
                    name: tool.tool.name.to_string(),
                    description: tool
                        .tool
                        .description
                        .as_ref()
                        .map(ToString::to_string)
                        .filter(|text| !text.trim().is_empty()),
                    exposed: matched_by.is_some(),
                    matched_by,
                }
            })
            .collect();
        rows.sort_by(|left, right| left.name.cmp(&right.name));
        rows
    }

    pub async fn cached_upstream_summary(
        &self,
        upstream_name: &str,
    ) -> Option<UpstreamCachedSummary> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        let discovered_tool_count = entry.tools.len();
        let exposed_tool_count = entry
            .tools
            .values()
            .filter(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
            .count();
        let discovered_resource_count = entry.resource_count;
        let exposed_resource_count = if entry.resource_health.is_routable() {
            entry.resource_count
        } else {
            0
        };
        let discovered_prompt_count = entry.prompt_count;
        let exposed_prompt_count = if entry.prompt_health.is_routable() {
            entry.prompt_count
        } else {
            0
        };

        Some(UpstreamCachedSummary {
            discovered_tool_count,
            exposed_tool_count,
            discovered_resource_count,
            exposed_resource_count,
            discovered_prompt_count,
            exposed_prompt_count,
        })
    }

    pub async fn upstream_runtime_metadata(
        &self,
        upstream_name: &str,
    ) -> Option<UpstreamRuntimeMetadata> {
        self.connections
            .read()
            .await
            .get(upstream_name)
            .map(|conn| conn.runtime.clone())
    }

    /// Return cached resource URIs keyed by upstream name (used in catalog snapshots).
    pub async fn cached_upstream_resource_uris(&self) -> Vec<(String, Vec<String>)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(_, entry)| !entry.resource_uris.is_empty())
            .map(|(name, entry)| (name.clone(), entry.resource_uris.clone()))
            .collect()
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

    /// Return the current tool health for one upstream.
    pub async fn upstream_tool_health(&self, upstream_name: &str) -> Option<UpstreamHealth> {
        let catalog = self.catalog.read().await;
        catalog.get(upstream_name).map(|entry| entry.tool_health)
    }

    /// Call a tool on an upstream server.
    ///
    /// Returns `None` if the upstream is not connected or the tool is not found.
    /// Enforces a response size cap (`LAB_UPSTREAM_MAX_RESPONSE_BYTES`, default 10 MB).
    ///
    /// Cap layering by transport:
    /// - **HTTP non-OAuth**: cap is enforced at the rmcp transport layer by
    ///   `BodyCappedHttpClient` (see `dispatch/upstream/http_client.rs`) —
    ///   bytes are checked during streaming, *before* allocation.
    /// - **stdio**: cap is post-hoc here (rmcp's stdio transport buffers the
    ///   full JSON response before we see it). The check at the end of this
    ///   function guards against forwarding oversized payloads but cannot
    ///   prevent the underlying allocation.
    /// - **HTTP OAuth**: also post-hoc for now — threading the cap through
    ///   `OauthClientCache` is tracked as a follow-up.
    ///
    /// The post-hoc check below is therefore defense-in-depth for HTTP
    /// non-OAuth and the primary line of defense for stdio / OAuth.
    pub async fn call_tool(
        &self,
        upstream_name: &str,
        params: CallToolRequestParams,
    ) -> Option<Result<CallToolResult, String>> {
        let start = Instant::now();
        let tool_name = params.name.to_string();
        let event = UpstreamRequestLog::tool(upstream_name, &tool_name, false);
        let peer = self
            .acquire_peer(upstream_name, UpstreamCapability::Tools, "tool.call")
            .await?;
        log_upstream_request_start(event);
        let result = match tokio::time::timeout(self.request_timeout, peer.call_tool(params)).await
        {
            Ok(result) => result.map_err(|e| {
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(
                    event,
                    elapsed_ms,
                    "upstream_error",
                    Some(&e),
                    None,
                    None,
                );
                format!("upstream call failed: {e}")
            }),
            Err(_) => {
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(event, elapsed_ms, "timeout", None, None, None);
                Err(format!(
                    "upstream call timed out after {}ms",
                    self.request_timeout.as_millis()
                ))
            }
        };

        // Enforce response size cap.
        if let Ok(ref r) = result {
            let response_size = estimate_response_size(r);
            let max_bytes = max_response_bytes();
            if response_size > max_bytes {
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(
                    event,
                    elapsed_ms,
                    "response_too_large",
                    None,
                    Some(response_size),
                    Some(max_bytes),
                );
                return Some(Err(format!(
                    "upstream response too large ({response_size} bytes, max {max_bytes})"
                )));
            }
            let elapsed_ms = start.elapsed().as_millis();
            log_upstream_request_finish(event, elapsed_ms, Some(response_size));
        }

        Some(result)
    }

    /// Record a failure for an upstream, potentially marking it unhealthy.
    ///
    /// After [`CIRCUIT_BREAKER_THRESHOLD`] consecutive failures, the upstream
    /// is excluded from `list_tools` until a successful re-probe.
    /// Render the synthetic `lab://gateway/servers` document.
    ///
    /// Lists every registered upstream (regardless of health) with the
    /// tool count an agent would see in the corresponding schema document.
    pub async fn gateway_servers_doc(&self) -> Value {
        let catalog = self.catalog.read().await;
        let mut servers: Vec<Value> = catalog
            .iter()
            .map(|(name, e)| {
                let tool_count = e
                    .tools
                    .values()
                    .filter(|t| e.exposure_policy.matches(&t.tool.name))
                    .count();
                serde_json::json!({
                    "name": name,
                    "tool_count": tool_count,
                    "prompt_count": e.prompt_count,
                    "resource_count": e.resource_count,
                    "tool_health": health_str(e.tool_health),
                    "tool_last_error": e.tool_last_error,
                })
            })
            .collect();
        servers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        serde_json::json!({ "servers": servers })
    }

    /// Render the synthetic `lab://gateway/<name>/schema` document.
    ///
    /// Returns `None` when the upstream is not registered. Tools hidden by
    /// the upstream's `ToolExposurePolicy` are omitted. `input_schema` and
    /// `meta` are passed through verbatim from the cached tool definition.
    pub async fn gateway_server_schema(&self, name: &str) -> Option<Value> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(name)?;
        let mut tools: Vec<Value> = entry
            .tools
            .values()
            .filter(|t| entry.exposure_policy.matches(&t.tool.name))
            .map(|t| {
                serde_json::json!({
                    "name": t.tool.name.as_ref(),
                    "description": t.tool.description.as_ref().map(|s| s.as_ref()),
                    "input_schema": t.input_schema,
                    "meta": t.tool.meta,
                })
            })
            .collect();
        tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Some(serde_json::json!({
            "name": name,
            "tools": tools,
            "health": health_str(entry.tool_health),
            "last_error": entry.tool_last_error,
        }))
    }

    /// Synthetic gateway resources to emit from `list_resources`.
    ///
    /// Returns one entry for `lab://gateway/servers` plus one
    /// `lab://gateway/<name>/schema` entry per registered upstream.
    pub async fn gateway_synthetic_resources(&self) -> Vec<Resource> {
        let mut out = vec![
            RawResource::new("lab://gateway/servers", "gateway/servers")
                .with_description("Index of upstream MCP servers registered with the gateway")
                .with_mime_type("application/json")
                .no_annotation(),
        ];
        let catalog = self.catalog.read().await;
        let mut names: Vec<&String> = catalog.keys().collect();
        names.sort();
        for name in names {
            out.push(
                RawResource::new(
                    format!("lab://gateway/{name}/schema"),
                    format!("gateway/{name}/schema"),
                )
                .with_description(format!("Tool schemas for upstream `{name}`"))
                .with_mime_type("application/json")
                .no_annotation(),
            );
        }
        out
    }

    /// List resources from all resource-proxy-enabled upstreams.
    ///
    /// Resources are prefixed with `lab://upstream/{name}/` to avoid collisions.
    pub async fn list_upstream_resources(&self) -> Vec<Resource> {
        let peers = routable_upstream_peers(self, UpstreamCapability::Resources).await;
        if peers.is_empty() {
            return Vec::new();
        }

        // Issue RPCs in parallel, then sort by upstream name for deterministic order.
        let mut futures = FuturesUnordered::new();
        for (name, peer) in peers {
            futures.push(async move {
                let result = peer.list_resources(None).await;
                (name, result)
            });
        }

        let mut results: Vec<(String, Result<_, _>)> = Vec::new();
        while let Some(item) = futures.next().await {
            results.push(item);
        }
        results.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let mut resources = Vec::new();
        for (name, result) in results {
            match result {
                Ok(result) => {
                    self.record_success_for(&name, UpstreamCapability::Resources)
                        .await;
                    let resource_uris = result
                        .resources
                        .iter()
                        .map(|resource| bare_upstream_resource_uri(&resource.uri).to_string())
                        .collect();
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.resource_count = result.resources.len();
                            entry.resource_uris = resource_uris;
                        }
                    }
                    for mut resource in result.resources {
                        rewrite_resource_uri(&mut resource, &name);
                        resources.push(resource);
                    }
                }
                Err(e) => {
                    self.record_failure_for(
                        &name,
                        UpstreamCapability::Resources,
                        format!("failed to list resources from upstream: {e}"),
                    )
                    .await;
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.resource_count = 0;
                            entry.resource_uris.clear();
                        }
                    }
                    tracing::warn!(
                        upstream = %name,
                        error = %e,
                        "failed to list resources from upstream"
                    );
                }
            }
        }

        resources
    }

    pub async fn subject_scoped_resources(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) -> Vec<Resource> {
        let mut futures = FuturesUnordered::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs
            .iter()
            .filter(|config| config.oauth.is_some() && config.proxy_resources)
        {
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

        let mut resources = Vec::new();
        while let Some((name, result)) = futures.next().await {
            let Ok(conn) = result else {
                continue;
            };
            match conn.peer.list_resources(None).await {
                Ok(result) => {
                    for mut resource in result.resources {
                        rewrite_resource_uri(&mut resource, &name);
                        resources.push(resource);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream resource discovery failed"
                    );
                }
            }
        }

        resources
    }

    /// Read a resource from an upstream, given a prefixed URI.
    ///
    /// Expects URIs in the form `lab://upstream/{name}/{original_uri}`.
    /// Returns `None` if the upstream name is not found or not resource-enabled.
    pub async fn read_upstream_resource(
        &self,
        uri: &str,
    ) -> Option<Result<ReadResourceResult, String>> {
        let start = Instant::now();
        let prefix = "lab://upstream/";
        let rest = uri.strip_prefix(prefix)?;

        // Extract upstream name and original URI
        let slash_pos = rest.find('/')?;
        let upstream_name = &rest[..slash_pos];
        let original_uri = &rest[slash_pos + 1..];

        // Check if this upstream has resource proxying enabled.
        // Clone the vec and drop the lock before any async work.
        let is_resource_enabled = {
            let resource_names = self.resource_upstreams.read().await;
            if !resource_names.iter().any(|n| n == upstream_name) {
                false
            } else {
                let catalog = self.catalog.read().await;
                catalog
                    .get(upstream_name)
                    .is_some_and(|entry| entry.resource_health.is_routable())
            }
        };
        if !is_resource_enabled {
            return None;
        }

        // Clone the peer handle out, then drop the lock before awaiting.
        let peer = self
            .acquire_peer(
                upstream_name,
                UpstreamCapability::Resources,
                "resource.read",
            )
            .await?;

        let redacted_uri = redact_resource_uri_for_logging(uri);
        let event = UpstreamRequestLog::resource(upstream_name, redacted_uri, false);
        log_upstream_request_start(event);

        let params = rmcp::model::ReadResourceRequestParams::new(original_uri);

        let result =
            match tokio::time::timeout(self.request_timeout, peer.read_resource(params)).await {
                Ok(Ok(result)) => {
                    let normalized = normalize_resource_result_uri(result, uri);
                    Ok(normalized)
                }
                Ok(Err(e)) => {
                    self.record_failure_for(
                        upstream_name,
                        UpstreamCapability::Resources,
                        format!("upstream resource read failed: {e}"),
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
                    Err(format!("upstream resource read failed: {e}"))
                }
                Err(_) => {
                    let message = format!(
                        "upstream resource read timed out after {}ms",
                        self.request_timeout.as_millis()
                    );
                    self.record_failure_for(
                        upstream_name,
                        UpstreamCapability::Resources,
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
                    Err(message)
                }
            };

        // Enforce the same response size cap as call_tool (post-hoc).
        if let Ok(ref r) = result {
            let response_size = serde_json::to_string(r).map_or(0, |s| s.len());
            let max_bytes = max_response_bytes();
            if response_size > max_bytes {
                self.record_failure_for(
                    upstream_name,
                    UpstreamCapability::Resources,
                    format!(
                        "upstream resource response too large ({response_size} bytes, max {max_bytes})"
                    ),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "response_too_large",
                    None,
                    Some(response_size),
                    Some(max_bytes),
                );
                return Some(Err(format!(
                    "upstream resource response too large ({response_size} bytes, max {max_bytes})"
                )));
            }
            self.record_success_for(upstream_name, UpstreamCapability::Resources)
                .await;
            log_upstream_request_finish(event, start.elapsed().as_millis(), Some(response_size));
        }

        Some(result)
    }

    pub async fn subject_scoped_read_resource(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        uri: &str,
    ) -> Result<ReadResourceResult, String> {
        let start = Instant::now();
        let prefix = format!("lab://upstream/{}/", config.name);
        let original_uri = uri
            .strip_prefix(&prefix)
            .ok_or_else(|| "resource uri does not match upstream".to_string())?;
        let redacted_uri = redact_resource_uri_for_logging(uri);
        let event = UpstreamRequestLog::resource(&config.name, redacted_uri, true)
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
                    UpstreamCapability::Resources,
                    format!("upstream resource connect failed: {error}"),
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
        match tokio::time::timeout(
            self.request_timeout,
            conn.peer
                .read_resource(rmcp::model::ReadResourceRequestParams::new(original_uri)),
        )
        .await
        {
            Ok(Ok(result)) => {
                // Size check before recording success so an oversized response
                // does not advance the circuit breaker's healthy counter.
                let response_size = serde_json::to_string(&result).map_or(0, |s| s.len());
                let max_bytes = max_response_bytes();
                if response_size > max_bytes {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Resources,
                        format!(
                            "upstream resource response too large ({response_size} bytes, max {max_bytes})"
                        ),
                    )
                    .await;
                    log_upstream_request_error(
                        event,
                        start.elapsed().as_millis(),
                        "response_too_large",
                        None,
                        Some(response_size),
                        Some(max_bytes),
                    );
                    return Err(format!(
                        "upstream resource response too large ({response_size} bytes, max {max_bytes})"
                    ));
                }
                self.record_success_for(&config.name, UpstreamCapability::Resources)
                    .await;
                let normalized = normalize_resource_result_uri(result, uri);
                log_upstream_request_finish(
                    event,
                    start.elapsed().as_millis(),
                    Some(response_size),
                );
                Ok(normalized)
            }
            Ok(Err(error)) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Resources,
                    format!("upstream resource read failed: {error}"),
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
                Err(format!("upstream resource read failed: {error}"))
            }
            Err(_) => {
                let message = format!(
                    "upstream resource read timed out after {}ms",
                    self.request_timeout.as_millis()
                );
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Resources,
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
                Err(message)
            }
        }
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
    use super::super::types::ToolExposurePolicy;
    use super::testsupport::*;
    use super::*;
    use crate::mcp::logging::logging_level_rank;
    use crate::mcp::server::LabMcpServer;
    use rmcp::model::{LoggingLevel, ResourceContents};
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

    #[tokio::test]
    async fn seed_lazy_upstreams_records_enabled_names_without_connections() {
        let pool = UpstreamPool::new();
        let configs = vec![
            named_test_upstream_config("alpha"),
            named_test_upstream_config("beta"),
            named_disabled_test_upstream_config("disabled"),
        ];

        pool.seed_lazy_upstreams(&configs).await;

        assert_eq!(pool.upstream_count().await, 2);
        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert!(pool.cached_upstream_summary("alpha").await.is_some());
        assert!(pool.cached_upstream_summary("beta").await.is_some());
        assert!(pool.cached_upstream_summary("disabled").await.is_none());
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_connects_only_requested_upstream() {
        let pool = UpstreamPool::new();
        let configs = vec![
            named_test_upstream_config("slow"),
            named_test_upstream_config("fast"),
        ];
        pool.seed_lazy_upstreams(&configs).await;

        let fast_seen = Arc::new(AtomicBool::new(false));
        let slow_seen = Arc::new(AtomicBool::new(false));
        let connector: TestUpstreamConnector = {
            let fast_seen = Arc::clone(&fast_seen);
            let slow_seen = Arc::clone(&slow_seen);
            Arc::new(move |config| {
                let fast_seen = Arc::clone(&fast_seen);
                let slow_seen = Arc::clone(&slow_seen);
                Box::pin(async move {
                    match config.name.as_str() {
                        "fast" => fast_seen.store(true, Ordering::Relaxed),
                        "slow" => slow_seen.store(true, Ordering::Relaxed),
                        other => panic!("unexpected upstream {other}"),
                    }
                    Ok((None, vec![test_tool("ping")]))
                })
            })
        };

        pool.ensure_tools_for_upstream_with_connector(&configs[1], None, connector)
            .await
            .expect("fast connects");

        assert!(fast_seen.load(Ordering::Relaxed));
        assert!(!slow_seen.load(Ordering::Relaxed));
        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert_eq!(pool.healthy_tools_for_upstream("fast").await.len(), 1);
        assert!(pool.healthy_tools_for_upstream("slow").await.is_empty());
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_singleflights_concurrent_connects() {
        let pool = Arc::new(UpstreamPool::new());
        let config = named_test_upstream_config("alpha");
        pool.seed_lazy_upstreams(std::slice::from_ref(&config))
            .await;

        let connect_count = Arc::new(AtomicUsize::new(0));
        let connector: TestUpstreamConnector = {
            let connect_count = Arc::clone(&connect_count);
            Arc::new(move |_config| {
                let connect_count = Arc::clone(&connect_count);
                Box::pin(async move {
                    connect_count.fetch_add(1, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok((None, vec![test_tool("ping")]))
                })
            })
        };

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            let config = config.clone();
            let connector = Arc::clone(&connector);
            tasks.push(tokio::spawn(async move {
                pool.ensure_tools_for_upstream_with_connector(&config, None, connector)
                    .await
                    .expect("lazy connect succeeds")
            }));
        }

        let results = futures::future::join_all(tasks).await;
        let connected = results
            .into_iter()
            .map(|result| result.expect("task joins"))
            .filter(|connected| *connected)
            .count();
        assert_eq!(connected, 1);
        assert_eq!(connect_count.load(Ordering::Relaxed), 1);
        assert_eq!(pool.healthy_tools_for_upstream("alpha").await.len(), 1);
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_records_lazy_connect_failures() {
        let pool = UpstreamPool::new();
        let config = UpstreamConfig {
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            command: None,
            ..named_test_upstream_config("broken")
        };
        pool.seed_lazy_upstreams(std::slice::from_ref(&config))
            .await;

        let err = pool
            .ensure_tools_for_upstream(&config, None, None)
            .await
            .expect_err("connect should fail");

        assert!(!err.to_string().is_empty());
        let last_error = pool
            .upstream_tool_last_error("broken")
            .await
            .expect("lazy failure is recorded");
        assert!(last_error.contains("lazy upstream connect failed"));
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_preserves_other_resource_upstreams() {
        let pool = UpstreamPool::new();
        let mut alpha = named_test_upstream_config("alpha");
        alpha.proxy_resources = true;
        let mut beta = named_test_upstream_config("beta");
        beta.proxy_resources = true;
        pool.seed_lazy_upstreams(&[alpha.clone(), beta.clone()])
            .await;

        pool.ensure_tools_for_upstream_with_connector(
            &alpha,
            None,
            Arc::new(|_config| Box::pin(async { Ok((None, vec![test_tool("ping")])) })),
        )
        .await
        .expect("lazy connect succeeds");

        assert_eq!(
            *pool.resource_upstreams.read().await,
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[tokio::test]
    async fn disabled_upstream_reprobe_is_inert() {
        let pool = UpstreamPool::new();
        let mut config = test_upstream_config();
        config.enabled = false;
        config.command = Some("definitely-not-spawned".to_string());

        let result = pool
            .reprobe_tools_for_upstream(&config)
            .await
            .expect("disabled reprobe should not error");

        assert!(!result);
        assert!(pool.find_tool("anything").await.is_none());
    }

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

    #[test]
    fn normalize_resource_result_uri_rewrites_all_contents() {
        let result = ReadResourceResult::new(vec![
            ResourceContents::text("hello", "http://upstream/resource"),
            ResourceContents::blob("YWJj", "file:///tmp/upstream"),
        ]);

        let normalized =
            normalize_resource_result_uri(result, "lab://upstream/demo/http://upstream/resource");

        let uris: Vec<_> = normalized
            .contents
            .iter()
            .map(|content| match content {
                ResourceContents::TextResourceContents { uri, .. }
                | ResourceContents::BlobResourceContents { uri, .. } => uri.as_str(),
            })
            .collect();

        assert_eq!(
            uris,
            vec![
                "lab://upstream/demo/http://upstream/resource",
                "lab://upstream/demo/http://upstream/resource",
            ]
        );
    }

    #[tokio::test]
    async fn empty_pool_has_no_tools() {
        let pool = UpstreamPool::new();
        assert!(pool.healthy_tools().await.is_empty());
        assert_eq!(pool.upstream_count().await, 0);
    }

    #[tokio::test]
    async fn call_tool_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .call_tool("slow", CallToolRequestParams::new("slow.tool"))
            .await
            .expect("upstream is connected")
            .expect_err("slow tool call should time out");

        assert!(result.contains("timed out"));
    }

    #[tokio::test]
    async fn read_resource_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .read_upstream_resource("lab://upstream/slow/file:///tmp/slow")
            .await
            .expect("resource upstream is enabled")
            .expect_err("slow resource read should time out");

        assert!(result.contains("timed out"));
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

    #[tokio::test]
    async fn successful_resource_listing_populates_snapshot_cache() {
        let pool = static_catalog_pool("static").await;

        let resources = pool.list_upstream_resources().await;
        let listed_uris: Vec<_> = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect();
        assert_eq!(
            listed_uris,
            vec![
                "lab://upstream/static/file:///tmp/upstream-one",
                "lab://upstream/static/file:///tmp/upstream-two",
            ]
        );

        let cached = pool.cached_upstream_resource_uris().await;
        assert_eq!(
            cached,
            vec![(
                "static".to_string(),
                vec![
                    "file:///tmp/upstream-one".to_string(),
                    "file:///tmp/upstream-two".to_string(),
                ],
            )]
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
        assert!(
            snapshot
                .resources
                .contains("lab://upstream/static/file:///tmp/upstream-one")
        );
        assert!(
            snapshot
                .resources
                .contains("lab://upstream/static/file:///tmp/upstream-two")
        );
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
    async fn hidden_upstream_tools_do_not_appear_in_listings() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let tools = test_upstream_tools(
            &upstream_name,
            &["search_repos", "github_create_issue", "delete_repo"],
        );
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["search_repos".into(), "github_*".into()])
                .expect("policy");

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        let names: Vec<String> = pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|t| t.tool.name.to_string())
            .collect();
        assert!(names.contains(&"search_repos".to_string()));
        assert!(names.contains(&"github_create_issue".to_string()));
        assert!(!names.contains(&"delete_repo".to_string()));
    }

    #[tokio::test]
    async fn hidden_upstream_tools_cannot_be_called_directly() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let tools = test_upstream_tools(&upstream_name, &["search_repos", "delete_repo"]);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["search_repos".into()]).expect("policy");

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        assert!(pool.find_tool("search_repos").await.is_some());
        assert!(pool.find_tool("delete_repo").await.is_none());
    }

    #[tokio::test]
    async fn in_process_registration_isolates_slow_services_from_fast_services() {
        use futures::future::BoxFuture;
        use lab_apis::core::action::ActionSpec;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "status.read",
            description: "Read status",
            destructive: false,
            returns: "Value",
            params: &[],
        }];

        fn dispatch(
            _action: String,
            _params: Value,
        ) -> std::pin::Pin<
            Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
        > {
            Box::pin(async { Ok(Value::Null) })
        }

        fn service(name: &'static str) -> RegisteredService {
            RegisteredService {
                name,
                description: "test service",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
                status: "available",
                actions: ACTIONS,
                dispatch,
            }
        }

        let pool = UpstreamPool::new();
        let fast_seen = Arc::new(AtomicUsize::new(0));
        let fast_seen_for_connector = Arc::clone(&fast_seen);
        let connector: InProcessConnector = Arc::new(move |service| {
            let fast_seen = Arc::clone(&fast_seen_for_connector);
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    if service.name == "slow" {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        anyhow::bail!("slow service failed to start");
                    }

                    fast_seen.fetch_add(1, Ordering::SeqCst);
                    let upstream_name: Arc<str> = Arc::from(in_process_upstream_name(service.name));
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: Vec::new(),
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        let registration = tokio::spawn({
            let pool = pool.clone();
            async move {
                pool.register_in_process_service_list_with_connector(
                    vec![service("slow"), service("fast")],
                    connector,
                )
                .await;
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            fast_seen.load(Ordering::SeqCst),
            1,
            "fast service should register before slow service finishes"
        );

        registration.await.expect("registration task");
        assert_eq!(pool.upstream_count().await, 2);
    }

    #[tokio::test]
    async fn failed_in_process_registration_does_not_hide_healthy_peer_tools() {
        use futures::future::BoxFuture;
        use lab_apis::core::action::ActionSpec;

        static ACTIONS: &[ActionSpec] = &[ActionSpec {
            name: "status.read",
            description: "Read status",
            destructive: false,
            returns: "Value",
            params: &[],
        }];

        fn dispatch(
            _action: String,
            _params: Value,
        ) -> std::pin::Pin<
            Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
        > {
            Box::pin(async { Ok(Value::Null) })
        }

        fn service(name: &'static str) -> RegisteredService {
            RegisteredService {
                name,
                description: "test service",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
                status: "available",
                actions: ACTIONS,
                dispatch,
            }
        }

        let pool = UpstreamPool::new();
        let connector: InProcessConnector = Arc::new(|service| {
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    if service.name == "bad" {
                        anyhow::bail!("bad service failed to start");
                    }

                    let upstream_name: Arc<str> = Arc::from(in_process_upstream_name(service.name));
                    let tool = rmcp::model::Tool::new(
                        "status.read",
                        "Read status",
                        Arc::new(serde_json::Map::new()),
                    );
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: vec![tool],
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        pool.register_in_process_service_list_with_connector(
            vec![service("bad"), service("good")],
            connector,
        )
        .await;

        let good_tools = pool
            .healthy_tools_for_upstream(&in_process_upstream_name("good"))
            .await;
        let bad_tools = pool
            .healthy_tools_for_upstream(&in_process_upstream_name("bad"))
            .await;

        assert_eq!(good_tools.len(), 1);
        assert_eq!(good_tools[0].tool.name.as_ref(), "status.read");
        assert!(bad_tools.is_empty());
        assert_eq!(pool.upstream_count().await, 2);
    }

    #[test]
    fn observability_source_covers_pool_acquire_reprobe_and_drain_events() {
        // The pool was split into `pool.rs` + the `pool/` child modules, so the
        // observability instrumentation now lives across several files. Scan the
        // whole upstream-pool source tree (pool.rs + every pool/*.rs) so this
        // guard stays robust as code relocates between modules. A missing string
        // here means a real dropped-instrumentation regression — never delete an
        // assertion to make this test pass; add the file the string moved into.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/dispatch/upstream");
        let mut source =
            std::fs::read_to_string(format!("{dir}/pool.rs")).expect("read pool.rs source");
        let pool_dir = format!("{dir}/pool");
        if let Ok(entries) = std::fs::read_dir(&pool_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    source.push_str(
                        &std::fs::read_to_string(&path).expect("read pool child module source"),
                    );
                }
            }
        }
        for expected in [
            "action = \"upstream.acquire\"",
            "elapsed_ms",
            "pool_size",
            "connection_count",
            "action = \"upstream.reprobe\"",
            "operation = \"health\"",
            "action = \"upstream.pool.drain\"",
            "cancelled_probe_count",
            "kind = \"upstream_pool_empty\"",
            "kind = \"upstream_not_connected\"",
            "fn log_upstream_request_start",
            "fn log_upstream_request_finish",
            "fn log_upstream_request_error",
            "action = \"upstream.request\"",
        ] {
            assert!(
                source.contains(expected),
                "missing upstream pool observability field `{expected}`"
            );
        }
    }

    #[tokio::test]
    async fn gateway_servers_doc_lists_one_healthy_upstream() {
        use std::sync::Arc;

        let pool = UpstreamPool::new();
        let mut tools = HashMap::new();
        tools.insert(
            "search".to_string(),
            UpstreamTool {
                tool: rmcp::model::Tool::new(
                    "search",
                    "search the index",
                    Arc::new(serde_json::Map::new()),
                ),
                input_schema: Some(serde_json::json!({"type": "object"})),
                output_schema: None,
                upstream_name: Arc::from("alpha"),
                destructive: false,
            },
        );
        let entry = healthy_in_process_entry(Arc::from("alpha"), tools);
        pool.catalog
            .write()
            .await
            .insert("alpha".to_string(), entry);

        let doc = pool.gateway_servers_doc().await;
        let servers = doc
            .get("servers")
            .and_then(|v| v.as_array())
            .expect("servers array");
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s["name"], "alpha");
        assert_eq!(s["tool_count"], 1);
        assert_eq!(s["tool_health"], "healthy");
        assert!(s["tool_last_error"].is_null());
        assert_eq!(s["prompt_count"], 0);
        assert_eq!(s["resource_count"], 0);
    }

    #[tokio::test]
    async fn gateway_server_schema_respects_exposure_policy() {
        use std::sync::Arc;

        let make_tool = |name: &'static str| UpstreamTool {
            tool: rmcp::model::Tool::new(name, "desc", Arc::new(serde_json::Map::new())),
            input_schema: Some(serde_json::json!({"type": "object"})),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: false,
        };

        let mut tools = HashMap::new();
        tools.insert("github_create".into(), make_tool("github_create"));
        tools.insert("delete_repo".into(), make_tool("delete_repo"));

        let mut entry = healthy_in_process_entry(Arc::from("alpha"), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["github_*".into()]).expect("policy");

        let pool = UpstreamPool::new();
        pool.catalog
            .write()
            .await
            .insert("alpha".to_string(), entry);

        let doc = pool.gateway_server_schema("alpha").await.expect("doc");
        let names: Vec<&str> = doc["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec!["github_create"]);
        assert_eq!(doc["health"], "healthy");
        assert!(doc["last_error"].is_null());
        assert_eq!(doc["name"], "alpha");
    }

    #[tokio::test]
    async fn gateway_server_schema_unknown_upstream_returns_none() {
        let pool = UpstreamPool::new();
        assert!(pool.gateway_server_schema("nope").await.is_none());
    }

    #[tokio::test]
    async fn gateway_synthetic_resources_lists_index_and_per_upstream() {
        use std::sync::Arc;

        let pool = UpstreamPool::new();
        let entry = healthy_in_process_entry(Arc::from("alpha"), HashMap::new());
        pool.catalog
            .write()
            .await
            .insert("alpha".to_string(), entry);
        let entry = healthy_in_process_entry(Arc::from("beta"), HashMap::new());
        pool.catalog.write().await.insert("beta".to_string(), entry);

        let resources = pool.gateway_synthetic_resources().await;
        let uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();
        assert!(uris.iter().any(|u| u == "lab://gateway/servers"));
        assert!(uris.iter().any(|u| u == "lab://gateway/alpha/schema"));
        assert!(uris.iter().any(|u| u == "lab://gateway/beta/schema"));
        assert_eq!(uris.len(), 3);
    }
}
