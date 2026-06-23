//! Circuit-breaker and health accounting for the upstream pool.
//!
//! These methods record per-capability success/failure, drive the
//! consecutive-failure circuit breaker, expose last-error and reprobe-due
//! queries, and surface upstream counts/status. They are an `impl UpstreamPool`
//! block on the struct defined in `pool.rs`, so the private `catalog` and
//! `connections` fields are visible without annotation.

use std::time::Instant;

use super::super::types;
use super::super::types::{UpstreamCapability, UpstreamEntry, UpstreamHealth};
use super::UpstreamPool;

impl UpstreamPool {
    pub async fn record_failure(&self, upstream_name: &str, error: impl Into<String>) {
        self.record_failure_for(upstream_name, UpstreamCapability::Tools, error)
            .await;
    }

    /// Record a failure for a specific upstream capability, potentially marking it unhealthy.
    ///
    /// After [`CIRCUIT_BREAKER_THRESHOLD`] consecutive failures, the upstream
    /// is excluded from the matching capability listing until a successful re-probe.
    pub async fn record_failure_for(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        error: impl Into<String>,
    ) {
        let mut catalog = self.catalog.write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            let error = error.into();
            let new_count = match entry.health_for(capability) {
                UpstreamHealth::Healthy => 1,
                UpstreamHealth::Unhealthy {
                    consecutive_failures,
                } => consecutive_failures + 1,
            };
            entry.set_health_for(
                capability,
                UpstreamHealth::Unhealthy {
                    consecutive_failures: new_count,
                },
            );
            if entry.unhealthy_since_for(capability).is_none() {
                entry.set_unhealthy_since_for(capability, Some(Instant::now()));
            }
            entry.set_last_error_for(capability, Some(error.clone()));
            if new_count >= types::CIRCUIT_BREAKER_THRESHOLD {
                tracing::warn!(
                    upstream = %upstream_name,
                    capability = ?capability,
                    consecutive_failures = new_count,
                    error = %error,
                    "circuit breaker open — upstream excluded from capability listing"
                );
            }
        }
    }

    /// Record a success for an upstream capability, resetting the circuit breaker.
    pub async fn record_success(&self, upstream_name: &str) {
        self.record_success_for(upstream_name, UpstreamCapability::Tools)
            .await;
    }

    /// Record a success for a specific upstream capability, resetting the circuit breaker.
    pub async fn record_success_for(&self, upstream_name: &str, capability: UpstreamCapability) {
        let mut catalog = self.catalog.write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            if !entry.health_for(capability).is_routable() {
                tracing::info!(
                    upstream = %upstream_name,
                    capability = ?capability,
                    "circuit breaker reset — upstream healthy"
                );
            }
            entry.set_health_for(capability, UpstreamHealth::Healthy);
            entry.set_unhealthy_since_for(capability, None);
            entry.set_last_error_for(capability, None);
        }
    }

    /// Return the most relevant last error for an upstream, if any capability has one.
    pub async fn upstream_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Tools)
            .or_else(|| entry.last_error_for(UpstreamCapability::Resources))
            .or_else(|| entry.last_error_for(UpstreamCapability::Prompts))
            .map(ToOwned::to_owned)
    }

    /// Return the last tools-capability error for an upstream, if any.
    pub async fn upstream_tool_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Tools)
            .map(ToOwned::to_owned)
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn insert_entry_for_tests(&self, name: &str, entry: UpstreamEntry) {
        self.catalog.write().await.insert(name.to_string(), entry);
    }

    /// Test-only: insert a fully-formed `UpstreamEntry` into the catalog.
    pub async fn insert_entry_for_test(&self, name: &str, entry: UpstreamEntry) {
        self.catalog.write().await.insert(name.to_string(), entry);
    }

    /// Check if an upstream capability is due for a re-probe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn should_reprobe(&self, upstream_name: &str) -> bool {
        self.should_reprobe_for(upstream_name, UpstreamCapability::Tools)
            .await
    }

    /// Check if a specific upstream capability is due for a re-probe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn should_reprobe_for(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
    ) -> bool {
        let catalog = self.catalog.read().await;
        if let Some(entry) = catalog.get(upstream_name)
            && entry.health_for(capability).is_open()
            && let Some(since) = entry.unhealthy_since_for(capability)
        {
            return since.elapsed() >= types::REPROBE_INTERVAL;
        }
        false
    }

    /// Filter out upstream tools whose names collide with built-in service tools.
    ///
    /// Built-in lab services permanently take precedence. Upstream tools with
    /// colliding names are dropped with a warning.
    pub async fn filter_collisions(&self, builtin_names: &[&str]) {
        let mut catalog = self.catalog.write().await;
        for entry in catalog.values_mut() {
            let collisions: Vec<String> = entry
                .tools
                .keys()
                .filter(|name| builtin_names.contains(&name.as_str()))
                .cloned()
                .collect();
            for name in &collisions {
                tracing::warn!(
                    upstream = %entry.name,
                    tool = %name,
                    "upstream tool name collides with built-in service — rejecting upstream tool"
                );
                entry.tools.remove(name);
            }
        }
    }

    /// Get the number of connected upstreams.
    pub async fn upstream_count(&self) -> usize {
        self.catalog.read().await.len()
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn connection_count_for_tests(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Get names of all registered upstreams with their tool health status.
    pub async fn upstream_status(&self) -> Vec<(String, UpstreamHealth)> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .map(|e| (e.name.to_string(), e.tool_health))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::super::entries::healthy_in_process_entry;
    use super::*;

    #[tokio::test]
    async fn upstream_last_error_tracks_capability_failure_details() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Resources,
            "resource listing returned 401 unauthorized",
        )
        .await;

        assert_eq!(
            pool.upstream_last_error("github").await.as_deref(),
            Some("resource listing returned 401 unauthorized")
        );

        pool.record_success_for("github", UpstreamCapability::Resources)
            .await;
        assert_eq!(pool.upstream_last_error("github").await, None);
    }

    #[tokio::test]
    async fn upstream_tool_last_error_ignores_non_tool_failures() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Resources,
            "resource listing returned 401 unauthorized",
        )
        .await;
        pool.record_failure_for(
            "github",
            UpstreamCapability::Prompts,
            "prompt listing returned 501 unsupported",
        )
        .await;

        assert_eq!(pool.upstream_tool_last_error("github").await, None);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Tools,
            "tool listing returned 500 internal error",
        )
        .await;

        assert_eq!(
            pool.upstream_tool_last_error("github").await.as_deref(),
            Some("tool listing returned 500 internal error")
        );
    }

    /// Helper: read the current tool-capability health for an upstream.
    async fn tool_health(pool: &UpstreamPool, name: &str) -> UpstreamHealth {
        let catalog = pool.catalog.read().await;
        catalog
            .get(name)
            .expect("entry present")
            .health_for(UpstreamCapability::Tools)
    }

    #[tokio::test]
    async fn circuit_breaker_opens_after_threshold_then_closes_on_success() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());
        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        // Starts healthy/routable.
        assert!(tool_health(&pool, "github").await.is_routable());
        assert!(!tool_health(&pool, "github").await.is_open());

        // Record CIRCUIT_BREAKER_THRESHOLD consecutive failures. The breaker
        // should only open on the final one.
        for i in 1..types::CIRCUIT_BREAKER_THRESHOLD {
            pool.record_failure_for(
                "github",
                UpstreamCapability::Tools,
                format!("tool listing failed (attempt {i})"),
            )
            .await;
            assert!(
                tool_health(&pool, "github").await.is_routable(),
                "breaker must stay closed before reaching the threshold (after {i} failures)"
            );
            assert!(!tool_health(&pool, "github").await.is_open());
        }

        // The threshold-th consecutive failure opens the breaker.
        pool.record_failure_for(
            "github",
            UpstreamCapability::Tools,
            "tool listing failed (threshold hit)",
        )
        .await;

        let opened = tool_health(&pool, "github").await;
        assert!(
            opened.is_open(),
            "breaker must be open after CIRCUIT_BREAKER_THRESHOLD failures"
        );
        assert!(!opened.is_routable(), "open breaker must not be routable");
        assert!(matches!(
            opened,
            UpstreamHealth::Unhealthy {
                consecutive_failures
            } if consecutive_failures == types::CIRCUIT_BREAKER_THRESHOLD
        ));

        // A single success closes/recovers the breaker.
        pool.record_success_for("github", UpstreamCapability::Tools)
            .await;

        let recovered = tool_health(&pool, "github").await;
        assert!(
            matches!(recovered, UpstreamHealth::Healthy),
            "success must reset breaker to Healthy"
        );
        assert!(recovered.is_routable());
        assert!(!recovered.is_open());
        // Last-error and unhealthy-since are cleared on recovery.
        assert_eq!(pool.upstream_tool_last_error("github").await, None);
    }
}
