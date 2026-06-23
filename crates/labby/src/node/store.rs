use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;

use crate::node::checkin::{NodeHello, NodeMetadataUpload, NodeStatus};
use crate::node::log_event::NodeLogEvent;
use crate::node::log_store::SqliteNodeLogStore;

// FIX: VecDeque replaces Vec so trim-from-front is O(1) per pop instead of
// O(n) element shifting. Under 100 nodes sending burst logs this eliminates
// 100 x O(10k) shifts inside the global BTreeMap write lock.
const MAX_LOG_EVENTS_PER_NODE: usize = 10_000;

#[derive(Debug, Clone)]
pub struct NodeStore {
    inner: Arc<RwLock<BTreeMap<String, NodeSnapshot>>>,
    /// Optional durable SQLite log store. `None` when master is not running
    /// or SQLite could not be opened (graceful degradation to in-memory only).
    log_store: Option<SqliteNodeLogStore>,
}

impl Default for NodeStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            log_store: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    pub node_id: String,
    pub connected: bool,
    pub last_seen: SystemTime,
    pub role: Option<String>,
    pub status: Option<NodeStatus>,
    pub metadata: Option<NodeMetadataUpload>,
    /// Recent in-memory log ring buffer (latest MAX_LOG_EVENTS_PER_NODE events).
    /// VecDeque allows O(1) pop_front for trim operations.
    pub logs: VecDeque<NodeLogEvent>,
}

impl NodeStore {
    /// Create a `NodeStore` backed by a durable SQLite log store.
    #[allow(dead_code)]
    pub fn with_log_store(log_store: SqliteNodeLogStore) -> Self {
        Self {
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            log_store: Some(log_store),
        }
    }

    pub async fn record_hello(&self, hello: NodeHello) {
        let mut inner = self.inner.write().await;
        let previous = inner.get(&hello.node_id);
        let is_new = previous.is_none();
        let previous_connected = previous.map(|snapshot| snapshot.connected).unwrap_or(false);
        let previous_role = previous.and_then(|snapshot| snapshot.role.clone());
        let previous_version =
            previous.and_then(|snapshot| snapshot.status.as_ref()?.version.clone());
        let snapshot = inner
            .entry(hello.node_id.clone())
            .or_insert_with(|| NodeSnapshot {
                node_id: hello.node_id.clone(),
                connected: true,
                last_seen: SystemTime::now(),
                role: None,
                status: None,
                metadata: None,
                logs: VecDeque::new(),
            });
        snapshot.node_id = hello.node_id.clone();
        snapshot.connected = true;
        snapshot.last_seen = SystemTime::now();
        snapshot.role = Some(hello.role.clone());
        tracing::info!(
            surface = "node", service = "store", action = "node.hello",
            event = "node.state_changed",
            node_id = %hello.node_id,
            role = %hello.role,
            version = %hello.version,
            connected = true,
            previous_connected,
            previous_role = previous_role.as_deref(),
            previous_version = previous_version.as_deref(),
            is_new_node = is_new,
            "node hello recorded",
        );
    }

    pub async fn record_status(&self, status: NodeStatus) {
        let mut inner = self.inner.write().await;
        let snapshot = inner
            .entry(status.node_id.clone())
            .or_insert_with(|| NodeSnapshot {
                node_id: status.node_id.clone(),
                connected: status.connected,
                last_seen: SystemTime::now(),
                role: None,
                status: None,
                metadata: None,
                logs: VecDeque::new(),
            });
        snapshot.node_id = status.node_id.clone();
        snapshot.connected = status.connected;
        snapshot.last_seen = SystemTime::now();
        tracing::debug!(
            surface = "node", service = "store", action = "node.status",
            node_id = %status.node_id,
            connected = status.connected,
            "node status recorded",
        );
        snapshot.status = Some(status);
    }

    pub async fn set_connected(&self, node_id: &str, connected: bool) {
        let mut inner = self.inner.write().await;
        let snapshot = inner
            .entry(node_id.to_string())
            .or_insert_with(|| NodeSnapshot {
                node_id: node_id.to_string(),
                connected,
                last_seen: SystemTime::now(),
                role: None,
                status: None,
                metadata: None,
                logs: VecDeque::new(),
            });
        let prev_connected = snapshot.connected;
        snapshot.connected = connected;
        snapshot.last_seen = SystemTime::now();
        if let Some(status) = snapshot.status.as_mut() {
            status.connected = connected;
        }
        if prev_connected != connected {
            let role = snapshot.role.as_deref();
            let version = snapshot
                .status
                .as_ref()
                .and_then(|status| status.version.as_deref());
            if connected {
                tracing::info!(
                    surface = "node", service = "store", action = "node.connected",
                    event = "node.connected",
                    node_id = %node_id,
                    role,
                    version,
                    connected,
                    previous_connected = prev_connected,
                    "node marked connected",
                );
            } else {
                tracing::info!(
                    surface = "node", service = "store", action = "node.disconnected",
                    event = "node.disconnected",
                    node_id = %node_id,
                    role,
                    version,
                    connected,
                    previous_connected = prev_connected,
                    "node marked disconnected",
                );
            }
        }
    }

    pub async fn node(&self, node_id: &str) -> Option<NodeSnapshot> {
        let inner = self.inner.read().await;
        inner.get(node_id).cloned()
    }

    pub async fn list_nodes(&self) -> Vec<NodeSnapshot> {
        let inner = self.inner.read().await;
        inner.values().cloned().collect()
    }

    pub async fn record_metadata(&self, metadata: NodeMetadataUpload) {
        let mut inner = self.inner.write().await;
        let snapshot = inner
            .entry(metadata.node_id.clone())
            .or_insert_with(|| NodeSnapshot {
                node_id: metadata.node_id.clone(),
                connected: false,
                last_seen: SystemTime::now(),
                role: None,
                status: None,
                metadata: None,
                logs: VecDeque::new(),
            });
        snapshot.node_id = metadata.node_id.clone();
        snapshot.last_seen = SystemTime::now();
        snapshot.metadata = Some(metadata);
    }

    pub async fn record_logs(&self, node_id: &str, events: Vec<NodeLogEvent>) {
        // PERF: Update the in-memory snapshot under the write lock, then release
        // the lock before awaiting the async mpsc send to the SQLite writer task.
        // Holding the lock through an async send would serialize all node check-ins
        // behind the 4096-bounded channel.
        {
            let mut inner = self.inner.write().await;
            let snapshot = inner
                .entry(node_id.to_string())
                .or_insert_with(|| NodeSnapshot {
                    node_id: node_id.to_string(),
                    connected: false,
                    last_seen: SystemTime::now(),
                    role: None,
                    status: None,
                    metadata: None,
                    logs: VecDeque::new(),
                });
            snapshot.last_seen = SystemTime::now();
            for event in &events {
                snapshot.logs.push_back(event.clone());
            }
            // Trim from the front with O(1) VecDeque::pop_front.
            while snapshot.logs.len() > MAX_LOG_EVENTS_PER_NODE {
                snapshot.logs.pop_front();
            }
        } // Write lock released here -- before any async await.

        // Send to SQLite writer task (best-effort; errors logged inside the store).
        if let Some(store) = &self.log_store {
            for event in events {
                if let Err(error) = store.ingest(event).await {
                    tracing::warn!(
                        surface = "node",
                        service = "store",
                        action = "record_logs",
                        kind = "internal_error",
                        error,
                        "failed to ingest node log event into sqlite store",
                    );
                }
            }
        }
    }

    /// Search log events for a node.
    ///
    /// When a durable SQLite store is present, delegates to it for indexed
    /// full-history search. Falls back to the in-memory VecDeque when no store
    /// is configured (e.g., on non-master nodes).
    pub async fn search_logs_for_node(
        &self,
        node_id: &str,
        needle: &str,
        offset: usize,
        limit: usize,
    ) -> Vec<NodeLogEvent> {
        self.search_logs_for_node_with_range(node_id, needle, None, None, offset, limit)
            .await
    }

    /// Search log events with optional timestamp range filtering.
    ///
    /// `since_ms` and `until_ms` are inclusive bounds in Unix milliseconds.
    pub async fn search_logs_for_node_with_range(
        &self,
        node_id: &str,
        needle: &str,
        since_ms: Option<i64>,
        until_ms: Option<i64>,
        offset: usize,
        limit: usize,
    ) -> Vec<NodeLogEvent> {
        if let Some(store) = &self.log_store {
            match store
                .search(
                    node_id.to_string(),
                    needle.to_string(),
                    since_ms,
                    until_ms,
                    offset,
                    limit,
                )
                .await
            {
                Ok(events) => return events,
                Err(error) => {
                    tracing::warn!(
                        surface = "node",
                        service = "store",
                        action = "search_logs_for_node",
                        kind = "internal_error",
                        error,
                        "sqlite log search failed; falling back to in-memory store",
                    );
                }
            }
        }

        // Fallback: in-memory VecDeque search.
        let inner = self.inner.read().await;
        let Some(snapshot) = inner.get(node_id) else {
            return Vec::new();
        };

        let normalized_needle = needle.to_ascii_lowercase();
        let effective_limit = limit.max(1).min(1_000);
        snapshot
            .logs
            .iter()
            .filter(|event| {
                if !normalized_needle.is_empty()
                    && !event
                        .message
                        .to_ascii_lowercase()
                        .contains(&normalized_needle)
                {
                    return false;
                }
                if let Some(since) = since_ms {
                    if event.timestamp_unix_ms < since {
                        return false;
                    }
                }
                if let Some(until) = until_ms {
                    if event.timestamp_unix_ms > until {
                        return false;
                    }
                }
                true
            })
            .skip(offset)
            .take(effective_limit)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn node_connection_logs_keep_observability_fields() {
        let source = include_str!("store.rs");
        for field in [
            "event = \"node.state_changed\"",
            "event = \"node.connected\"",
            "event = \"node.disconnected\"",
            "node_id = %node_id",
            "role",
            "version",
            "previous_connected",
        ] {
            assert!(source.contains(field), "missing log field: {field}");
        }
    }
}
