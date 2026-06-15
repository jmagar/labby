//! Pool lifecycle: `drain_for_swap` tears down all connections, probe tasks, and
//! catalog state when the pool is swapped out (e.g. on config reload).

use std::time::Instant;

use futures::future::join_all;

use super::UpstreamPool;

impl UpstreamPool {
    pub async fn drain_for_swap(&self, reason: &'static str) {
        let started = Instant::now();
        let catalog_count = self.catalog.read().await.len();
        let connection_count = self.connections.read().await.len();
        let probe_task_count = self.probe_tasks.read().await.len();
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.drain",
            event = "start",
            operation = "pool.drain",
            reason,
            pool_size = catalog_count,
            connection_count,
            probe_task_count,
            "upstream pool drain start"
        );

        // Cancel the background subject-connection sweep task (P-H2) before
        // evicting connections so it does not race the drain.
        if let Some(cancel) = self.subject_sweep_task.write().await.take() {
            cancel.cancel();
        }

        // Evict all subject-scoped cached connections first so the
        // per-`(upstream, subject)` handles are dropped before the pool-level
        // connections are shut down (P-C1 cleanup).
        self.evict_all_subject_connections().await;

        let cancelled_probe_count = {
            let mut tasks = self.probe_tasks.write().await;
            let count = tasks.len();
            for cancel in tasks.values() {
                cancel.cancel();
            }
            tasks.clear();
            count
        };
        let drained_connection_count = {
            let mut connections = self.connections.write().await;
            let count = connections.len();
            let drained = connections.drain().collect::<Vec<_>>();
            drop(connections);
            // Shut down all connections in parallel so an N-upstream pool
            // drains in ~1 shutdown timeout rather than N × shutdown timeout
            // (P-H2).
            let futs: Vec<_> = drained
                .into_iter()
                .map(|(upstream_name, connection)| async move {
                    connection.shutdown(&upstream_name, reason).await;
                })
                .collect();
            join_all(futs).await;
            count
        };
        let drained_catalog_count = {
            let mut catalog = self.catalog.write().await;
            let count = catalog.len();
            catalog.clear();
            count
        };
        self.resource_upstreams.write().await.clear();

        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.pool.drain",
            event = "finish",
            operation = "pool.drain",
            reason,
            elapsed_ms = started.elapsed().as_millis(),
            drained_catalog_count,
            drained_connection_count,
            cancelled_probe_count,
            "upstream pool drain finish"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::*;

    /// P-H2: after `drain_for_swap`, the drained pool's connections, catalog,
    /// and subject-connection cache are all empty.  The purpose of the test is
    /// to confirm the parallel-drain path reaches completion correctly (not just
    /// that the serial path works) and that subject connections are evicted as
    /// part of the drain.
    #[tokio::test]
    async fn drain_for_swap_clears_all_pool_state() {
        let pool = static_catalog_pool("alpha").await;

        // Confirm the pool is non-empty before draining.
        assert_eq!(pool.connection_count_for_tests().await, 1);
        assert!(pool.cached_upstream_summary("alpha").await.is_some());

        pool.drain_for_swap("test.drain").await;

        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert!(pool.cached_upstream_summary("alpha").await.is_none());
        assert!(pool.subject_connections.read().await.is_empty());
    }

    /// P-H2: `drain_for_swap` on a pool with multiple upstreams completes
    /// correctly in parallel — all connections and catalog entries removed.
    #[tokio::test]
    async fn drain_for_swap_clears_multiple_upstreams_in_parallel() {
        // Build a pool with two independent upstreams using the in-process fixture.
        let pool = static_catalog_pool("alpha").await;
        pool.insert_live_tool_server_for_tests(
            "beta",
            std::sync::Arc::new(tokio::sync::RwLock::new(vec!["beta.tool".to_string()])),
        )
        .await;

        assert_eq!(pool.connection_count_for_tests().await, 2);

        pool.drain_for_swap("test.multi_drain").await;

        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert_eq!(pool.catalog.read().await.len(), 0);
    }

    /// P-H2 + pool-swap semantics: a FRESH pool installed BEFORE the old pool is
    /// drained is immediately accessible — the new pool's upstreams are reachable
    /// even while the old pool is being shut down.
    ///
    /// This exercises the build-first / swap / drain-after pattern introduced by
    /// P-H2 at the `GatewayManager` level, verified here at the pool level by
    /// simulating the swap sequence with two pools and an `Arc<RwLock<_>>` swap
    /// handle (analogous to `GatewayRuntimeHandle`).
    #[tokio::test]
    async fn fresh_pool_reachable_while_old_pool_drains() {
        use std::sync::Arc;
        use tokio::sync::RwLock;

        // Old pool: has upstream "alpha".
        let old_pool = static_catalog_pool("alpha").await;
        assert_eq!(old_pool.connection_count_for_tests().await, 1);

        // Fresh pool: has upstream "beta" (simulates a reload that changed one upstream).
        let fresh_pool = static_catalog_pool("beta").await;

        // Simulate the swap: install fresh_pool into the shared handle FIRST.
        let handle: Arc<RwLock<Option<Arc<_>>>> =
            Arc::new(RwLock::new(Some(Arc::clone(&fresh_pool))));

        // Fresh pool is immediately reachable after swap.
        let live = handle.read().await.clone().expect("pool is live");
        assert!(live.cached_upstream_summary("beta").await.is_some());
        assert!(live.cached_upstream_summary("alpha").await.is_none());

        // Now drain the old pool (happens after the swap in the real reload path).
        old_pool.drain_for_swap("test.after_swap_drain").await;

        // Fresh pool (beta) is still intact — drain only affected old_pool.
        let live = handle
            .read()
            .await
            .clone()
            .expect("pool still live after drain");
        assert!(live.cached_upstream_summary("beta").await.is_some());
        assert_eq!(live.connection_count_for_tests().await, 1);
    }
}
