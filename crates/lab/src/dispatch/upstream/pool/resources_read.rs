//! Resource reads: `read_upstream_resource` and `subject_scoped_read_resource`.
//!
//! Both acquire/connect the upstream peer, read the resource with a request
//! timeout, normalize the returned URIs to the gateway form, enforce the
//! response-size cap, and emit structured request logs.
//!
//! NO-TOUCH (plan §6): `subject_scoped_read_resource` retains its `subject`
//! argument threading and the `redact_resource_uri_for_logging` call; bodies are
//! moved byte-identical from `pool.rs`.

use std::time::Instant;

use rmcp::model::ReadResourceResult;

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::helpers::{
    max_response_bytes, normalize_resource_result_uri, redact_resource_uri_for_logging,
    upstream_transport,
};
use super::logging::{
    UpstreamRequestLog, log_upstream_request_error, log_upstream_request_finish,
    log_upstream_request_start,
};

impl UpstreamPool {
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
        // P-C1: reuse cached per-(upstream,subject) connection instead of opening fresh.
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
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
            peer.read_resource(rmcp::model::ReadResourceRequestParams::new(original_uri)),
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
                self.evict_subject_connection(&config.name, subject).await;
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
                self.evict_subject_connection(&config.name, subject).await;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU8;

    use rmcp::model::LoggingLevel;
    use tokio::sync::RwLock;

    use crate::mcp::logging::logging_level_rank;
    use crate::mcp::server::LabMcpServer;
    use crate::registry::ToolRegistry;

    use super::super::testsupport::*;

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
    async fn read_resource_times_out_slow_upstream_response() {
        let pool = slow_response_pool("slow").await;

        let result = pool
            .read_upstream_resource("lab://upstream/slow/file:///tmp/slow")
            .await
            .expect("resource upstream is enabled")
            .expect_err("slow resource read should time out");

        assert!(result.contains("timed out"));
    }
}
