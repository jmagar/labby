//! `UpstreamPool` — manages connections to upstream MCP servers.
//!
//! Connects to configured upstreams via HTTP (`StreamableHttpClientTransport`)
//! or stdio (child process), discovers their tools, and caches schemas.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, GetPromptRequestParams, GetPromptResult, LoggingLevel,
    Prompt, ReadResourceResult, Resource, ResourceContents,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::config::UpstreamConfig;
use crate::dispatch::redact::{redact_stdio_value, redact_url};
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::registry::{RegisteredService, ToolRegistry};

use super::auth::{configured_bearer_token, websocket_authorization_header};
use super::transport::websocket::{
    WebSocketTransportConfig, connect as connect_websocket_transport, jitter_delay, parse_ws_url,
    reprobe_backoff,
};
use super::types;
use super::types::{
    ToolExposurePolicy, UpstreamCapability, UpstreamEntry, UpstreamHealth, UpstreamRuntimeMetadata,
    UpstreamRuntimeOwner, UpstreamTool, UpstreamToolExposureRow,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpstreamCachedSummary {
    pub discovered_tool_count: usize,
    pub exposed_tool_count: usize,
    pub discovered_resource_count: usize,
    pub exposed_resource_count: usize,
    pub discovered_prompt_count: usize,
    pub exposed_prompt_count: usize,
}

/// Per-upstream timeout for initial discovery (`list_tools`).
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-service timeout for in-process peer registration and capability probing.
const IN_PROCESS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Per-request timeout for upstream tool/resource/prompt RPCs.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default maximum response size from upstream servers (10 MB).
const DEFAULT_MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

const IN_PROCESS_PEER_BUFFER_BYTES: usize = 256 * 1024;

pub fn in_process_upstream_name(service_name: &str) -> String {
    format!("__in_process__{service_name}")
}

/// Read the max response size from env or use the default.
fn max_response_bytes() -> usize {
    std::env::var("LAB_UPSTREAM_MAX_RESPONSE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES)
}

fn upstream_transport(config: &UpstreamConfig) -> &'static str {
    if config.url.as_deref().is_some_and(is_websocket_url) {
        "websocket"
    } else if config.url.is_some() {
        "http"
    } else {
        "stdio"
    }
}

fn is_websocket_url(url: &str) -> bool {
    matches!(
        url::Url::parse(url)
            .ok()
            .map(|parsed| parsed.scheme().to_string())
            .as_deref(),
        Some("ws" | "wss")
    )
}

/// Strip query strings and fragments from resource URIs before logging.
///
/// SECURITY: Upstream MCP servers may return resource URIs containing pre-signed
/// tokens or OAuth credentials in query parameters. Only scheme+host+path is safe to log.
pub(crate) fn redact_resource_uri_for_logging(uri: &str) -> &str {
    let cut = uri.find('?').or_else(|| uri.find('#')).unwrap_or(uri.len());
    &uri[..cut]
}

fn upstream_target_redacted(config: &UpstreamConfig) -> String {
    // SECURITY: Never log raw URLs or command fragments without central redaction.
    match &config.url {
        Some(url_str) => redact_url(url_str),
        None => config
            .command
            .as_deref()
            .map(redact_stdio_value)
            .or_else(|| Some("<missing>".to_string()))
            .expect("static fallback is present"),
    }
}

/// Collect upstream peers for a capability in deterministic name order.
async fn routable_upstream_peers(
    pool: &UpstreamPool,
    capability: UpstreamCapability,
) -> Vec<(String, rmcp::service::Peer<RoleClient>)> {
    let mut names: Vec<String> = {
        let catalog = pool.catalog.read().await;
        let mut names = match capability {
            UpstreamCapability::Resources => {
                let resource_names = pool.resource_upstreams.read().await;
                resource_names
                    .iter()
                    .filter(|name| {
                        catalog
                            .get(*name)
                            .is_some_and(|entry| entry.health_for(capability).is_routable())
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            }
            UpstreamCapability::Tools | UpstreamCapability::Prompts => catalog
                .iter()
                .filter(|(_, entry)| entry.health_for(capability).is_routable())
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>(),
        };
        names.sort_unstable();
        names.dedup();
        names
    };

    let acquire_started = Instant::now();
    tracing::debug!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.acquire",
        event = "start",
        operation = "connection.acquire",
        requested_operation = "capability.route",
        capability = capability_name(capability),
        requested_count = names.len(),
        "upstream pool acquire start"
    );
    let connections = pool.connections.read().await;
    let connection_count = connections.len();
    let peers = names
        .drain(..)
        .filter_map(|name| connections.get(&name).map(|conn| (name, conn.peer.clone())))
        .collect::<Vec<_>>();
    drop(connections);
    let pool_size = pool.catalog.read().await.len();
    let elapsed_ms = acquire_started.elapsed().as_millis();
    if peers.is_empty() {
        tracing::warn!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.acquire",
            event = "empty",
            operation = "connection.acquire",
            requested_operation = "capability.route",
            capability = capability_name(capability),
            elapsed_ms,
            kind = "upstream_pool_empty",
            pool_size,
            connection_count,
            "upstream pool acquire empty"
        );
    } else {
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.acquire",
            event = "finish",
            operation = "connection.acquire",
            requested_operation = "capability.route",
            capability = capability_name(capability),
            elapsed_ms,
            pool_size,
            connection_count,
            acquired_count = peers.len(),
            "upstream pool acquire finish"
        );
    }
    peers
}

/// Returns true if the error represents a capability the upstream simply doesn't support
/// (method not found / not implemented). These are healthy — the upstream just doesn't
/// expose that capability, which is fine.
fn is_capability_unsupported(error: &rmcp::ServiceError) -> bool {
    let msg = error.to_string();
    msg.contains("Method not found")
        || msg.contains("method_not_found")
        || msg.contains("-32601")
        || msg.contains("Not implemented")
}

fn capability_name(capability: UpstreamCapability) -> &'static str {
    match capability {
        UpstreamCapability::Tools => "tools",
        UpstreamCapability::Prompts => "prompts",
        UpstreamCapability::Resources => "resources",
    }
}

#[derive(Clone, Copy)]
struct UpstreamRequestLog<'a> {
    upstream: &'a str,
    capability: &'static str,
    operation: &'static str,
    subject_scoped: bool,
    transport: Option<&'static str>,
    item_kind: Option<&'static str>,
    item: Option<&'a str>,
}

impl<'a> UpstreamRequestLog<'a> {
    fn tool(upstream: &'a str, tool: &'a str, subject_scoped: bool) -> Self {
        Self {
            upstream,
            capability: "tools",
            operation: "tool.call",
            subject_scoped,
            transport: None,
            item_kind: Some("tool"),
            item: Some(tool),
        }
    }

    fn resource(upstream: &'a str, resource_uri: &'a str, subject_scoped: bool) -> Self {
        Self {
            upstream,
            capability: "resources",
            operation: "resource.read",
            subject_scoped,
            transport: None,
            item_kind: Some("resource_uri"),
            item: Some(resource_uri),
        }
    }

    fn prompt(upstream: &'a str, prompt: &'a str, subject_scoped: bool) -> Self {
        Self {
            upstream,
            capability: "prompts",
            operation: "prompt.get",
            subject_scoped,
            transport: None,
            item_kind: Some("prompt"),
            item: Some(prompt),
        }
    }

    fn with_transport(mut self, transport: &'static str) -> Self {
        self.transport = Some(transport);
        self
    }
}

fn log_upstream_request_start(event: UpstreamRequestLog<'_>) {
    tracing::debug!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.request",
        event = "start",
        upstream = %event.upstream,
        capability = event.capability,
        operation = event.operation,
        subject_scoped = event.subject_scoped,
        transport = event.transport,
        item_kind = event.item_kind,
        item = event.item,
        "upstream.request.start"
    );
}

fn log_upstream_request_finish(
    event: UpstreamRequestLog<'_>,
    elapsed_ms: u128,
    response_bytes: Option<usize>,
) {
    tracing::info!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.request",
        event = "finish",
        upstream = %event.upstream,
        capability = event.capability,
        operation = event.operation,
        subject_scoped = event.subject_scoped,
        transport = event.transport,
        item_kind = event.item_kind,
        item = event.item,
        elapsed_ms,
        response_bytes,
        "upstream.request.finish"
    );
}

fn log_upstream_request_error(
    event: UpstreamRequestLog<'_>,
    elapsed_ms: u128,
    kind: &'static str,
    error: Option<&dyn std::fmt::Display>,
    response_bytes: Option<usize>,
    max_bytes: Option<usize>,
) {
    tracing::warn!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.request",
        event = "error",
        upstream = %event.upstream,
        capability = event.capability,
        operation = event.operation,
        subject_scoped = event.subject_scoped,
        transport = event.transport,
        item_kind = event.item_kind,
        item = event.item,
        elapsed_ms,
        kind,
        error = error.map(tracing::field::display),
        response_bytes,
        max_bytes,
        "upstream.request.error"
    );
}

async fn discover_capability_counts(
    name: &str,
    peer: &rmcp::service::Peer<RoleClient>,
    proxy_resources: bool,
    proxy_prompts: bool,
) -> (
    usize,
    Option<String>,
    UpstreamHealth,
    usize,
    Option<String>,
    UpstreamHealth,
) {
    let (resource_count, resource_error, resource_health) = if proxy_resources {
        tracing::info!(upstream = %name, capability = "resources", "starting upstream capability discovery");
        match tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_resources(None)).await {
            Ok(Ok(result)) => (result.resources.len(), None, UpstreamHealth::Healthy),
            Ok(Err(ref error)) if is_capability_unsupported(error) => {
                (0, None, UpstreamHealth::Healthy)
            }
            Ok(Err(error)) => (
                0,
                Some(format!("failed to list resources from upstream: {error}")),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
            Err(_) => (
                0,
                Some(format!(
                    "listing resources from upstream timed out after {}s",
                    DISCOVERY_TIMEOUT.as_secs()
                )),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
        }
    } else {
        (0, None, UpstreamHealth::Healthy)
    };

    let (prompt_count, prompt_error, prompt_health) = if proxy_prompts {
        tracing::info!(upstream = %name, capability = "prompts", "starting upstream capability discovery");
        match tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_prompts(None)).await {
            Ok(Ok(result)) => (result.prompts.len(), None, UpstreamHealth::Healthy),
            Ok(Err(ref error)) if is_capability_unsupported(error) => {
                (0, None, UpstreamHealth::Healthy)
            }
            Ok(Err(error)) => (
                0,
                Some(format!("failed to list prompts from upstream: {error}")),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
            Err(_) => (
                0,
                Some(format!(
                    "listing prompts from upstream timed out after {}s",
                    DISCOVERY_TIMEOUT.as_secs()
                )),
                UpstreamHealth::Unhealthy {
                    consecutive_failures: 1,
                },
            ),
        }
    } else {
        (0, None, UpstreamHealth::Healthy)
    };

    if let Some(error) = &resource_error {
        tracing::warn!(upstream = %name, error = %error, "failed to discover upstream resources");
    }
    if let Some(error) = &prompt_error {
        tracing::warn!(upstream = %name, error = %error, "failed to discover upstream prompts");
    }

    (
        resource_count,
        resource_error,
        resource_health,
        prompt_count,
        prompt_error,
        prompt_health,
    )
}

/// Merge upstream prompts deterministically and return the winning owner for each prompt.
fn merge_upstream_prompts(
    builtin_names: &[&str],
    mut upstream_prompts: Vec<(String, Vec<Prompt>)>,
) -> (Vec<Prompt>, HashMap<String, String>) {
    upstream_prompts.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    let mut prompts = Vec::new();
    let mut owners = HashMap::new();
    let mut seen_names: std::collections::HashSet<String> = builtin_names
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    for (upstream_name, upstream_prompts) in upstream_prompts {
        for prompt in upstream_prompts {
            let prompt_name = prompt.name.to_string();
            if seen_names.insert(prompt_name.clone()) {
                owners.insert(prompt_name, upstream_name.clone());
                prompts.push(prompt);
            } else {
                tracing::warn!(
                    upstream = %upstream_name,
                    prompt = %prompt.name,
                    "duplicate prompt name encountered while merging upstream prompts"
                );
            }
        }
    }

    (prompts, owners)
}

/// Normalize a proxied resource read so its contents use the gateway URI.
fn normalize_resource_result_uri(
    mut result: ReadResourceResult,
    gateway_uri: &str,
) -> ReadResourceResult {
    for content in &mut result.contents {
        match content {
            ResourceContents::TextResourceContents { uri, .. }
            | ResourceContents::BlobResourceContents { uri, .. } => {
                *uri = gateway_uri.to_string();
            }
        }
    }

    result
}

/// Rewrite an upstream resource's URI to the gateway-prefixed form.
///
/// Strips any embedded upstream name from existing `lab://upstream/…` URIs
/// and re-prefixes with the caller's `upstream_name`.
fn rewrite_resource_uri(resource: &mut Resource, upstream_name: &str) {
    let bare_uri = bare_upstream_resource_uri(&resource.uri);
    resource.uri = format!("lab://upstream/{upstream_name}/{bare_uri}");
}

fn bare_upstream_resource_uri(uri: &str) -> &str {
    uri.strip_prefix("lab://upstream/")
        .and_then(|rest| rest.split_once('/').map(|x| x.1).or(Some(rest)))
        .unwrap_or(uri)
}

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

impl std::fmt::Debug for UpstreamConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamConnection").finish_non_exhaustive()
    }
}

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
    fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    async fn acquire_peer(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        requested_operation: &'static str,
    ) -> Option<rmcp::service::Peer<RoleClient>> {
        let acquire_started = Instant::now();
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.acquire",
            event = "start",
            operation = "connection.acquire",
            requested_operation,
            upstream = %upstream_name,
            capability = capability_name(capability),
            "upstream pool acquire start"
        );
        let connections = self.connections.read().await;
        let connection_count = connections.len();
        let peer = connections.get(upstream_name).map(|conn| conn.peer.clone());
        drop(connections);
        let pool_size = self.catalog.read().await.len();
        let elapsed_ms = acquire_started.elapsed().as_millis();
        if peer.is_some() {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.acquire",
                event = "finish",
                operation = "connection.acquire",
                requested_operation,
                upstream = %upstream_name,
                capability = capability_name(capability),
                elapsed_ms,
                pool_size,
                connection_count,
                "upstream pool acquire finish"
            );
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.acquire",
                event = "empty",
                operation = "connection.acquire",
                requested_operation,
                upstream = %upstream_name,
                capability = capability_name(capability),
                elapsed_ms,
                kind = "upstream_not_connected",
                pool_size,
                connection_count,
                "upstream pool acquire empty"
            );
        }
        peer
    }

    pub async fn drain_for_swap(&self, reason: &'static str) {
        let started = Instant::now();
        let catalog_count = self.catalog.read().await.len();
        let connection_count = self.connections.read().await.len();
        let probe_task_count = self.probe_tasks.read().await.len();
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.drain",
            event = "start",
            operation = "pool.drain",
            reason,
            pool_size = catalog_count,
            connection_count,
            probe_task_count,
            "upstream pool drain start"
        );

        let cancelled_probe_count = {
            let mut tasks = self.probe_tasks.write().await;
            let count = tasks.len();
            for cancel in tasks.values() {
                cancel.cancel();
            }
            tasks.clear();
            count
        };
        let drained_connection_count = {
            let mut connections = self.connections.write().await;
            let count = connections.len();
            connections.clear();
            count
        };
        let drained_catalog_count = {
            let mut catalog = self.catalog.write().await;
            let count = catalog.len();
            catalog.clear();
            count
        };
        self.resource_upstreams.write().await.clear();

        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.drain",
            event = "finish",
            operation = "pool.drain",
            reason,
            elapsed_ms = started.elapsed().as_millis(),
            drained_catalog_count,
            drained_connection_count,
            cancelled_probe_count,
            "upstream pool drain finish"
        );
    }

    /// Connect to all configured upstreams in parallel and discover their tools.
    ///
    /// Each upstream gets a 15-second timeout. Failures are logged and the
    /// upstream is marked unhealthy, but do not prevent other upstreams from
    /// connecting.
    #[allow(clippy::too_many_lines)]
    pub async fn discover_all(&self, configs: &[UpstreamConfig]) {
        if configs.is_empty() {
            return;
        }

        // Validate name uniqueness and URI-safety before starting discovery.
        let mut seen_names = std::collections::HashSet::new();
        for config in configs {
            if !seen_names.insert(&config.name) {
                tracing::warn!(
                    upstream = %config.name,
                    "duplicate upstream name — skipping all but the first"
                );
            }
            if config.name.contains('/') || config.name.contains('?') || config.name.contains('#') {
                tracing::warn!(
                    upstream = %config.name,
                    "upstream name contains URI-unsafe characters (/, ?, #) — skipping"
                );
            }
        }

        // Track which upstreams have resource/prompt proxying enabled.
        let resource_names: Vec<String> = configs
            .iter()
            .filter(|c| c.enabled)
            .filter(|c| c.proxy_resources)
            .map(|c| c.name.clone())
            .collect();
        *self.resource_upstreams.write().await = resource_names;

        let mut futures = FuturesUnordered::new();
        let mut processed_names = std::collections::HashSet::new();
        let oauth_client_cache = self.oauth_client_cache.clone();
        let runtime_origin = self.runtime_origin.clone();
        let runtime_owner = self.runtime_owner.clone();

        for config in configs {
            if !config.enabled {
                continue;
            }
            // Skip duplicates (only process the first occurrence of each name).
            if !processed_names.insert(&config.name) {
                continue;
            }
            // Skip names with URI-unsafe characters.
            if config.name.contains('/') || config.name.contains('?') || config.name.contains('#') {
                continue;
            }
            // Validate config
            if let Err(msg) = validate_upstream_config(config) {
                tracing::warn!(
                    upstream = %config.name,
                    "skipping upstream: {msg}"
                );
                continue;
            }

            let config = config.clone();
            let probe_config = config.clone();
            let oauth_client_cache = oauth_client_cache.clone();
            let runtime_origin = runtime_origin.clone();
            let runtime_owner = runtime_owner.clone();
            futures.push(async move {
                let name = config.name.clone();
                match tokio::time::timeout(
                    DISCOVERY_TIMEOUT,
                    connect_upstream(
                        &config,
                        None,
                        oauth_client_cache.as_ref(),
                        runtime_origin.as_deref(),
                        runtime_owner.as_ref(),
                    ),
                )
                .await
                {
                    Ok(Ok((conn, tools))) => {
                        let (
                            resource_count,
                            resource_last_error,
                            resource_health,
                            prompt_count,
                            prompt_last_error,
                            prompt_health,
                        ) = discover_capability_counts(
                            &name,
                            &conn.peer,
                            config.proxy_resources,
                            config.proxy_prompts,
                        )
                        .await;
                        tracing::info!(
                            upstream = %name,
                            transport = upstream_transport(&config),
                            target = %upstream_target_redacted(&config),
                            tool_count = tools.len(),
                            resource_count,
                            prompt_count,
                            "upstream discovery succeeded"
                        );
                        Ok((
                            name,
                            config.expose_tools.clone(),
                            conn,
                            tools,
                            resource_count,
                            resource_last_error,
                            resource_health,
                            prompt_count,
                            prompt_last_error,
                            prompt_health,
                        ))
                    }
                    Ok(Err(e)) => {
                        let error = e.to_string();
                        tracing::warn!(
                            upstream = %name,
                            transport = upstream_transport(&config),
                            target = %upstream_target_redacted(&config),
                            error = %error,
                            "upstream discovery failed"
                        );
                        Err((name, error))
                    }
                    Err(_) => {
                        let error = format!(
                            "upstream discovery timed out after {}s",
                            DISCOVERY_TIMEOUT.as_secs()
                        );
                        tracing::warn!(
                            upstream = %name,
                            transport = upstream_transport(&config),
                            target = %upstream_target_redacted(&config),
                            timeout_secs = DISCOVERY_TIMEOUT.as_secs(),
                            "upstream discovery timed out"
                        );
                        Err((name, error))
                    }
                }
            });

            self.ensure_probe_task(probe_config);
        }

        // Track all tool names across upstreams to detect duplicates.
        let mut global_tool_names: HashMap<String, String> = HashMap::new();

        while let Some(result) = futures.next().await {
            match result {
                Ok((
                    name,
                    expose_tools,
                    conn,
                    tools,
                    resource_count,
                    resource_last_error,
                    resource_health,
                    prompt_count,
                    prompt_last_error,
                    prompt_health,
                )) => {
                    let upstream_name: Arc<str> = Arc::from(name.as_str());
                    let mut tool_map: HashMap<String, UpstreamTool> = HashMap::new();
                    for tool in tools {
                        let schema = if tool.input_schema.is_empty() {
                            None
                        } else {
                            Some(Value::Object((*tool.input_schema).clone()))
                        };
                        let tool_name = tool.name.to_string();
                        // Reject duplicate tool names across upstreams.
                        if let Some(existing_upstream) = global_tool_names.get(&tool_name) {
                            tracing::warn!(
                                tool = %tool_name,
                                upstream = %name,
                                existing_upstream = %existing_upstream,
                                "duplicate tool name across upstreams — skipping"
                            );
                            continue;
                        }
                        global_tool_names.insert(tool_name.clone(), name.clone());
                        tool_map.insert(
                            tool_name,
                            UpstreamTool {
                                tool,
                                input_schema: schema,
                                upstream_name: Arc::clone(&upstream_name),
                            },
                        );
                    }

                    let exposure_policy = resolve_exposure_policy(&name, expose_tools);

                    let entry = UpstreamEntry {
                        name: Arc::clone(&upstream_name),
                        tools: tool_map,
                        exposure_policy,
                        prompt_count,
                        resource_count,
                        prompt_names: Vec::new(),
                        resource_uris: Vec::new(),
                        tool_health: UpstreamHealth::Healthy,
                        prompt_health,
                        resource_health,
                        tool_unhealthy_since: None,
                        prompt_unhealthy_since: (!prompt_health.is_routable()).then(Instant::now),
                        resource_unhealthy_since: (!resource_health.is_routable())
                            .then(Instant::now),
                        tool_last_error: None,
                        prompt_last_error,
                        resource_last_error,
                    };

                    self.catalog.write().await.insert(name.clone(), entry);
                    self.connections.write().await.insert(name, conn);
                }
                Err((name, error_message)) => {
                    let entry = UpstreamEntry {
                        name: Arc::from(name.as_str()),
                        tools: HashMap::new(),
                        exposure_policy: ToolExposurePolicy::All,
                        prompt_count: 0,
                        resource_count: 0,
                        prompt_names: Vec::new(),
                        resource_uris: Vec::new(),
                        tool_health: UpstreamHealth::Unhealthy {
                            consecutive_failures: 1,
                        },
                        prompt_health: UpstreamHealth::Unhealthy {
                            consecutive_failures: 1,
                        },
                        resource_health: UpstreamHealth::Unhealthy {
                            consecutive_failures: 1,
                        },
                        tool_unhealthy_since: Some(Instant::now()),
                        prompt_unhealthy_since: Some(Instant::now()),
                        resource_unhealthy_since: Some(Instant::now()),
                        tool_last_error: Some(error_message.clone()),
                        prompt_last_error: Some(error_message.clone()),
                        resource_last_error: Some(error_message),
                    };
                    self.catalog.write().await.insert(name, entry);
                }
            }
        }
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
                        attempt = attempt.saturating_add(1);
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
                            kind = "upstream_reprobe_failed",
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

    pub async fn reprobe_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
    ) -> anyhow::Result<bool> {
        self.reprobe_upstream(config).await
    }

    async fn replace_catalog_tools(&self, config: &UpstreamConfig, tools: Vec<rmcp::model::Tool>) {
        let exposure_policy = resolve_exposure_policy(&config.name, config.expose_tools.clone());
        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let tools = tools
            .into_iter()
            .map(|tool| {
                let name = tool.name.to_string();
                (
                    name,
                    UpstreamTool {
                        input_schema: (!tool.input_schema.is_empty())
                            .then(|| Value::Object((*tool.input_schema).clone())),
                        tool,
                        upstream_name: Arc::clone(&upstream_name),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let mut catalog = self.catalog.write().await;
        if let Some(entry) = catalog.get_mut(&config.name) {
            entry.tools = tools;
            entry.exposure_policy = exposure_policy;
        }
    }

    pub async fn discover_all_with_in_process_peers(
        &self,
        configs: &[UpstreamConfig],
        registry: &ToolRegistry,
    ) {
        self.discover_all(configs).await;
        let services: Vec<RegisteredService> = registry
            .services()
            .iter()
            .filter(|service| !service.actions.is_empty())
            .cloned()
            .collect();
        self.register_in_process_service_list(services).await;
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
                        let schema = if tool.input_schema.is_empty() {
                            None
                        } else {
                            Some(Value::Object((*tool.input_schema).clone()))
                        };
                        tool_map.insert(
                            tool.name.to_string(),
                            UpstreamTool {
                                tool,
                                input_schema: schema,
                                upstream_name: Arc::clone(&registration.entry_name),
                            },
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
    /// NOTE: The size check is post-hoc — rmcp materializes the full response before
    /// we can inspect it. This guards against forwarding oversized payloads to callers
    /// but cannot prevent the memory allocation itself. A streaming limit would require
    /// rmcp transport-level support.
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
    pub async fn record_failure(&self, upstream_name: &str, error: impl Into<String>) {
        self.record_failure_for(upstream_name, UpstreamCapability::Tools, error)
            .await;
    }

    /// Record a failure for a specific upstream capability, potentially marking it unhealthy.
    ///
    /// After [`CIRCUIT_BREAKER_THRESHOLD`] consecutive failures, the upstream
    /// is excluded from the matching capability listing until a successful re-probe.
    pub async fn record_failure_for(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        error: impl Into<String>,
    ) {
        let mut catalog = self.catalog.write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            let error = error.into();
            let new_count = match entry.health_for(capability) {
                UpstreamHealth::Healthy => 1,
                UpstreamHealth::Unhealthy {
                    consecutive_failures,
                } => consecutive_failures + 1,
            };
            entry.set_health_for(
                capability,
                UpstreamHealth::Unhealthy {
                    consecutive_failures: new_count,
                },
            );
            if entry.unhealthy_since_for(capability).is_none() {
                entry.set_unhealthy_since_for(capability, Some(Instant::now()));
            }
            entry.set_last_error_for(capability, Some(error.clone()));
            if new_count >= types::CIRCUIT_BREAKER_THRESHOLD {
                tracing::warn!(
                    upstream = %upstream_name,
                    capability = ?capability,
                    consecutive_failures = new_count,
                    error = %error,
                    "circuit breaker open — upstream excluded from capability listing"
                );
            }
        }
    }

    /// Record a success for an upstream capability, resetting the circuit breaker.
    pub async fn record_success(&self, upstream_name: &str) {
        self.record_success_for(upstream_name, UpstreamCapability::Tools)
            .await;
    }

    /// Record a success for a specific upstream capability, resetting the circuit breaker.
    pub async fn record_success_for(&self, upstream_name: &str, capability: UpstreamCapability) {
        let mut catalog = self.catalog.write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            if !entry.health_for(capability).is_routable() {
                tracing::info!(
                    upstream = %upstream_name,
                    capability = ?capability,
                    "circuit breaker reset — upstream healthy"
                );
            }
            entry.set_health_for(capability, UpstreamHealth::Healthy);
            entry.set_unhealthy_since_for(capability, None);
            entry.set_last_error_for(capability, None);
        }
    }

    /// Return the most relevant last error for an upstream, if any capability has one.
    pub async fn upstream_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Tools)
            .or_else(|| entry.last_error_for(UpstreamCapability::Resources))
            .or_else(|| entry.last_error_for(UpstreamCapability::Prompts))
            .map(ToOwned::to_owned)
    }

    /// Return the last tools-capability error for an upstream, if any.
    pub async fn upstream_tool_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Tools)
            .map(ToOwned::to_owned)
    }

    #[cfg(test)]
    pub async fn insert_entry_for_tests(&self, name: &str, entry: UpstreamEntry) {
        self.catalog.write().await.insert(name.to_string(), entry);
    }

    /// Check if an upstream capability is due for a re-probe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn should_reprobe(&self, upstream_name: &str) -> bool {
        self.should_reprobe_for(upstream_name, UpstreamCapability::Tools)
            .await
    }

    /// Check if a specific upstream capability is due for a re-probe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn should_reprobe_for(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
    ) -> bool {
        let catalog = self.catalog.read().await;
        if let Some(entry) = catalog.get(upstream_name)
            && entry.health_for(capability).is_open()
            && let Some(since) = entry.unhealthy_since_for(capability)
        {
            return since.elapsed() >= types::REPROBE_INTERVAL;
        }
        false
    }

    /// Filter out upstream tools whose names collide with built-in service tools.
    ///
    /// Built-in lab services permanently take precedence. Upstream tools with
    /// colliding names are dropped with a warning.
    pub async fn filter_collisions(&self, builtin_names: &[&str]) {
        let mut catalog = self.catalog.write().await;
        for entry in catalog.values_mut() {
            let collisions: Vec<String> = entry
                .tools
                .keys()
                .filter(|name| builtin_names.contains(&name.as_str()))
                .cloned()
                .collect();
            for name in &collisions {
                tracing::warn!(
                    upstream = %entry.name,
                    tool = %name,
                    "upstream tool name collides with built-in service — rejecting upstream tool"
                );
                entry.tools.remove(name);
            }
        }
    }

    /// Get the number of connected upstreams.
    pub async fn upstream_count(&self) -> usize {
        self.catalog.read().await.len()
    }

    /// Get names of all registered upstreams with their tool health status.
    pub async fn upstream_status(&self) -> Vec<(String, UpstreamHealth)> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .map(|e| (e.name.to_string(), e.tool_health))
            .collect()
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

/// Estimate the serialized size of a `CallToolResult`.
///
/// Uses `serde_json::to_string` as a reasonable approximation. Not exact
/// (ignores transport framing) but sufficient for the size cap guard.
fn estimate_response_size(result: &CallToolResult) -> usize {
    serde_json::to_string(result).map_or(0, |s| s.len())
}

/// Validate an upstream config entry.
fn validate_upstream_config(config: &UpstreamConfig) -> Result<(), String> {
    if config.name.is_empty() {
        return Err("upstream name cannot be empty".into());
    }

    if config.url.is_some() && config.command.is_some() {
        return Err("upstream must not set both 'url' and 'command'".into());
    }

    // Must have either a URL or a command
    if config.url.is_none() && config.command.is_none() {
        return Err("upstream must have either 'url' or 'command'".into());
    }

    if let Some(ref url_str) = config.url {
        // Reject schemes outside the supported HTTP and WebSocket transports.
        if !url_str.starts_with("http://")
            && !url_str.starts_with("https://")
            && !url_str.starts_with("ws://")
            && !url_str.starts_with("wss://")
        {
            return Err(format!(
                "upstream URL must use http://, https://, ws://, or wss:// scheme, got: {url_str}"
            ));
        }
        // Parse with url::Url to reliably check the host.
        let parsed = url::Url::parse(url_str)
            .map_err(|e| format!("invalid upstream URL `{url_str}`: {e}"))?;
        if let Some(host) = parsed.host_str() {
            // Reject bind-all addresses (0.0.0.0 or ::).
            let normalized = host.trim_start_matches('[').trim_end_matches(']');
            if normalized == "0.0.0.0" || normalized == "::" {
                return Err("upstream URL must not use 0.0.0.0 or :: (bind-all addresses)".into());
            }
        }
    }

    Ok(())
}

/// Connect to a single upstream MCP server and discover its tools.
async fn connect_upstream(
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    let started = Instant::now();
    tracing::debug!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.connect",
        event = "attempt",
        operation = "connection.acquire",
        upstream = %config.name,
        transport = upstream_transport(config),
        target = %upstream_target_redacted(config),
        subject_scoped = subject.is_some(),
        "upstream connection acquire attempt"
    );
    let result = if let Some(ref url) = config.url {
        if is_websocket_url(url) {
            connect_websocket_upstream(url, config).await
        } else {
            connect_http_upstream(url, config, subject, oauth_client_cache).await
        }
    } else if let Some(ref command) = config.command {
        connect_stdio_upstream(command, &config.args, config, runtime_origin, runtime_owner).await
    } else {
        Err(anyhow::anyhow!(
            "upstream {} has neither url nor command",
            config.name
        ))
    };
    match &result {
        Ok((_, tools)) => tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.connect",
            event = "finish",
            operation = "connection.acquire",
            upstream = %config.name,
            transport = upstream_transport(config),
            target = %upstream_target_redacted(config),
            subject_scoped = subject.is_some(),
            tool_count = tools.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream connection acquire finish"
        ),
        Err(error) => tracing::warn!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.connect",
            event = "error",
            operation = "connection.acquire",
            upstream = %config.name,
            transport = upstream_transport(config),
            target = %upstream_target_redacted(config),
            subject_scoped = subject.is_some(),
            kind = "upstream_connect_error",
            error = %error,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream connection acquire error"
        ),
    }
    result
}

async fn connect_websocket_upstream(
    url: &str,
    config: &UpstreamConfig,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "websocket",
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    if config.oauth.is_some() {
        anyhow::bail!(
            "upstream {} declares oauth, but websocket upstream oauth is not yet supported",
            config.name
        );
    }

    let parsed = parse_ws_url(url).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let authorization = websocket_authorization_header(config);
    let transport = connect_websocket_transport(
        WebSocketTransportConfig::new(parsed.to_string()).with_authorization(authorization),
    );
    let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(transport).await?;
    let peer = service.peer().clone();
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "websocket",
        action = "upstream.connect.finish", tool_count = tools.len(),
        "upstream connect finish",
    );
    Ok((
        UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

fn stable_jitter_seed(name: &str, attempt: u32) -> u64 {
    let mut hash = 1_469_598_103_934_665_603_u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash ^ u64::from(attempt)
}

/// Connect to an HTTP upstream MCP server.
async fn connect_http_upstream(
    url: &str,
    config: &UpstreamConfig,
    subject: Option<&str>,
    oauth_client_cache: Option<&OauthClientCache>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "http",
        action = "upstream.connect.start", target = %upstream_target_redacted(config),
        "upstream connect start",
    );
    let transport_config = StreamableHttpClientTransportConfig::with_uri(url);

    // OAuth path: when the upstream declares oauth config, build an AuthClient.
    if config.oauth.is_some() {
        let subject = subject.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires an authenticated subject; discovery must be request-scoped",
                config.name
            )
        })?;
        let cache = oauth_client_cache.ok_or_else(|| {
            anyhow::anyhow!(
                "upstream {} requires OAuth but no auth client cache is registered",
                config.name
            )
        })?;

        let auth_client = cache
            .get_or_build(config, subject)
            .await
            .map_err(|e| anyhow::anyhow!("oauth_required: {e}"))?;

        let worker = StreamableHttpClientWorker::new((*auth_client).clone(), transport_config);
        let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(worker).await?;
        let peer = service.peer().clone();
        let tools = peer.list_all_tools().await?;
        return Ok((
            UpstreamConnection {
                _client_service: service,
                _server_task: None,
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
            tools,
        ));
    }

    // Non-OAuth path: optionally inject a static bearer token from env.
    let mut transport_config = transport_config;
    if let Some(ref env_name) = config.bearer_token_env {
        if let Some(token) = configured_bearer_token(env_name) {
            transport_config.auth_header = Some(token);
        } else {
            tracing::warn!(
                upstream = %config.name,
                env_var = %env_name,
                "bearer_token_env configured but env var not set"
            );
        }
    }

    let client = reqwest::Client::builder()
        .timeout(DEFAULT_REQUEST_TIMEOUT)
        .build()?;
    let worker = StreamableHttpClientWorker::new(client, transport_config);
    let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(worker).await?;
    let peer = service.peer().clone();
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "http",
        action = "upstream.connect.finish", tool_count = tools.len(),
        "upstream connect finish",
    );

    Ok((
        UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

/// Connect to a stdio upstream MCP server (child process).
async fn connect_stdio_upstream(
    command: &str,
    args: &[String],
    config: &UpstreamConfig,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    #[cfg(unix)]
    use process_wrap::tokio::{CommandWrap, ProcessGroup};
    use rmcp::transport::child_process::TokioChildProcess;
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Command::new(command);
    cmd.args(args);

    // Set bearer token env var on the child if configured
    if let Some(ref env_name) = config.bearer_token_env
        && let Some(token) = configured_bearer_token(env_name)
    {
        cmd.env(env_name, &token);
    }

    #[cfg(unix)]
    let (process, _stderr) = {
        let mut wrapped = CommandWrap::from(cmd);
        wrapped.wrap(ProcessGroup::leader());
        TokioChildProcess::builder(wrapped)
            .stderr(Stdio::null())
            .spawn()?
    };
    #[cfg(not(unix))]
    let (process, _stderr) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::null())
        .spawn()?;

    let pid = process.id();
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "stdio",
        action = "upstream.connect.start", command = %command, pid = ?pid,
        "upstream connect start",
    );
    let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(process).await?;
    let peer = service.peer().clone();

    // Discover tools
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "stdio",
        action = "upstream.connect.finish", pid = ?pid, tool_count = tools.len(),
        "upstream connect finish",
    );

    let conn = UpstreamConnection {
        _client_service: service,
        _server_task: None,
        peer,
        runtime: UpstreamRuntimeMetadata {
            pid,
            pgid: pid,
            started_at: Some(std::time::SystemTime::now()),
            origin: runtime_origin_label(runtime_origin, runtime_owner),
            owner: runtime_owner.cloned(),
        },
    };

    Ok((conn, tools))
}

fn runtime_origin_label(
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> Option<String> {
    if let Some(raw) = runtime_owner
        .and_then(|owner| owner.raw.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(raw.to_string());
    }

    if let Some(origin) = runtime_origin
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(origin.to_string());
    }

    for (prefix, session_key) in [
        ("claude-code", "CLAUDE_SESSION_ID"),
        ("codex", "CODEX_SESSION_ID"),
    ] {
        if let Ok(session) = std::env::var(session_key) {
            let trimmed = session.trim();
            if !trimmed.is_empty() {
                return Some(format!("{prefix}:{trimmed}"));
            }
        }
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let trimmed = term_program.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    Some("gateway-managed".to_string())
}

async fn connect_in_process_service_peer(
    service: &RegisteredService,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        service = service.name,
        phase = "in_process.connect.start",
        "connecting in-process peer"
    );
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let mut registry = ToolRegistry::new();
    registry.register(service.clone());
    let server = LabMcpServer {
        registry: Arc::new(registry),
        gateway_manager: None,
        node_role: None,
        peers: Arc::new(RwLock::new(Vec::new())),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Emergency))),
    };
    let service_name = service.name;
    let server_task = tokio::spawn(async move {
        tracing::info!(
            service = service_name,
            phase = "in_process.server.spawned",
            "starting in-process server task"
        );
        match server.serve(server_transport).await {
            Ok(running) => {
                tracing::info!(
                    service = service_name,
                    phase = "in_process.server.ready",
                    "in-process server transport ready"
                );
                if let Err(error) = running.waiting().await {
                    tracing::warn!(service = service_name, phase = "in_process.server.waiting.error", error = %error, "in-process server exited with error");
                }
            }
            Err(error) => {
                tracing::warn!(service = service_name, phase = "in_process.server.serve.error", error = %error, "failed to start in-process server");
            }
        }
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> =
        ().serve(client_transport).await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.client.ready",
        "in-process client transport ready"
    );
    let peer = client_service.peer().clone();
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.start",
        "requesting in-process tool list"
    );
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.finish",
        tool_count = tools.len(),
        "in-process tool list received"
    );

    Ok((
        UpstreamConnection {
            _client_service: client_service,
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}

fn healthy_in_process_entry(name: Arc<str>, tools: HashMap<String, UpstreamTool>) -> UpstreamEntry {
    UpstreamEntry {
        name,
        tools,
        exposure_policy: ToolExposurePolicy::All,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

fn failed_in_process_entry(name: Arc<str>, error_message: String) -> UpstreamEntry {
    UpstreamEntry {
        name,
        tools: HashMap::new(),
        exposure_policy: ToolExposurePolicy::All,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Unhealthy {
            consecutive_failures: 1,
        },
        prompt_health: UpstreamHealth::Unhealthy {
            consecutive_failures: 1,
        },
        resource_health: UpstreamHealth::Unhealthy {
            consecutive_failures: 1,
        },
        tool_unhealthy_since: Some(Instant::now()),
        prompt_unhealthy_since: Some(Instant::now()),
        resource_unhealthy_since: Some(Instant::now()),
        tool_last_error: Some(error_message.clone()),
        prompt_last_error: Some(error_message.clone()),
        resource_last_error: Some(error_message),
    }
}

fn failed_in_process_entry_from_existing(
    mut existing: UpstreamEntry,
    error_message: String,
) -> UpstreamEntry {
    existing.tool_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    existing.prompt_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    existing.resource_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    existing.tool_unhealthy_since = Some(Instant::now());
    existing.prompt_unhealthy_since = Some(Instant::now());
    existing.resource_unhealthy_since = Some(Instant::now());
    existing.tool_last_error = Some(error_message.clone());
    existing.prompt_last_error = Some(error_message.clone());
    existing.resource_last_error = Some(error_message);
    existing
}

fn resolve_exposure_policy(
    upstream_name: &str,
    expose_tools: Option<Vec<String>>,
) -> ToolExposurePolicy {
    match ToolExposurePolicy::from_optional(expose_tools) {
        Ok(policy) => policy,
        Err(error) => {
            tracing::warn!(
                upstream = %upstream_name,
                error = %error,
                "invalid upstream exposure policy; hiding all upstream tools"
            );
            ToolExposurePolicy::AllowList(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration};
    use crate::mcp::server::LabMcpServer;
    use rmcp::model::{
        AnnotateAble, ErrorData, ListPromptsResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, PromptMessage, PromptMessageRole, RawResource,
        ReadResourceRequestParams, ServerCapabilities, ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tracing_subscriber::layer::SubscriberExt;

    fn test_upstream_config() -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        }
    }

    #[test]
    fn upstream_target_redacts_url_credentials_and_sensitive_query_values() {
        let mut config = test_upstream_config();
        config.url = Some("https://user:pass@example.com/mcp?token=secret&mode=1#frag".into());

        assert_eq!(
            upstream_target_redacted(&config),
            "https://example.com/mcp?token=[redacted]&mode=1"
        );
    }

    #[test]
    fn upstream_target_redacts_stdio_secret_flags() {
        let mut config = test_upstream_config();
        config.command = Some("--api-key=secret".into());

        assert_eq!(upstream_target_redacted(&config), "--api-key=[redacted]");
    }

    #[test]
    fn upstream_request_log_helpers_emit_documented_fields_and_inherit_request_id() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("labby=debug"))
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);

        let span = tracing::info_span!(
            "dispatch",
            surface = "api",
            service = "gateway",
            action = "call_tool",
            request_id = "req-123"
        );
        let _entered = span.enter();

        let event = UpstreamRequestLog::tool("github", "search_repos", false);
        log_upstream_request_start(event);
        log_upstream_request_finish(event, 7, Some(128));
        log_upstream_request_error(event, 9, "upstream_error", Some(&"boom"), None, None);

        drop(_entered);
        drop(_guard);

        let logs = crate::test_support::captured_logs(&buf);
        for expected in [
            "\"request_id\":\"req-123\"",
            "\"surface\":\"dispatch\"",
            "\"service\":\"upstream.pool\"",
            "\"action\":\"upstream.request\"",
            "\"upstream\":\"github\"",
            "\"capability\":\"tools\"",
            "\"operation\":\"tool.call\"",
            "\"event\":\"start\"",
            "\"event\":\"finish\"",
            "\"event\":\"error\"",
            "\"elapsed_ms\":\"7\"",
            "\"elapsed_ms\":\"9\"",
            "\"kind\":\"upstream_error\"",
        ] {
            assert!(
                logs.contains(expected),
                "missing upstream request log field `{expected}` in:\n{logs}"
            );
        }
    }

    #[test]
    fn validate_rejects_empty_name() {
        let config = UpstreamConfig {
            enabled: true,
            name: String::new(),
            url: Some("http://localhost:8080".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    #[test]
    fn validate_rejects_non_http_scheme() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some("ftp://example.com".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    #[test]
    fn validate_rejects_bind_all_addresses() {
        for url in &["http://0.0.0.0:8080", "http://[::]/mcp", "http://[::]:8080"] {
            let config = UpstreamConfig {
                enabled: true,
                name: "test".into(),
                url: Some((*url).into()),
                bearer_token_env: None,
                command: None,
                args: vec![],
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            };
            assert!(
                validate_upstream_config(&config).is_err(),
                "should reject {url}"
            );
        }
    }

    #[test]
    fn validate_accepts_valid_http_url() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some("http://localhost:8080/mcp".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_ok());
    }

    #[test]
    fn validate_accepts_valid_websocket_urls() {
        for url in ["ws://localhost:8080/mcp", "wss://example.com/socket"] {
            let config = UpstreamConfig {
                enabled: true,
                name: "test".into(),
                url: Some(url.into()),
                bearer_token_env: None,
                command: None,
                args: vec![],
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                tool_search: crate::config::ToolSearchConfig::default(),
            };
            assert!(
                validate_upstream_config(&config).is_ok(),
                "{url} should validate"
            );
        }
    }

    #[test]
    fn validate_accepts_stdio_command() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: Some("my-mcp-server".into()),
            args: vec!["--port".into(), "8080".into()],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_ok());
    }

    #[test]
    fn validate_rejects_both_url_and_command() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: Some("http://localhost:8080".into()),
            bearer_token_env: None,
            command: Some("my-mcp-server".into()),
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    #[test]
    fn validate_rejects_no_url_or_command() {
        let config = UpstreamConfig {
            enabled: true,
            name: "test".into(),
            url: None,
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: None,
            tool_search: crate::config::ToolSearchConfig::default(),
        };
        assert!(validate_upstream_config(&config).is_err());
    }

    fn oauth_http_config() -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "oauth-upstream".into(),
            url: Some("http://127.0.0.1:8080/mcp".into()),
            bearer_token_env: None,
            command: None,
            args: vec![],
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: "client-id".into(),
                    client_secret_env: None,
                },
                scopes: None,
            }),
            tool_search: crate::config::ToolSearchConfig::default(),
        }
    }

    #[tokio::test]
    async fn subject_scoped_upstream_requires_authenticated_subject_for_oauth_http_connect() {
        let config = oauth_http_config();
        let error = connect_http_upstream(
            config.url.as_deref().expect("url"),
            &config,
            None,
            Some(&OauthClientCache::new(Arc::new(dashmap::DashMap::new()))),
        )
        .await
        .expect_err("missing subject should fail");

        assert!(
            error
                .to_string()
                .contains("requires an authenticated subject")
        );
    }

    #[tokio::test]
    async fn subject_scoped_upstream_requires_registered_cache_for_oauth_http_connect() {
        let config = oauth_http_config();
        let error = connect_http_upstream(
            config.url.as_deref().expect("url"),
            &config,
            Some("alice"),
            None,
        )
        .await
        .expect_err("missing cache should fail");

        assert!(
            error
                .to_string()
                .contains("no auth client cache is registered")
        );
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

    #[derive(Clone, Default)]
    struct StaticCatalogServer {
        list_prompts_count: Arc<AtomicUsize>,
        get_prompt_count: Arc<AtomicUsize>,
        fail_list_prompts: Arc<AtomicBool>,
    }

    impl ServerHandler for StaticCatalogServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_resources()
                    .enable_prompts()
                    .build(),
            )
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![
                RawResource::new("file:///tmp/upstream-one", "upstream-one").no_annotation(),
                RawResource::new(
                    "lab://upstream/old-name/file:///tmp/upstream-two",
                    "upstream-two",
                )
                .no_annotation(),
            ]))
        }

        async fn list_prompts(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            self.list_prompts_count.fetch_add(1, Ordering::SeqCst);
            if self.fail_list_prompts.load(Ordering::SeqCst) {
                return Err(ErrorData::internal_error(
                    "prompt listing failed for test",
                    None,
                ));
            }

            Ok(ListPromptsResult::with_all_items(vec![
                Prompt::new("upstream.prompt.one", Some("first prompt"), None),
                Prompt::new("upstream.prompt.two", Some("second prompt"), None),
            ]))
        }

        async fn get_prompt(
            &self,
            request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResult, ErrorData> {
            self.get_prompt_count.fetch_add(1, Ordering::SeqCst);
            Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                PromptMessageRole::User,
                format!("proxied {}", request.name),
            )]))
        }
    }

    async fn static_catalog_pool(upstream_name: &str) -> Arc<UpstreamPool> {
        static_catalog_pool_with_server(upstream_name, StaticCatalogServer::default()).await
    }

    async fn static_catalog_pool_with_server(
        upstream_name: &str,
        server: StaticCatalogServer,
    ) -> Arc<UpstreamPool> {
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("static catalog server starts");
            running.waiting().await.expect("static catalog server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("static catalog client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog.write().await.insert(
            upstream_name.to_string(),
            UpstreamEntry {
                name: Arc::clone(&upstream_name_arc),
                tools: HashMap::new(),
                exposure_policy: ToolExposurePolicy::All,
                prompt_count: 0,
                resource_count: 0,
                prompt_names: Vec::new(),
                resource_uris: Vec::new(),
                tool_health: UpstreamHealth::Healthy,
                prompt_health: UpstreamHealth::Healthy,
                resource_health: UpstreamHealth::Healthy,
                tool_unhealthy_since: None,
                prompt_unhealthy_since: None,
                resource_unhealthy_since: None,
                tool_last_error: None,
                prompt_last_error: None,
                resource_last_error: None,
            },
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service,
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
        );
        pool.resource_upstreams
            .write()
            .await
            .push(upstream_name.to_string());

        pool
    }

    struct SlowResponseServer;

    impl ServerHandler for SlowResponseServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_resources()
                    .enable_prompts()
                    .build(),
            )
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult::with_all_items(Vec::new()))
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, ErrorData> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(CallToolResult::success(Vec::new()))
        }

        async fn read_resource(
            &self,
            _request: ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<ReadResourceResult, ErrorData> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(ReadResourceResult::new(Vec::new()))
        }

        async fn get_prompt(
            &self,
            _request: GetPromptRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<GetPromptResult, ErrorData> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(GetPromptResult::new(Vec::new()))
        }
    }

    async fn slow_response_pool(upstream_name: &str) -> Arc<UpstreamPool> {
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = SlowResponseServer
                .serve(server_transport)
                .await
                .expect("slow response server starts");
            running.waiting().await.expect("slow response server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("slow response client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new().with_request_timeout(Duration::from_millis(25)));
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog.write().await.insert(
            upstream_name.to_string(),
            UpstreamEntry {
                name: Arc::clone(&upstream_name_arc),
                tools: HashMap::new(),
                exposure_policy: ToolExposurePolicy::All,
                prompt_count: 1,
                resource_count: 1,
                prompt_names: vec!["slow.prompt".to_string()],
                resource_uris: vec!["file:///tmp/slow".to_string()],
                tool_health: UpstreamHealth::Healthy,
                prompt_health: UpstreamHealth::Healthy,
                resource_health: UpstreamHealth::Healthy,
                tool_unhealthy_since: None,
                prompt_unhealthy_since: None,
                resource_unhealthy_since: None,
                tool_last_error: None,
                prompt_last_error: None,
                resource_last_error: None,
            },
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service,
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
        );
        pool.resource_upstreams
            .write()
            .await
            .push(upstream_name.to_string());

        pool
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
        let mut tools = HashMap::new();
        for name in ["search_repos", "github_create_issue", "delete_repo"] {
            let schema = Arc::new(serde_json::Map::new());
            let tool = rmcp::model::Tool::new(name, format!("{name} description"), schema);
            tools.insert(
                name.to_string(),
                UpstreamTool {
                    tool,
                    input_schema: None,
                    upstream_name: Arc::clone(&upstream_name),
                },
            );
        }
        let entry = UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools,
            exposure_policy: ToolExposurePolicy::from_patterns(vec![
                "search_repos".to_string(),
                "github_*".to_string(),
            ])
            .expect("policy"),
            prompt_count: 0,
            resource_count: 0,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: UpstreamHealth::Healthy,
            prompt_health: UpstreamHealth::Healthy,
            resource_health: UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        };

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
        let mut tools = HashMap::new();
        for name in ["search_repos", "delete_repo"] {
            let schema = Arc::new(serde_json::Map::new());
            let tool = rmcp::model::Tool::new(name, format!("{name} description"), schema);
            tools.insert(
                name.to_string(),
                UpstreamTool {
                    tool,
                    input_schema: None,
                    upstream_name: Arc::clone(&upstream_name),
                },
            );
        }
        let entry = UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools,
            exposure_policy: ToolExposurePolicy::from_patterns(vec!["search_repos".into()])
                .expect("policy"),
            prompt_count: 0,
            resource_count: 0,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: UpstreamHealth::Healthy,
            prompt_health: UpstreamHealth::Healthy,
            resource_health: UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        };

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        assert!(pool.find_tool("search_repos").await.is_some());
        assert!(pool.find_tool("delete_repo").await.is_none());
    }

    #[tokio::test]
    async fn upstream_last_error_tracks_capability_failure_details() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            exposure_policy: ToolExposurePolicy::All,
            prompt_count: 0,
            resource_count: 0,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: UpstreamHealth::Healthy,
            prompt_health: UpstreamHealth::Healthy,
            resource_health: UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        };

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Resources,
            "resource listing returned 401 unauthorized",
        )
        .await;

        assert_eq!(
            pool.upstream_last_error("github").await.as_deref(),
            Some("resource listing returned 401 unauthorized")
        );

        pool.record_success_for("github", UpstreamCapability::Resources)
            .await;
        assert_eq!(pool.upstream_last_error("github").await, None);
    }

    #[tokio::test]
    async fn upstream_tool_last_error_ignores_non_tool_failures() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            exposure_policy: ToolExposurePolicy::All,
            prompt_count: 0,
            resource_count: 0,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: UpstreamHealth::Healthy,
            prompt_health: UpstreamHealth::Healthy,
            resource_health: UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        };

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Resources,
            "resource listing returned 401 unauthorized",
        )
        .await;
        pool.record_failure_for(
            "github",
            UpstreamCapability::Prompts,
            "prompt listing returned 501 unsupported",
        )
        .await;

        assert_eq!(pool.upstream_tool_last_error("github").await, None);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Tools,
            "tool listing returned 500 internal error",
        )
        .await;

        assert_eq!(
            pool.upstream_tool_last_error("github").await.as_deref(),
            Some("tool listing returned 500 internal error")
        );
    }

    #[test]
    fn failed_in_process_entry_from_existing_preserves_last_known_good_catalog() {
        let upstream_name: Arc<str> = Arc::from("labby::github-chat");
        let schema = Arc::new(serde_json::Map::new());
        let tool = rmcp::model::Tool::new("query_repository", "Query a GitHub repository", schema);
        let mut tools = HashMap::new();
        tools.insert(
            "query_repository".to_string(),
            UpstreamTool {
                tool,
                input_schema: None,
                upstream_name: Arc::clone(&upstream_name),
            },
        );

        let existing = UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools,
            exposure_policy: ToolExposurePolicy::from_patterns(vec![
                "query_repository".to_string(),
            ])
            .expect("policy"),
            prompt_count: 2,
            resource_count: 3,
            prompt_names: vec!["prompt.one".into(), "prompt.two".into()],
            resource_uris: vec!["lab://resource/one".into(), "lab://resource/two".into()],
            tool_health: UpstreamHealth::Healthy,
            prompt_health: UpstreamHealth::Healthy,
            resource_health: UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        };

        let failed = failed_in_process_entry_from_existing(
            existing,
            "in-process peer registration timed out after 5s".to_string(),
        );

        assert_eq!(failed.tools.len(), 1);
        assert!(failed.tools.contains_key("query_repository"));
        assert_eq!(failed.prompt_count, 2);
        assert_eq!(failed.resource_count, 3);
        assert_eq!(failed.prompt_names.len(), 2);
        assert_eq!(failed.resource_uris.len(), 2);
        assert!(matches!(
            failed.exposure_policy,
            ToolExposurePolicy::AllowList(_)
        ));
        assert!(matches!(
            failed.tool_health,
            UpstreamHealth::Unhealthy {
                consecutive_failures: 1
            }
        ));
        assert_eq!(
            failed.tool_last_error.as_deref(),
            Some("in-process peer registration timed out after 5s")
        );
        assert_eq!(
            failed.prompt_last_error.as_deref(),
            Some("in-process peer registration timed out after 5s")
        );
        assert_eq!(
            failed.resource_last_error.as_deref(),
            Some("in-process peer registration timed out after 5s")
        );
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
    fn invalid_exposure_policy_fails_closed() {
        let policy = resolve_exposure_policy("github", Some(vec!["   ".to_string()]));
        assert_eq!(policy, ToolExposurePolicy::AllowList(Vec::new()));
        assert!(!policy.matches("search_repos"));
    }

    #[test]
    fn observability_source_covers_pool_acquire_reprobe_and_drain_events() {
        let source = include_str!("pool.rs");
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
}
