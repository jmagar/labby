//! `McpRegistryClient` — MCP Registry v0.1 API methods.

use std::time::Duration;

use crate::core::{ApiError, Auth, HttpClient};

use super::error::RegistryError;
use super::types::{
    ListServersParams, ServerJSON, ServerListResponse, ServerResponse, ValidationResult,
};

/// Default registry base URL.
pub const DEFAULT_BASE_URL: &str = "https://registry.modelcontextprotocol.io";

/// Client for the official MCP Registry at <https://registry.modelcontextprotocol.io>.
///
/// The registry requires no auth; pass `Auth::None` for normal use. A custom
/// `reqwest::Client` with redirect following disabled is built internally to
/// prevent SSRF via malicious registry entries that redirect to internal addresses.
pub struct McpRegistryClient {
    http: HttpClient,
}

impl McpRegistryClient {
    /// Build a client against `base_url` with the given auth strategy.
    ///
    /// Pass `Auth::None` for the official public registry. Pass `Auth::Bearer`
    /// or `Auth::Token` when targeting a private registry mirror that requires
    /// authentication.
    ///
    /// # Errors
    /// Returns [`RegistryError::Api`] if the TLS backend or redirect policy
    /// fails to initialise.
    pub fn new(base_url: &str, auth: Auth) -> Result<Self, RegistryError> {
        let inner = reqwest::Client::builder()
            .user_agent(concat!("lab-apis/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            // Disable redirects to prevent SSRF via registry entries that
            // redirect to internal/private addresses.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ApiError::Internal(format!("reqwest::Client::build: {e}")))?;

        Ok(Self {
            http: HttpClient::from_parts(base_url, auth, inner),
        })
    }

    // -----------------------------------------------------------------------
    // Public API methods
    // -----------------------------------------------------------------------

    /// List servers from the registry.
    ///
    /// Maps to `GET /v0.1/servers`. Only the upstream-relevant fields from
    /// `params` are forwarded (search, limit, cursor). Lab-local filter fields
    /// (featured, reviewed, etc.) are applied against the local store.
    ///
    /// # Errors
    /// Returns [`RegistryError::Api`] on transport or HTTP failure.
    pub async fn list_servers(
        &self,
        params: ListServersParams,
    ) -> Result<ServerListResponse, RegistryError> {
        let query = params.to_upstream_query_pairs();
        Ok(self.http.get_json_query("/v0.1/servers", &query).await?)
    }

    /// Fetch a specific server version from the registry.
    ///
    /// Maps to `GET /v0.1/servers/{name}/versions/{version}`.
    ///
    /// Pass `version = "latest"` to fetch the most recent version — the
    /// registry handles `latest` natively; it is not special-cased here.
    ///
    /// # Errors
    /// - [`RegistryError::InvalidInput`] if `name` is blank after trimming.
    /// - [`RegistryError::Api`] on transport or HTTP failure.
    pub async fn get_server(
        &self,
        name: &str,
        version: &str,
    ) -> Result<ServerResponse, RegistryError> {
        if name.trim().is_empty() {
            return Err(RegistryError::InvalidInput {
                message: "server name must not be empty".into(),
            });
        }
        if version.trim().is_empty() {
            return Err(RegistryError::InvalidInput {
                message: "server version must not be empty".into(),
            });
        }
        let encoded_name = HttpClient::encode_path_segment(name);
        let encoded_version = HttpClient::encode_path_segment(version);
        let path = format!("/v0.1/servers/{encoded_name}/versions/{encoded_version}");
        Ok(self.http.get_json(&path).await?)
    }

    /// List all known versions of a server.
    ///
    /// Maps to `GET /v0.1/servers/{name}/versions`.
    ///
    /// # Errors
    /// - [`RegistryError::InvalidInput`] if `name` is blank after trimming.
    /// - [`RegistryError::Api`] on transport or HTTP failure.
    pub async fn list_versions(&self, name: &str) -> Result<ServerListResponse, RegistryError> {
        if name.trim().is_empty() {
            return Err(RegistryError::InvalidInput {
                message: "server name must not be empty".into(),
            });
        }
        let encoded_name = HttpClient::encode_path_segment(name);
        let path = format!("/v0.1/servers/{encoded_name}/versions");
        Ok(self.http.get_json(&path).await?)
    }

    /// Validate a server JSON definition against the registry schema.
    ///
    /// Maps to `POST /v0.1/validate`.
    ///
    /// # Errors
    /// Returns [`RegistryError::Api`] on transport or HTTP failure.
    pub async fn validate(
        &self,
        server_json: &ServerJSON,
    ) -> Result<ValidationResult, RegistryError> {
        Ok(self.http.post_json("/v0.1/validate", server_json).await?)
    }

    /// Health probe called by the `ServiceClient` impl in `mcpregistry.rs`.
    pub(super) async fn health_probe(&self) -> Result<(), RegistryError> {
        Ok(self.http.get_void("/v0.1/health").await?)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_client() -> McpRegistryClient {
        McpRegistryClient::new(DEFAULT_BASE_URL, Auth::None)
            .expect("client construction should succeed")
    }

    #[test]
    fn constructor_succeeds_with_default_url() {
        let _ = make_client();
    }

    #[tokio::test]
    async fn get_server_rejects_blank_name() {
        let err = make_client().get_server("", "1.0.0").await.unwrap_err();
        assert!(
            matches!(err, RegistryError::InvalidInput { ref message } if message.contains("must not be empty")),
            "expected InvalidInput, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_server_rejects_whitespace_only_name() {
        let err = make_client().get_server("   ", "latest").await.unwrap_err();
        assert!(matches!(err, RegistryError::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn get_server_rejects_blank_version() {
        let err = make_client()
            .get_server("io.modelcontextprotocol/everything", "")
            .await
            .unwrap_err();
        assert!(
            matches!(err, RegistryError::InvalidInput { ref message } if message.contains("must not be empty")),
            "expected InvalidInput for blank version, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_server_rejects_whitespace_only_version() {
        let err = make_client()
            .get_server("io.modelcontextprotocol/everything", "   ")
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn list_versions_rejects_blank_name() {
        let err = make_client().list_versions("").await.unwrap_err();
        assert!(
            matches!(err, RegistryError::InvalidInput { ref message } if message.contains("must not be empty")),
            "expected InvalidInput, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn list_versions_rejects_whitespace_only_name() {
        let err = make_client().list_versions("\t").await.unwrap_err();
        assert!(matches!(err, RegistryError::InvalidInput { .. }));
    }
}
