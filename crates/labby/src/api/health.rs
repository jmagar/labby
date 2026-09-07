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
//! 3. **Core provider reachable** — trusted-host mode is not ready unless its
//!    configured private Core provider answers the negotiated protocol health.
//!
//! **FLAG for AUTH agent:** `AppState` was not modified. Readiness is derived
//! from *existing* fields (`registry`, `gateway_manager`). If AUTH needs an
//! explicit `ready: AtomicBool` flag set at a precise moment during serve
//! start-up, that can replace predicate 1 without a breaking layout change.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

use super::state::AppState;

/// Response body for health/readiness probes.
#[cfg_attr(feature = "api-docs", derive(utoipa::ToSchema))]
#[derive(Debug, serde::Serialize)]
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
    /// Sealed runtime capability profile. Integrated mode reports a distinct
    /// value so Core can reject a standalone/all-features artifact at
    /// readiness time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_profile: Option<&'static str>,
    /// Private provider protocol accepted by the integrated profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_protocol: Option<&'static str>,
    /// Structured, non-secret state for the managed Depot authority projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_projection:
        Option<crate::dispatch::depot::authority_projection::ManagedProjectionReadiness>,
}

/// Liveness probe. Returns 200 as long as the process is running.
pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let uptime_s = state.server_start.elapsed().as_secs();
    let integrated = state.trusted_host_verifier.is_some();
    Json(HealthResponse {
        status: "ok".to_string(),
        mode: Some(if integrated {
            "integrated-gateway".to_string()
        } else {
            "gateway-host".to_string()
        }),
        pid: Some(std::process::id()),
        uptime_s: Some(uptime_s),
        pending: None,
        capability_profile: Some(if integrated {
            "unraid-core-integrated-v1"
        } else {
            "standalone-gateway-v1"
        }),
        provider_protocol: integrated.then_some("1.0"),
        authority_projection: crate::dispatch::depot::authority_projection::projection_readiness(),
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

    if state.enabled_services.contains("stash") {
        match state.file_stash_runtime.status().await {
            crate::file_stash::FileStashStatus::Ready => {}
            crate::file_stash::FileStashStatus::Recovering => {
                pending.push("File Stash recovery is still in progress".to_string());
            }
            crate::file_stash::FileStashStatus::Blocked(reason) => {
                pending.push(format!("File Stash unavailable: {reason:?}"));
            }
            crate::file_stash::FileStashStatus::Shutdown => {
                pending.push("File Stash is shutting down".to_string());
            }
        }
    }

    if let Some(reason) =
        crate::dispatch::depot::authority_projection::managed_projection_readiness_pending()
    {
        pending.push(reason);
    }

    // Predicate 2: when a gateway manager is wired, the pool must be present.
    //
    // The pool is `None` until `gateway.reload` completes its first successful
    // upstream discovery pass. Orchestrators (Kubernetes, Compose health-checks)
    // should wait for this before routing traffic so that MCP tool listings are
    // non-empty on first request.
    #[cfg(feature = "gateway")]
    {
        if let Some(manager) = &state.gateway_manager {
            if manager.current_pool().await.is_none() {
                pending.push("gateway pool not yet initialised".to_string());
            }

            if state.trusted_host_verifier.is_some() {
                let core_provider_health = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    manager.core_provider_health(),
                )
                .await;
                if !matches!(core_provider_health, Ok(Ok(()))) {
                    pending.push("Core provider unavailable or incompatible".to_string());
                }
            }
        } else if state.trusted_host_verifier.is_some() {
            pending.push("integrated gateway manager is unavailable".to_string());
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
                capability_profile: None,
                provider_protocol: None,
                authority_projection:
                    crate::dispatch::depot::authority_projection::projection_readiness(),
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
                capability_profile: None,
                provider_protocol: None,
                authority_projection:
                    crate::dispatch::depot::authority_projection::projection_readiness(),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn integrated_health_self_reports_the_sealed_profile() {
        let verifier = Arc::new(labby_auth::trusted_host::TrustedHostVerifier::new(1, []));
        let response = health(State(AppState::new().with_trusted_host_verifier(verifier))).await;

        assert_eq!(response.0.mode.as_deref(), Some("integrated-gateway"));
        assert_eq!(
            response.0.capability_profile,
            Some("unraid-core-integrated-v1")
        );
        assert_eq!(response.0.provider_protocol, Some("1.0"));
    }

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

    #[tokio::test]
    async fn integrated_ready_fails_closed_without_the_gateway_manager() {
        let verifier = Arc::new(labby_auth::trusted_host::TrustedHostVerifier::new(1, []));
        let state = AppState::new().with_trusted_host_verifier(verifier);

        let response = ready(State(state)).await.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// When a gateway manager is wired but the pool has not yet loaded, `/ready`
    /// must return 503 with `pending` naming the unsatisfied predicate.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn ready_returns_503_when_gateway_pool_absent() {
        use std::sync::Arc;

        use crate::dispatch::gateway::config_store::test_gateway_manager;
        use crate::dispatch::gateway::manager::GatewayRuntimeHandle;

        let runtime = GatewayRuntimeHandle::default();
        let directory = tempfile::tempdir().expect("tempdir");
        // Pool starts as None — manager is wired but pool not yet loaded.
        let manager = Arc::new(test_gateway_manager(
            directory.path().join("config.toml"),
            runtime,
        ));
        let state = AppState::new().with_gateway_manager(manager);

        let resp = ready(State(state)).await.into_response();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "/ready must return 503 when gateway pool is not yet initialised"
        );
    }
}
