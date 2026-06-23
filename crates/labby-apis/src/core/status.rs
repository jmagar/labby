//! `ServiceStatus` — uniform health-check result shape.
//!
//! Returned by [`crate::core::traits::ServiceClient::health`]. Consumed by
//! `labby health`, `labby doctor`, the TUI, and the `system.status` MCP action
//! on every service.
//!
//! Rules:
//! - `reachable = false` ⇒ `auth_ok = false` and `version = None`
//! - Never panic; network errors map to `reachable = false` + `message`
//! - Health probes have a hard 5s timeout regardless of `HttpClient` defaults

use serde::{Deserialize, Serialize};

/// Result of a service health probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// True if TCP/HTTP reached the host.
    pub reachable: bool,
    /// True if credentials were accepted (only meaningful when `reachable`).
    pub auth_ok: bool,
    /// Reported server version, if exposed by the service.
    pub version: Option<String>,
    /// Round-trip latency for the health probe, in milliseconds.
    pub latency_ms: u64,
    /// Optional human-readable detail (error string, banner, etc.).
    pub message: Option<String>,
}

impl ServiceStatus {
    /// Construct an unreachable status from an error message.
    #[must_use]
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self {
            reachable: false,
            auth_ok: false,
            version: None,
            latency_ms: 0,
            message: Some(message.into()),
        }
    }

    /// Construct a degraded status: service is reachable but returning errors.
    ///
    /// Use this for `Server`, `RateLimited`, `Decode`, etc. where the host
    /// responded — as opposed to `unreachable()` for network-level failures.
    #[must_use]
    pub fn degraded(message: impl Into<String>) -> Self {
        Self {
            reachable: true,
            auth_ok: true,
            version: None,
            latency_ms: 0,
            message: Some(message.into()),
        }
    }
}
