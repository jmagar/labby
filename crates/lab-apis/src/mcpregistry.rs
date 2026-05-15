//! MCP Registry client — browse and search the official MCP server registry.
//!
//! Wraps the public MCP Registry API at <https://registry.modelcontextprotocol.io>.
//! No auth is required. Five endpoints are exposed:
//!
//! - `list_servers` — paginated server search (`GET /v0.1/servers`)
//! - `get_server` — fetch a named server/version (`GET /v0.1/servers/{name}/versions/{version}`)
//! - `list_versions` — all versions of a server (`GET /v0.1/servers/{name}/versions`)
//! - `validate` — validate a server JSON definition (`POST /v0.1/validate`)
//! - `health` — health probe (`GET /v0.1/health`) via `ServiceClient`

/// `McpRegistryClient` — registry API methods.
pub mod client;

/// `RegistryError` (thiserror).
pub mod error;

/// Request/response types for the MCP Registry v0.1 API.
pub mod types;

pub use client::McpRegistryClient;
pub use error::RegistryError;

use std::time::Instant;

use crate::core::plugin::{Category, EnvVar, PluginMeta};
use crate::core::{ApiError, ServiceClient, ServiceStatus};

impl ServiceClient for McpRegistryClient {
    fn name(&self) -> &'static str {
        "mcpregistry"
    }

    fn service_type(&self) -> &'static str {
        "ai"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let start = Instant::now();
        match self.health_probe().await {
            Ok(()) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: None,
            }),
            Err(RegistryError::Api(ApiError::Auth)) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: false,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: Some("auth failed".into()),
            }),
            Err(e) => Ok(ServiceStatus::unreachable(e.to_string())),
        }
    }
}

/// Compile-time metadata for the mcpregistry module.
pub const META: PluginMeta = PluginMeta {
    name: "mcpregistry",
    display_name: "MCP Registry",
    description: "MCP Registry v0.1 — discover and validate MCP servers",
    category: Category::Marketplace,
    docs_url: "https://registry.modelcontextprotocol.io",
    required_env: &[],
    optional_env: &[EnvVar {
        name: "MCPREGISTRY_URL",
        description: "Override registry URL (default: https://registry.modelcontextprotocol.io)",
        example: "https://registry.modelcontextprotocol.io",
        secret: false,
        ui: None,
    }],
    default_port: None,
    supports_multi_instance: false,
};
