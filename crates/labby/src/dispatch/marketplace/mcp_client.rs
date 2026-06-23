//! McpRegistry client construction helpers for `mcp.*` actions in marketplace dispatch.
//!
//! Uses `[mcpregistry].url` from `config.toml`, falling back to the official
//! public registry URL when the setting is absent.

use std::sync::OnceLock;

use labby_apis::mcpregistry::client::McpRegistryClient;

use crate::config;
use crate::dispatch::error::ToolError;

/// Process-wide singleton `McpRegistryClient`.
///
/// Initialized on the first call to `require_mcp_client`. Using a `OnceLock`
/// avoids re-reading config and re-constructing a new HTTP client on every
/// `mcp.*` dispatch (lab-77y5.12).
static CLIENT: OnceLock<McpRegistryClient> = OnceLock::new();

/// Return the singleton McpRegistry client, initializing it once from config.
pub fn require_mcp_client() -> Result<&'static McpRegistryClient, ToolError> {
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let url = configured_registry_url()?;
    let client = McpRegistryClient::new(&url).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("McpRegistry client init failed: {e}"),
    })?;
    // `OnceLock::get_or_init` is not fallible, so we use `set` and fall back to
    // `get` if a concurrent initializer already won the race.
    drop(CLIENT.set(client));
    Ok(CLIENT.get().expect("OnceLock was just set"))
}

pub fn configured_registry_url() -> Result<String, ToolError> {
    let cfg = config::load_toml(&config::toml_candidates()).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("load config.toml: {e}"),
    })?;
    Ok(config::mcpregistry_url(&cfg).to_string())
}

// TODO(lab-zxx5.3): Add ACP client stub/placeholder here once ACP registry
// client is available in lab-apis. The three-client architecture will be:
// 1. Marketplace filesystem client (always available, in client.rs)
// 2. McpRegistryClient (this file, configured through config.toml)
// 3. AcpRegistryClient (placeholder — lab-zxx5.3)
