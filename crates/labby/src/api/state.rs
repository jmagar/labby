//! Shared application state for axum handlers.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::catalog::{Catalog, build_catalog};
use crate::config::LabConfig;
use crate::dispatch::clients::ServiceClients;
use crate::registry::{ToolRegistry, build_default_registry};

const DEFAULT_PROTECTED_MCP_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Application state passed to every axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    /// Loaded once under the daemon installation lifecycle lock; never request-time I/O.
    pub(crate) installation_id: Option<Arc<str>>,
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
    /// Optional Depot control-plane client. Authority remains server-held.
    pub depot: Arc<crate::dispatch::depot::DepotClient>,
    /// Resolved discovery runtime shared by all browser adapters.
    pub depot_manager: Arc<crate::dispatch::depot::manager::Manager>,
    /// Durable provider lifecycle dispatch, available only with browser OAuth authority.
    pub depot_admin: Option<Arc<crate::dispatch::depot::admin::Admin>>,
    depot_store: Option<Arc<crate::dispatch::depot::store::Store>>,
    depot_policy: crate::dispatch::depot::network::NetworkPolicy,
    /// Shared HTTP client for protected MCP reverse proxy requests.
    pub protected_mcp_http_client: reqwest::Client,
    /// Shared public OAuth callback relay forwarder.
    pub public_relay_forwarder: Arc<crate::oauth::public_relay::PublicRelayForwarder>,
    /// Live public OAuth callback relay registry manager.
    ///
    /// `None` means the public relay is not enabled for this process.
    pub public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
    /// Protected-route scoped MCP services, keyed by the configured route name.
    ///
    /// Host/path matching happens before this lookup. Keeping one router per
    /// route prevents equal paths on different public hosts from overwriting
    /// each other in a shared Axum router.
    pub protected_mcp_routers: Option<Arc<HashMap<String, axum::Router>>>,
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
    /// OAuth-mode auth server state, mounted only when LABBY_AUTH_MODE=oauth.
    pub oauth_state: Option<Arc<labby_auth::state::AuthState>>,
    /// Provider-independent store for project-bound browser sessions.
    pub project_session_state: Option<Arc<labby_auth::project_session::ProjectSessionState>>,
    /// Cached actor-key deriver used at authenticated bind boundaries.
    pub actor_key_deriver: Option<Arc<crate::observability::activity::ActorKeyDeriver>>,
    /// Core assertion verifier for sealed integrated trusted-host mode.
    ///
    /// When present, the API router requires a fresh delegated actor assertion
    /// on every request after the Unix listener has accepted the peer.
    pub trusted_host_verifier: Option<Arc<labby_auth::trusted_host::TrustedHostVerifier>>,
    /// Shared gateway manager for runtime upstream pool access and config mutation.
    ///
    /// `None` when gateway management is not wired for this process.
    #[cfg(feature = "gateway")]
    pub gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
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
    /// Static bearer token (LABBY_MCP_HTTP_TOKEN), if configured.
    ///
    /// Stored on AppState so handlers outside the auth middleware
    /// (e.g. `/auth/session`) can validate the same token. The middleware
    /// remains the canonical enforcement point for `/v1/*`.
    pub bearer_token: Option<Arc<str>>,
    /// HTTP bind host resolved by `labby serve`.
    pub http_bind_host: Option<Arc<String>>,
    /// Process-scoped owner of the access-store lifecycle.
    ///
    /// The default is a conservative, non-I/O unavailable runtime. Server
    /// startup replaces it after resolving and observing the configured store.
    pub(crate) access_runtime: Arc<crate::access::AccessRuntime>,
    pub(crate) file_stash_runtime: Arc<crate::file_stash::FileStashRuntime>,
    /// Daemon-owned proof lifecycle orchestration. `None` fails closed and
    /// keeps the local bootstrap routes unavailable until startup wires L6.
    pub(crate) access_bootstrap_proof:
        Option<Arc<dyn crate::api::services::access_bootstrap_proof::AccessBootstrapProofService>>,
    /// Shared uncached credential/session adapter backed by the production
    /// published-policy authority.
    pub(crate) access_credential_adapter: Option<Arc<crate::access::AccessCredentialAdapter>>,
    #[cfg(feature = "skills")]
    pub(crate) skill_library: Option<
        Arc<
            crate::dispatch::skill_library::dispatch::SkillLibraryService<
                crate::skills::registry::FirstPartyGeneration,
            >,
        >,
    >,
    #[cfg(feature = "skills")]
    pub(crate) skill_library_imports:
        Option<Arc<crate::dispatch::skill_library::import::ImportCoordinator>>,
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
            installation_id: None,
            catalog,
            registry: Arc::new(registry),
            clients,
            depot: Arc::new(crate::dispatch::depot::DepotClient::disabled()),
            depot_manager: Arc::new(crate::dispatch::depot::manager::Manager::default()),
            depot_admin: None,
            depot_store: None,
            depot_policy: Default::default(),
            protected_mcp_http_client,
            // `PublicRelayForwarder::new()` only fails on reqwest client
            // build errors (e.g. TLS backend init failure), the same class
            // of infallible-in-practice startup error already accepted for
            // `protected_mcp_http_client` above via `.expect(...)`.
            public_relay_forwarder: Arc::new(
                crate::oauth::public_relay::PublicRelayForwarder::new()
                    .expect("public relay forwarder configuration is valid"),
            ),
            public_relay: None,
            protected_mcp_routers: None,
            enabled_services: Arc::new(enabled_services),
            auth_config: None,
            config: Arc::new(LabConfig::default()),
            oauth_state: None,
            project_session_state: None,
            actor_key_deriver: None,
            trusted_host_verifier: None,
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            web_assets_dir: None,
            embedded_web_assets: false,
            workspace_root: None,
            web_ui_auth_disabled: false,
            bearer_token: None,
            http_bind_host: None,
            access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
            access_bootstrap_proof: None,
            access_credential_adapter: None,
            #[cfg(feature = "skills")]
            skill_library: None,
            #[cfg(feature = "skills")]
            skill_library_imports: None,
            server_start: std::time::Instant::now(),
        }
    }

    /// Attach the resolved auth configuration.
    #[must_use]
    pub fn with_auth_config(mut self, config: labby_auth::config::AuthConfig) -> Self {
        self.auth_config = Some(Arc::new(config));
        self
    }

    /// Attach the initialized process-scoped access runtime.
    #[must_use]
    pub(crate) fn with_access_runtime(
        mut self,
        runtime: Arc<crate::access::AccessRuntime>,
    ) -> Self {
        self.access_runtime = runtime;
        self
    }

    #[must_use]
    pub(crate) fn with_file_stash_runtime(
        mut self,
        runtime: Arc<crate::file_stash::FileStashRuntime>,
    ) -> Self {
        self.file_stash_runtime = runtime;
        self
    }

    #[must_use]
    pub(crate) fn with_access_bootstrap_proof(
        mut self,
        service: Arc<dyn crate::api::services::access_bootstrap_proof::AccessBootstrapProofService>,
    ) -> Self {
        self.access_bootstrap_proof = Some(service);
        self
    }

    #[must_use]
    pub(crate) fn with_access_credential_adapter(
        mut self,
        adapter: Arc<crate::access::AccessCredentialAdapter>,
    ) -> Self {
        self.access_credential_adapter = Some(adapter);
        self
    }

    #[cfg(feature = "skills")]
    #[must_use]
    pub(crate) fn with_skill_library(
        mut self,
        service: Arc<
            crate::dispatch::skill_library::dispatch::SkillLibraryService<
                crate::skills::registry::FirstPartyGeneration,
            >,
        >,
    ) -> Self {
        self.skill_library = Some(service);
        self
    }

    #[cfg(feature = "skills")]
    #[must_use]
    pub(crate) fn with_skill_library_imports(
        mut self,
        imports: Arc<crate::dispatch::skill_library::import::ImportCoordinator>,
    ) -> Self {
        self.skill_library_imports = Some(imports);
        self
    }

    #[must_use]
    pub fn with_config(mut self, config: LabConfig) -> Self {
        self.depot_manager = Arc::new(crate::dispatch::depot::manager::Manager::new(
            &config.depot,
            Default::default(),
            Default::default(),
        ));
        self.config = Arc::new(config);
        self
    }

    #[must_use]
    pub fn with_depot_snapshot(
        mut self,
        secrets: crate::dispatch::depot::manager::SecretSnapshot,
        policy: crate::dispatch::depot::network::NetworkPolicy,
    ) -> Self {
        self.depot_policy = policy.clone();
        self.depot_manager = Arc::new(crate::dispatch::depot::manager::Manager::new(
            &self.config.depot,
            secrets,
            policy,
        ));
        self
    }

    #[must_use]
    pub fn with_depot_storage(
        mut self,
        config: PathBuf,
        environment: PathBuf,
        state: PathBuf,
    ) -> Self {
        self.depot_store = Some(Arc::new(crate::dispatch::depot::store::Store::new(
            config,
            environment,
            state,
        )));
        self
    }

    #[must_use]
    pub fn with_protected_mcp_routers(mut self, routers: HashMap<String, axum::Router>) -> Self {
        self.protected_mcp_routers = Some(Arc::new(routers));
        self
    }

    #[must_use]
    pub fn with_public_relay_manager(
        mut self,
        manager: Arc<crate::oauth::public_relay::PublicRelayRegistryManager>,
    ) -> Self {
        self.public_relay = Some(manager);
        self
    }

    #[must_use]
    pub fn with_oauth_state(mut self, auth_state: labby_auth::state::AuthState) -> Self {
        if let Some(store) = self.depot_store.clone() {
            self.depot_admin = Some(Arc::new(crate::dispatch::depot::admin::Admin::new(
                Arc::clone(&self.depot_manager),
                store,
                labby_auth::reauth::Proofs::new(auth_state.store.clone()),
                self.depot_policy.clone(),
            )));
        }
        self.oauth_state = Some(Arc::new(auth_state));
        self
    }

    #[must_use]
    pub fn with_project_session_state(
        mut self,
        state: labby_auth::project_session::ProjectSessionState,
    ) -> Self {
        self.project_session_state = Some(Arc::new(state));
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

    #[must_use]
    pub fn with_trusted_host_verifier(
        mut self,
        verifier: Arc<labby_auth::trusted_host::TrustedHostVerifier>,
    ) -> Self {
        self.trusted_host_verifier = Some(verifier);
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

    /// Attach the static bearer token (LABBY_MCP_HTTP_TOKEN) so handlers
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
}

fn protected_mcp_connect_timeout() -> Duration {
    crate::config::resolved_protected_mcp_connect_timeout_secs()
        .filter(|seconds| *seconds > 0)
        .map_or(
            Duration::from_secs(DEFAULT_PROTECTED_MCP_CONNECT_TIMEOUT_SECS),
            Duration::from_secs,
        )
}

fn build_protected_mcp_http_client() -> reqwest::Client {
    // See entrypoint.rs::run for why this call is needed under
    // "rustls-no-provider" -- idempotent, safe to ignore Err. entrypoint::run
    // already installs it for the real binary; test binaries don't go
    // through it.
    drop(rustls::crypto::ring::default_provider().install_default());
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

#[cfg(test)]
mod access_runtime_tests {
    use super::*;

    #[tokio::test]
    async fn default_state_uses_non_io_unavailable_access_runtime() {
        let state = AppState::new();

        assert_eq!(
            state.access_runtime.status().await,
            crate::access::AccessRuntimeStatus::Blocked(
                crate::access::AccessBlockedReason::Unavailable
            )
        );
    }

    #[tokio::test]
    async fn access_runtime_builder_keeps_the_injected_process_owner() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Arc::new(
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
        );
        let state = AppState::new().with_access_runtime(Arc::clone(&runtime));

        assert!(Arc::ptr_eq(&state.access_runtime, &runtime));
    }
}
