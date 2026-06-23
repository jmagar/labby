//! Service probe logic for `service.probe` and `audit.full`.
//!
//! URL resolution comes exclusively from pre-built `ServiceClients` (which were
//! constructed from env at startup). Caller-supplied URLs in params are rejected
//! by `params.rs`. This is the SSRF defense boundary.

use std::sync::Arc;

use labby_apis::core::ServiceStatus;
use tokio::sync::Semaphore;

use crate::dispatch::clients::ServiceClients;
use crate::dispatch::error::ToolError;

use super::types::{Finding, Severity};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe a single named service and return a `Finding`.
///
/// Returns `Err` when the service name is not in the known service list.
pub async fn probe_service(
    clients: &ServiceClients,
    service: &str,
    _instance: Option<&str>,
) -> Result<Finding, ToolError> {
    let all_names = all_service_names();
    if !all_names.contains(&service) {
        return Err(ToolError::InvalidParam {
            message: format!("unknown service `{service}`"),
            param: "service".to_string(),
        });
    }
    let status = health_by_name_owned(clients, service).await;
    Ok(status_to_finding(service, &status))
}

/// Run only service probes, without system or auth checks.
///
/// Used by `labby doctor services`. Results are sent to `tx` as they complete,
/// in parallel bounded by `Semaphore(5)`.
pub async fn stream_service_probes(
    clients: Arc<ServiceClients>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    let sem = Arc::new(Semaphore::new(5));
    let mut handles = Vec::new();

    for service_name in configured_service_names(&clients) {
        let sem = sem.clone();
        let clients = clients.clone();
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let status = health_by_name_owned(&clients, &service_name);
            let finding = status_to_finding(&service_name, &status.await);
            tx.send(finding).await.ok();
        }));
    }

    for handle in handles {
        handle.await.ok();
    }
}

/// Run `audit.full`: system checks followed by all configured service probes.
///
/// Results are sent to `tx` as they complete. System checks are emitted
/// synchronously first; service probes run in parallel bounded by `Semaphore(5)`.
pub async fn stream_audit_full(
    clients: Arc<ServiceClients>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    // Emit system and auth checks immediately (no network I/O).
    for finding in super::system::run_system_checks() {
        if tx.send(finding).await.is_err() {
            return;
        }
    }
    for finding in super::system::run_auth_checks() {
        if tx.send(finding).await.is_err() {
            return;
        }
    }

    let sem = Arc::new(Semaphore::new(5));
    let mut handles = Vec::new();

    for service_name in configured_service_names(&clients) {
        let sem = sem.clone();
        let clients = clients.clone();
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let status = health_by_name_owned(&clients, &service_name);
            let finding = status_to_finding(&service_name, &status.await);
            tx.send(finding).await.ok();
        }));
    }

    for handle in handles {
        handle.await.ok();
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// All known service names (compiled in), regardless of configuration.
fn all_service_names() -> Vec<&'static str> {
    let names: Vec<&'static str> = Vec::new();
    names
}

/// Names of services that have a configured (non-None) client.
fn configured_service_names(_clients: &ServiceClients) -> Vec<String> {
    let names = Vec::new();
    names
}

/// Async version suitable for use inside `tokio::spawn`.
async fn health_by_name_owned(_clients: &ServiceClients, service: &str) -> ServiceStatus {
    ServiceStatus::unreachable(format!("unknown service `{service}`"))
}
fn status_to_finding(service: &str, status: &ServiceStatus) -> Finding {
    let severity = if !status.reachable {
        Severity::Fail
    } else if !status.auth_ok {
        Severity::Warn
    } else {
        Severity::Ok
    };
    let message = match (&status.message, status.reachable, status.auth_ok) {
        (Some(msg), _, _) => {
            // Truncate to prevent HTML error pages from flooding output.
            let msg = msg.trim();
            let truncated = if msg.chars().count() > 120 {
                let prefix: String = msg.chars().take(120).collect();
                format!("{prefix}…")
            } else {
                msg.to_string()
            };
            format!("{truncated} ({}ms)", status.latency_ms)
        }
        (None, true, true) => format!("healthy ({}ms)", status.latency_ms),
        (None, true, false) => format!("reachable but auth failed ({}ms)", status.latency_ms),
        (None, false, _) => "unreachable".to_string(),
    };
    Finding {
        service: service.to_string(),
        check: "health".to_string(),
        severity,
        message,
    }
}
