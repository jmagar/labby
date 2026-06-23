//! Lazy upstream seeding and on-demand tool discovery.
//!
//! These methods seed the catalog from config without connecting, then connect
//! upstreams on demand (`ensure_tools_for_upstream`), single-flighting concurrent
//! requests through a per-upstream lock. `replace_catalog_tools` is the shared
//! catalog mutator after a tools probe; it is `pub(super)` because `probe.rs`
//! (`reprobe_upstream`) calls it across the module boundary (see plan §3.0/§2.1).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;

use lab_runtime::gateway_config::UpstreamConfig;

use super::super::types::{UpstreamCapability, UpstreamRuntimeOwner};
#[cfg(test)]
use super::TestUpstreamConnector;
use super::UpstreamPool;
use super::connect::connect_upstream_with_client;
use super::entries::{lazy_upstream_entry, resolve_exposure_policy};
use super::helpers::{
    DISCOVERY_TIMEOUT, cached_upstream_tool, upstream_name_is_uri_safe, upstream_target_redacted,
    upstream_transport,
};
use super::validate::validate_upstream_config;

/// Validate an upstream config entry and, if valid, return the catalog entry
/// that should be inserted for it.
///
/// Returns `None` (and emits a `WARN`) when the config should be skipped:
/// disabled, URI-unsafe name, or failing `validate_upstream_config`.
///
/// This helper removes the duplicated validation+entry-build logic that used to
/// live in both `seed_lazy_upstreams` and `ensure_lazy_upstream_entry` (Q-M3).
fn validated_lazy_entry(config: &UpstreamConfig) -> Option<super::super::types::UpstreamEntry> {
    if !config.enabled {
        return None;
    }
    if !upstream_name_is_uri_safe(&config.name) {
        tracing::warn!(
            upstream = %config.name,
            "upstream name contains URI-unsafe characters (/, ?, #) — skipping"
        );
        return None;
    }
    if let Err(msg) = validate_upstream_config(config) {
        tracing::warn!(
            upstream = %config.name,
            "skipping upstream: {msg}"
        );
        return None;
    }
    Some(lazy_upstream_entry(config, Arc::from(config.name.as_str())))
}

impl UpstreamPool {
    /// Seed the upstream catalog from config without starting any upstream runtime.
    pub async fn seed_lazy_upstreams(&self, configs: &[UpstreamConfig]) {
        let mut catalog = self.catalog.write().await;
        let mut resource_names = Vec::new();
        let mut processed_names = std::collections::HashSet::new();

        for config in configs {
            if !processed_names.insert(&config.name) {
                continue;
            }
            let Some(entry) = validated_lazy_entry(config) else {
                continue;
            };

            catalog.entry(config.name.clone()).or_insert(entry);

            if config.proxy_resources {
                resource_names.push(config.name.clone());
            }
        }

        resource_names.sort_unstable();
        resource_names.dedup();
        *self.resource_upstreams.write().await = resource_names;
    }

    async fn ensure_lazy_upstream_entry(&self, config: &UpstreamConfig) {
        let Some(entry) = validated_lazy_entry(config) else {
            return;
        };
        self.catalog
            .write()
            .await
            .entry(config.name.clone())
            .or_insert(entry);
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
            connect_upstream_with_client(
                config,
                subject,
                self.oauth_client_cache.as_ref(),
                self.runtime_origin.as_deref(),
                runtime_owner,
                Some(&self.shared_http_client),
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
        self.reprobe_tools_for_upstream_as(config, None, None).await
    }

    pub async fn reprobe_tools_for_upstream_as(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        runtime_owner: Option<&UpstreamRuntimeOwner>,
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
        let connect_lock = self.lazy_connect_lock(&config.name).await;
        let _connect_guard = connect_lock.lock().await;
        self.reprobe_upstream(config, oauth_subject, runtime_owner)
            .await
    }

    pub(super) async fn replace_catalog_tools(
        &self,
        config: &UpstreamConfig,
        tools: Vec<rmcp::model::Tool>,
    ) {
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
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use super::super::testsupport::*;
    use super::*;

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
}
