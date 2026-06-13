use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use rmcp::RoleServer;
use rmcp::service::Peer;
use tokio::sync::RwLock;
#[cfg(feature = "gateway")]
use tokio::sync::mpsc;

#[cfg(feature = "gateway")]
use crate::dispatch::gateway::types::GatewayCatalogDiff;

/// Per-peer notification timeout (P-L2 fix).
///
/// A slow or hung peer must not stall the entire fanout: one unresponsive
/// client would block every other connected session if notifications were sent
/// serially.  Notifications are now sent concurrently via `join_all`, and each
/// peer's future is individually bounded by this timeout so a single stalled
/// peer drops out without affecting the rest.
const PEER_NOTIFY_TIMEOUT: Duration = Duration::from_secs(5);

/// MCP-specific peer fanout that forwards catalog-change notifications to all
/// connected `rmcp::Peer<RoleServer>` instances.
///
/// This keeps `rmcp` types out of the dispatch layer while allowing
/// `GatewayManager` to notify peers when the upstream pool changes.
#[derive(Clone, Default)]
pub struct PeerNotifier {
    pub peers: Arc<RwLock<Vec<Peer<RoleServer>>>>,
}

impl PeerNotifier {
    #[cfg(feature = "gateway")]
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<GatewayCatalogDiff>) {
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "notifier.start",
            subsystem = "mcp_server",
            phase = "peer_notifier.start",
            "starting MCP peer catalog-change notifier"
        );
        while let Some(diff) = rx.recv().await {
            self.notify_catalog_changes(&diff).await;
        }
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "notifier.stop",
            subsystem = "mcp_server",
            phase = "peer_notifier.stop",
            "MCP peer catalog-change notifier stopped"
        );
    }

    #[cfg(feature = "gateway")]
    async fn notify_catalog_changes(&self, diff: &GatewayCatalogDiff) {
        let peers = self.peers.read().await.clone();
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "catalog.notify",
            subsystem = "mcp_server",
            phase = "catalog.notify",
            peer_count = peers.len(),
            tools_changed = diff.tools_changed,
            resources_changed = diff.resources_changed,
            prompts_changed = diff.prompts_changed,
            "broadcasting catalog change to connected peers"
        );

        // P-L2 fix: notify all peers concurrently so one slow peer cannot
        // stall the fanout.  Each peer future is bounded by PEER_NOTIFY_TIMEOUT
        // so a hung session times out independently.
        let notify_futures = peers.iter().enumerate().map(|(index, peer)| {
            let peer = peer.clone();
            let diff = diff.clone();
            async move {
                let result = tokio::time::timeout(PEER_NOTIFY_TIMEOUT, async {
                    if diff.tools_changed && peer.notify_tool_list_changed().await.is_err() {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            phase = "tools",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "failed to notify peer about catalog change; pruning stale session"
                        );
                        return false;
                    }
                    if diff.resources_changed && peer.notify_resource_list_changed().await.is_err()
                    {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            phase = "resources",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "failed to notify peer about catalog change; pruning stale session"
                        );
                        return false;
                    }
                    if diff.prompts_changed && peer.notify_prompt_list_changed().await.is_err() {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            phase = "prompts",
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "failed to notify peer about catalog change; pruning stale session"
                        );
                        return false;
                    }
                    true
                })
                .await;
                match result {
                    Ok(alive) => alive,
                    Err(_elapsed) => {
                        tracing::warn!(
                            surface = "mcp",
                            service = "peers",
                            action = "peer.disconnect",
                            peer_index = index,
                            timeout_ms = PEER_NOTIFY_TIMEOUT.as_millis(),
                            tools_changed = diff.tools_changed,
                            resources_changed = diff.resources_changed,
                            prompts_changed = diff.prompts_changed,
                            "peer notification timed out; pruning stale session"
                        );
                        false
                    }
                }
            }
        });

        let snapshot_len = peers.len();
        let results = join_all(notify_futures).await;
        let alive: Vec<Peer<RoleServer>> = peers
            .into_iter()
            .zip(results)
            .filter_map(|(peer, ok)| ok.then_some(peer))
            .collect();

        let pruned = snapshot_len.saturating_sub(alive.len());

        let mut guard = self.peers.write().await;
        // Preserve peers that connected after we took the snapshot so they are
        // not incorrectly GC'd — identical to the original serial logic.
        let added_since_snapshot = guard.split_off(snapshot_len);
        *guard = alive;
        guard.extend(added_since_snapshot);
        let total = guard.len();
        if pruned > 0 {
            tracing::info!(
                surface = "mcp",
                service = "peers",
                action = "peer.gc",
                pruned_count = pruned,
                active_count = total,
                "pruned stale MCP peer sessions after catalog notify",
            );
        } else {
            tracing::debug!(
                surface = "mcp",
                service = "peers",
                action = "peer.gc",
                active_count = total,
                "catalog notify complete — all peers alive",
            );
        }
    }
}
