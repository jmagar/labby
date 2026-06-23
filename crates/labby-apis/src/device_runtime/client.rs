use std::future::Future;
use std::time::Duration;

use super::types::SearchLogsRequest;
use crate::core::{ApiError, Auth, HttpClient};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

const DEVICE_RUNTIME_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct DeviceRuntimeClient {
    http: HttpClient,
}

impl DeviceRuntimeClient {
    /// Build a client against the device-runtime surface.
    ///
    /// # Errors
    /// Returns [`ApiError`] if the shared HTTP client cannot be built.
    pub fn new(base_url: impl Into<String>, auth: Auth) -> Result<Self, ApiError> {
        let http = HttpClient::new(base_url, auth)?;
        Ok(Self { http })
    }

    pub async fn fetch_devices(&self) -> Result<serde_json::Value, ApiError> {
        self.with_timeout(self.http.get_json("/v1/nodes")).await
    }

    pub async fn fetch_device(&self, node_id: &str) -> Result<serde_json::Value, ApiError> {
        let encoded_id = utf8_percent_encode(node_id, NON_ALPHANUMERIC).to_string();
        self.with_timeout(self.http.get_json(&format!("/v1/nodes/{encoded_id}")))
            .await
    }

    pub async fn fetch_enrollments(&self) -> Result<serde_json::Value, ApiError> {
        self.with_timeout(self.http.get_json("/v1/nodes/enrollments"))
            .await
    }

    pub async fn approve_enrollment(
        &self,
        node_id: &str,
        note: Option<&str>,
    ) -> Result<serde_json::Value, ApiError> {
        let encoded_id = utf8_percent_encode(node_id, NON_ALPHANUMERIC).to_string();
        self.with_timeout(self.http.post_json(
            &format!("/v1/nodes/enrollments/{encoded_id}/approve"),
            &serde_json::json!({ "note": note }),
        ))
        .await
    }

    pub async fn deny_enrollment(
        &self,
        node_id: &str,
        reason: Option<&str>,
    ) -> Result<serde_json::Value, ApiError> {
        let encoded_id = utf8_percent_encode(node_id, NON_ALPHANUMERIC).to_string();
        self.with_timeout(self.http.post_json(
            &format!("/v1/nodes/enrollments/{encoded_id}/deny"),
            &serde_json::json!({ "reason": reason }),
        ))
        .await
    }

    pub async fn post_log_ingest<T: serde::Serialize + Sync>(
        &self,
        payload: &T,
    ) -> Result<serde_json::Value, ApiError> {
        self.with_timeout(self.http.post_json("/v1/logs/ingest", payload))
            .await
    }

    pub async fn search_logs(
        &self,
        node_id: &str,
        query: &str,
    ) -> Result<serde_json::Value, ApiError> {
        let request = SearchLogsRequest {
            node_id: node_id.to_string(),
            query: query.to_string(),
        };
        self.with_timeout(self.http.post_json("/v1/nodes/logs/search", &request))
            .await
    }

    async fn with_timeout<T>(
        &self,
        future: impl Future<Output = Result<T, ApiError>>,
    ) -> Result<T, ApiError> {
        tokio::time::timeout(DEVICE_RUNTIME_TIMEOUT, future)
            .await
            .map_err(|_| ApiError::Network("request timed out".to_string()))?
    }
}

/// Alias for [`DeviceRuntimeClient`] used after the `device → node` module rename.
pub type NodeRuntimeClient = DeviceRuntimeClient;
