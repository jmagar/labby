//! Generic URL health prober.
//!
//! `DoctorClient` takes a pre-resolved URL from the caller (never from
//! environment variables or the filesystem) and probes it via HTTP GET.
//! The dispatch layer is responsible for URL resolution; this module performs
//! only pure HTTP I/O.

use std::time::Instant;

use crate::core::error::ApiError;
use crate::core::{Auth, HttpClient, ServiceStatus};

use super::error::DoctorError;

/// Generic health prober. Probes the given `base_url` via `GET /`.
///
/// Zero env I/O and zero filesystem I/O — all config is injected by the caller.
pub struct DoctorClient {
    http: HttpClient,
}

impl DoctorClient {
    /// Build a new client targeting `base_url`.
    ///
    /// # Errors
    /// Returns [`DoctorError::Api`] if TLS initialisation fails.
    pub fn new(base_url: &str) -> Result<Self, DoctorError> {
        Ok(Self {
            http: HttpClient::new(base_url, Auth::None)?,
        })
    }

    /// Send a `GET /` and report reachability.
    ///
    /// - `Ok(())` from `get_void` → fully reachable
    /// - `Auth` error → reachable but `auth_ok = false`
    /// - `Server` error → reachable but degraded
    /// - `Network` → unreachable (true transport failure)
    /// - Other non-network errors (`NotFound`, `RateLimited`, `Validation`,
    ///   `Decode`, `Internal`) → reachable but degraded
    ///
    /// # Errors
    /// Always returns `Ok`; transport errors are captured inside `ServiceStatus`.
    pub async fn probe(&self) -> Result<ServiceStatus, DoctorError> {
        let start = Instant::now();
        let elapsed = || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        match self.http.get_void("/").await {
            Ok(()) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: None,
                latency_ms: elapsed(),
                message: None,
            }),
            Err(ApiError::Auth) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: false,
                version: None,
                latency_ms: elapsed(),
                message: Some("authentication failed".to_string()),
            }),
            Err(ApiError::Server { status, .. }) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: None,
                latency_ms: elapsed(),
                message: Some(format!("HTTP {status}")),
            }),
            Err(ApiError::Network(msg)) => Ok(ServiceStatus::unreachable(msg)),
            Err(e) => {
                // Non-network errors (NotFound, RateLimited, Validation,
                // Decode, Internal) mean the host responded — classify as
                // degraded rather than unreachable to avoid false "service
                // down" reports.
                let mut status = ServiceStatus::degraded(e.to_string());
                status.latency_ms = elapsed();
                Ok(status)
            }
        }
    }
}
