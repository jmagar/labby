//! `UpstreamConnection` lifecycle: `Debug`, `Drop`, graceful `shutdown`, and the
//! pool's `acquire_peer` accessor.
//!
//! The `UpstreamConnection` and `UpstreamPool` struct definitions stay in
//! `pool.rs`; this descendant module only carries their `impl` bodies, so the
//! private fields are visible without annotation. `shutdown` and `acquire_peer`
//! are promoted to `pub(super)` because they are called from sibling modules
//! (`lifecycle`, `probe`, `ensure`, `tools_call`, `resources_read`,
//! `prompts_get`).

use std::time::Instant;

#[cfg(unix)]
use crate::process::unix::{
    pid_is_alive, terminate_process_group_sigkill, terminate_process_group_sigterm,
};

use super::super::types::UpstreamCapability;
use super::helpers::STDIO_SHUTDOWN_TIMEOUT;
use super::logging::capability_name;
use super::{UpstreamConnection, UpstreamPool};

impl std::fmt::Debug for UpstreamConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamConnection").finish_non_exhaustive()
    }
}

/// Sync Drop: SIGTERM+SIGKILL the process group if any, then abort any
/// in-process server task. Last-resort abandonment cleanup for stdio
/// upstreams whose connect future was dropped without going through
/// `shutdown()` — discovery timeouts, cancelled `buffer_unordered` futures,
/// pool drops, `insert()` overwrites, etc.
///
/// The async `shutdown()` graceful path zeroes `self.runtime.pgid` and
/// takes `_server_task` before its first `.await` so this Drop no-ops on
/// the graceful path.
///
/// Process-group kill is `#[cfg(unix)]`-gated (no Windows equivalent in the
/// same shape), but `_server_task.abort()` runs on all platforms — without
/// it a dropped in-process upstream would leak the spawned tokio task.
impl Drop for UpstreamConnection {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.runtime.pgid.take() {
            // No sleep — Drop must not block. Kernel handles TERM/KILL race.
            if let Err(error) = terminate_process_group_sigterm(pgid) {
                tracing::warn!(
                    target: "upstream.connection",
                    pgid,
                    ?error,
                    "process group SIGTERM failed on drop"
                );
            }
            if let Err(error) = terminate_process_group_sigkill(pgid) {
                tracing::warn!(
                    target: "upstream.connection",
                    pgid,
                    ?error,
                    "process group SIGKILL failed on drop"
                );
            } else {
                tracing::debug!(
                    target: "upstream.connection",
                    pgid,
                    "process group reaped on connection drop"
                );
            }
        }
        if let Some(handle) = self._server_task.take() {
            handle.abort();
        }
    }
}

impl UpstreamConnection {
    pub(super) async fn shutdown(mut self, upstream_name: &str, reason: &'static str) {
        // Clone runtime BEFORE taking pgid so subsequent log lines surface
        // the actual pgid (otherwise `runtime.pgid` reads as None after
        // `.take()` clears it).
        let runtime = self.runtime.clone();
        // INVARIANT: take pgid BEFORE any `.await` so the consuming Drop
        // sees `None` and no-ops. This prevents double-kill on the graceful
        // path. `runtime_pgid` carries the value through the function so the
        // graceful TERM→sleep→KILL sequence below can still target the
        // process group.
        #[cfg(unix)]
        let runtime_pgid = self.runtime.pgid.take();
        let started = Instant::now();
        let result = self
            ._client_service
            .close_with_timeout(STDIO_SHUTDOWN_TIMEOUT)
            .await;
        if let Some(server_task) = self._server_task.take() {
            server_task.abort();
        }

        #[cfg(unix)]
        if let (Some(pid), Some(pgid)) = (runtime.pid, runtime_pgid)
            && pid_is_alive(pid)
        {
            let _ = terminate_process_group_sigterm(pgid);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if pid_is_alive(pid) {
                let _ = terminate_process_group_sigkill(pgid);
            }
        }

        match result {
            Ok(Some(_)) => tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "finish",
                operation = "connection.shutdown",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream connection shutdown finished"
            ),
            Ok(None) => tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "timeout",
                operation = "connection.shutdown",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                timeout_ms = STDIO_SHUTDOWN_TIMEOUT.as_millis(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream connection shutdown timed out"
            ),
            Err(error) => tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.connection.shutdown",
                event = "error",
                operation = "connection.shutdown",
                upstream = upstream_name,
                reason,
                pid = ?runtime.pid,
                pgid = ?runtime.pgid,
                error = %error,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream connection shutdown failed"
            ),
        }
    }
}

impl UpstreamPool {
    pub(super) async fn acquire_peer(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        requested_operation: &'static str,
    ) -> Option<rmcp::service::Peer<rmcp::RoleClient>> {
        let acquire_started = Instant::now();
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.acquire",
            event = "start",
            operation = "connection.acquire",
            requested_operation,
            upstream = %upstream_name,
            capability = capability_name(capability),
            "upstream pool acquire start"
        );
        let connections = self.connections.read().await;
        let connection_count = connections.len();
        let peer = connections.get(upstream_name).map(|conn| conn.peer.clone());
        drop(connections);
        let pool_size = self.catalog.read().await.len();
        let elapsed_ms = acquire_started.elapsed().as_millis();
        if peer.is_some() {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.acquire",
                event = "finish",
                operation = "connection.acquire",
                requested_operation,
                upstream = %upstream_name,
                capability = capability_name(capability),
                elapsed_ms,
                pool_size,
                connection_count,
                "upstream pool acquire finish"
            );
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.acquire",
                event = "empty",
                operation = "connection.acquire",
                requested_operation,
                upstream = %upstream_name,
                capability = capability_name(capability),
                elapsed_ms,
                kind = "upstream_not_connected",
                pool_size,
                connection_count,
                "upstream pool acquire empty"
            );
        }
        peer
    }
}
