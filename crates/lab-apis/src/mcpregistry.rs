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

use crate::core::plugin::{Category, EnvVar, PluginMeta};

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
