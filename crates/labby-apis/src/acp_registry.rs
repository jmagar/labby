//! ACP Agent Registry client — discover ACP-compatible AI coding agents.
//!
//! This module wraps the read-only CDN endpoint at
//! <https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json>.
//! No auth required; all reads are unauthenticated.
//! The only env var is `ACP_REGISTRY_URL` (optional; defaults to the official CDN).

pub mod client;
pub mod error;
pub mod installer;
pub mod types;

/// The canonical SSRF guard is defined in the dependency-free `labby-primitives`
/// leaf crate (`labby_primitives::ssrf`) and re-exported through `crate::core::ssrf`;
/// re-exported again here so existing `acp_registry::ssrf` / `super::ssrf` paths
/// keep working unchanged.
pub use crate::core::ssrf;

pub use client::AcpRegistryClient;
pub use error::AcpRegistryError;
pub use installer::{AcpInstaller, AcpInstallerError, InstallOutcome, InstallSpec};
pub use ssrf::SsrfError;

use std::time::Instant;

use crate::core::plugin::{Category, EnvVar, PluginMeta};
use crate::core::{ApiError, ServiceClient, ServiceStatus};

/// Compile-time metadata for the acp_registry module.
pub const META: PluginMeta = PluginMeta {
    name: "acp_registry",
    display_name: "ACP Registry",
    description: "ACP Agent Registry — discover and install ACP-compatible AI coding agents",
    category: Category::Marketplace,
    docs_url: "https://agentclientprotocol.com",
    required_env: &[],
    optional_env: &[EnvVar {
        name: "ACP_REGISTRY_URL",
        description: "Override the ACP Agent Registry CDN URL (defaults to the official endpoint)",
        example: "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json",
        secret: false,
        ui: None,
    }],
    default_port: None,
    supports_multi_instance: false,
};

impl ServiceClient for AcpRegistryClient {
    fn name(&self) -> &'static str {
        "acp_registry"
    }

    fn service_type(&self) -> &'static str {
        "marketplace"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let start = Instant::now();
        match self.health_probe().await {
            Ok(_) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: None,
            }),
            // Network-level failures (DNS, connect, timeout) ⇒ unreachable.
            Err(AcpRegistryError::Request(ApiError::Network(e))) => {
                Ok(ServiceStatus::unreachable(e.to_string()))
            }
            // Server responded but with an error status, or we couldn't decode
            // the body — the host is reachable, the service is just degraded.
            Err(e) => Ok(ServiceStatus::degraded(e.to_string())),
        }
    }
}
