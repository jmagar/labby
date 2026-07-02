//! Shared application state for axum handlers.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "acp")]
use crate::acp::registry::AcpSessionRegistry;
use crate::catalog::{Catalog, build_catalog};
use crate::config::{LabConfig, NodeRole};
use crate::dispatch::clients::ServiceClients;
#[cfg(feature = "nodes")]
use crate::node::enrollment::store::EnrollmentStore;
#[cfg(feature = "nodes")]
use crate::node::store::NodeStore;
use crate::registry::{ToolRegistry, build_default_registry};

const DEFAULT_PROTECTED_MCP_CONNECT_TIMEOUT_SECS: u64 = 10;
const PROTECTED_MCP_CONNECT_TIMEOUT_ENV: &str = "LAB_PROTECTED_MCP_CONNECT_TIMEOUT_SECS";

/// Application state passed to every axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    /// Pre-built service+action catalog for discovery endpoints.
    pub catalog: Arc<Catalog>,
    /// Tool registry with dispatch functions for each service.
    ///
    /// Used by `build_router_with_bearer` to enforce runtime service filtering:
    /// only services present in the registry get their HTTP routes mounted,
    /// even when their compile-time feature flag is enabled.
    pub registry: Arc<ToolRegistry>,
    /// Pre-built service clients for connection pool reuse.
    pub clients: Arc<ServiceClients>,
    /// Shared HTTP client for protected MCP reverse proxy requests.
    pub protected_mcp_http_client: reqwest::Client,
    /// Router containing protected route scoped MCP services, mounted by
    /// host/path after protected route auth.
    pub protected_mcp_router: Option<Arc<axum::Router>>,
    /// Runtime-enabled service names derived from the registry.
    ///
    /// The HTTP router checks this set to decide which per-service route groups
    /// to mount.  When `--services` filtering is applied, only the listed names
    /// appear here, so filtered-out services have no reachable POST endpoint.
    #[allow(dead_code)]
    pub enabled_services: Arc<HashSet<String>>,
    /// Resolved auth configuration, if present.
    ///
    /// Stored in `AppState` so that handlers (e.g. protected resource metadata,
    /// WWW-Authenticate headers) can read from resolved config rather than
    /// re-reading env vars at request time.
    pub auth_config: Option<Arc<labby_auth::config::AuthConfig>>,
    /// Resolved lab configuration loaded at server startup.
    pub config: Arc<LabConfig>,
    /// OAuth-mode auth server state, mounted only when LAB_AUTH_MODE=oauth.
    pub oauth_state: Option<Arc<labby_auth::state::AuthState>>,
    /// Cached actor-key deriver used at authenticated bind boundaries.
    pub actor_key_deriver: Option<Arc<crate::observability::activity::ActorKeyDeriver>>,
    /// Shared gateway manager for runtime upstream pool access and config mutation.
    ///
    /// `None` when gateway management is not wired for this process.
    #[cfg(feature = "gateway")]
    pub gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
    /// Shared fleet state store for node runtime ingestion.
    #[cfg(feature = "nodes")]
    pub node_store: Option<Arc<NodeStore>>,
    /// Shared durable enrollment store for fleet websocket admission control.
    #[cfg(feature = "nodes")]
    pub enrollment_store: Option<Arc<EnrollmentStore>>,
    /// Shared local-master log runtime used by API SSE and adapter-local lookups.
    pub logs_system: Option<Arc<crate::dispatch::logs::types::LogSystem>>,
    /// Shared ACP session registry for browser chat/session routes.
    #[cfg(feature = "acp")]
    pub acp_registry: Arc<AcpSessionRegistry>,
    /// Resolved node role for the current process.
    pub node_role: Option<NodeRole>,
    /// Optional directory containing exported Labby web assets.
    pub web_assets_dir: Option<Arc<PathBuf>>,
    /// Whether to serve Labby assets embedded into the lab binary.
    pub embedded_web_assets: bool,
    /// Instant at which the server became ready (used by `/health` uptime_s).
    pub server_start: std::time::Instant,
    /// Canonical absolute path of the configured workspace root, or
    /// `None` when `workspace.root` is invalid at startup.
    /// Backs the `dispatch/fs/` service (workspace filesystem browser).
    #[allow(dead_code)] // Used by fs HTTP routes when that surface is mounted.
    pub workspace_root: Option<Arc<PathBuf>>,
    /// When true, `/v1/*` skips auth middleware for hosted UI requests.
    pub web_ui_auth_disabled: bool,
    /// Static bearer token (LAB_MCP_HTTP_TOKEN), if configured.
    ///
    /// Stored on AppState so handlers outside the auth middleware
    /// (e.g. `/auth/session`) can validate the same token. The middleware
    /// remains the canonical enforcement point for `/v1/*`.
    pub bearer_token: Option<Arc<str>>,
    /// HTTP bind host resolved by `labby serve`.
    pub http_bind_host: Option<Arc<String>>,
    /// Shared SQLite-backed MCP registry store for `/v0.1` read endpoints.
    ///
    /// `None` when the `marketplace` feature is disabled or the store failed to open.
    #[cfg(feature = "marketplace")]
    pub registry_store: Option<Arc<crate::dispatch::marketplace::store::RegistryStore>>,
}

impl AppState {
    /// Build state from the default (all enabled features) registry.
    #[must_use]
    pub fn new() -> Self {
        let registry = build_default_registry();
        Self::from_registry(registry)
    }

    /// Build state from a pre-filtered or pre-built registry.
    ///
    /// Use this when the caller has already applied service filtering (e.g.
    /// `--services` on `labby serve`) so that the HTTP surface
    /// respects the same service set as the stdio surface.
    ///
    /// `enabled_services` is derived from the registry entries so the router
    /// can skip mounting handlers for services that were filtered out.
    ///
    /// # ACP registry invariant
    ///
    /// `acp_registry` is initialized to a fresh, uninstalled registry.
    /// When ACP dispatch actions are in scope, call `.with_acp_registry(arc)` with
    /// the same `Arc` passed to `dispatch::acp::install_registry()` in `cli/serve.rs`.
    /// Skipping this step causes HTTP handlers and MCP dispatch to use different
    /// registry instances, silently losing session visibility.
    #[must_use]
    pub fn from_registry(registry: ToolRegistry) -> Self {
        let enabled_services: HashSet<String> = registry
            .services()
            .iter()
            .map(|e| e.name.to_string())
            .collect();
        let catalog = Arc::new(build_catalog(&registry));
        let clients = Arc::new(ServiceClients::from_env());
        let protected_mcp_http_client = build_protected_mcp_http_client();
        Self {
            catalog,
            registry: Arc::new(registry),
            clients,
            protected_mcp_http_client,
            protected_mcp_router: None,
            enabled_services: Arc::new(enabled_services),
            auth_config: None,
            config: Arc::new(LabConfig::default()),
            oauth_state: None,
            actor_key_deriver: None,
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            #[cfg(feature = "nodes")]
            node_store: None,
            #[cfg(feature = "nodes")]
            enrollment_store: None,
            logs_system: None,
            #[cfg(feature = "acp")]
            acp_registry: Arc::new(AcpSessionRegistry::new()),
            node_role: None,
            web_assets_dir: None,
            embedded_web_assets: false,
            workspace_root: None,
            web_ui_auth_disabled: false,
            bearer_token: None,
            http_bind_host: None,
            server_start: std::time::Instant::now(),
            #[cfg(feature = "marketplace")]
            registry_store: None,
        }
    }

    /// Attach the resolved auth configuration.
    #[must_use]
    pub fn with_auth_config(mut self, config: labby_auth::config::AuthConfig) -> Self {
        self.auth_config = Some(Arc::new(config));
        self
    }

    #[must_use]
    pub fn with_config(mut self, config: LabConfig) -> Self {
        self.config = Arc::new(config);
        self
    }

    #[must_use]
    pub fn with_protected_mcp_router(mut self, router: axum::Router) -> Self {
        self.protected_mcp_router = Some(Arc::new(router));
        self
    }

    #[must_use]
    pub fn with_oauth_state(mut self, auth_state: labby_auth::state::AuthState) -> Self {
        self.oauth_state = Some(Arc::new(auth_state));
        self
    }

    #[must_use]
    pub fn with_actor_key_deriver(
        mut self,
        deriver: crate::observability::activity::ActorKeyDeriver,
    ) -> Self {
        self.actor_key_deriver = Some(Arc::new(deriver));
        self
    }

    /// Attach the shared gateway manager.
    #[cfg(feature = "gateway")]
    #[must_use]
    #[allow(dead_code)] // Called by `labby serve` when gateway runtime is wired.
    pub fn with_gateway_manager(
        mut self,
        manager: Arc<crate::dispatch::gateway::manager::GatewayManager>,
    ) -> Self {
        self.gateway_manager = Some(manager);
        self
    }

    #[cfg(feature = "nodes")]
    #[must_use]
    pub fn with_node_store(mut self, store: Arc<NodeStore>) -> Self {
        self.node_store = Some(store);
        self
    }

    #[cfg(feature = "nodes")]
    #[must_use]
    pub fn with_enrollment_store(mut self, store: Arc<EnrollmentStore>) -> Self {
        self.enrollment_store = Some(store);
        self
    }

    #[must_use]
    pub fn with_log_system(mut self, system: Arc<crate::dispatch::logs::types::LogSystem>) -> Self {
        self.logs_system = Some(system);
        self
    }

    #[must_use]
    pub fn with_node_role(mut self, role: NodeRole) -> Self {
        self.node_role = Some(role);
        self
    }

    /// Attach an exported Labby assets directory for static web serving.
    #[must_use]
    pub fn with_web_assets_dir(mut self, dir: PathBuf) -> Self {
        self.web_assets_dir = Some(Arc::new(dir));
        self.embedded_web_assets = false;
        self
    }

    /// Enable Labby assets embedded into the lab binary.
    #[must_use]
    pub fn with_embedded_web_assets(mut self) -> Self {
        self.embedded_web_assets = true;
        self
    }

    #[must_use]
    pub fn web_assets_enabled(&self) -> bool {
        self.web_assets_dir.is_some() || self.embedded_web_assets
    }

    /// Attach the canonical workspace-root path for the filesystem browser
    /// service. Callers should pass an already-canonicalized, existing
    /// absolute path — the fs service assumes `starts_with` checks against
    /// this value are sound.
    #[must_use]
    #[allow(dead_code)] // Called by `labby serve` when fs HTTP routes are enabled.
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = Some(Arc::new(root));
        self
    }

    /// Disable auth on `/v1/*` while leaving `/mcp` auth unchanged.
    #[must_use]
    pub fn with_web_ui_auth_disabled(mut self, disabled: bool) -> Self {
        self.web_ui_auth_disabled = disabled;
        self
    }

    /// Attach the static bearer token (LAB_MCP_HTTP_TOKEN) so handlers
    /// outside the auth middleware can validate it.
    #[must_use]
    pub fn with_bearer_token(mut self, token: Option<Arc<str>>) -> Self {
        self.bearer_token = token;
        self
    }

    #[must_use]
    pub fn with_http_bind_host(mut self, host: impl Into<String>) -> Self {
        self.http_bind_host = Some(Arc::new(host.into()));
        self
    }

    /// Returns `true` unless the current process is explicitly in `NonMaster` role.
    ///
    /// **Scope of this check vs. `require_master_store`** (lab-zxx5.27):
    /// Both read `self.node_role` — that's the security invariant. But they
    /// answer different questions:
    /// - `is_master()` is used for ROUTE MOUNTING ("should this server expose
    ///   controller-only routes at all?"). Returns `true` when `node_role` is
    ///   `None` (unset = legacy default of master) or `Some(Master)`.
    /// - `require_master_store()` is used PER-REQUEST ("can THIS request
    ///   access the node store?") and additionally requires the fleet store
    ///   to be configured. On a master-roled server without a store it
    ///   fails closed at the handler.
    ///
    /// This asymmetry is intentional. Both paths fail closed on NonMaster.
    /// A master-roled server without a store mounts controller routes (via
    /// `is_master()`) but each request still fails at `require_master_store`
    /// with a handler-level error, surfacing the misconfiguration. Do NOT
    /// add the store check to `is_master()` — router-level gating on
    /// runtime-state presence is the wrong layer.
    ///
    /// **Security note:** any NEW authorization gate in the codebase must
    /// read `self.node_role` to stay consistent with this pair. Do not add
    /// an alternate field or a separate role lookup.
    #[must_use]
    pub fn is_master(&self) -> bool {
        !matches!(self.node_role, Some(NodeRole::NonMaster))
    }

    /// Attach a pre-built ACP session registry so `AppState` shares the same `Arc`
    /// as the process-global dispatch slot installed in `cli/serve.rs`.
    ///
    /// **Must be called** after `dispatch::acp::install_registry()` with the same `Arc`
    /// whenever ACP dispatch actions are in scope. See the invariant note on
    /// `from_registry()`.
    #[cfg(feature = "acp")]
    #[must_use]
    pub fn with_acp_registry(mut self, registry: Arc<AcpSessionRegistry>) -> Self {
        self.acp_registry = registry;
        self
    }

    /// Attach the shared MCP registry store for `/v0.1` read endpoints.
    #[cfg(feature = "marketplace")]
    #[must_use]
    pub fn with_registry_store(
        mut self,
        store: Arc<crate::dispatch::marketplace::store::RegistryStore>,
    ) -> Self {
        self.registry_store = Some(store);
        self
    }
}

fn protected_mcp_connect_timeout() -> Duration {
    std::env::var(PROTECTED_MCP_CONNECT_TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(
            Duration::from_secs(DEFAULT_PROTECTED_MCP_CONNECT_TIMEOUT_SECS),
            Duration::from_secs,
        )
}

fn build_protected_mcp_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        // Keep long-lived MCP streams possible, but fail unreachable upstreams
        // instead of letting proxy connection attempts hang indefinitely.
        .connect_timeout(protected_mcp_connect_timeout())
        .build()
        .expect("protected MCP HTTP client configuration is valid")
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
