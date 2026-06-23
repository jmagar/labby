//! `ServiceClient` trait — shared surface every service implements.
//!
//! Uses native `async fn in trait` (Rust 1.75+), no `async-trait` macro.
//! Per the locked conventions, `dyn ServiceClient` is forbidden — health
//! checks dispatch via a generated `match` over a concrete client enum.

use std::future::Future;

use crate::core::error::ApiError;
use crate::core::status::ServiceStatus;

/// Common surface implemented by every service client.
pub trait ServiceClient: Send + Sync {
    /// Short module name (matches `PluginMeta::name`, e.g. `"radarr"`).
    fn name(&self) -> &'static str;

    /// Logical category label (e.g. `"servarr"`, `"media"`, `"network"`).
    /// String-form rather than the `Category` enum so external implementors
    /// can use labels we don't ship.
    fn service_type(&self) -> &'static str;

    /// Probe the service for reachability, auth, and version.
    ///
    /// # Errors
    /// Returns [`ApiError`] for transport-level failures the probe could not
    /// translate into a [`ServiceStatus`]. Well-behaved implementations should
    /// map network errors into
    /// `ServiceStatus { reachable: false, .. }` and return `Ok(...)` instead.
    fn health(&self) -> impl Future<Output = Result<ServiceStatus, ApiError>> + Send;
}
