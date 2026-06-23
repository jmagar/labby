//! Doctor service — compile-time metadata + shared types.
//!
//! Doctor is a Bootstrap utility with no external service URL. It has no
//! `ServiceClient` implementation — health checks go through the typed
//! clients in `lab/src/dispatch/doctor/`.

pub mod client;
pub mod error;
pub mod types;

pub use client::DoctorClient;
pub use error::DoctorError;
pub use types::ProbeResult;

use crate::core::plugin::{Category, PluginMeta};

/// Compile-time metadata for the doctor module.
pub const META: PluginMeta = PluginMeta {
    name: "doctor",
    display_name: "Doctor",
    description: "Comprehensive health audit: env vars, system probes, and service reachability",
    category: Category::Bootstrap,
    docs_url: "https://github.com/jmagar/lab",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
