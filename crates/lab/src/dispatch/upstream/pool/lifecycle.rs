//! Pool lifecycle: `drain_for_swap` tears down all connections, probe tasks, and
//! catalog state when the pool is swapped out (e.g. on config reload).

use std::time::Instant;

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
            for (upstream_name, connection) in drained {
                connection.shutdown(&upstream_name, reason).await;
            }
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
