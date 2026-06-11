//! Upstream discovery: connect to all configured upstreams, probe capabilities,
//! populate the catalog/connection maps, and schedule background reprobe tasks.
//!
//! `discover_all_inner` is the bulk-discovery engine; the `discover_all*` public
//! variants and the in-process-peer-registering wrappers are thin shims over it.
//! `routable_upstream_peers` collects healthy peers for a capability in
//! deterministic order and is `pub(super)` because the tools/resources/prompts
//! modules call it across the module boundary.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;
use rmcp::RoleClient;

use crate::config::UpstreamConfig;
use crate::registry::ToolRegistry;

use super::super::types::{
    ToolExposurePolicy, UpstreamCapability, UpstreamEntry, UpstreamHealth, UpstreamTool,
};
use super::UpstreamPool;
use super::capability::discover_capability_counts;
use super::connect::connect_upstream;
use super::entries::resolve_exposure_policy;
use super::helpers::{
    DISCOVERY_TIMEOUT, cached_upstream_tool, classify_upstream_error,
    upstream_discovery_concurrency, upstream_name_is_uri_safe, upstream_target_redacted,
    upstream_transport,
};
use super::logging::capability_name;
use super::validate::validate_upstream_config;

#[derive(Clone, Copy)]
enum DiscoveryLifecycle {
    LongLived,
    Ephemeral,
}

/// Collect upstream peers for a capability in deterministic name order.
pub(super) async fn routable_upstream_peers(
    pool: &UpstreamPool,
    capability: UpstreamCapability,
    allowed_upstreams: Option<&BTreeSet<String>>,
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
        if let Some(allowed) = allowed_upstreams {
            names.retain(|name| allowed.contains(name));
        }
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

impl UpstreamPool {
    /// Connect to all configured upstreams and discover their tools.
    ///
    /// Each upstream gets a 15-second timeout. Failures are logged and the
    /// upstream is marked unhealthy, but do not prevent other upstreams from
    /// connecting.
    #[allow(clippy::too_many_lines)]
    async fn discover_all_inner(
        &self,
        configs: &[UpstreamConfig],
        oauth_subject: Option<&str>,
        lifecycle: DiscoveryLifecycle,
    ) {
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
            if !upstream_name_is_uri_safe(&config.name) {
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

        let mut discovery_jobs = Vec::new();
        let mut probe_configs = Vec::new();
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
            if !upstream_name_is_uri_safe(&config.name) {
                continue;
            }
            // OAuth HTTP upstreams require an explicit subject to select the
            // correct token set. Subject-less discovery must skip them so one
            // user's tool view is never cached globally by accident.
            if config.oauth.is_some() && oauth_subject.is_none() {
                tracing::info!(
                    upstream = %config.name,
                    transport = upstream_transport(config),
                    target = %upstream_target_redacted(config),
                    "skipping shared upstream discovery for subject-scoped OAuth upstream"
                );
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
            let subject = config
                .oauth
                .as_ref()
                .and(oauth_subject)
                .map(ToOwned::to_owned);
            probe_configs.push(config.clone());
            discovery_jobs.push((config, subject));
        }

        let discovery_concurrency = upstream_discovery_concurrency();
        let mut futures = futures::stream::iter(discovery_jobs)
            .map(|(config, subject)| {
                let oauth_client_cache = oauth_client_cache.clone();
                let runtime_origin = runtime_origin.clone();
                let runtime_owner = runtime_owner.clone();
                async move {
                    let name = config.name.clone();
                    match tokio::time::timeout(
                        DISCOVERY_TIMEOUT,
                        connect_upstream(
                            &config,
                            subject.as_deref(),
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
                            let kind = classify_upstream_error(&error);
                            tracing::warn!(
                                upstream = %name,
                                transport = upstream_transport(&config),
                                target = %upstream_target_redacted(&config),
                                kind,
                                error = %error,
                                "upstream discovery failed"
                            );
                            Err((name, error))
                        }
                        Err(_) => {
                            let error = format!(
                                "upstream discovery timed out after {}s waiting for {} MCP list_tools response from {}",
                                DISCOVERY_TIMEOUT.as_secs(),
                                upstream_transport(&config),
                                upstream_target_redacted(&config)
                            );
                            tracing::warn!(
                                upstream = %name,
                                transport = upstream_transport(&config),
                                target = %upstream_target_redacted(&config),
                                kind = "timeout",
                                timeout_secs = DISCOVERY_TIMEOUT.as_secs(),
                                "upstream discovery timed out"
                            );
                            Err((name, error))
                        }
                    }
                }
            })
            .buffer_unordered(discovery_concurrency);

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
                        let (_, upstream_tool) = cached_upstream_tool(tool, &upstream_name);
                        tool_map.insert(tool_name, upstream_tool);
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

        if matches!(lifecycle, DiscoveryLifecycle::LongLived) {
            for config in probe_configs {
                self.ensure_probe_task(config).await;
            }
        }
    }

    /// Connect to non-OAuth upstreams and discover their tools.
    ///
    /// OAuth upstreams are intentionally skipped because they need a request or
    /// gateway subject to select the right upstream token set.
    pub async fn discover_all(&self, configs: &[UpstreamConfig]) {
        self.discover_all_inner(configs, None, DiscoveryLifecycle::LongLived)
            .await;
    }

    /// Connect to all configured upstreams, using `subject` for OAuth upstreams.
    ///
    /// This is for gateway-owned discovery where the subject is an explicit
    /// shared identity, not for subject-less startup discovery.
    pub async fn discover_all_for_subject(&self, configs: &[UpstreamConfig], subject: &str) {
        self.discover_all_inner(configs, Some(subject), DiscoveryLifecycle::LongLived)
            .await;
    }

    pub async fn discover_all_for_subject_ephemeral(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) {
        self.discover_all_inner(configs, Some(subject), DiscoveryLifecycle::Ephemeral)
            .await;
    }

    pub async fn discover_all_with_in_process_peers(
        &self,
        configs: &[UpstreamConfig],
        registry: &ToolRegistry,
    ) {
        self.discover_all(configs).await;
        self.register_in_process_service_peers(registry).await;
    }

    pub async fn discover_all_for_subject_with_in_process_peers(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        registry: &ToolRegistry,
    ) {
        self.discover_all_for_subject(configs, subject).await;
        self.register_in_process_service_peers(registry).await;
    }

    pub async fn discover_all_for_subject_ephemeral_with_in_process_peers(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        registry: &ToolRegistry,
    ) {
        self.discover_all_for_subject_ephemeral(configs, subject)
            .await;
        self.register_in_process_service_peers(registry).await;
    }
}
