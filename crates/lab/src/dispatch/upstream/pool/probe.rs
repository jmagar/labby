//! Background reprobe scheduling and the reprobe/heartbeat engine.
//!
//! `ensure_probe_task` spawns a per-upstream background loop that periodically
//! calls `reprobe_upstream` (heartbeat existing connections, reconnect on
//! failure) with jittered backoff. Both are `pub(super)` because they are called
//! across module boundaries — `ensure_probe_task` from `discover.rs` and
//! `reprobe_upstream` from `ensure.rs` (see plan §2.1).

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::config::UpstreamConfig;

use super::super::transport::websocket::{jitter_delay, reprobe_backoff};
use super::super::types;
use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::connect::connect_upstream;
use super::connect::stable_jitter_seed;
use super::helpers::{
    AUTH_FAILURE_REPROBE_ATTEMPT_FLOOR, DISCOVERY_TIMEOUT, auth_error_should_backoff_aggressively,
    classify_upstream_error, upstream_transport,
};

impl UpstreamPool {
    pub(super) fn ensure_probe_task(&self, config: UpstreamConfig) {
        if config.oauth.is_some() {
            return;
        }

        let pool = self.clone();
        tokio::spawn(async move {
            let mut tasks = pool.probe_tasks.write().await;
            if tasks.contains_key(&config.name) {
                return;
            }
            let cancel = CancellationToken::new();
            tasks.insert(config.name.clone(), cancel.clone());
            drop(tasks);
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "scheduled",
                operation = "health",
                upstream = %config.name,
                transport = upstream_transport(&config),
                "upstream reprobe scheduled"
            );

            let mut attempt = 0_u32;
            loop {
                let base = reprobe_backoff(attempt);
                let sleep_for = if attempt == 0 {
                    types::REPROBE_INTERVAL
                } else {
                    jitter_delay(base, stable_jitter_seed(&config.name, attempt))
                };
                tracing::debug!(
                    surface = "dispatch",
                    service = "upstream.pool",
                    action = "upstream.reprobe",
                    event = "sleep",
                    operation = "health",
                    upstream = %config.name,
                    transport = upstream_transport(&config),
                    attempt,
                    sleep_ms = sleep_for.as_millis(),
                    "upstream reprobe sleep scheduled"
                );
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "cancelled",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            "upstream reprobe cancelled"
                        );
                        break;
                    },
                    _ = tokio::time::sleep(sleep_for) => {}
                }

                let reprobe_started = Instant::now();
                match pool.reprobe_upstream(&config).await {
                    Ok(true) => {
                        tracing::info!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "finish",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            changed = true,
                            "upstream reprobe succeeded"
                        );
                        attempt = 0;
                    }
                    Ok(false) => {
                        tracing::debug!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "finish",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            changed = false,
                            "upstream reprobe skipped"
                        );
                    }
                    Err(error) => {
                        let kind = classify_upstream_error(&error.to_string());
                        attempt = attempt.saturating_add(1);
                        if auth_error_should_backoff_aggressively(kind) {
                            attempt = attempt.max(AUTH_FAILURE_REPROBE_ATTEMPT_FLOOR);
                        }
                        tracing::warn!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "error",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            kind,
                            error = %error,
                            "upstream reprobe failed"
                        );
                    }
                }
            }
        });
    }

    pub(super) async fn reprobe_upstream(&self, config: &UpstreamConfig) -> anyhow::Result<bool> {
        let started = Instant::now();
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "start",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(config),
            "upstream reprobe start"
        );
        let existing_peer = {
            let connections = self.connections.read().await;
            connections
                .get(&config.name)
                .map(|connection| connection.peer.clone())
        };

        if let Some(peer) = existing_peer {
            match tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_all_tools()).await {
                Ok(Ok(tools)) => {
                    self.replace_catalog_tools(config, tools).await;
                    self.record_success_for(&config.name, UpstreamCapability::Tools)
                        .await;
                    tracing::info!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.finish",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream heartbeat succeeded"
                    );
                    return Ok(true);
                }
                Ok(Err(error)) => {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        format!("upstream heartbeat failed: {error}"),
                    )
                    .await;
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.error",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        kind = "upstream_heartbeat_failed",
                        error = %error,
                        "upstream heartbeat failed"
                    );
                }
                Err(_) => {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        "upstream heartbeat timed out",
                    )
                    .await;
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.error",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        kind = "timeout",
                        timeout_secs = DISCOVERY_TIMEOUT.as_secs(),
                        "upstream heartbeat timed out"
                    );
                }
            }
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "empty",
                operation = "health",
                upstream = %config.name,
                transport = upstream_transport(config),
                elapsed_ms = started.elapsed().as_millis(),
                kind = "upstream_not_connected",
                "upstream reprobe found no existing connection"
            );
        }

        let stale_connection = {
            let mut connections = self.connections.write().await;
            connections.remove(&config.name)
        };
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.reprobe.reconnect")
                .await;
        }

        let (conn, tools) = connect_upstream(
            config,
            None,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
        )
        .await?;
        {
            let mut connections = self.connections.write().await;
            connections.insert(config.name.clone(), conn);
        }
        self.replace_catalog_tools(config, tools).await;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "reconnect.finish",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(config),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream reprobe reconnect succeeded"
        );
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::*;
    use super::*;

    #[tokio::test]
    async fn disabled_upstream_reprobe_is_inert() {
        let pool = UpstreamPool::new();
        let mut config = test_upstream_config();
        config.enabled = false;
        config.command = Some("definitely-not-spawned".to_string());

        let result = pool
            .reprobe_tools_for_upstream(&config)
            .await
            .expect("disabled reprobe should not error");

        assert!(!result);
        assert!(pool.find_tool("anything").await.is_none());
    }

    #[test]
    fn observability_source_covers_pool_acquire_reprobe_and_drain_events() {
        // The pool was split into `pool.rs` + the `pool/` child modules, so the
        // observability instrumentation now lives across several files. Scan the
        // whole upstream-pool source tree (pool.rs + every pool/*.rs) so this
        // guard stays robust as code relocates between modules. A missing string
        // here means a real dropped-instrumentation regression — never delete an
        // assertion to make this test pass; add the file the string moved into.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/dispatch/upstream");
        let mut source =
            std::fs::read_to_string(format!("{dir}/pool.rs")).expect("read pool.rs source");
        let pool_dir = format!("{dir}/pool");
        if let Ok(entries) = std::fs::read_dir(&pool_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    source.push_str(
                        &std::fs::read_to_string(&path).expect("read pool child module source"),
                    );
                }
            }
        }
        for expected in [
            "action = \"upstream.acquire\"",
            "elapsed_ms",
            "pool_size",
            "connection_count",
            "action = \"upstream.reprobe\"",
            "operation = \"health\"",
            "action = \"upstream.pool.drain\"",
            "cancelled_probe_count",
            "kind = \"upstream_pool_empty\"",
            "kind = \"upstream_not_connected\"",
            "fn log_upstream_request_start",
            "fn log_upstream_request_finish",
            "fn log_upstream_request_error",
            "action = \"upstream.request\"",
        ] {
            assert!(
                source.contains(expected),
                "missing upstream pool observability field `{expected}`"
            );
        }
    }
}
