//! `UpstreamPool` — manages connections to upstream MCP servers.
//!
//! Connects to configured upstreams via HTTP (`StreamableHttpClientTransport`)
//! or stdio (child process), discovers their tools, and caches schemas.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use rmcp::RoleClient;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use crate::config::UpstreamConfig;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::registry::RegisteredService;

use super::types::{UpstreamEntry, UpstreamRuntimeMetadata, UpstreamRuntimeOwner};

mod capability;
mod capability_call;
mod connect;
mod connect_stdio;
mod connection;
mod discover;
mod ensure;
mod entries;
mod health;
mod helpers;
mod lifecycle;
mod logging;
mod probe;
mod prompts_get;
mod prompts_list;
mod registration;
mod relay;
mod resources_list;
mod resources_read;
#[cfg(test)]
mod testsupport;
mod tools;
mod tools_call;
mod validate;

use helpers::DEFAULT_REQUEST_TIMEOUT;
pub(crate) use helpers::redact_resource_uri_for_logging;
pub use helpers::{UpstreamCachedSummary, in_process_upstream_name};
pub(crate) use tools::tool_has_mcp_app_ui_resource;
// Catalog size caps are used by pool child modules directly via `super::tools::*`.
// No external consumer references them through this path, so no `pub use` needed.

/// A cached subject-scoped connection entry.  Holds the live peer plus the
/// tool list that was discovered when the connection was opened.  Protected
/// by the `subject_connect_locks` single-flight gate so only one connect
/// runs per `(upstream, subject)` key at a time.
///
/// See `connection.rs:acquire_or_connect_subject` for the full cache logic
/// (P-C1 fix).
pub(super) struct SubjectScopedConnection {
    /// The full upstream connection (keeps the running service + server task alive).
    pub(super) _connection: UpstreamConnection,
    /// Cloned peer handle — pre-cloned so `acquire_or_connect_subject` can
    /// return it on the cache-hit fast path without re-cloning under write lock.
    pub(super) peer: rmcp::service::Peer<RoleClient>,
    /// Tool list discovered at connect time (avoids a round-trip on
    /// every owner-lookup call).
    pub(super) tools: Vec<rmcp::model::Tool>,
    /// Wall-clock instant when this entry was last used.
    pub(super) last_used: Instant,
}

/// Upstream connection pool — holds live connections and discovered tool catalogs.
#[derive(Clone)]
pub struct UpstreamPool {
    /// Discovered upstream state, keyed by upstream name.
    catalog: Arc<RwLock<HashMap<String, UpstreamEntry>>>,
    /// Live client connections, keyed by upstream name.
    /// Each is an `Arc<Peer<RoleClient>>` that can `call_tool` / `list_tools`.
    connections: Arc<RwLock<HashMap<String, UpstreamConnection>>>,
    /// Names of upstreams that have `proxy_resources=true`.
    resource_upstreams: Arc<RwLock<Vec<String>>>,
    /// Per-upstream OAuth managers, keyed by upstream name.
    /// `None` when the server was started without OAuth support.
    oauth_client_cache: Option<OauthClientCache>,
    /// Background reprobe task cancellation tokens, keyed by upstream name.
    probe_tasks: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Per-upstream lazy connection gates to prevent duplicate cold starts.
    lazy_connect_locks: Arc<RwLock<HashMap<String, Arc<Mutex<()>>>>>,
    /// Per-`(upstream, subject)` cached connections for the OAuth / subject-scoped
    /// proxy path.  Reused across calls for the same subject so we pay TLS +
    /// `initialize` + `tools/list` only once per idle-TTL window (P-C1 fix).
    ///
    /// Keyed by `(upstream_name, subject)`.
    subject_connections: Arc<RwLock<HashMap<(String, String), SubjectScopedConnection>>>,
    /// Per-`(upstream, subject)` single-flight locks so concurrent first-requests
    /// for the same key do not open duplicate OAuth connections (mirrors the
    /// `lazy_connect_locks` gate used by the normal pool path).
    subject_connect_locks: Arc<RwLock<HashMap<(String, String), Arc<Mutex<()>>>>>,
    /// Per-`(upstream, downstream-session)` cached **relay** connections.
    ///
    /// Distinct from `subject_connections` because the cached connection is
    /// served with a `RelayClientHandler` bound to one specific downstream
    /// agent peer (`UpstreamConnection<RelayClientHandler>`, a different type).
    /// The session component of the key is what guarantees a cached relay
    /// connection is never reused across agents — see `pool/relay.rs`.
    relay_connections: Arc<RwLock<HashMap<(String, u64), relay::RelayCachedConnection>>>,
    /// Single-flight locks for the relay-connection cache, mirroring
    /// `subject_connect_locks`.
    relay_connect_locks: Arc<RwLock<HashMap<(String, u64), Arc<Mutex<()>>>>>,
    /// Cancellation token for the background subject-connection sweep task.
    /// `None` until the first subject-scoped connect arms it; cancelled and
    /// cleared on `drain_for_swap` (P-H2). Mirrors the `probe_tasks` lifecycle.
    subject_sweep_task: Arc<RwLock<Option<CancellationToken>>>,
    /// Request/session identity stamped onto spawned stdio upstreams.
    runtime_origin: Option<String>,
    /// Structured owner metadata stamped onto spawned stdio upstreams.
    runtime_owner: Option<UpstreamRuntimeOwner>,
    /// Maximum time to wait for an upstream tool/resource/prompt response.
    request_timeout: Duration,
    /// Optional connector for in-process (built-in) service peers.
    /// When set, built-in lab services are reachable via the upstream pool.
    in_process_connector: Option<InProcessConnector>,
    /// Shared `reqwest::Client` used for ALL non-OAuth HTTP upstream connections.
    ///
    /// `reqwest::Client` is internally `Arc`-wrapped and holds a connection pool:
    /// sharing it means TLS sessions and keep-alive connections are reused across
    /// upstreams rather than rebuilt on every `connect_http_upstream` call (P-M10).
    pub(super) shared_http_client: Arc<reqwest::Client>,
}

/// A live connection to an upstream MCP server.
///
/// Generic over the client handler `H` (default `()`). Almost every connection
/// uses the unit handler `()` — which declines server→client requests — and is
/// stored in the pool maps as `UpstreamConnection<()>`. The relay path
/// (`pool/relay.rs`) constructs an `UpstreamConnection<RelayClientHandler>` for
/// a dedicated, ephemeral connection that forwards elicitation/sampling/roots to
/// the downstream agent. Only the `serve()` handler differs; every field below
/// (peer ops, process reaping, shutdown) is handler-agnostic.
pub(crate) struct UpstreamConnection<H = ()>
where
    H: rmcp::ClientHandler,
{
    /// The running client service handle — kept alive to maintain the connection.
    pub(crate) _client_service: rmcp::service::RunningService<RoleClient, H>,
    /// Background task holding an in-process server alive when applicable.
    pub(crate) _server_task: Option<tokio::task::JoinHandle<()>>,
    /// The peer handle for making requests.
    pub(crate) peer: rmcp::service::Peer<RoleClient>,
    /// Runtime metadata for process-backed upstreams.
    pub(crate) runtime: UpstreamRuntimeMetadata,
}

pub(crate) struct InProcessRegistration {
    pub(crate) connection: Option<UpstreamConnection>,
    pub(crate) tools: Vec<rmcp::model::Tool>,
    pub(crate) entry_name: Arc<str>,
    pub(crate) upstream_name: String,
}

pub(crate) type InProcessConnector = Arc<
    dyn Fn(RegisteredService) -> BoxFuture<'static, anyhow::Result<InProcessRegistration>>
        + Send
        + Sync,
>;

#[cfg(test)]
type TestUpstreamConnector = Arc<
    dyn Fn(
            UpstreamConfig,
        ) -> BoxFuture<
            'static,
            anyhow::Result<(Option<UpstreamConnection>, Vec<rmcp::model::Tool>)>,
        > + Send
        + Sync,
>;

impl UpstreamPool {
    /// Create a new empty pool.
    #[must_use]
    pub fn new() -> Self {
        // Build a shared reqwest::Client that lives for the pool's lifetime.
        // All non-OAuth HTTP connects reuse this client so TLS sessions and
        // keep-alive connections are pooled across upstreams (P-M10).
        let shared_http_client = Arc::new(
            reqwest::Client::builder()
                .timeout(DEFAULT_REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
        );
        Self {
            catalog: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            resource_upstreams: Arc::new(RwLock::new(Vec::new())),
            oauth_client_cache: None,
            probe_tasks: Arc::new(RwLock::new(HashMap::new())),
            lazy_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            subject_connections: Arc::new(RwLock::new(HashMap::new())),
            subject_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            relay_connections: Arc::new(RwLock::new(HashMap::new())),
            relay_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            subject_sweep_task: Arc::new(RwLock::new(None)),
            runtime_origin: None,
            runtime_owner: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            in_process_connector: None,
            shared_http_client,
        }
    }

    /// Attach a connector for in-process (built-in) service peers.
    ///
    /// When provided, built-in lab services are registered as in-process
    /// upstream peers rather than external HTTP/stdio connections.
    #[must_use]
    pub(crate) fn with_in_process_connector(mut self, connector: InProcessConnector) -> Self {
        self.in_process_connector = Some(connector);
        self
    }

    /// Attach the per-`(upstream, subject)` OAuth client cache so the pool can
    /// authenticate OAuth-protected upstreams.
    ///
    /// Must be called before `discover_all` for OAuth upstreams to connect successfully.
    #[must_use]
    pub fn with_oauth_client_cache(mut self, cache: OauthClientCache) -> Self {
        self.oauth_client_cache = Some(cache);
        self
    }

    #[must_use]
    pub fn with_runtime_origin(mut self, origin: Option<String>) -> Self {
        self.runtime_origin = origin;
        self
    }

    #[must_use]
    pub fn with_runtime_owner(mut self, owner: Option<UpstreamRuntimeOwner>) -> Self {
        self.runtime_owner = owner;
        self
    }

    pub(crate) fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    #[cfg(test)]
    pub(crate) fn request_timeout(&self) -> Duration {
        self.request_timeout
    }
}

impl Default for UpstreamPool {
    fn default() -> Self {
        Self::new()
    }
}
