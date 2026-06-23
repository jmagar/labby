//! `AcpRegistryClient` construction helpers for `agent.*` actions in the
//! marketplace dispatch.
//!
//! Unlike `mcp_client`, the ACP registry has a sensible public default
//! (`REGISTRY_DEFAULT_URL` from the SDK), so absence of `LAB_ACP_REGISTRY_URL`
//! is not a configuration error — it falls back to the official CDN.

#[cfg(feature = "acp_registry")]
use labby_apis::acp_registry::client::{AcpRegistryClient, REGISTRY_DEFAULT_URL};

use crate::dispatch::error::ToolError;
#[cfg(feature = "acp_registry")]
use crate::dispatch::helpers::env_non_empty;

/// Return the resolved ACP Registry base URL.
#[cfg(feature = "acp_registry")]
pub fn configured_registry_url() -> String {
    env_non_empty("LAB_ACP_REGISTRY_URL").unwrap_or_else(|| REGISTRY_DEFAULT_URL.to_string())
}

/// Build an `AcpRegistryClient` against `LAB_ACP_REGISTRY_URL` or the public default.
#[cfg(feature = "acp_registry")]
pub fn require_acp_client() -> Result<AcpRegistryClient, ToolError> {
    let url = configured_registry_url();
    AcpRegistryClient::new(&url).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("AcpRegistry client init failed: {e}"),
    })
}

/// Structured `not_configured` error stub — kept for symmetry with `mcp_client`.
/// Currently unreachable because `require_acp_client` always succeeds (default URL).
#[cfg(feature = "acp_registry")]
#[allow(dead_code)]
pub fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_configured".to_string(),
        message: "ACP registry client unavailable".to_string(),
    }
}

// Off-feature stub so callers can still link without `acp_registry`.
#[cfg(not(feature = "acp_registry"))]
pub fn require_acp_client() -> Result<(), ToolError> {
    Err(ToolError::Sdk {
        sdk_kind: "not_configured".to_string(),
        message: "acp_registry feature is not enabled in this build".to_string(),
    })
}
