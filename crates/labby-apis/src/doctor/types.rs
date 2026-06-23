//! Shared types for the doctor service.

use serde::{Deserialize, Serialize};

use crate::core::ServiceStatus;

/// Result of probing a single named service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    /// Service name (e.g. `"radarr"`).
    pub service: String,
    /// Health status from the service's `health()` method.
    pub status: ServiceStatus,
}
