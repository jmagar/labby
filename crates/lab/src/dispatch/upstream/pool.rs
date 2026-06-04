//! `UpstreamPool` — manages connections to upstream MCP servers.
//!
//! Connects to configured upstreams via HTTP (`StreamableHttpClientTransport`)
//! or stdio (child process), discovers their tools, and caches schemas.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
// Re-export the catalog size caps so tests and gateway code can reference them
// without reaching into a submodule path.
#[allow(unused_imports)]
pub use tools::{MAX_UPSTREAM_PROMPTS, MAX_UPSTREAM_RESOURCES, MAX_UPSTREAM_TOOLS};

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
    /// Request/session identity stamped onto spawned stdio upstreams.
    runtime_origin: Option<String>,
    /// Structured owner metadata stamped onto spawned stdio upstreams.
    runtime_owner: Option<UpstreamRuntimeOwner>,
    /// Maximum time to wait for an upstream tool/resource/prompt response.
    request_timeout: Duration,
    /// Optional connector for in-process (built-in) service peers.
    /// When set, built-in lab services are reachable via the upstream pool.
    in_process_connector: Option<InProcessConnector>,
}

/// A live connection to an upstream MCP server.
pub(crate) struct UpstreamConnection {
    /// The running client service handle — kept alive to maintain the connection.
    pub(crate) _client_service: rmcp::service::RunningService<RoleClient, ()>,
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
        Self {
            catalog: Arc::new(RwLock::new(HashMap::new())),
            connections: Arc::new(RwLock::new(HashMap::new())),
            resource_upstreams: Arc::new(RwLock::new(Vec::new())),
            oauth_client_cache: None,
            probe_tasks: Arc::new(RwLock::new(HashMap::new())),
            lazy_connect_locks: Arc::new(RwLock::new(HashMap::new())),
            runtime_origin: None,
            runtime_owner: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            in_process_connector: None,
        }
    }

    /// Attach a connector for in-process (built-in) service peers.
    ///
    /// When provided, built-in lab services are registered as in-process
    /// upstream peers rather than external HTTP/stdio connections.
    #[must_use]
    pub fn with_in_process_connector(mut self, connector: InProcessConnector) -> Self {
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

    #[cfg(test)]
    pub(super) fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl Default for UpstreamPool {
    fn default() -> Self {
        Self::new()
    }
}
