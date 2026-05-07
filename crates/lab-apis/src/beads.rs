//! Beads issue tracker, backed by a Dolt SQL server over the MySQL protocol.

pub mod client;
pub mod error;
pub mod types;

pub use client::{BeadsClient, DoltConnection};
pub use error::BeadsError;

use crate::core::error::ApiError;
use crate::core::plugin::{Category, EnvVar, PluginMeta};
use crate::core::plugin_ui::{SECRET_OPTIONAL_FIELD, TEXT_FIELD, TEXT_OPTIONAL_FIELD};
use crate::core::status::ServiceStatus;
use crate::core::traits::ServiceClient;

pub const META: PluginMeta = PluginMeta {
    name: "beads",
    display_name: "Beads",
    description: "Git/Dolt-backed issue tracker, queried directly over the Dolt SQL protocol",
    category: Category::Bootstrap,
    docs_url: "https://gastownhall.github.io/beads/",
    required_env: &[EnvVar {
        name: "BEADS_DOLT_URL",
        // The MUST be a `mysql://` (or `mysqls://`) DSN; URL_FIELD's pattern
        // hardcodes `^https?://` and would reject those schemes, so we use
        // TEXT_FIELD for plain string validation.
        description: "Dolt SQL endpoint as a MySQL connection URL (e.g. mysql://host:3306/)",
        example: "mysql://dolt.local:3306/",
        secret: false,
        ui: Some(&TEXT_FIELD),
    }],
    optional_env: &[
        EnvVar {
            name: "BEADS_DOLT_USER",
            description: "Username for the Dolt SQL server",
            example: "root",
            secret: false,
            ui: Some(&TEXT_OPTIONAL_FIELD),
        },
        EnvVar {
            name: "BEADS_DOLT_PASSWORD",
            description: "Password for the Dolt SQL server",
            example: "",
            secret: true,
            ui: Some(&SECRET_OPTIONAL_FIELD),
        },
        EnvVar {
            name: "BEADS_DEFAULT_PROJECT",
            description: "Database name selected when a request omits `project`",
            example: "lab",
            secret: false,
            ui: Some(&TEXT_OPTIONAL_FIELD),
        },
    ],
    default_port: Some(3306),
    supports_multi_instance: false,
};

impl ServiceClient for BeadsClient {
    fn name(&self) -> &'static str {
        "beads"
    }

    fn service_type(&self) -> &'static str {
        "bootstrap"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let status = self
            .health_status()
            .await
            .map_err(|err| ApiError::Internal(err.to_string()))?;
        Ok(ServiceStatus {
            reachable: status.reachable,
            auth_ok: status.reachable,
            version: status.version,
            latency_ms: 0,
            message: status.message,
        })
    }
}
