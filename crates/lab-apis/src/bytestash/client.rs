//! `ByteStashClient` — snippet management methods.

use std::time::Instant;

use serde_json::Value;

use crate::core::{ApiError, Auth, HttpClient, ServiceClient, ServiceStatus};

use super::error::ByteStashError;
use super::types::{AuthCredentials, ShareCreateRequest, SnippetWriteRequest};

/// Client for a `ByteStash` instance.
pub struct ByteStashClient {
    http: HttpClient,
}

impl ByteStashClient {
    /// Build a client against `base_url` with the given auth.
    ///
    /// `ByteStash` uses JWT bearer auth: pass `Auth::Bearer { token: jwt }`.
    ///
    /// # Errors
    /// Returns [`ByteStashError::Api`] if the TLS backend fails to initialise.
    pub fn new(base_url: &str, auth: Auth) -> Result<Self, ByteStashError> {
        Ok(Self {
            http: HttpClient::new(base_url, auth)?,
        })
    }

    async fn get_value(&self, path: &str) -> Result<Value, ByteStashError> {
        Ok(self.http.get_json(path).await?)
    }

    async fn post_value<B: serde::Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Value, ByteStashError> {
        Ok(self.http.post_json(path, body).await?)
    }

    async fn put_value<B: serde::Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Value, ByteStashError> {
        Ok(self.http.put_json(path, body).await?)
    }

    async fn patch_value<B: serde::Serialize + Sync>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<Value, ByteStashError> {
        Ok(self.http.patch_json(path, body).await?)
    }

    async fn delete_value(&self, path: &str) -> Result<(), ByteStashError> {
        Ok(self.http.delete(path).await?)
    }

    /// Health check. Uses the auth configuration endpoint as a cheap probe.
    ///
    /// # Errors
    /// Returns `ByteStashError::Api` on HTTP failure.
    pub async fn probe(&self) -> Result<(), ByteStashError> {
        drop(self.get_value("/api/auth/config").await?);
        Ok(())
    }

    /// Retrieve auth-provider configuration.
    pub async fn auth_config(&self) -> Result<Value, ByteStashError> {
        self.get_value("/api/auth/config").await
    }

    /// Register a new user.
    pub async fn auth_register(&self, body: &AuthCredentials) -> Result<Value, ByteStashError> {
        self.post_value("/api/auth/register", body).await
    }

    /// Log in and receive a token.
    pub async fn auth_login(&self, body: &AuthCredentials) -> Result<Value, ByteStashError> {
        self.post_value("/api/auth/login", body).await
    }

    /// List the caller's snippets.
    pub async fn snippets_list(&self) -> Result<Value, ByteStashError> {
        self.get_value("/api/snippets").await
    }

    /// Get one snippet.
    pub async fn snippet_get(&self, id: &str) -> Result<Value, ByteStashError> {
        let id = HttpClient::encode_path_segment(id);
        self.get_value(&format!("/api/snippets/{id}")).await
    }

    /// Create a snippet.
    pub async fn snippets_create(
        &self,
        body: &SnippetWriteRequest,
    ) -> Result<Value, ByteStashError> {
        self.post_value("/api/snippets", body).await
    }

    /// Update a snippet.
    pub async fn snippets_update<B: serde::Serialize + Sync>(
        &self,
        id: &str,
        body: &B,
    ) -> Result<Value, ByteStashError> {
        let id = HttpClient::encode_path_segment(id);
        self.put_value(&format!("/api/snippets/{id}"), body).await
    }

    /// Delete a snippet.
    pub async fn snippets_delete(&self, id: &str) -> Result<(), ByteStashError> {
        let id = HttpClient::encode_path_segment(id);
        self.delete_value(&format!("/api/snippets/{id}")).await
    }

    /// List public snippets.
    pub async fn snippets_public_list(&self) -> Result<Value, ByteStashError> {
        self.get_value("/api/public/snippets").await
    }

    /// Get one public snippet.
    pub async fn snippets_public_get(&self, id: &str) -> Result<Value, ByteStashError> {
        let id = HttpClient::encode_path_segment(id);
        self.get_value(&format!("/api/public/snippets/{id}")).await
    }

    /// Create a share link for a snippet.
    pub async fn snippets_share_create(
        &self,
        body: &ShareCreateRequest,
    ) -> Result<Value, ByteStashError> {
        self.post_value("/api/share", body).await
    }

    /// Get a shared snippet.
    pub async fn snippets_share_get(&self, share_id: &str) -> Result<Value, ByteStashError> {
        let share_id = HttpClient::encode_path_segment(share_id);
        self.get_value(&format!("/api/share/{share_id}")).await
    }

    /// List all categories in use across the caller's snippets.
    ///
    /// `ByteStash` has no dedicated categories endpoint — this derives the list
    /// from the snippets response and deduplicates.
    pub async fn categories_list(&self) -> Result<Value, ByteStashError> {
        let snippets = self.snippets_list().await?;
        let mut cats: Vec<String> = snippets
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .flat_map(|s| {
                s["categories"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|c| c.as_str().map(ToOwned::to_owned))
                    .collect::<Vec<_>>()
            })
            .collect();
        cats.sort_unstable();
        cats.dedup();
        Ok(serde_json::json!(cats))
    }

    /// List users (admin only — requires `ByteStash` with admin routes).
    pub async fn users_list(&self) -> Result<Value, ByteStashError> {
        self.get_value("/api/admin/users").await
    }

    /// Toggle a user's active status (admin only — requires `ByteStash` with admin routes).
    pub async fn users_toggle_active(&self, id: &str) -> Result<Value, ByteStashError> {
        let id = HttpClient::encode_path_segment(id);
        self.patch_value(
            &format!("/api/admin/users/{id}/toggle-active"),
            &serde_json::json!({}),
        )
        .await
    }

    /// Delete a user (admin only — requires `ByteStash` with admin routes).
    pub async fn users_delete(&self, id: &str) -> Result<(), ByteStashError> {
        let id = HttpClient::encode_path_segment(id);
        self.delete_value(&format!("/api/admin/users/{id}")).await
    }
}

impl ServiceClient for ByteStashClient {
    fn name(&self) -> &'static str {
        "bytestash"
    }

    fn service_type(&self) -> &'static str {
        "notes"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let start = Instant::now();
        match self.probe().await {
            Ok(()) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: None,
            }),
            Err(ByteStashError::Api(ApiError::Auth)) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: false,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: Some("auth failed".into()),
            }),
            Err(ByteStashError::Api(e)) => Ok(ServiceStatus::unreachable(e.to_string())),
        }
    }
}
