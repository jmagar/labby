//! Dozzle read-only service module.

pub mod client;
pub mod error;
pub mod types;

pub use client::DozzleClient;
pub use error::DozzleError;

use crate::core::error::ApiError;
use crate::core::plugin::{Category, EnvVar, PluginMeta};
use crate::core::plugin_ui::{SECRET_OPTIONAL_FIELD, URL_FIELD};
use crate::core::status::ServiceStatus;
use crate::core::traits::ServiceClient;

/// Compile-time metadata for the Dozzle service.
pub const META: PluginMeta = PluginMeta {
    name: "dozzle",
    display_name: "Dozzle",
    description: "Read-only container log observation through Dozzle",
    category: Category::Network,
    docs_url: "https://dozzle.dev/guide/",
    required_env: &[EnvVar {
        name: "DOZZLE_URL",
        description: "Base URL for the Dozzle service",
        example: "http://localhost:8080",
        secret: false,
        ui: Some(&URL_FIELD),
    }],
    optional_env: &[EnvVar {
        name: "DOZZLE_SESSION_COOKIE",
        description: "Optional Dozzle jwt session cookie",
        example: "jwt=...",
        secret: true,
        ui: Some(&SECRET_OPTIONAL_FIELD),
    }],
    default_port: None,
    supports_multi_instance: false,
};

impl ServiceClient for DozzleClient {
    fn name(&self) -> &'static str {
        "dozzle"
    }

    fn service_type(&self) -> &'static str {
        "bootstrap"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let start = std::time::Instant::now();
        self.health().await.map_err(|e| match e {
            DozzleError::Api(err) => err,
            other => ApiError::Internal(other.to_string()),
        })?;
        Ok(ServiceStatus {
            reachable: true,
            auth_ok: true,
            version: None,
            latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            message: None,
        })
    }
}
