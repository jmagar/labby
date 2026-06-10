//! Tool invocation: `subject_scoped_call_tool` (OAuth-subject-aware) and
//! `call_tool`. Both acquire the upstream peer, invoke the tool with a request
//! timeout, enforce the response-size cap, and emit structured request logs.
//!
//! NO-TOUCH (plan §6): `subject_scoped_call_tool` retains its `subject` argument
//! threading; the bodies are moved byte-identical from `pool.rs`.

use std::time::Instant;

use rmcp::model::{CallToolRequestParams, CallToolResult};

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::helpers::{estimate_response_size, max_response_bytes, upstream_transport};
use super::logging::{
    UpstreamRequestLog, log_upstream_request_error, log_upstream_request_finish,
    log_upstream_request_start,
};

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
        match tokio::time::timeout(self.request_timeout, peer.call_tool(params)).await {
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
                self.evict_subject_connection(&config.name, subject).await;
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
                self.evict_subject_connection(&config.name, subject).await;
                let elapsed_ms = start.elapsed().as_millis();
                log_upstream_request_error(event, elapsed_ms, "timeout", None, None, None);
                Err(message)
            }
        }
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
}

#[cfg(test)]
mod tests {
    use rmcp::model::CallToolRequestParams;

    use super::super::testsupport::*;

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
}
