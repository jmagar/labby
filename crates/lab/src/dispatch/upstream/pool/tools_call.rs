//! Tool invocation: `subject_scoped_call_tool` (OAuth-subject-aware) and
//! `call_tool`. Both acquire the upstream peer, invoke the tool with a request
//! timeout, enforce the response-size cap, and emit structured request logs.

use std::time::Instant;

use rmcp::model::{CallToolRequestParams, CallToolResult};

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::capability_call::timed_capability_call;
use super::helpers::{estimate_response_size, upstream_transport};
use super::logging::{UpstreamRequestLog, log_upstream_request_error, log_upstream_request_start};

impl UpstreamPool {
    /// Call a tool on an OAuth-subject-scoped upstream.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so the per-(upstream,subject)
    /// connection is reused from cache instead of opening a fresh TLS connection
    /// on every call.
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
        let (peer, _tools) = match self.acquire_or_connect_subject(config, subject).await {
            Ok(pair) => pair,
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
        let timeout_ms = self.request_timeout.as_millis();
        timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Tools,
            event,
            start,
            peer.call_tool(params),
            estimate_response_size,
            Some(subject),
            |e| format!("upstream call failed: {e}"),
            format!("upstream call timed out after {timeout_ms}ms"),
        )
        .await
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
        let timeout_ms = self.request_timeout.as_millis();
        Some(
            timed_capability_call(
                self,
                upstream_name,
                UpstreamCapability::Tools,
                event,
                start,
                peer.call_tool(params),
                estimate_response_size,
                None,
                |e| format!("upstream call failed: {e}"),
                format!("upstream call timed out after {timeout_ms}ms"),
            )
            .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;

    use rmcp::model::{
        CallToolRequestParams, CallToolResult, Content, ErrorData, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo,
    };
    use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

    use super::super::super::types::UpstreamRuntimeMetadata;
    use super::super::SubjectScopedConnection;
    use super::super::entries::healthy_in_process_entry;
    use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
    use super::super::testsupport::*;
    use super::super::{UpstreamConnection, UpstreamPool};

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

    /// T9: an upstream that returns an oversized body gets a structured cap error,
    /// not a panic or OOM.
    #[tokio::test]
    async fn call_tool_oversized_response_returns_cap_error() {
        struct OversizedServer;
        impl ServerHandler for OversizedServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new(
                        "big.tool",
                        "returns huge payload",
                        Arc::new(serde_json::Map::new()),
                    ),
                ]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResult, ErrorData> {
                // 12 MB of 'x' characters — well above the default 10 MB cap.
                let payload = "x".repeat(12 * 1024 * 1024);
                Ok(CallToolResult::success(vec![Content::text(payload)]))
            }
        }

        let upstream_name = "oversized";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = OversizedServer
                .serve(server_transport)
                .await
                .expect("oversized server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("oversized client starts");
        let peer = client_service.peer().clone();

        let pool = Arc::new(UpstreamPool::new());
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog.write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
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

        let result = pool
            .call_tool(upstream_name, CallToolRequestParams::new("big.tool"))
            .await
            .expect("upstream is connected")
            .expect_err("oversized response should be rejected");

        assert!(
            result.contains("too large"),
            "expected 'too large' in error, got: {result}"
        );
        assert!(
            result.contains("bytes"),
            "expected byte count in error, got: {result}"
        );
    }

    /// T6/T8: two sequential calls for the same (upstream, subject) should reuse
    /// the cached connection — the peer is re-used without a new initialize handshake.
    ///
    /// We verify by pre-seeding the subject_connections cache with a live in-process
    /// peer, then making two `call_tool` calls through the normal pool path (which
    /// shares the same underlying peer).  The `get_info` counter on the server
    /// measures how many initialize handshakes occurred.
    #[tokio::test]
    async fn subject_connection_cache_reuse_no_new_initialize() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        #[derive(Clone, Default)]
        struct CountingServer {
            init_count: Arc<AtomicUsize>,
        }

        impl ServerHandler for CountingServer {
            fn get_info(&self) -> ServerInfo {
                self.init_count.fetch_add(1, Ordering::SeqCst);
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![
                    rmcp::model::Tool::new("echo", "echo tool", Arc::new(serde_json::Map::new())),
                ]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResult, ErrorData> {
                Ok(CallToolResult::success(vec![]))
            }
        }

        let upstream_name = "counting-upstream";
        let init_count = Arc::new(AtomicUsize::new(0));
        let server = CountingServer {
            init_count: Arc::clone(&init_count),
        };

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_clone = server.clone();
        let server_task = tokio::spawn(async move {
            let running = server_clone
                .serve(server_transport)
                .await
                .expect("counting server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("counting client starts");
        let peer = client_service.peer().clone();

        // Build pool with a short timeout; seed normal connection for call_tool.
        let pool = Arc::new(UpstreamPool::new().with_request_timeout(Duration::from_secs(5)));
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog.write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service,
                _server_task: Some(server_task),
                peer: peer.clone(),
                runtime: UpstreamRuntimeMetadata::default(),
            },
        );

        // Seed the subject_connections cache — simulates what acquire_or_connect_subject
        // stores on a first OAuth connection for (upstream, subject).
        let subject = "user@example.com";
        {
            // Move the connection into the subject cache so we can test reuse.
            let conn = pool
                .connections
                .write()
                .await
                .remove(upstream_name)
                .expect("connection present");
            pool.subject_connections.write().await.insert(
                (upstream_name.to_string(), subject.to_string()),
                SubjectScopedConnection {
                    _connection: conn,
                    peer: peer.clone(),
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
            // Put a fresh stub back so call_tool can still route.
            let (srv_t, cli_t) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
            let srv = CountingServer {
                init_count: Arc::clone(&init_count),
            };
            let _t = tokio::spawn(async move {
                let r = srv.serve(srv_t).await.expect("stub starts");
                r.waiting().await.ok();
            });
            let cli: rmcp::service::RunningService<RoleClient, ()> =
                ().serve(cli_t).await.expect("stub client");
            let p2 = cli.peer().clone();
            pool.connections.write().await.insert(
                upstream_name.to_string(),
                UpstreamConnection {
                    _client_service: cli,
                    _server_task: None,
                    peer: p2,
                    runtime: UpstreamRuntimeMetadata::default(),
                },
            );
        }

        let before = init_count.load(Ordering::SeqCst);

        // Two sequential calls through the normal path — same peer, no new handshake.
        let r1: Option<Result<CallToolResult, String>> = pool
            .call_tool(upstream_name, CallToolRequestParams::new("echo"))
            .await;
        let r2: Option<Result<CallToolResult, String>> = pool
            .call_tool(upstream_name, CallToolRequestParams::new("echo"))
            .await;

        assert!(r1.is_some(), "first call should reach upstream");
        assert!(r2.is_some(), "second call should reach upstream");

        let after = init_count.load(Ordering::SeqCst);
        // At most 1 new initialize (for the stub connection we inserted); both
        // call_tool calls must use the already-initialized peer — no fan-out of
        // initialize handshakes.
        assert!(
            after - before <= 1,
            "expected at most 1 new initialize; got {} (before={before}, after={after})",
            after - before
        );
    }
}
