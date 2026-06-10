//! Liveness and readiness probes.
//!
//! `GET /health` — process is up. Always returns 200.
//! `GET /ready`  — process is ready to serve traffic. Returns 503 until all
//!                  readiness predicates are met.
//!
//! ## Readiness predicates
//!
//! 1. **Registry non-empty** — at least one service is registered in the tool
//!    registry. This passes immediately after `build_default_registry()` runs
//!    during `AppState` construction; a zero-service registry indicates a build
//!    misconfiguration rather than a transient boot condition.
//!
//! 2. **Gateway pool present** — when a gateway manager is wired into
//!    `AppState`, the upstream pool must have completed at least one successful
//!    load (i.e. `current_pool()` is `Some`). When no manager is wired this
//!    predicate is skipped (not every deployment uses the gateway).
//!
//! **FLAG for AUTH agent:** `AppState` was not modified. Readiness is derived
//! from *existing* fields (`registry`, `gateway_manager`). If AUTH needs an
//! explicit `ready: AtomicBool` flag set at a precise moment during serve
//! start-up, that can replace predicate 1 without a breaking layout change.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

use super::state::AppState;

/// Response body for health/readiness probes.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    /// Status string: `"ok"` for liveness, `"ready"` or `"not_ready"` for
    /// readiness.
    pub status: String,
    /// Process role: `"master"` or `"node"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// OS process ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Seconds since the server started accepting requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_s: Option<u64>,
    /// Human-readable list of predicates not yet satisfied.
    /// Present only on 503 responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Vec<String>>,
}

/// Liveness probe. Returns 200 as long as the process is running.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let uptime_s = state.server_start.elapsed().as_secs();
    let mode = if state.is_master() { "master" } else { "node" };
    Json(HealthResponse {
        status: "ok".to_string(),
        mode: Some(mode.to_string()),
        pid: Some(std::process::id()),
        uptime_s: Some(uptime_s),
        pending: None,
    })
}

/// Readiness probe. Returns 200 once all predicates are satisfied, 503
/// otherwise.
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let mut pending: Vec<String> = Vec::new();

    // Predicate 1: registry must have at least one service registered.
    //
    // `build_default_registry()` always populates the registry before
    // `AppState::from_registry` completes, so this predicate passes in all
    // normal deployments.  A zero-service registry indicates a build or
    // feature-flag misconfiguration.
    if state.registry.services().is_empty() {
        pending.push("no services registered in tool registry".to_string());
    }

    // Predicate 2: when a gateway manager is wired, the pool must be present.
    //
    // The pool is `None` until `gateway.reload` completes its first successful
    // upstream discovery pass. Orchestrators (Kubernetes, Compose health-checks)
    // should wait for this before routing traffic so that MCP tool listings are
    // non-empty on first request.
    if let Some(manager) = &state.gateway_manager {
        if manager.current_pool().await.is_none() {
            pending.push("gateway pool not yet initialised".to_string());
        }
    }

    if pending.is_empty() {
        (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ready".to_string(),
                mode: None,
                pid: None,
                uptime_s: None,
                pending: None,
            }),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "not_ready".to_string(),
                mode: None,
                pid: None,
                uptime_s: None,
                pending: Some(pending),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default `AppState` has no gateway manager wired and a populated registry
    /// (all features enabled at compile time), so `/ready` must return 200.
    #[tokio::test]
    async fn ready_returns_200_when_no_gateway_manager() {
        let state = AppState::new();
        // Sanity-check our predicate: registry must be non-empty with --all-features.
        assert!(
            !state.registry.services().is_empty(),
            "AppState::new() must populate the registry; got 0 services"
        );
        let resp = ready(State(state)).await.into_response();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/ready must return 200 when no gateway manager is wired"
        );
    }

    /// When a gateway manager is wired but the pool has not yet loaded, `/ready`
    /// must return 503 with `pending` naming the unsatisfied predicate.
    #[tokio::test]
    async fn ready_returns_503_when_gateway_pool_absent() {
        use std::path::PathBuf;
        use std::sync::Arc;

        use crate::dispatch::gateway::manager::GatewayManager;
        use crate::dispatch::gateway::manager::GatewayRuntimeHandle;

        let runtime = GatewayRuntimeHandle::default();
        // Pool starts as None — manager is wired but pool not yet loaded.
        let manager = Arc::new(GatewayManager::new(PathBuf::from("/tmp/test"), runtime));
        let state = AppState::new().with_gateway_manager(manager);

        let resp = ready(State(state)).await.into_response();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "/ready must return 503 when gateway pool is not yet initialised"
        );
    }
}
