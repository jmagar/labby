//! Resource reads: `read_upstream_resource` and `subject_scoped_read_resource`.
//!
//! Both acquire/connect the upstream peer, read the resource with a request
//! timeout, normalize the returned URIs to the gateway form, enforce the
//! response-size cap, and emit structured request logs.

use std::time::Instant;

use rmcp::model::ReadResourceResult;

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::timed_capability_call;
use super::helpers::{
    estimate_resource_response_size, normalize_resource_result_uri,
    redact_resource_uri_for_logging, upstream_transport,
};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};

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
        let gateway_uri = uri.to_string();
        let timeout_ms = self.request_timeout.as_millis();

        // Use timed_capability_call for timeout/size-cap/log skeleton, then
        // normalize the URI in the success path.
        Some(
            timed_capability_call(
                self,
                upstream_name,
                UpstreamCapability::Resources,
                event,
                start,
                peer.read_resource(params),
                estimate_resource_response_size,
                None,
                |e| format!("upstream resource read failed: {e}"),
                format!("upstream resource read timed out after {timeout_ms}ms"),
            )
            .await
            .map(|result| normalize_resource_result_uri(result, &gateway_uri)),
        )
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
        let params = rmcp::model::ReadResourceRequestParams::new(original_uri);
        let gateway_uri = uri.to_string();
        let timeout_ms = self.request_timeout.as_millis();

        timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Resources,
            event,
            start,
            peer.read_resource(params),
            estimate_resource_response_size,
            Some(subject),
            |e| format!("upstream resource read failed: {e}"),
            format!("upstream resource read timed out after {timeout_ms}ms"),
        )
        .await
        .map(|result| normalize_resource_result_uri(result, &gateway_uri))
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

    /// T9: an upstream that returns an oversized resource body gets a structured
    /// cap error — not a panic or OOM.
    #[tokio::test]
    async fn read_resource_oversized_response_returns_cap_error() {
        use std::collections::HashMap;

        use rmcp::model::{
            AnnotateAble, ErrorData, ListResourcesResult, PaginatedRequestParams, RawResource,
            ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

        use super::super::super::types::UpstreamRuntimeMetadata;
        use super::super::entries::healthy_in_process_entry;
        use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
        use super::super::{UpstreamConnection, UpstreamPool};

        struct OversizedResourceServer;
        impl ServerHandler for OversizedResourceServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            }
            async fn list_resources(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListResourcesResult, ErrorData> {
                Ok(ListResourcesResult::with_all_items(vec![
                    RawResource::new("file:///tmp/big", "big-resource").no_annotation(),
                ]))
            }
            async fn read_resource(
                &self,
                _: rmcp::model::ReadResourceRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ReadResourceResult, ErrorData> {
                // 12 MB of text — above the default 10 MB cap.
                let payload = "x".repeat(12 * 1024 * 1024);
                Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    "file:///tmp/big",
                    payload,
                )]))
            }
        }

        let upstream_name = "oversized-resource";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _server_task = tokio::spawn(async move {
            let running = OversizedResourceServer
                .serve(server_transport)
                .await
                .expect("oversized resource server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("oversized resource client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
        entry.resource_count = 1;
        entry.resource_uris = vec!["file:///tmp/big".to_string()];
        pool.catalog
            .write()
            .await
            .insert(upstream_name.to_string(), entry);
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service,
                _server_task: Some(_server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
        );
        pool.resource_upstreams
            .write()
            .await
            .push(upstream_name.to_string());

        let uri = format!("lab://upstream/{upstream_name}/file:///tmp/big");
        let result = pool
            .read_upstream_resource(&uri)
            .await
            .expect("resource upstream is enabled")
            .expect_err("oversized resource should be rejected");

        assert!(
            result.contains("too large"),
            "expected 'too large' in error, got: {result}"
        );
        assert!(
            result.contains("bytes"),
            "expected byte count in error, got: {result}"
        );
    }
}
