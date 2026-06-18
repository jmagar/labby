//! Relaying `ClientHandler` for upstream server→client requests.
//!
//! The pool's normal upstream connections are served with the unit handler
//! (`().serve(...)`), which advertises no client capabilities and declines any
//! `elicitation/create`, `sampling/createMessage`, or `roots/list` request a
//! server sends back. That severs the server→client half of MCP: an upstream
//! that needs interactive confirmation (elicitation), an LLM completion
//! (sampling), or the caller's roots cannot reach the agent driving the
//! gateway.
//!
//! [`RelayClientHandler`] is the bridge. Each instance closes over the single
//! downstream `Peer<RoleServer>` — the agent connection whose in-flight
//! `call_tool` opened this upstream connection — and forwards server→client
//! requests straight down to it. The relay therefore only makes sense on a
//! **dedicated, non-multiplexed** upstream connection: one connection per
//! in-flight downstream call, so an upstream elicitation maps unambiguously to
//! the one agent that should answer it. A pooled connection shared by many
//! agents has no single "current" downstream peer to forward to — which is
//! exactly why the existing pool path uses `()` and this is opt-in.
//!
//! ## Capability mirroring
//!
//! `get_info()` advertises to the upstream only the server→client capabilities
//! the downstream agent itself declared (elicitation / sampling / roots). If
//! the agent cannot elicit, the gateway does not claim it can, so a well-behaved
//! upstream will not attempt it. This keeps the proxied capability set honest
//! end to end instead of advertising support the gateway cannot actually honor.
//!
//! ## Live entry point
//!
//! [`UpstreamPool::call_tool_relayed`] opens a dedicated connection via the
//! generic `connect_upstream_with_handler` seam (so HTTP, WebSocket, stdio, and
//! OAuth all reuse the existing transport + process-reaping machinery), invokes
//! one tool with the relay handler installed, and shuts the connection down. The
//! MCP raw-proxy path calls it (behind an opt-in env gate) when the downstream
//! agent advertises elicitation. Cost: one fresh connect per call — the gate
//! keeps that off the default hot path.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use rmcp::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientInfo, CreateElicitationRequestParams,
    CreateElicitationResult, CreateMessageRequestParams, CreateMessageResult, ListRootsResult,
};
use rmcp::service::{Peer, RequestContext};
use rmcp::{ClientHandler, RoleClient, RoleServer};
use tokio::sync::Mutex;

use crate::config::UpstreamConfig;

use super::super::types::UpstreamCapability;
use super::connect::connect_upstream_with_handler;
use super::helpers::{SUBJECT_CONN_IDLE_TTL, SUBJECT_CONN_MAX_ENTRIES};
use super::{UpstreamConnection, UpstreamPool};

/// A client handler that relays an upstream server's server→client requests
/// (elicitation, sampling, roots) down to the gateway's downstream agent peer.
///
/// Construct one per dedicated upstream connection with [`RelayClientHandler::new`].
#[derive(Clone)]
pub(crate) struct RelayClientHandler {
    /// The downstream agent connection to forward requests to.
    downstream: Peer<RoleServer>,
    /// Name of the upstream this handler is attached to (for logging only).
    upstream_name: Arc<str>,
}

impl RelayClientHandler {
    pub(crate) fn new(downstream: Peer<RoleServer>, upstream_name: Arc<str>) -> Self {
        Self {
            downstream,
            upstream_name,
        }
    }
}

/// Map a downstream `ServiceError` into the `McpError` returned to the upstream.
///
/// The upstream sees a generic `internal_error`; the underlying cause is logged
/// at the gateway rather than leaked verbatim across the proxy boundary.
fn relay_error(upstream: &str, capability: &str, error: &rmcp::service::ServiceError) -> McpError {
    tracing::warn!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.relay",
        upstream = %upstream,
        capability,
        kind = "upstream_relay_error",
        error = %error,
        "relaying upstream server->client request to downstream agent failed",
    );
    McpError::internal_error(
        format!("relay of {capability} to downstream agent failed"),
        None,
    )
}

impl ClientHandler for RelayClientHandler {
    /// Advertise to the upstream exactly the server→client capabilities the
    /// downstream agent declared. Anything the agent cannot do, the gateway
    /// does not claim on its behalf.
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        if let Some(downstream_info) = self.downstream.peer_info() {
            info.capabilities.elicitation = downstream_info.capabilities.elicitation.clone();
            info.capabilities.sampling = downstream_info.capabilities.sampling.clone();
            info.capabilities.roots = downstream_info.capabilities.roots.clone();
        }
        info
    }

    /// Relay an upstream elicitation request to the downstream agent.
    async fn create_elicitation(
        &self,
        params: CreateElicitationRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, McpError> {
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay",
            upstream = %self.upstream_name,
            capability = "elicitation",
            "relaying upstream elicitation to downstream agent",
        );
        self.downstream
            .create_elicitation(params)
            .await
            .map_err(|e| relay_error(&self.upstream_name, "elicitation", &e))
    }

    /// Relay an upstream sampling request to the downstream agent.
    async fn create_message(
        &self,
        params: CreateMessageRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateMessageResult, McpError> {
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay",
            upstream = %self.upstream_name,
            capability = "sampling",
            "relaying upstream sampling request to downstream agent",
        );
        self.downstream
            .create_message(params)
            .await
            .map_err(|e| relay_error(&self.upstream_name, "sampling", &e))
    }

    /// Relay an upstream roots request to the downstream agent.
    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, McpError> {
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay",
            upstream = %self.upstream_name,
            capability = "roots",
            "relaying upstream roots request to downstream agent",
        );
        self.downstream
            .list_roots()
            .await
            .map_err(|e| relay_error(&self.upstream_name, "roots", &e))
    }
}

/// A cached relay connection, keyed in the pool by `(upstream, session_id)`.
///
/// The `RelayClientHandler` inside `_connection` is bound to **one** downstream
/// agent peer (the session identified by the key). Because the key includes the
/// downstream session id, a cached entry is only ever reused by the same agent
/// — never shared across sessions, which is what keeps relayed elicitation from
/// being misrouted.
pub(super) struct RelayCachedConnection {
    /// Keeps the relay-served running service (and any stdio child) alive.
    _connection: UpstreamConnection<RelayClientHandler>,
    /// Pre-cloned upstream peer for the cache-hit fast path.
    peer: Peer<RoleClient>,
    /// Wall-clock instant when this entry was last used.
    last_used: Instant,
}

/// Evict least-recently-used relay connections until the map holds at most
/// `max_entries`, sparing the about-to-be-inserted `protect` key. Mirrors
/// `connection::evict_lru_over_cap` for the relay-typed cache.
fn evict_relay_lru_over_cap(
    cache: &mut HashMap<(String, u64), RelayCachedConnection>,
    max_entries: usize,
    protect: &(String, u64),
) -> Vec<(String, UpstreamConnection<RelayClientHandler>)> {
    let mut evicted = Vec::new();
    while cache.len() > max_entries {
        let lru_key = cache
            .iter()
            .filter(|(k, _)| *k != protect)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(k, _)| k.clone());
        match lru_key {
            Some(key) => {
                if let Some(entry) = cache.remove(&key) {
                    evicted.push((key.0, entry._connection));
                }
            }
            None => break,
        }
    }
    evicted
}

impl UpstreamPool {
    /// Call a single tool on an upstream over a **relay-handled** connection
    /// that is cached per `(upstream, downstream-session)`.
    ///
    /// Unlike [`UpstreamPool::call_tool`] (a pooled, multiplexed `()`
    /// connection), the connection here is served with a [`RelayClientHandler`]
    /// bound to `downstream`, so any server→client request the upstream raises
    /// mid-call (elicitation/sampling/roots) is forwarded to that one agent.
    ///
    /// `session_id` must uniquely identify the downstream agent connection (the
    /// gateway mints one per `LabMcpServer` session). It is the second half of
    /// the cache key, which is what guarantees a cached relay connection is
    /// never reused by a *different* agent — the property that makes the
    /// upstream→agent mapping unambiguous even though the connection is reused
    /// across calls within the session.
    ///
    /// Reuses the generic `connect_upstream_with_handler` seam, so every
    /// transport (HTTP, WebSocket, stdio, OAuth-HTTP) and the stdio
    /// process-reaping guard work unchanged. `subject` is forwarded for
    /// OAuth-scoped upstreams (`None` for the common non-OAuth case).
    ///
    /// Returns `None` only when no connection could be established — mirroring
    /// `call_tool`'s "not connected" signal.
    pub async fn call_tool_relayed(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        params: CallToolRequestParams,
        downstream: Peer<RoleServer>,
        session_id: u64,
    ) -> Option<Result<CallToolResult, String>> {
        let peer = self
            .acquire_or_connect_relay(config, subject, downstream, session_id)
            .await?;

        let timeout = self.request_timeout;
        let started = Instant::now();
        let outcome = match tokio::time::timeout(timeout, peer.call_tool(params)).await {
            Ok(Ok(result)) => Some(Ok(result)),
            Ok(Err(error)) => {
                // A failed call may mean the cached connection went bad; drop it
                // so the next call reconnects rather than reusing a dead peer.
                self.evict_relay_connection(&config.name, session_id).await;
                Some(Err(format!("relayed upstream call failed: {error}")))
            }
            Err(_) => {
                self.evict_relay_connection(&config.name, session_id).await;
                Some(Err(format!(
                    "relayed upstream call timed out after {}ms",
                    timeout.as_millis()
                )))
            }
        };

        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.relay.call",
            upstream = %config.name,
            session_id,
            subject_scoped = subject.is_some(),
            elapsed_ms = started.elapsed().as_millis(),
            "relayed upstream tool call complete",
        );
        outcome
    }

    /// Return a cached relay peer for `(upstream, session_id)`, or open and
    /// cache a new relay connection. Mirrors `acquire_or_connect_subject`:
    /// write-locked fast path with inline TTL + dead-transport eviction, then a
    /// per-key single-flight slow path.
    async fn acquire_or_connect_relay(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        downstream: Peer<RoleServer>,
        session_id: u64,
    ) -> Option<Peer<RoleClient>> {
        let key = (config.name.clone(), session_id);

        // Fast path: fresh, live cached entry.
        {
            let mut cache = self.relay_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL
                    && !entry.peer.is_transport_closed()
                {
                    entry.last_used = Instant::now();
                    return Some(entry.peer.clone());
                }
                cache.remove(&key);
            }
        }

        self.ensure_subject_sweep_task().await;

        // Slow path: per-key single-flight so concurrent first calls do not open
        // duplicate connections.
        let connect_lock: Arc<Mutex<()>> = {
            let mut locks = self.relay_connect_locks.write().await;
            locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = connect_lock.lock().await;

        // Re-check after acquiring the lock.
        {
            let mut cache = self.relay_connections.write().await;
            if let Some(entry) = cache.get_mut(&key) {
                if entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL
                    && !entry.peer.is_transport_closed()
                {
                    entry.last_used = Instant::now();
                    return Some(entry.peer.clone());
                }
                cache.remove(&key);
            }
        }

        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let handler = RelayClientHandler::new(downstream, Arc::clone(&upstream_name));
        let (conn, _tools) = match connect_upstream_with_handler(
            config,
            subject,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            self.runtime_owner.as_ref(),
            Some(&self.shared_http_client),
            handler,
        )
        .await
        {
            Ok(pair) => pair,
            Err(error) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("relayed upstream connect failed: {error}"),
                )
                .await;
                return None;
            }
        };

        let peer = conn.peer.clone();
        // Enforce the LRU cap BEFORE inserting so a burst of unique sessions
        // cannot push the live-peer count past the bound; shut evicted peers
        // down off-lock.
        let evicted = {
            let mut cache = self.relay_connections.write().await;
            let evicted = evict_relay_lru_over_cap(&mut cache, SUBJECT_CONN_MAX_ENTRIES - 1, &key);
            cache.insert(
                key.clone(),
                RelayCachedConnection {
                    _connection: conn,
                    peer: peer.clone(),
                    last_used: Instant::now(),
                },
            );
            evicted
        };
        for (name, evicted_conn) in evicted {
            evicted_conn.shutdown(&name, "relay.cache.lru_evict").await;
        }

        Some(peer)
    }

    /// Evict and shut down the cached relay connection for one
    /// `(upstream, session)` pair (called on a failed/timed-out call).
    pub(super) async fn evict_relay_connection(&self, upstream_name: &str, session_id: u64) {
        let key = (upstream_name.to_string(), session_id);
        let removed = self.relay_connections.write().await.remove(&key);
        if let Some(entry) = removed {
            entry
                ._connection
                .shutdown(upstream_name, "relay.cache.evict")
                .await;
        }
    }

    /// Evict every cached relay connection for one upstream.
    pub(super) async fn evict_relay_connections_for(&self, upstream_name: &str) {
        let drained: Vec<_> = {
            let mut cache = self.relay_connections.write().await;
            let keys = cache
                .keys()
                .filter(|(name, _)| name == upstream_name)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key, entry)))
                .collect()
        };
        for ((name, _session), entry) in drained {
            entry
                ._connection
                .shutdown(&name, "relay.cache.upstream_reconcile")
                .await;
        }
    }

    /// Evict all cached relay connections (called during pool drain).
    pub(super) async fn evict_all_relay_connections(&self) {
        let drained: Vec<_> = self.relay_connections.write().await.drain().collect();
        for ((name, _session), entry) in drained {
            entry._connection.shutdown(&name, "relay.cache.drain").await;
        }
    }

    /// Sweep the relay-connection cache: evict entries past the idle TTL or
    /// whose upstream transport has closed, shutting their peers down off-lock.
    /// Also prunes orphan single-flight locks. Returns
    /// `(connections_evicted, locks_pruned)`.
    pub(super) async fn sweep_relay_connections(&self) -> (usize, usize) {
        let expired = {
            let mut cache = self.relay_connections.write().await;
            let stale_keys: Vec<(String, u64)> = cache
                .iter()
                .filter(|(_, entry)| {
                    entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL
                        || entry.peer.is_transport_closed()
                })
                .map(|(key, _)| key.clone())
                .collect();
            stale_keys
                .into_iter()
                .filter_map(|key| cache.remove(&key).map(|entry| (key.0, entry._connection)))
                .collect::<Vec<_>>()
        };
        let connections_evicted = expired.len();
        for (name, conn) in expired {
            conn.shutdown(&name, "relay.cache.sweep").await;
        }

        let locks_pruned = {
            let cache = self.relay_connections.read().await;
            let mut locks = self.relay_connect_locks.write().await;
            let before = locks.len();
            locks.retain(|key, lock| cache.contains_key(key) || Arc::strong_count(lock) > 1);
            before - locks.len()
        };

        (connections_evicted, locks_pruned)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, Content,
        CreateElicitationRequestParams, CreateElicitationResult, ElicitationAction,
        ElicitationSchema, ErrorData, PaginatedRequestParams, PrimitiveSchema, ServerCapabilities,
        ServerInfo,
    };
    use rmcp::service::{RequestContext, RunningService};
    use rmcp::{ClientHandler, RoleClient, RoleServer, ServerHandler, ServiceExt};

    use std::time::Instant;

    use crate::dispatch::upstream::types::UpstreamRuntimeMetadata;

    use super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
    use super::*;

    /// A mock agent (downstream client) that answers any elicitation by
    /// accepting with `{"confirm": true}`. Advertises elicitation support.
    #[derive(Clone)]
    struct AnsweringAgent;

    impl ClientHandler for AnsweringAgent {
        fn get_info(&self) -> ClientInfo {
            let mut info = ClientInfo::default();
            info.capabilities = ClientCapabilities::builder().enable_elicitation().build();
            info
        }

        async fn create_elicitation(
            &self,
            _params: CreateElicitationRequestParams,
            _context: RequestContext<RoleClient>,
        ) -> Result<CreateElicitationResult, McpError> {
            let mut content = serde_json::Map::new();
            content.insert("confirm".to_string(), serde_json::Value::Bool(true));
            Ok(CreateElicitationResult::new(ElicitationAction::Accept)
                .with_content(serde_json::Value::Object(content)))
        }
    }

    /// A trivial downstream-facing server: just enough to hand back a
    /// `Peer<RoleServer>` once the agent connects.
    #[derive(Clone)]
    struct TrivialServer;

    impl ServerHandler for TrivialServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::default()
        }
    }

    /// A mock upstream server whose `call_tool` issues a server→client
    /// elicitation mid-call and reports whether it was accepted.
    #[derive(Clone)]
    struct ElicitingUpstream;

    impl ServerHandler for ElicitingUpstream {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, ErrorData> {
            let schema = ElicitationSchema::builder()
                .required_property(
                    "confirm",
                    PrimitiveSchema::Boolean(rmcp::model::BooleanSchema::default()),
                )
                .build()
                .expect("schema builds");
            let params = CreateElicitationRequestParams::FormElicitationParams {
                meta: None,
                message: "confirm the action?".to_string(),
                requested_schema: schema,
            };
            let result = context
                .peer
                .create_elicitation(params)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            let confirmed = matches!(result.action, ElicitationAction::Accept);
            Ok(CallToolResult::success(vec![Content::text(format!(
                "confirmed={confirmed}"
            ))]))
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
            Ok(rmcp::model::ListToolsResult::with_all_items(vec![
                rmcp::model::Tool::new(
                    "echo".to_string(),
                    "echoes confirmation".to_string(),
                    Arc::new(serde_json::Map::new()),
                ),
            ]))
        }
    }

    /// End-to-end proof: an upstream elicitation, raised during a tool call, is
    /// relayed through the gateway's [`RelayClientHandler`] to the downstream
    /// agent, answered, and the answer flows back to the upstream — all over a
    /// dedicated connection.
    #[tokio::test]
    async fn upstream_elicitation_is_relayed_to_downstream_agent() {
        // 1. Wire the gateway's downstream side to a mock agent that answers
        //    elicitation. The gateway-server peer is what the relay forwards to.
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _agent_task = tokio::spawn(async move {
            let running = AnsweringAgent
                .serve(agent_transport)
                .await
                .expect("agent connects");
            running.waiting().await.expect("agent runs");
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("gateway server side connects");
        let downstream = gw_server.peer().clone();

        // 2. Wire the gateway's upstream side to a mock upstream that elicits.
        //    The dedicated connection is served with the relay handler.
        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _upstream_task = tokio::spawn(async move {
            let running = ElicitingUpstream
                .serve(upstream_transport)
                .await
                .expect("upstream connects");
            running.waiting().await.expect("upstream runs");
        });
        let gw_client = RelayClientHandler::new(downstream, Arc::from("test-upstream"))
            .serve(gw_client_transport)
            .await
            .expect("relayed upstream connection establishes");
        let upstream_peer = gw_client.peer().clone();

        // 3. Drive a tool call on the upstream. Its handler elicits → relay →
        //    agent → accept → back to the upstream, which reports the outcome.
        let result = upstream_peer
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .expect("tool call succeeds with relayed elicitation");

        let text = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .expect("tool result has text content");
        assert_eq!(
            text, "confirmed=true",
            "the upstream should observe the downstream agent's acceptance"
        );
    }

    /// Without the relay, the unit handler declines elicitation, so the same
    /// upstream tool call reports `confirmed=false`. This pins the behavioral
    /// difference the relay introduces.
    #[tokio::test]
    async fn unit_handler_declines_upstream_elicitation() {
        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _upstream_task = tokio::spawn(async move {
            let running = ElicitingUpstream
                .serve(upstream_transport)
                .await
                .expect("upstream connects");
            running.waiting().await.expect("upstream runs");
        });
        let gw_client: RunningService<RoleClient, ()> = ()
            .serve(gw_client_transport)
            .await
            .expect("plain upstream connection establishes");
        let upstream_peer = gw_client.peer().clone();

        let result = upstream_peer
            .call_tool(CallToolRequestParams::new("echo"))
            .await
            .expect("tool call still completes");

        let text = result
            .content
            .iter()
            .find_map(|c| c.as_text().map(|t| t.text.clone()))
            .expect("tool result has text content");
        assert_eq!(
            text, "confirmed=false",
            "the unit handler declines elicitation, so nothing is confirmed"
        );
    }

    /// `call_tool_relayed` returns `None` (the "not connected" signal, mirroring
    /// `call_tool`) when the dedicated connect fails — here because the config
    /// names neither a URL nor a command. Proves the orchestration's
    /// connect-failure path without needing a live transport.
    #[tokio::test]
    async fn call_tool_relayed_returns_none_when_connect_fails() {
        // A downstream agent peer is required by the signature; the connect
        // fails before it is ever used.
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let _agent_task = tokio::spawn(async move {
            let running = ().serve(agent_transport).await.expect("agent connects");
            running.waiting().await.expect("agent runs");
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("gateway server side connects");
        let downstream = gw_server.peer().clone();

        let pool = UpstreamPool::new();
        // Neither `url` nor `command` set → connect_upstream_with_handler errors.
        let config = super::super::testsupport::test_upstream_config();

        let result = pool
            .call_tool_relayed(
                &config,
                None,
                CallToolRequestParams::new("anything"),
                downstream,
                1,
            )
            .await;

        assert!(
            result.is_none(),
            "a failed dedicated connect should surface as None"
        );
    }

    /// Build a live `RelayCachedConnection` over in-memory duplex transports for
    /// the cache-ops tests. Returns the entry plus the downstream-server running
    /// service, which the caller must keep alive (dropping it closes the agent
    /// peer the relay handler is bound to).
    async fn live_relay_cached_connection(
        last_used: Instant,
    ) -> (
        RelayCachedConnection,
        RunningService<RoleServer, TrivialServer>,
    ) {
        let (gw_server_transport, agent_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ().serve(agent_transport).await {
                running.waiting().await.ok();
            }
        });
        let gw_server = TrivialServer
            .serve(gw_server_transport)
            .await
            .expect("downstream server connects");
        let downstream = gw_server.peer().clone();

        let (upstream_transport, gw_client_transport) =
            tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        tokio::spawn(async move {
            if let Ok(running) = ElicitingUpstream.serve(upstream_transport).await {
                running.waiting().await.ok();
            }
        });
        let service = RelayClientHandler::new(downstream, Arc::from("up"))
            .serve(gw_client_transport)
            .await
            .expect("relay client connects");
        let peer = service.peer().clone();
        let conn = UpstreamConnection {
            _client_service: service,
            _server_task: None,
            peer: peer.clone(),
            runtime: UpstreamRuntimeMetadata::default(),
        };
        (
            RelayCachedConnection {
                _connection: conn,
                peer,
                last_used,
            },
            gw_server,
        )
    }

    /// `evict_all_relay_connections` empties the cache (and shuts the cached
    /// connections down) — the drain path.
    #[tokio::test]
    async fn relay_cache_evict_all_clears_entries() {
        let pool = UpstreamPool::new();
        let (entry, _keepalive) = live_relay_cached_connection(Instant::now()).await;
        pool.relay_connections
            .write()
            .await
            .insert(("up".to_string(), 7), entry);
        assert_eq!(pool.relay_connections.read().await.len(), 1);

        pool.evict_all_relay_connections().await;
        assert!(pool.relay_connections.read().await.is_empty());
    }

    /// `evict_relay_connection` removes only the targeted `(upstream, session)`
    /// entry, leaving a different session's entry intact.
    #[tokio::test]
    async fn relay_cache_evict_one_is_scoped_to_session() {
        let pool = UpstreamPool::new();
        let (a, _ka) = live_relay_cached_connection(Instant::now()).await;
        let (b, _kb) = live_relay_cached_connection(Instant::now()).await;
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("up".to_string(), 1), a);
            cache.insert(("up".to_string(), 2), b);
        }

        pool.evict_relay_connection("up", 1).await;

        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining, vec![("up".to_string(), 2)]);
    }

    /// `sweep_relay_connections` evicts entries past the idle TTL while keeping
    /// fresh ones.
    #[tokio::test]
    async fn relay_cache_sweep_evicts_idle_entries() {
        use std::time::{Duration, Instant};

        let pool = UpstreamPool::new();
        let stale_used = Instant::now()
            .checked_sub(SUBJECT_CONN_IDLE_TTL + Duration::from_secs(60))
            .expect("instant in range");
        let (stale, _ks) = live_relay_cached_connection(stale_used).await;
        let (fresh, _kf) = live_relay_cached_connection(Instant::now()).await;
        {
            let mut cache = pool.relay_connections.write().await;
            cache.insert(("up".to_string(), 1), stale);
            cache.insert(("up".to_string(), 2), fresh);
        }

        let (evicted, _pruned) = pool.sweep_relay_connections().await;
        assert_eq!(evicted, 1, "only the idle-TTL-expired entry should evict");

        let remaining: Vec<_> = pool
            .relay_connections
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining, vec![("up".to_string(), 2)]);
    }
}
