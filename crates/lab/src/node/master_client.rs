use anyhow::Result;
use lab_apis::core::Auth;
use lab_apis::device_runtime::client::NodeRuntimeClient;

use crate::config::LabConfig;
use crate::node::identity::resolve_local_hostname;

#[derive(Debug, Clone)]
pub struct MasterClient {
    base_url: String,
    inner: NodeRuntimeClient,
}

impl MasterClient {
    #[allow(dead_code)]
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Self::with_bearer_token(base_url, None)
    }

    pub fn with_bearer_token(
        base_url: impl Into<String>,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        let base_url = base_url.into();
        let auth = bearer_token.map_or(Auth::None, |token| Auth::Bearer { token });
        let inner = NodeRuntimeClient::new(base_url.clone(), auth)?;
        Ok(Self { base_url, inner })
    }

    pub async fn fetch_devices(&self) -> Result<serde_json::Value> {
        self.inner.fetch_devices().await.map_err(Into::into)
    }

    pub async fn fetch_device(&self, node_id: &str) -> Result<serde_json::Value> {
        self.inner.fetch_device(node_id).await.map_err(Into::into)
    }

    /// Returns `true` when the controller reports an active WebSocket connection
    /// for the given node. Returns `false` if the node is known but not connected,
    /// or if the node is not in inventory.
    ///
    /// Unlike `fetch_device`, this returns `false` on node-not-found rather than
    /// an error — it is intended as a polling helper for rollout verification, not
    /// as an inventory query.
    pub async fn node_connected(&self, node_id: &str) -> Result<bool> {
        match self.fetch_device(node_id).await {
            Ok(value) => Ok(value
                .get("connected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)),
            Err(error) => {
                // If the node is simply not found, it's not connected.
                // Use typed downcast rather than string matching to avoid false positives
                // from proxy error bodies that happen to contain "not found".
                if error
                    .downcast_ref::<lab_apis::core::ApiError>()
                    .is_some_and(|e| matches!(e, lab_apis::core::ApiError::NotFound))
                {
                    return Ok(false);
                }
                Err(error)
            }
        }
    }

    /// Poll `node_connected` until the node is connected or the timeout elapses.
    ///
    /// Returns `Ok(())` on success.
    /// Returns `Err` when the timeout expires without a successful `true` response.
    ///
    /// Transport errors are logged as warnings and treated as "not yet connected"
    /// — they will not abort the poll early.
    pub async fn wait_for_node_connected(
        &self,
        node_id: &str,
        timeout: std::time::Duration,
    ) -> Result<()> {
        use std::time::Instant;
        let deadline = Instant::now() + timeout;
        let mut attempt: u32 = 0;
        loop {
            match self.node_connected(node_id).await {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        surface = "node",
                        service = "update",
                        action = "node_connected.poll",
                        node_id = %node_id,
                        attempt,
                        error = %error,
                        "node_connected poll returned error",
                    );
                }
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    surface = "node",
                    service = "master_client",
                    action = "node.connect_wait",
                    kind = "timeout",
                    node_id = %node_id,
                    timeout_ms = timeout.as_millis(),
                    "wait_for_node_connected timed out",
                );
                anyhow::bail!(
                    "timed out waiting for node `{node_id}` to reconnect to controller ({}s)",
                    timeout.as_secs()
                );
            }
            attempt += 1;
            // Exponential backoff: 2s, 4s, 8s, capped at 16s.
            let delay =
                std::time::Duration::from_secs(std::cmp::min(2u64.saturating_pow(attempt), 16));
            tokio::time::sleep(delay).await;
        }
    }

    pub async fn fetch_enrollments(&self) -> Result<serde_json::Value> {
        self.inner.fetch_enrollments().await.map_err(Into::into)
    }

    pub async fn approve_enrollment(
        &self,
        node_id: &str,
        note: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.inner
            .approve_enrollment(node_id, note)
            .await
            .map_err(Into::into)
    }

    pub async fn deny_enrollment(
        &self,
        node_id: &str,
        reason: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.inner
            .deny_enrollment(node_id, reason)
            .await
            .map_err(Into::into)
    }

    pub async fn post_log_ingest(&self, payload: &serde_json::Value) -> Result<serde_json::Value> {
        self.inner
            .post_log_ingest(payload)
            .await
            .map_err(Into::into)
    }

    pub async fn search_logs(&self, node_id: &str, query: &str) -> Result<serde_json::Value> {
        self.inner
            .search_logs(node_id, query)
            .await
            .map_err(Into::into)
    }

    pub fn from_config(config: &LabConfig, port_override: Option<u16>) -> Result<Self> {
        let host = match config.controller_host() {
            Some(host) => host.to_string(),
            None => resolve_local_hostname()?,
        };
        let port = port_override.or(config.mcp.port).unwrap_or(8765);
        Self::with_bearer_token(format!("http://{host}:{port}"), master_bearer_token())
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

fn master_bearer_token() -> Option<String> {
    std::env::var("LAB_MCP_HTTP_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Helper: create a `MasterClient` pointed at a wiremock server.
    async fn client_for(server: &MockServer) -> MasterClient {
        MasterClient::new(server.uri()).expect("MasterClient::new")
    }

    // ----------------------------------------------------------------
    // wait_for_node_connected — success on first poll
    // ----------------------------------------------------------------

    /// When the very first poll returns `{"connected": true}`, the function
    /// should return `Ok(())` immediately without sleeping.
    #[tokio::test]
    async fn wait_connected_returns_ok_immediately_when_first_poll_succeeds() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/nodes/testnode"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"connected": true})),
            )
            .mount(&server)
            .await;

        let client = client_for(&server).await;
        let result = client
            .wait_for_node_connected("testnode", std::time::Duration::from_secs(5))
            .await;

        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    // ----------------------------------------------------------------
    // wait_for_node_connected — timeout when polls keep returning false
    // ----------------------------------------------------------------

    /// A zero-duration timeout means the deadline has already passed after the
    /// first poll (which returns false). The function must bail without sleeping.
    #[tokio::test]
    async fn wait_connected_returns_err_on_timeout() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v1/nodes/testnode"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"connected": false})),
            )
            .mount(&server)
            .await;

        let client = client_for(&server).await;
        // Zero-duration timeout: deadline fires immediately after the first poll.
        let result = client
            .wait_for_node_connected("testnode", std::time::Duration::ZERO)
            .await;

        assert!(result.is_err(), "expected Err(timeout), got Ok(())");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "error should mention timeout, got: {msg}"
        );
    }

    // ----------------------------------------------------------------
    // wait_for_node_connected — retries until success
    // ----------------------------------------------------------------

    /// The first poll returns `{"connected": false}`; the second returns
    /// `{"connected": true}`. The 2s backoff sleep (2^1 = 2s) is real wall time,
    /// which is acceptable for this behavioral test.
    #[tokio::test]
    async fn wait_connected_retries_and_succeeds_on_second_poll() {
        let server = MockServer::start().await;

        // First call: not connected.
        Mock::given(method("GET"))
            .and(path("/v1/nodes/testnode"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"connected": false})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Subsequent calls: connected.
        Mock::given(method("GET"))
            .and(path("/v1/nodes/testnode"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"connected": true})),
            )
            .mount(&server)
            .await;

        let client = client_for(&server).await;
        let result = client
            .wait_for_node_connected("testnode", std::time::Duration::from_secs(10))
            .await;

        assert!(
            result.is_ok(),
            "expected Ok(()) after retry, got {result:?}"
        );

        // Confirm both mocks were exercised (at least 2 requests).
        assert!(
            server.received_requests().await.unwrap().len() >= 2,
            "expected at least 2 HTTP requests"
        );
    }

    // ----------------------------------------------------------------
    // wait_for_node_connected — transport errors are swallowed, not fatal
    // ----------------------------------------------------------------

    /// A 500 response causes `node_connected` to return `Err`. That error must
    /// be logged as WARN and treated as "not yet connected" rather than aborting
    /// the poll loop. The second poll returns `{"connected": true}`.
    #[tokio::test]
    async fn wait_connected_swallows_transport_error_and_retries() {
        let server = MockServer::start().await;

        // First call: server error — triggers the `Err` arm in the loop.
        Mock::given(method("GET"))
            .and(path("/v1/nodes/testnode"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        // Second call: node is now connected.
        Mock::given(method("GET"))
            .and(path("/v1/nodes/testnode"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"connected": true})),
            )
            .mount(&server)
            .await;

        let client = client_for(&server).await;
        let result = client
            .wait_for_node_connected("testnode", std::time::Duration::from_secs(10))
            .await;

        assert!(
            result.is_ok(),
            "transport error should be swallowed, not fatal; got {result:?}"
        );
        // Verify the loop actually retried (not an immediate return on 500).
        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.len() >= 2,
            "transport error should trigger retry; only {} request(s) made",
            reqs.len()
        );
    }
}
