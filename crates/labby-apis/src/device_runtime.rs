//! Device-runtime control-plane client.

pub mod client;
pub mod types;

pub use client::DeviceRuntimeClient;

use std::time::Instant;

use crate::core::plugin::{Category, PluginMeta};
use crate::core::{ApiError, ServiceClient, ServiceStatus};

pub const META: PluginMeta = PluginMeta {
    name: "device_runtime",
    display_name: "Device Runtime",
    description: "Lab device-runtime control plane client for fleet devices",
    category: Category::Bootstrap,
    docs_url: "https://github.com/jmagar/lab/blob/main/docs/runtime/DEVICE_RUNTIME.md",
    required_env: &[],
    optional_env: &[],
    default_port: Some(8765),
    supports_multi_instance: false,
};

impl ServiceClient for DeviceRuntimeClient {
    fn name(&self) -> &'static str {
        "device_runtime"
    }

    fn service_type(&self) -> &'static str {
        "bootstrap"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let start = Instant::now();
        match self.fetch_devices().await {
            Ok(_) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: None,
            }),
            Err(ApiError::Auth) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: false,
                version: None,
                latency_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                message: Some("authentication failed".to_string()),
            }),
            Err(ApiError::Network(msg)) => Ok(ServiceStatus::unreachable(msg)),
            Err(error) => Ok(ServiceStatus::degraded(error.to_string())),
        }
    }
}
