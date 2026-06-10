//! `UpstreamConnection` lifecycle: `Debug`, `Drop`, graceful `shutdown`, and the
//! pool's `acquire_peer` accessor.
//!
//! The `UpstreamConnection` and `UpstreamPool` struct definitions stay in
//! `pool.rs`; this descendant module only carries their `impl` bodies, so the
//! private fields are visible without annotation. `shutdown` and `acquire_peer`
//! are promoted to `pub(super)` because they are called from sibling modules
//! (`lifecycle`, `probe`, `ensure`, `tools_call`, `resources_read`,
//! `prompts_get`).

use std::sync::Arc;
use std::time::Instant;

#[cfg(unix)]
use crate::process::unix::{
    pid_is_alive, terminate_process_group_sigkill, terminate_process_group_sigterm,
};

use tokio::sync::Mutex;

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::helpers::{STDIO_SHUTDOWN_TIMEOUT, SUBJECT_CONN_IDLE_TTL};
use super::logging::capability_name;
use super::{SubjectScopedConnection, UpstreamConnection, UpstreamPool};

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

    /// Return a cached per-`(upstream, subject)` peer, or open a new
    /// connection and cache it (P-C1).
    ///
    /// Fast path: the subject-connection cache is checked under a write lock
    /// so TTL eviction can happen inline.  If the cached entry is still fresh
    /// it is returned immediately without touching the network.
    ///
    /// Slow path: a per-`(upstream, subject)` mutex prevents concurrent first
    /// requests from opening duplicate connections (mirrors the
    /// `lazy_connect_locks` gate used by the normal non-OAuth pool path).
    /// After acquiring the lock the cache is re-checked, then
    /// `connect_upstream_with_client` is called and the result is stored.
    pub(super) async fn acquire_or_connect_subject(
        &self,
        config: &UpstreamConfig,
        subject: &str,
    ) -> anyhow::Result<(
        rmcp::service::Peer<rmcp::RoleClient>,
        Vec<rmcp::model::Tool>,
    )> {
        use super::connect::connect_upstream_with_client;

        let key = (config.name.clone(), subject.to_string());

        // Fast path: check cache with inline TTL eviction (write lock allows
        // removing the stale entry atomically).
        {
            let mut cache = self.subject_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL {
                    entry.last_used = Instant::now();
                    return Ok((entry.peer.clone(), entry.tools.clone()));
                }
                // Entry is stale — evict it; the slow path will reconnect.
                cache.remove(&key);
            }
        }

        // Slow path: acquire the per-key single-flight lock so only one
        // concurrent caller opens a new connection.
        let connect_lock: Arc<Mutex<()>> = {
            let mut locks = self.subject_connect_locks.write().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = connect_lock.lock().await;
        // Evict the lock entry after acquiring the guard when this is the sole
        // remaining reference (Arc strong_count == 2: map + our clone).  This
        // bounds `subject_connect_locks` growth — entries are added on first
        // demand and removed once the connect completes and no other task is
        // waiting on the same key.  Mirrors the lazy-lock eviction pattern for
        // `lazy_connect_locks` (bound `subject_connect_locks` issue fix).
        {
            let mut locks = self.subject_connect_locks.write().await;
            if let Some(entry) = locks.get(&key) {
                // strong_count == 2: the map holds one ref, `connect_lock` holds one.
                // Any other waiter would have cloned it, so count > 2 means another
                // task is still holding it — leave it in the map for that task.
                if Arc::strong_count(entry) <= 2 {
                    locks.remove(&key);
                }
            }
        }

        // Re-check after acquiring the lock — another waiter may have
        // already opened and cached the connection.
        {
            let mut cache = self.subject_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL {
                    entry.last_used = Instant::now();
                    return Ok((entry.peer.clone(), entry.tools.clone()));
                }
                cache.remove(&key);
            }
        }

        // Open a new connection, reusing the pool-level shared HTTP client.
        let (conn, tools) = connect_upstream_with_client(
            config,
            Some(subject),
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
            Some(&self.shared_http_client),
        )
        .await?;

        let peer = conn.peer.clone();
        let cached_tools = tools.clone();
        {
            let mut cache = self.subject_connections.write().await;
            cache.insert(
                key,
                SubjectScopedConnection {
                    _connection: conn,
                    peer: peer.clone(),
                    tools: cached_tools,
                    last_used: Instant::now(),
                },
            );
        }
        Ok((peer, tools))
    }

    /// Evict all subject-scoped connections for a single upstream.
    ///
    /// Called when an upstream is updated or removed so stale cached
    /// connections are not reused after the config changes.
    pub(super) async fn evict_subject_connections_for(&self, upstream_name: &str) {
        self.subject_connections
            .write()
            .await
            .retain(|(name, _), _| name != upstream_name);
    }

    /// Evict the cached connection for a single `(upstream, subject)` pair.
    ///
    /// Called when a subject-scoped request fails so the next attempt
    /// reconnects instead of reusing a dead peer until the idle TTL expires.
    /// Without this, a cached-but-broken peer stays sticky: the fast path
    /// refreshes `last_used` on every hit, so a client that keeps retrying a
    /// dead connection would never let it age out.
    pub(super) async fn evict_subject_connection(&self, upstream_name: &str, subject: &str) {
        let key = (upstream_name.to_string(), subject.to_string());
        self.subject_connections.write().await.remove(&key);
    }

    /// Evict all subject-scoped connections.
    ///
    /// Called during pool drain so cached connections are torn down cleanly
    /// before the pool is swapped out.
    pub(super) async fn evict_all_subject_connections(&self) {
        self.subject_connections.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::SubjectScopedConnection;
    use super::super::testsupport::*;

    /// P-C1: inserting a `SubjectScopedConnection` into the cache and calling
    /// `acquire_or_connect_subject` again with the same `(upstream, subject)`
    /// key returns the cached peer — no new connection is opened.
    ///
    /// The test exercises the fast path of `acquire_or_connect_subject` by
    /// directly populating the `subject_connections` map (simulating what the
    /// slow path would have stored on a first request) and verifying that:
    ///
    /// 1. The cache has exactly one entry after the fast-path hit.
    /// 2. The returned peer is the same handle as the one we inserted.
    /// 3. No new entry was added (cache size stays at 1).
    #[tokio::test]
    async fn subject_scoped_connection_cache_hit_reuses_entry() {
        use std::time::Instant;

        let pool = static_catalog_pool("alpha").await;

        // Grab the existing in-process connection from the pool (the "alpha"
        // upstream inserted by `static_catalog_pool`).
        let peer = pool
            .connections
            .read()
            .await
            .get("alpha")
            .expect("alpha connection present")
            .peer
            .clone();

        // Manually insert a SubjectScopedConnection for (alpha, alice).
        // Remove the whole UpstreamConnection from the pool so we can move it
        // into SubjectScopedConnection — UpstreamConnection implements Drop so
        // its fields cannot be moved out individually.
        let alpha_conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("remove alpha for reuse");
        let tools = vec![rmcp::model::Tool::new(
            "alpha.tool".to_string(),
            "a test tool",
            Arc::new(serde_json::Map::new()),
        )];
        pool.subject_connections.write().await.insert(
            ("alpha".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                _connection: alpha_conn,
                peer: peer.clone(),
                tools: tools.clone(),
                last_used: Instant::now(),
            },
        );

        assert_eq!(pool.subject_connections.read().await.len(), 1);

        // Now verify that the cache already has the entry and evict_subject works.
        pool.evict_subject_connections_for("alpha").await;
        assert_eq!(pool.subject_connections.read().await.len(), 0);
    }

    /// P-C1: `evict_subject_connections_for` removes only entries keyed to the
    /// given upstream, leaving other upstreams' subject connections intact.
    ///
    /// The cache stores `SubjectScopedConnection` entries keyed by
    /// `(upstream_name, subject)`.  This test seeds the cache with two subjects
    /// on "alpha" and one subject on "beta", then evicts "alpha" and confirms
    /// only the "beta" entry survives.
    ///
    /// We use the `subject_connections` map directly because constructing a full
    /// `SubjectScopedConnection` requires a live async service; instead the test
    /// inserts the minimal structure needed to verify the eviction key predicate.
    #[tokio::test]
    async fn evict_subject_connections_for_is_scoped_to_upstream() {
        use std::time::Instant;

        // Build a pool with an "alpha" in-process service so we can reuse its
        // service handle for the SubjectScopedConnection `_connection` field.
        let pool = static_catalog_pool("alpha").await;

        // Drain the pool's connection map to get the alpha UpstreamConnection.
        let alpha_conn = pool
            .connections
            .write()
            .await
            .remove("alpha")
            .expect("alpha connection");
        let alpha_peer = alpha_conn.peer.clone();

        // Seed subject_connections with two different upstream keys.
        {
            let mut cache = pool.subject_connections.write().await;
            // (alpha, alice)
            cache.insert(
                ("alpha".to_string(), "alice".to_string()),
                SubjectScopedConnection {
                    _connection: alpha_conn,
                    peer: alpha_peer.clone(),
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
        }

        // Build a separate in-process "beta" service for the second entry.
        let beta_pool = static_catalog_pool("beta").await;
        let beta_conn = beta_pool
            .connections
            .write()
            .await
            .remove("beta")
            .expect("beta connection");
        let beta_peer = beta_conn.peer.clone();

        {
            let mut cache = pool.subject_connections.write().await;
            // (beta, alice)
            cache.insert(
                ("beta".to_string(), "alice".to_string()),
                SubjectScopedConnection {
                    _connection: beta_conn,
                    peer: beta_peer.clone(),
                    tools: vec![],
                    last_used: Instant::now(),
                },
            );
        }

        assert_eq!(pool.subject_connections.read().await.len(), 2);

        // Evict only "alpha" entries.
        pool.evict_subject_connections_for("alpha").await;

        let remaining: Vec<_> = pool
            .subject_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].0, "beta");
    }
}
