//! `labby serve` — start the MCP server.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use labby_auth::config::AuthMode;
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
#[cfg(feature = "gateway")]
use tokio::sync::mpsc;

use crate::access::AccessRuntime;
use crate::api::AppState;
use crate::config::{
    LabConfig, access_db_path, config_toml_path, dotenv_path, heal_env_file_permissions,
    resolve_auth_for_config,
};
#[cfg(feature = "gateway")]
use crate::dispatch::clients::SharedServiceClients;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::config_store::LabConfigStore;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::install_gateway_manager;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::{
    GatewayManager, GatewayManagerConfig, GatewayOauthConfig, GatewayRuntimeHandle,
};
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::types::CatalogChangeNotifier;
use crate::mcp::peers::PeerNotifier;
use crate::mcp::server::LabMcpServer;
use crate::output::theme::{CliTheme, ColorPolicy, RenderContext, RenderEnv};
#[cfg(target_os = "linux")]
use crate::process::unix::{exe_path, terminate_sigterm};
use crate::registry::{ToolRegistry, build_default_registry};
#[cfg(unix)]
use crate::unix_listener;

#[cfg(unix)]
type HostedUnixConfig = unix_listener::UnixListenerConfig;
#[cfg(not(unix))]
#[derive(Debug, Clone)]
struct HostedUnixConfig;

/// Aurora theme for `serve` startup banners. These print before the CLI
/// `--color` flag is in scope, so resolve styling from the environment
/// (`NO_COLOR`, stderr TTY) with the default `Auto` policy.
fn stderr_theme() -> CliTheme {
    CliTheme::from_context(RenderContext::from_policy(
        ColorPolicy::Auto,
        RenderEnv::stderr(),
    ))
}

/// Transport choices for `labby serve`.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Transport {
    /// stdin/stdout framing (available via `labby mcp`).
    Stdio,
    /// HTTP transport (default) — requires `LABBY_MCP_HTTP_TOKEN` or OAuth when exposed remotely.
    Http,
    /// Streamable HTTP served over a Unix-domain socket.
    #[value(name = "unix_socket", alias = "unix-socket")]
    UnixSocket,
}

#[derive(Debug, Subcommand)]
pub enum ServeCommand {
    /// Run the MCP server over stdio instead of the default HTTP transport.
    Mcp(McpArgs),
}

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Confirm that MCP should run over stdio.
    #[arg(long)]
    pub stdio: bool,
}

/// `labby mcp` arguments.
#[derive(Debug, Args)]
pub struct McpServeArgs {
    /// Comma- or space-separated list of services to enable. Empty = all.
    #[arg(long, value_delimiter = ',')]
    pub services: Vec<String>,
    /// Override the log filter level for this process.
    /// Sets `LABBY_LOG=labby=<level>,warn` before tracing init.
    /// Example: `--log-level debug`
    #[arg(long)]
    pub log_level: Option<String>,
}

/// `labby serve` arguments.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Comma- or space-separated list of services to enable. Empty = all.
    #[arg(long, value_delimiter = ',')]
    pub services: Vec<String>,
    /// Legacy transport selector. Prefer `labby serve` for HTTP and `labby mcp` for stdio.
    #[arg(long, value_enum, hide = true)]
    pub transport: Option<Transport>,
    /// Bind host for the HTTP transport.
    #[arg(long)]
    pub host: Option<String>,
    /// Bind port for the HTTP transport.
    #[arg(long)]
    pub port: Option<u16>,
    /// Override the log filter level for this process.
    /// Sets `LABBY_LOG=labby=<level>,warn` before tracing init.
    /// Example: `--log-level debug`
    #[arg(long)]
    pub log_level: Option<String>,
    #[command(subcommand)]
    pub command: Option<ServeCommand>,
}

/// Run the top-level `labby mcp` stdio shortcut.
pub async fn run_mcp(args: McpServeArgs, config: &LabConfig) -> Result<ExitCode> {
    run(
        ServeArgs {
            services: args.services,
            transport: Some(Transport::Stdio),
            host: None,
            port: None,
            log_level: args.log_level,
            command: None,
        },
        config,
    )
    .await
}

#[cfg(feature = "skills")]
fn bootstrap_skill_library(
    config: &LabConfig,
) -> Result<Arc<crate::dispatch::skill_library::ProcessSkillLibraryRuntime>> {
    use crate::dispatch::skill_library::blocking::BoundedBlockingExecutor;
    use crate::dispatch::skill_library::dispatch::{
        ActivationCoordinator, ArtifactFirstPartyProjection, GenerationProjection,
        SkillLibraryService,
    };
    use crate::skills::registry::{
        GenerationSeed, first_party_generation_manager, initialize_first_party_generation_manager,
    };

    let artifacts_root = labby_runtime::lab_home().join("artifacts");
    let store = Arc::new(
        labby_runtime::artifacts::ArtifactStore::new(&artifacts_root)
            .context("open Skill Library Artifact store")?,
    );
    let snapshot = store
        .library_snapshot()
        .context("load Skill Library metadata")?;
    let imports = configure_skill_library_imports(config, &artifacts_root)?;
    let controls = Arc::new(
        crate::dispatch::artifact_control::ArtifactControlPlane::from_config(&config.artifacts)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    );
    let blocking = BoundedBlockingExecutor::new(8, Duration::from_secs(2), Duration::from_secs(30))
        .map_err(|_| anyhow::anyhow!("invalid Skill Library blocking executor configuration"))?;
    initialize_first_party_generation_manager(GenerationSeed {
        version: snapshot.version,
        active_digest: snapshot.active_generation_digest.clone(),
    })
    .map_err(|_| {
        anyhow::anyhow!(
            "first-party Skill generation was initialized before persisted Skill Library state"
        )
    })?;
    let manager = first_party_generation_manager();
    let projection: Arc<dyn GenerationProjection<crate::skills::registry::FirstPartyGeneration>> =
        Arc::new(ArtifactFirstPartyProjection);
    let candidate = projection
        .prepare(&store, &snapshot, None)
        .context("build persisted Skill Library generation")?;
    let coordinator = Arc::new(ActivationCoordinator::from_cell(
        manager.generation_cell(),
        snapshot.version,
    ));
    coordinator.reconcile(candidate, snapshot.version);
    tracing::info!(
        subsystem = "startup",
        phase = "artifacts.ready",
        library_version = snapshot.version,
        active_skill_count = snapshot
            .records
            .values()
            .filter(|record| record.active_revision_id.is_some() && !record.archived)
            .count(),
        "persisted Skill Library generation ready"
    );
    let service = Arc::new(SkillLibraryService::new(
        store,
        blocking,
        coordinator,
        projection,
    ));
    let runtime = Arc::new(crate::dispatch::skill_library::ProcessSkillLibraryRuntime {
        service,
        imports,
        controls,
    });
    crate::dispatch::skill_library::install_process_runtime(Arc::clone(&runtime))
        .map_err(|_| anyhow::anyhow!("Skill Library runtime was already initialized"))?;

    Ok(runtime)
}

#[cfg(feature = "skills")]
fn configure_skill_library_imports(
    config: &LabConfig,
    artifacts_root: &Path,
) -> Result<Arc<crate::dispatch::skill_library::import::ImportCoordinator>> {
    crate::dispatch::skill_library::import::ImportCoordinator::from_config(
        &config.artifacts,
        &artifacts_root.join("acquisition"),
    )
    .map(Arc::new)
    .context("configure Skill Library exact-source adapters")
}

#[cfg(feature = "skills")]
fn bootstrap_selected_skill_library_with<T>(
    registry: &ToolRegistry,
    bootstrap: impl FnOnce() -> Result<T>,
) -> Result<Option<T>> {
    if ["artifacts", "bundles", "jobs", "sources", "uploads"]
        .iter()
        .any(|service| registry.service(service).is_some())
    {
        bootstrap().map(Some)
    } else {
        Ok(None)
    }
}

/// Run the serve subcommand.
pub fn run(args: ServeArgs, config: &LabConfig) -> impl Future<Output = Result<ExitCode>> {
    // Keep the long-lived server future off the CLI and stdio shortcut frames.
    // Those frames remain on the main thread during MCP initialization, which
    // must leave room for the protocol decoder on a one-mebibyte stack.
    Box::pin(run_server(args, config))
}

async fn initialize_selected_file_stash_runtime(
    registry: &ToolRegistry,
    config: &LabConfig,
) -> Arc<crate::file_stash::FileStashRuntime> {
    if registry.service("stash").is_none() {
        return Arc::new(crate::file_stash::FileStashRuntime::blocked());
    }
    match crate::config::file_stash_root_path(config) {
        Ok(root) => Arc::new(
            crate::file_stash::FileStashRuntime::initialize_with_preferences(
                root,
                config.file_stash.clone(),
            )
            .await,
        ),
        Err(_) => {
            tracing::warn!("file stash runtime unavailable: state root could not be resolved");
            Arc::new(crate::file_stash::FileStashRuntime::blocked())
        }
    }
}

async fn run_server(args: ServeArgs, config: &LabConfig) -> Result<ExitCode> {
    let transport = resolve_transport(
        args.transport,
        args.command.as_ref(),
        std::env::var("LABBY_MCP_TRANSPORT").ok(),
        config.mcp.transport.as_deref(),
    )?;
    tracing::info!(
        subsystem = "cli",
        phase = "serve.start",
        transport = ?transport,
        requested_service_count = args.services.len(),
        "starting serve command"
    );
    // Resolve TCP-only host/port settings only for the HTTP transport. Unix
    // and stdio modes must not fail because an unrelated HTTP env value is bad.
    let (host, port) = if matches!(transport, Transport::Http) {
        let host = args
            .host
            .or_else(|| std::env::var("LABBY_MCP_HTTP_HOST").ok())
            .or_else(|| config.mcp.host.clone())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let port = resolve_port(
            args.port,
            std::env::var("LABBY_MCP_HTTP_PORT").ok(),
            config.mcp.port,
        )?;
        (host, port)
    } else {
        ("127.0.0.1".to_string(), 8765)
    };
    let unix_listener_config = resolve_unix_listener_config(transport, &config.mcp)?;
    let peer_auth_enabled = unix_peer_auth_enabled(unix_listener_config.as_ref());
    let trusted_host_verifier =
        resolve_trusted_host_verifier(transport, unix_listener_config.as_ref(), peer_auth_enabled)?;
    let integrated_trusted_host = trusted_host_verifier.is_some();
    let config_path = config_toml_path()?;
    tracing::info!(
        subsystem = "startup",
        phase = "bootstrap.start",
        transport = ?transport,
        bind_host = %host,
        bind_port = port,
        config_path = %config_path.display(),
        requested_service_count = args.services.len(),
        "starting labby serve bootstrap"
    );

    crate::registry::set_runtime_built_in_upstream_apis_enabled(
        config.services.built_in_upstream_apis_enabled,
    );
    let registry = build_default_registry();
    let registry = crate::registry::filter_built_in_upstream_apis(
        registry,
        config.services.built_in_upstream_apis_enabled,
    );
    let registry = filter_registry(registry, &args.services)?;
    tracing::info!(
        subsystem = "startup",
        phase = "bootstrap.registry",
        selected_service_count = registry.services().len(),
        "service registry ready"
    );

    let stdio_mode = should_run_stdio(transport, args.command.as_ref());

    // `labby` should have exactly one canonical, long-running gateway; every
    // other invocation is a thin client to it. A stdio instance is normally
    // that canonical instance itself (self-sufficient, nothing to bridge
    // to) -- but if a real daemon is already reachable, become a pure
    // protocol bridge to it instead of standing up a second, independent
    // GatewayManager with its own config view, upstream connections, and
    // OAuth state. See `crate::live_gateway` and `crate::mcp::bridge` for the
    // full rationale; this mirrors what the `gateway` CLI subcommands
    // already do for their own dispatch.
    #[cfg(feature = "gateway")]
    if stdio_mode && let Some(live) = crate::live_gateway::detect(config, "mcp").await? {
        tracing::info!(
            subsystem = "startup",
            phase = "bridge.detected",
            transport = ?transport,
            "found a live labby serve daemon; running as a thin stdio bridge to it instead of a standalone instance"
        );
        return run_stdio_bridge(live).await;
    }

    // A real daemon owns the installation lifecycle before observing/opening
    // any durable access state. Thin-client stdio bridges return above and do
    // not contend with the daemon that owns this lock.
    let installation_paths = crate::installation::InstallationPaths::resolve()
        .context("resolve canonical Labby installation root")?;
    let _installation_lifecycle =
        crate::installation::InstallationLifecycleLock::acquire_daemon(&installation_paths)
            .context("acquire Labby daemon lifecycle lock")?;
    crate::dispatch::setup::access_bootstrap::reconcile_daemon_prepares(&installation_paths)
        .await
        .context("reconcile access-bootstrap prepare journal before serving")?;
    // Reconcile first: an existing journal with a missing identity must remain
    // a recovery error, not cause a new identity to be invented before refusal.
    let installation_id =
        crate::dispatch::setup::access_bootstrap::installation_id(&installation_paths)
            .context("load durable Labby installation identity")?;

    let access_runtime = match access_db_path() {
        Ok(path) => Arc::new(AccessRuntime::initialize(path).await),
        Err(_) => {
            // Access enforcement is not active yet, so preserve existing serve
            // availability while exposing a typed blocked runtime to every
            // transport. Do not log ambient path/config details.
            tracing::warn!("access runtime unavailable: state path could not be resolved");
            Arc::new(AccessRuntime::blocked_unavailable())
        }
    };
    let file_stash_runtime = initialize_selected_file_stash_runtime(&registry, config).await;

    let spawn_depth = resolve_lab_spawn_depth(std::env::var("LABBY_SPAWN_DEPTH").ok());
    let suppress_upstream_runtime = stdio_recursion_guard_active(stdio_mode, spawn_depth);
    let mut bearer_token = http_token();
    let auth_config =
        resolve_auth_for_config(&config).context("invalid HTTP auth configuration")?;
    let resource_registry = matches!(auth_config.mode, AuthMode::OAuth)
        .then(labby_auth::resource_registry::ResourceRegistry::new);
    // SECURITY: Only log metadata — never resolved secret values.
    // Safe fields: enum names, booleans, counts. Forbidden: URL strings, token values, key material.
    tracing::info!(
        subsystem = "api_server",
        phase = "auth.config",
        auth_mode = ?auth_config.mode,
        public_url_configured = auth_config.public_url.is_some(),
        bearer_token_configured = bearer_token.is_some(),
        "http auth configuration resolved"
    );
    let notifier = PeerNotifier::default();
    // WIRING (SEC): tighten loose ~/.labby/.env permissions at every startup so a
    // freshly-created file (which may be 0644) is corrected to 0600 before any
    // secrets are read.  heal_env_file_permissions is idempotent and a no-op on
    // non-Unix targets.  Do NOT call this after secrets are already in memory —
    // only the on-disk file mode matters here.
    if let Ok(env_path) = dotenv_path() {
        heal_env_file_permissions(&env_path);
    }

    #[cfg(feature = "skills")]
    let skill_library_runtime =
        bootstrap_selected_skill_library_with(&registry, || bootstrap_skill_library(config))?;
    #[cfg(feature = "gateway")]
    let gateway_manager = build_gateway_runtime(
        config,
        &auth_config,
        transport,
        spawn_depth,
        suppress_upstream_runtime,
        registry.clone(),
        notifier.clone(),
        resource_registry.clone(),
        integrated_trusted_host,
    )
    .await?;
    #[cfg(feature = "gateway")]
    let bootstrap_policy = Arc::new(
        crate::dispatch::access_bootstrap::GatewayBootstrapPolicyAuthority::new(
            gateway_manager.as_ref().clone(),
            access_runtime.as_ref().clone(),
        ),
    );
    #[cfg(feature = "gateway")]
    let access_credential_adapter = access_runtime.credential_adapter(bootstrap_policy.clone());
    #[cfg(feature = "gateway")]
    let access_bootstrap_proof = Arc::new(
        crate::dispatch::access_bootstrap::DaemonAccessBootstrapProofService::new(
            access_runtime.as_ref().clone(),
            bootstrap_policy,
        ),
    );
    #[cfg(not(feature = "gateway"))]
    reject_protected_routes_without_gateway(config)?;
    if stdio_mode {
        tracing::info!(
            subsystem = "api_server",
            phase = "disabled",
            "api server disabled for stdio transport"
        );
        tracing::info!(
            subsystem = "web_server",
            phase = "disabled",
            "web server disabled for stdio transport"
        );
        #[cfg(feature = "gateway")]
        {
            return run_stdio(
                Arc::new(registry),
                Arc::clone(&gateway_manager),
                Arc::clone(&access_runtime),
                Arc::clone(&file_stash_runtime),
                notifier,
                spawn_depth,
                suppress_upstream_runtime,
            )
            .await;
        }
        #[cfg(not(feature = "gateway"))]
        {
            return run_stdio(
                Arc::new(registry),
                Arc::clone(&access_runtime),
                Arc::clone(&file_stash_runtime),
                notifier,
                spawn_depth,
                suppress_upstream_runtime,
            )
            .await;
        }
    }

    if matches!(transport, Transport::Http) && host.is_empty() {
        anyhow::bail!("HTTP host cannot be empty — set LABBY_MCP_HTTP_HOST or mcp.host in config");
    }

    #[cfg(feature = "gateway")]
    crate::mcp::server::verify_upstream_subject_resolution_support()
        .context("verify upstream OAuth subject-resolution wiring")?;

    // First-run self-bootstrap (setup-wizard consolidation): when no MCP token
    // is configured, OAuth is not active, AND the bind is loopback, generate a
    // token + minimal ~/.labby/.env so the server can start and the operator can
    // reach /setup. Closes the headless bootstrap circularity.
    //
    // Loopback gate (HIGH-1): we deliberately do NOT auto-bootstrap on a
    // non-loopback bind. An explicit `--host 0.0.0.0` with no auth must still
    // hit the lab-319g safety gate below and bail — silently minting a token
    // would turn a misconfiguration into a publicly reachable server.
    //
    // The generated token is made authoritative in-process immediately
    // (`bearer_token = Some(token)`), so the running server always
    // authenticates with the token it just wrote even if the env reload fails.
    // We THEN reload the file via dotenvy so downstream LABBY_MCP_HTTP_TOKEN
    // readers also see it. dotenvy owns its
    // own set_var, keeping this crate unsafe-free (the workspace forbids
    // unsafe_code) and not overriding already-set vars.
    if !peer_auth_enabled
        && crate::dispatch::setup::should_bootstrap(
            bearer_token.is_some(),
            matches!(auth_config.mode, AuthMode::OAuth),
        )
        && (is_loopback_host(&host) || matches!(transport, Transport::UnixSocket))
    {
        match crate::dispatch::setup::bootstrap() {
            Ok(crate::dispatch::setup::BootstrapOutcome::Created { env_path, token }) => {
                bearer_token = Some(token.clone());
                if let Err(error) = dotenvy::from_path(&env_path) {
                    tracing::error!(
                        surface = "cli",
                        service = "serve",
                        error = %error,
                        "failed to reload generated ~/.labby/.env into process env; \
                         in-process token is authoritative, downstream env readers may not see it"
                    );
                }
                tracing::info!(
                    surface = "cli",
                    service = "serve",
                    "first run: generated LABBY_MCP_HTTP_TOKEN and wrote ~/.labby/.env"
                );
                // Do NOT print the token itself — stderr is commonly captured
                // by systemd/journald/Docker, which would persist the secret
                // (the project forbids logging secrets). The token is in the
                // 0600 .env; point the operator there instead.
                eprintln!("\n  Lab first-run setup");
                eprintln!(
                    "  Generated an MCP bearer token in {} (mode 0600).",
                    env_path.display()
                );
                if matches!(transport, Transport::Http) {
                    eprintln!("  Open http://{host}:{port}/setup to finish configuration.");
                } else {
                    eprintln!(
                        "  Connect through the configured Unix socket to finish configuration."
                    );
                }
                eprintln!(
                    "  For remote clients, read the token from that file (e.g. `grep LABBY_MCP_HTTP_TOKEN {}`).\n",
                    env_path.display()
                );
            }
            Ok(crate::dispatch::setup::BootstrapOutcome::AlreadyPresent { .. }) => {}
            Err(error) => {
                tracing::warn!(surface = "cli", service = "serve", error = %error, "first-run bootstrap skipped");
            }
        }
    }

    let credential_auth_configured =
        bearer_token.is_some() || matches!(auth_config.mode, AuthMode::OAuth);
    if integrated_trusted_host && credential_auth_configured {
        anyhow::bail!(
            "integrated trusted-host mode cannot combine Labby bearer or OAuth identity paths"
        );
    }
    if peer_auth_enabled && credential_auth_configured {
        anyhow::bail!(
            "Unix peer-credential authorization cannot be combined with bearer or OAuth authentication"
        );
    }
    let auth_configured = credential_auth_configured || peer_auth_enabled;

    // Safety gate: refuse to bind on a non-localhost HTTP address without
    // any auth configured (lab-319g). This prevents accidental
    // unauthenticated deployment on a LAN-accessible address.
    if matches!(transport, Transport::Http) && !auth_configured && !is_loopback_host(&host) {
        anyhow::bail!(
            "refusing to bind HTTP on {host}:{port} without authentication. \
             Set LABBY_MCP_HTTP_TOKEN or LABBY_AUTH_MODE=oauth, or bind to \
             127.0.0.1 for local-only access."
        );
    }
    if matches!(transport, Transport::UnixSocket) && !auth_configured {
        anyhow::bail!(
            "refusing to bind a Unix socket without authentication; configure a bearer token, OAuth, or mcp.peer_uid/mcp.peer_gid"
        );
    }

    let oauth_state = if matches!(auth_config.mode, AuthMode::OAuth) {
        Some(
            labby_auth::state::AuthState::new_with_resource_registry(
                auth_config.clone(),
                resource_registry
                    .clone()
                    .expect("OAuth mode initializes a resource registry"),
            )
            .await
            .context("initialize labby-auth OAuth state")?,
        )
    } else {
        None
    };
    let project_session_state = if let Some(oauth_state) = oauth_state.as_ref() {
        labby_auth::project_session::ProjectSessionState::from_store(
            oauth_state.store.clone(),
            "__Host-labby-session",
        )
        .expect("the fixed project session cookie name has a __Host- prefix")
    } else {
        labby_auth::project_session::ProjectSessionState::open(
            auth_config.sqlite_path.clone(),
            "__Host-labby-session",
        )
        .await
        .context("initialize project session store")?
    };

    let web_assets_dir = resolve_web_assets_dir(&config.web);
    let embedded_web_assets_enabled =
        web_assets_dir.is_none() && crate::api::web::embedded_web_assets_available();

    let oauth_enabled = matches!(auth_config.mode, AuthMode::OAuth);
    let depot_secrets = crate::dispatch::depot::manager::SecretSnapshot::capture(&config.depot);
    let depot_policy =
        crate::dispatch::depot::manager::host_policy(&config.depot).map_err(anyhow::Error::msg)?;

    let mut state = AppState::from_registry(registry)
        .with_config(config.clone())
        .with_depot_snapshot(depot_secrets, depot_policy)
        .with_depot_storage(
            config_path.clone(),
            dotenv_path().unwrap_or_else(|_| ".env".into()),
            config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("depot-transactions"),
        )
        .with_access_runtime(Arc::clone(&access_runtime))
        .with_file_stash_runtime(Arc::clone(&file_stash_runtime))
        .with_http_bind_host(host.clone());
    state.installation_id = Some(Arc::from(installation_id));
    #[cfg(feature = "gateway")]
    {
        state = state
            .with_access_credential_adapter(access_credential_adapter)
            .with_access_bootstrap_proof(access_bootstrap_proof);
    }
    state = state.with_project_session_state(project_session_state);
    #[cfg(feature = "skills")]
    if let Some(skill_library_runtime) = skill_library_runtime {
        state = state
            .with_skill_library(Arc::clone(&skill_library_runtime.service))
            .with_skill_library_imports(Arc::clone(&skill_library_runtime.imports));
    }
    let public_relay_store = crate::oauth::public_relay::PublicRelayRegistryStore::new(
        crate::oauth::public_relay::PublicRelayRegistryStore::default_path(),
    );
    let public_relay_registry_path = public_relay_store.path().to_path_buf();
    // The public relay is an optional feature. Only attempt to load — and
    // only warn on failure — when the sidecar registry file actually
    // exists; an absent file is the expected "not configured" state, not an
    // error, and shouldn't spam startup logs on every unconfigured install.
    // Matches the same `path().exists()` gate already used in
    // `cli/doctor.rs::load_optional_public_relay_manager`.
    if public_relay_store.path().exists() {
        match crate::oauth::public_relay::PublicRelayRegistryManager::load(public_relay_store).await
        {
            Ok(manager) => {
                tracing::info!(
                    subsystem = "startup",
                    phase = "oauth.public_relay.loaded",
                    registry_path = %manager.store().path().display(),
                    machine_count = manager.count().await,
                    "public oauth callback relay registry loaded"
                );
                let manager = Arc::new(manager);
                crate::oauth::public_relay::install_public_relay_manager(Arc::clone(&manager));
                state = state.with_public_relay_manager(manager);
            }
            Err(error) => {
                crate::oauth::public_relay::set_public_relay_manager(None);
                tracing::warn!(
                    subsystem = "startup",
                    phase = "oauth.public_relay.disabled",
                    registry_path = %public_relay_registry_path.display(),
                    kind = error.kind(),
                    error = %error,
                    "public oauth callback relay disabled because registry failed to load"
                );
            }
        }
    } else {
        crate::oauth::public_relay::set_public_relay_manager(None);
    }
    if credential_auth_configured {
        match crate::observability::activity::ActorKeyDeriver::load_or_create() {
            Ok(deriver) => {
                state = state.with_actor_key_deriver(deriver);
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "actor_key derivation disabled because actor-key secret could not be loaded"
                );
            }
        }
    }
    #[cfg(feature = "gateway")]
    {
        state = state.with_gateway_manager(Arc::clone(&gateway_manager));
    }
    state = state.with_auth_config(auth_config);
    if let Some(verifier) = trusted_host_verifier {
        state = state.with_trusted_host_verifier(verifier);
    }
    let web_ui_auth_disabled = if peer_auth_enabled || integrated_trusted_host {
        false
    } else {
        resolve_web_ui_auth_disabled(
            &config.web,
            web_assets_dir.is_some() || embedded_web_assets_enabled,
            oauth_enabled,
        )?
    };
    state = state.with_web_ui_auth_disabled(web_ui_auth_disabled);

    // lab-bg3e.3 Q5 reframe: prominent startup banner whenever the web UI
    // is reachable without authentication. v1 ships unsecured by design;
    // operators must understand any local process can write ~/.labby/.env.
    if web_ui_auth_disabled {
        let banner = "==================================================================\n\
                      ⚠  Lab web UI is running WITHOUT authentication.\n\
                      ⚠  Any local process can read or modify your configuration.\n\
                      ⚠  Set up OAuth (LABBY_AUTH_MODE=oauth) to secure the API.\n\
                      ==================================================================";
        eprintln!("\n{}\n", stderr_theme().warn(banner));
        tracing::warn!(
            subsystem = "web_server",
            phase = "startup.banner",
            "lab web UI started without authentication; any local process can write ~/.labby/.env"
        );
    }

    // Wire the configured workspace root into AppState so the fs
    // service serves `fs.list` / `fs.preview` without re-reading config
    // per request. Failure is non-fatal: invalid root keeps fs calls on the
    // structured `workspace_not_configured` path.
    //
    // Guarded by `feature = "fs"` so a build without fs cannot report the
    // service as enabled at startup just because a `[workspace].root` is
    // configured.
    #[cfg(feature = "fs")]
    {
        let workspace_runtime = crate::workspace::WorkspaceRuntimeBuilder::new(
            crate::workspace::WorkspaceRuntimeConfig {
                root: config.workspace.root.clone(),
                home: workspace_runtime_home(),
            },
        )
        .build();
        if let Some(root) = workspace_runtime.workspace_root() {
            tracing::info!(
                subsystem = "startup",
                phase = "fs.workspace_root",
                path = %root.display(),
                "workspace filesystem browser enabled"
            );
            state = state.with_workspace_root(root.to_path_buf());
        } else {
            tracing::warn!(
                subsystem = "startup",
                phase = "fs.workspace_root",
                error = workspace_runtime.workspace_root_error(),
                "workspace.root invalid; fs service disabled"
            );
        }
    }

    if let Some(web_assets_dir) = web_assets_dir {
        tracing::info!(
            subsystem = "web_server",
            phase = "assets.enabled",
            path = %web_assets_dir.display(),
            source = "filesystem",
            cache_policy = "index:no-store, assets:public max-age=31536000 immutable",
            "web assets detected"
        );
        state = state.with_web_assets_dir(web_assets_dir);
    } else if embedded_web_assets_enabled {
        tracing::info!(
            subsystem = "web_server",
            phase = "assets.enabled",
            source = "embedded",
            cache_policy = "index:no-store, assets:public max-age=31536000 immutable",
            "embedded Labby web assets detected"
        );
        state = state.with_embedded_web_assets();
    } else {
        tracing::info!(
            subsystem = "web_server",
            phase = "assets.disabled",
            "no web assets directory found"
        );
    }
    tracing::info!(
        subsystem = "startup",
        phase = "bootstrap.plan",
        api_server_enabled = true,
        web_server_enabled = state.web_assets_enabled(),
        mcp_server_enabled = matches!(transport, Transport::Http | Transport::UnixSocket),
        gateway_client_enabled = cfg!(feature = "gateway") && !config.upstream.is_empty(),
        oauth_upstream_enabled = config
            .upstream
            .iter()
            .any(|upstream| upstream.oauth.is_some()),
        web_ui_auth_disabled = state.web_ui_auth_disabled,
        "startup plan resolved"
    );

    let result = run_http(
        &host,
        port,
        bearer_token,
        state,
        oauth_state,
        &config.mcp,
        &config.api.cors_origins,
        notifier,
        matches!(transport, Transport::Http | Transport::UnixSocket),
        transport,
        unix_listener_config,
        peer_auth_enabled,
    )
    .await;
    file_stash_runtime.shutdown().await;
    result
}

#[cfg(feature = "fs")]
fn workspace_runtime_home() -> Option<PathBuf> {
    workspace_runtime_home_from_env_values(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
    )
}

#[cfg(feature = "fs")]
fn workspace_runtime_home_from_env_values(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    home.filter(|value| !value.is_empty())
        .or_else(|| userprofile.filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

fn resolve_web_assets_dir(web: &crate::config::WebPreferences) -> Option<PathBuf> {
    let from_env = std::env::var("LABBY_WEB_ASSETS_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let from_config = web.assets_dir.clone();

    [from_env, from_config]
        .into_iter()
        .flatten()
        .find(|path| path.join("index.html").is_file())
}

fn resolve_web_ui_auth_disabled(
    web: &crate::config::WebPreferences,
    web_assets_enabled: bool,
    oauth_enabled: bool,
) -> Result<bool> {
    if let Some(setting) = crate::config::resolve_web_ui_auth_disabled_env()? {
        if setting.legacy_alias {
            tracing::warn!(
                env_var = setting.source,
                canonical_env_var = crate::config::WEB_UI_AUTH_DISABLED_ENV,
                "legacy web UI auth-disable env var used; prefer canonical env var"
            );
        }
        return Ok(setting.disabled);
    }

    if let Some(disabled) = web.disable_auth {
        return Ok(disabled);
    }

    // This is `true` for the default bearer-only (no OAuth), embedded-web-UI
    // deployment shape — e.g. the Unraid plugin's rc.labby-started `labby
    // serve` before OAuth is set up. Since GET /auth/session is registered
    // unconditionally (api/router.rs), that default makes auth_session()
    // return a synthetic authenticated-admin session to unauthenticated
    // callers reaching the HTTP port. No real /v1/* access is granted
    // (gated separately by needs_auth), but it renders a misleading
    // "logged in" UI shell. Tracked in lab-0bl3m; not changed here.
    Ok(web_assets_enabled && !oauth_enabled)
}

#[cfg(unix)]
fn resolve_unix_listener_config(
    transport: Transport,
    preferences: &crate::config::McpPreferences,
) -> Result<Option<HostedUnixConfig>> {
    if !matches!(transport, Transport::UnixSocket) {
        return Ok(None);
    }
    unix_listener::resolve_config(preferences, &|key| std::env::var(key).ok()).map(Some)
}

#[cfg(not(unix))]
fn resolve_unix_listener_config(
    transport: Transport,
    _preferences: &crate::config::McpPreferences,
) -> Result<Option<HostedUnixConfig>> {
    if matches!(transport, Transport::UnixSocket) {
        anyhow::bail!("unix_socket transport is unsupported on this platform");
    }
    Ok(None)
}

#[cfg(unix)]
fn unix_peer_auth_enabled(config: Option<&HostedUnixConfig>) -> bool {
    config.is_some_and(|config| config.peer_policy.enabled())
}

#[cfg(not(unix))]
fn unix_peer_auth_enabled(_config: Option<&HostedUnixConfig>) -> bool {
    false
}

#[cfg(unix)]
fn resolve_trusted_host_verifier(
    transport: Transport,
    listener: Option<&HostedUnixConfig>,
    peer_auth_enabled: bool,
) -> Result<Option<Arc<labby_auth::trusted_host::TrustedHostVerifier>>> {
    let enabled = std::env::var("LABBY_INTEGRATED_TRUSTED_HOST")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE"));
    if !enabled {
        return Ok(None);
    }
    if !matches!(transport, Transport::UnixSocket) || !peer_auth_enabled {
        anyhow::bail!(
            "integrated trusted-host mode requires Unix socket transport with a peer UID or GID policy"
        );
    }
    let listener =
        listener.ok_or_else(|| anyhow::anyhow!("integrated trusted-host listener is missing"))?;
    if listener.abstract_socket() {
        anyhow::bail!(
            "integrated trusted-host mode requires a pathname Unix socket, not an abstract socket"
        );
    }
    let key_id = std::env::var("LABBY_TRUSTED_HOST_CURRENT_KEY_ID")
        .context("integrated trusted-host mode requires LABBY_TRUSTED_HOST_CURRENT_KEY_ID")?;
    let public_key = std::env::var("LABBY_TRUSTED_HOST_CURRENT_PUBLIC_KEY")
        .context("integrated trusted-host mode requires LABBY_TRUSTED_HOST_CURRENT_PUBLIC_KEY")?;
    let generation = std::env::var("LABBY_TRUSTED_HOST_AUTHORITY_GENERATION")
        .context("integrated trusted-host mode requires LABBY_TRUSTED_HOST_AUTHORITY_GENERATION")?
        .parse::<u64>()
        .context("LABBY_TRUSTED_HOST_AUTHORITY_GENERATION must be an unsigned integer")?;
    let key = labby_auth::trusted_host::TrustedHostKey::from_base64url(key_id, &public_key)
        .map_err(|_| {
            anyhow::anyhow!(
                "LABBY_TRUSTED_HOST_CURRENT_PUBLIC_KEY is not a valid Ed25519 base64url key"
            )
        })?;
    let previous_key_id = std::env::var("LABBY_TRUSTED_HOST_PREVIOUS_KEY_ID").ok();
    let previous_public_key = std::env::var("LABBY_TRUSTED_HOST_PREVIOUS_PUBLIC_KEY").ok();
    let previous = match (previous_key_id, previous_public_key) {
        (None, None) => None,
        (Some(previous_key_id), Some(public_key)) => {
            if previous_key_id == key.key_id {
                anyhow::bail!("integrated trusted-host current and previous key IDs must differ");
            }
            Some(
                labby_auth::trusted_host::TrustedHostKey::from_base64url(
                    previous_key_id,
                    &public_key,
                )
                .map_err(|_| {
                    anyhow::anyhow!(
                        "LABBY_TRUSTED_HOST_PREVIOUS_PUBLIC_KEY is not a valid Ed25519 base64url key"
                    )
                })?,
            )
        }
        _ => anyhow::bail!(
            "integrated trusted-host key overlap requires both LABBY_TRUSTED_HOST_PREVIOUS_KEY_ID and LABBY_TRUSTED_HOST_PREVIOUS_PUBLIC_KEY"
        ),
    };
    let keys = std::iter::once(key).chain(previous);
    Ok(Some(Arc::new(
        labby_auth::trusted_host::TrustedHostVerifier::new(generation, keys),
    )))
}

#[cfg(not(unix))]
fn resolve_trusted_host_verifier(
    _transport: Transport,
    _listener: Option<&HostedUnixConfig>,
    _peer_auth_enabled: bool,
) -> Result<Option<Arc<labby_auth::trusted_host::TrustedHostVerifier>>> {
    if std::env::var_os("LABBY_INTEGRATED_TRUSTED_HOST").is_some() {
        anyhow::bail!("integrated trusted-host mode requires Unix socket support");
    }
    Ok(None)
}

fn should_run_stdio(transport: Transport, command: Option<&ServeCommand>) -> bool {
    matches!(transport, Transport::Stdio)
        || matches!(command, Some(ServeCommand::Mcp(McpArgs { stdio: true })))
}

fn resolve_transport(
    cli: Option<Transport>,
    command: Option<&ServeCommand>,
    env: Option<String>,
    config: Option<&str>,
) -> Result<Transport> {
    if let Some(ServeCommand::Mcp(args)) = command {
        if !args.stdio {
            anyhow::bail!("`labby serve mcp` requires `--stdio`");
        }
        return Ok(Transport::Stdio);
    }
    if let Some(transport) = cli {
        return Ok(transport);
    }
    if let Some(value) = env {
        return Transport::from_str(&value, true)
            .map_err(|err| anyhow::anyhow!("invalid LABBY_MCP_TRANSPORT value `{value}`: {err}"));
    }
    if let Some(value) = config {
        return Transport::from_str(value, true)
            .map_err(|err| anyhow::anyhow!("invalid mcp.transport value `{value}`: {err}"));
    }
    Ok(Transport::Http)
}

fn resolve_port(cli: Option<u16>, env: Option<String>, config: Option<u16>) -> Result<u16> {
    if let Some(port) = cli {
        return Ok(port);
    }
    if let Some(value) = env {
        return value
            .parse::<u16>()
            .with_context(|| format!("invalid LABBY_MCP_HTTP_PORT value `{value}`"));
    }
    Ok(config.unwrap_or(8765))
}

/// Return the bearer token if configured, or `None` for auth-free operation.
fn http_token() -> Option<String> {
    std::env::var("LABBY_MCP_HTTP_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
}

/// Check whether a host string refers to a loopback address.
///
/// Handles both bare and bracketed IPv6 (e.g. `::1` and `[::1]`).
fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(normalized, "127.0.0.1" | "::1" | "localhost")
}

fn bind_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn filter_registry(registry: ToolRegistry, services: &[String]) -> Result<ToolRegistry> {
    if services.is_empty() {
        return Ok(registry);
    }
    let valid: Vec<&str> = registry.services().iter().map(|e| e.name).collect();
    let unknown: Vec<&str> = services
        .iter()
        .filter(|s| !valid.contains(&s.as_str()))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        anyhow::bail!(
            "unknown service(s): {}. Valid services: {}",
            unknown.join(", "),
            valid.join(", ")
        );
    }
    let mut out = ToolRegistry::new();
    for entry in registry.services() {
        if services.iter().any(|s| s == entry.name) {
            out.register(entry.clone());
        }
    }
    Ok(out)
}

async fn run_http(
    host: &str,
    port: u16,
    bearer_token: Option<String>,
    state: AppState,
    auth_state: Option<labby_auth::state::AuthState>,
    mcp_config: &crate::config::McpPreferences,
    config_cors_origins: &[String],
    notifier: PeerNotifier,
    mount_http_mcp: bool,
    transport: Transport,
    unix_listener_config: Option<HostedUnixConfig>,
    peer_auth_enabled: bool,
) -> Result<ExitCode> {
    #[cfg(feature = "gateway")]
    let code_mode_shutdown = state.gateway_manager.clone();
    // ── Single-master lock ────────────────────────────────────────────────────
    // Only one HTTP master instance may run per device at a time. Exits
    // immediately with a clear error if the lock is already held by another
    // process. This guard is NOT applied in stdio/MCP-only mode — `labby serve
    // mcp --stdio` may run freely alongside a running master.
    let _master_lock: std::fs::File = {
        // Keep explicitly isolated installations isolated all the way through
        // daemon ownership. This previously ignored LABBY_HOME and contended
        // with the operator's primary Labby instance even when every other
        // path had been redirected to a preview root.
        let lock_dir = std::env::var_os("LABBY_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/state/labby");
        std::fs::create_dir_all(&lock_dir)
            .with_context(|| format!("create master lock dir {}", lock_dir.display()))?;
        let lock_path = lock_dir.join("master.lock");
        let mut lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open master lock file {}", lock_path.display()))?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                eprintln!(
                    "{}",
                    stderr_theme().error(&format!(
                        "lab: another master instance is already running on this device \
                         (lock: {}). Use 'labby mcp' for node/MCP-only mode.",
                        lock_path.display()
                    ))
                );
                std::process::exit(1);
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(anyhow::Error::new(e)
                    .context(format!("acquire master lock ({})", lock_path.display())));
            }
        }
        let pid = std::process::id();
        use std::io::Write as _;
        let _write = writeln!(lock_file, "{pid}");
        tracing::info!(pid, lock_path = %lock_path.display(), "acquired master lock");
        lock_file // held alive until run_http returns — lock released on drop
    };
    // ── end single-master lock ────────────────────────────────────────────────

    let web_assets_enabled = state.web_assets_enabled();
    let bearer_token_configured = bearer_token.is_some();
    let resource_registry = auth_state
        .as_ref()
        .map(labby_auth::state::AuthState::resource_registry);
    tracing::info!(
        subsystem = "api_server",
        phase = "router.build.start",
        bind_host = %host,
        bind_port = port,
        cors_origin_count = config_cors_origins.len(),
        http_mcp_enabled = mount_http_mcp,
        web_ui_auth_disabled = state.web_ui_auth_disabled,
        bearer_token_configured,
        "building http router"
    );
    #[cfg(feature = "gateway")]
    let router = build_http_router(
        state,
        bearer_token,
        auth_state,
        mcp_config,
        config_cors_origins,
        notifier,
        mount_http_mcp,
        peer_auth_enabled,
    )?;
    #[cfg(not(feature = "gateway"))]
    let router = build_http_router(
        state,
        bearer_token,
        auth_state,
        mcp_config,
        config_cors_origins,
        notifier,
        mount_http_mcp,
        peer_auth_enabled,
    )?;
    tracing::info!(
        subsystem = "api_server",
        phase = "router.build.finish",
        http_mcp_enabled = mount_http_mcp,
        "http router ready"
    );
    let listener_status = HostedListenerStatus {
        web_assets_enabled,
        bearer_token_configured,
        mount_http_mcp,
        #[cfg(unix)]
        peer_auth_enabled,
    };
    let hosted_listener = async move {
        match transport {
            Transport::Http => serve_tcp_listener(host, port, router, listener_status).await,
            Transport::UnixSocket => {
                let unix_config = unix_listener_config.ok_or_else(|| {
                    anyhow::anyhow!("unix_socket transport resolved without listener configuration")
                })?;
                serve_unix_listener(unix_config, router, listener_status).await
            }
            Transport::Stdio => {
                anyhow::bail!("stdio transport reached hosted listener startup unexpectedly")
            }
        }
    };
    let hosted_result = if let Some(registry) = resource_registry {
        tokio::select! {
            result = hosted_listener => result,
            never = prune_resource_leases(registry) => match never {},
        }
    } else {
        hosted_listener.await
    };
    #[cfg(feature = "gateway")]
    if let Some(manager) = code_mode_shutdown {
        manager.shutdown_code_mode_runner_pool().await;
    }
    hosted_result?;
    Ok(ExitCode::SUCCESS)
}

async fn prune_resource_leases(
    registry: labby_auth::resource_registry::ResourceRegistry,
) -> std::convert::Infallible {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let pruned = registry.prune_expired_resource_leases(std::time::SystemTime::now());
        if pruned > 0 {
            tracing::debug!(
                resource_lease_count = registry.lease_count(),
                pruned_resource_lease_count = pruned,
                "expired OAuth resource leases pruned"
            );
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HostedListenerStatus {
    web_assets_enabled: bool,
    bearer_token_configured: bool,
    mount_http_mcp: bool,
    #[cfg(unix)]
    peer_auth_enabled: bool,
}

fn notify_systemd_ready(service: &'static str) {
    #[cfg(all(feature = "systemd", unix))]
    {
        if std::env::var_os("NOTIFY_SOCKET").is_some() {
            if let Err(error) = sd_notify::notify(&[sd_notify::NotifyState::Ready]) {
                tracing::warn!(
                    surface = "api",
                    service,
                    action = "sd_notify.error",
                    error = %error,
                    "sd_notify failed"
                );
            } else {
                tracing::info!(
                    surface = "api",
                    service,
                    action = "sd_notify.ready",
                    "systemd READY=1 sent"
                );
            }
        }
    }
    #[cfg(not(all(feature = "systemd", unix)))]
    let _ = service;
}

#[cfg(unix)]
async fn wait_for_reload_signals(transport: &'static str) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigusr1 =
        signal(SignalKind::user_defined1()).context("failed to register SIGUSR1 handler")?;
    loop {
        sigusr1.recv().await;
        tracing::info!(
            surface = "api",
            service = transport,
            action = "config.reload",
            "SIGUSR1 received; config reload triggered",
        );
        // Future: re-read config.toml and apply diffs here.
    }
}

#[cfg(unix)]
async fn wait_for_shutdown_signal(transport: &'static str) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm =
        signal(SignalKind::terminate()).context("failed to register SIGTERM handler")?;
    tokio::select! {
        _ = sigterm.recv() => {}
        result = tokio::signal::ctrl_c() => {
            result.context("failed to register Ctrl-C handler")?;
        }
    }
    tracing::info!(
        surface = "api",
        service = transport,
        action = "shutdown.signal",
        "shutdown signal received; stopping hosted listener"
    );
    Ok(())
}

async fn serve_tcp_listener(
    host: &str,
    port: u16,
    router: axum::Router,
    status: HostedListenerStatus,
) -> Result<()> {
    let HostedListenerStatus {
        web_assets_enabled,
        bearer_token_configured,
        mount_http_mcp,
        ..
    } = status;
    // Parse and validate the address at bind time, not at CLI parse time.
    let addr = bind_addr(host, port);
    tracing::info!(
        subsystem = "api_server",
        phase = "listener.bind.start",
        addr,
        transport = "http",
        "binding HTTP listener"
    );
    let listener = bind_or_reclaim(&addr, port).await?;
    notify_systemd_ready("http");
    tracing::info!(
        subsystem = "api_server",
        phase = "ready",
        addr,
        transport = "http",
        pid = std::process::id(),
        route = "/v1,/health,/ready",
        bearer_token_configured,
        "api server ready"
    );
    tracing::info!(
        subsystem = "web_server",
        phase = if web_assets_enabled {
            "ready"
        } else {
            "disabled"
        },
        addr,
        transport = "http",
        pid = std::process::id(),
        route = "/",
    );
    tracing::info!(
        subsystem = "mcp_server",
        phase = if mount_http_mcp { "ready" } else { "disabled" },
        addr,
        pid = std::process::id(),
        route = "/mcp",
        transport = "http",
    );
    tracing::info!(
        subsystem = "startup",
        phase = "ready",
        addr,
        transport = "http",
        pid = std::process::id(),
        web_server_enabled = web_assets_enabled,
        mcp_server_enabled = mount_http_mcp,
        "labby serve ready"
    );

    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    #[cfg(unix)]
    tokio::select! {
        result = axum::serve(listener, service) => { result?; }
        result = wait_for_reload_signals("http") => { result?; }
        result = wait_for_shutdown_signal("http") => { result?; }
    }
    #[cfg(not(unix))]
    axum::serve(listener, service).await?;
    Ok(())
}

#[cfg(unix)]
async fn serve_unix_listener(
    config: HostedUnixConfig,
    mut router: axum::Router,
    status: HostedListenerStatus,
) -> Result<()> {
    let HostedListenerStatus {
        web_assets_enabled,
        bearer_token_configured,
        mount_http_mcp,
        peer_auth_enabled,
    } = status;
    let socket_kind = if config.abstract_socket() {
        "abstract"
    } else {
        "filesystem"
    };
    tracing::info!(
        subsystem = "api_server",
        phase = "listener.bind.start",
        transport = "unix_socket",
        socket_kind,
        socket_mode = ?config.mode(),
        socket_uid = ?config.owner_uid(),
        socket_gid = ?config.owner_gid(),
        peer_auth_enabled,
        "binding Unix-domain listener"
    );
    let listener = unix_listener::bind(&config).await?;

    router = router.layer(axum::Extension(unix_listener::loopback_connect_info()));
    if peer_auth_enabled {
        router = router.layer(axum::middleware::from_fn(unix_listener::inject_peer_auth));
    }

    notify_systemd_ready("unix_socket");
    tracing::info!(
        subsystem = "api_server",
        phase = "ready",
        transport = "unix_socket",
        socket_kind,
        pid = std::process::id(),
        route = "/v1,/health,/ready",
        bearer_token_configured,
        peer_auth_enabled,
        "api server ready"
    );
    tracing::info!(
        subsystem = "web_server",
        phase = if web_assets_enabled {
            "ready"
        } else {
            "disabled"
        },
        transport = "unix_socket",
        socket_kind,
        pid = std::process::id(),
        route = "/",
    );
    tracing::info!(
        subsystem = "mcp_server",
        phase = if mount_http_mcp { "ready" } else { "disabled" },
        transport = "unix_socket",
        socket_kind,
        pid = std::process::id(),
        route = "/mcp",
    );
    tracing::info!(
        subsystem = "startup",
        phase = "ready",
        transport = "unix_socket",
        socket_kind,
        pid = std::process::id(),
        web_server_enabled = web_assets_enabled,
        mcp_server_enabled = mount_http_mcp,
        "labby serve ready"
    );

    let service = router.into_make_service_with_connect_info::<unix_listener::UnixConnectInfo>();
    tokio::select! {
        result = axum::serve(listener, service) => { result?; }
        result = wait_for_reload_signals("unix_socket") => { result?; }
        result = wait_for_shutdown_signal("unix_socket") => { result?; }
    }
    Ok(())
}

#[cfg(not(unix))]
async fn serve_unix_listener(
    _config: HostedUnixConfig,
    _router: axum::Router,
    _status: HostedListenerStatus,
) -> Result<()> {
    anyhow::bail!("unix_socket transport is unsupported on this platform")
}

async fn log_mcp_request(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().to_string();
    // Queries and session ids are opaque caller-controlled values. They can
    // contain credentials, so request observability records only safe shape
    // metadata rather than their raw contents.
    let path = req.uri().path().to_string();
    let query_present = req.uri().query().is_some();
    let mcp_session_present = req.headers().contains_key("mcp-session-id");
    let authorization_present = req
        .headers()
        .contains_key(axum::http::header::AUTHORIZATION);
    let user_agent = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>")
        .to_string();
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<none>")
        .to_string();

    tracing::info!(
        surface = "mcp",
        subsystem = "mcp_server",
        action = "http_request",
        method = %method,
        path = %path,
        query_present,
        mcp_session_present,
        authorization_present,
        user_agent = %user_agent,
        origin = %origin,
        "incoming MCP HTTP request"
    );

    next.run(req).await
}

fn build_http_router(
    state: AppState,
    bearer_token: Option<String>,
    auth_state: Option<labby_auth::state::AuthState>,
    mcp_config: &crate::config::McpPreferences,
    config_cors_origins: &[String],
    notifier: PeerNotifier,
    mount_http_mcp: bool,
    external_auth_configured: bool,
) -> Result<axum::Router> {
    let mcp_router = if mount_http_mcp {
        // Build the MCP streamable HTTP service in the serve path (not in the
        // router module) to avoid an api->mcp dependency.
        let mcp_service = build_mcp_service(&state, mcp_config, notifier.clone())?;
        Some(
            axum::Router::new()
                .nest_service("/mcp", mcp_service)
                .layer(axum::middleware::from_fn(log_mcp_request)),
        )
    } else {
        None
    };
    #[cfg(feature = "gateway")]
    let protected_mcp_routers = build_protected_mcp_routers(&state, mcp_config, notifier)?;
    #[cfg(not(feature = "gateway"))]
    let protected_mcp_routers: Option<std::collections::HashMap<String, axum::Router>> = None;
    let state = if let Some(routers) = protected_mcp_routers {
        state.with_protected_mcp_routers(routers)
    } else {
        state
    };

    Ok(crate::api::router::build_router_with_external_auth(
        state,
        bearer_token,
        auth_state,
        mcp_router,
        config_cors_origins,
        external_auth_configured,
    ))
}

#[cfg(feature = "gateway")]
async fn build_gateway_runtime(
    config: &LabConfig,
    auth_config: &labby_auth::config::AuthConfig,
    transport: Transport,
    spawn_depth: Option<u32>,
    suppress_upstream_runtime: bool,
    registry: ToolRegistry,
    notifier: PeerNotifier,
    resource_registry: Option<labby_auth::resource_registry::ResourceRegistry>,
    integrated_trusted_host: bool,
) -> Result<Arc<GatewayManager>> {
    let gateway_runtime = GatewayRuntimeHandle::default();
    let upstream_oauth_runtime = if suppress_upstream_runtime {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            transport = ?transport,
            spawn_depth,
            "upstream oauth runtime skipped because stdio recursion guard is active"
        );
        None
    } else {
        let upstream_oauth_key = std::env::var("LABBY_OAUTH_ENCRYPTION_KEY").ok();
        if matches!(transport, Transport::Stdio) {
            crate::oauth::upstream_stdio::build_stdio_upstream_oauth_runtime(
                &config.upstream,
                auth_config,
                upstream_oauth_key.as_deref(),
            )
            .await?
        } else {
            crate::oauth::upstream::runtime::build_upstream_oauth_runtime(
                &config.upstream,
                auth_config,
                upstream_oauth_key.as_deref(),
            )
            .await?
        }
    };
    tracing::info!(
        subsystem = "gateway_client",
        phase = "discovery.lazy.start",
        upstream_count = config.upstream.len(),
        oauth_upstream_count = config
            .upstream
            .iter()
            .filter(|upstream| upstream.oauth.is_some())
            .count(),
        "preparing lazy upstream gateway catalog"
    );
    crate::config::set_process_code_mode_enabled(config.code_mode.enabled);
    let usage_store = if crate::config::usage_telemetry_enabled() {
        match labby_gateway::usage::UsageStore::open(crate::config::usage_db_path()?).await {
            Ok(store) => Some(Arc::new(store)),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to open gateway usage store; usage telemetry disabled for this run"
                );
                None
            }
        }
    } else {
        None
    };
    let step_journal = if crate::config::codemode_journal_enabled() {
        match labby_gateway::codemode_journal::StepJournalStore::open(
            crate::config::codemode_journal_db_path()?,
        )
        .await
        {
            Ok(store) => Some(Arc::new(store)),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to open Code Mode step journal; journaling disabled for this run"
                );
                None
            }
        }
    } else {
        None
    };
    let mut pool_builder = crate::dispatch::upstream::pool::UpstreamPool::new()
        .with_request_timeout(config.upstream_request_timeout())
        .with_relay_timeout(config.upstream_relay_timeout())
        .with_in_process_connector(crate::composition::in_process_connector())
        .with_usage_store(usage_store.clone());
    if let Some(rt) = &upstream_oauth_runtime {
        pool_builder = pool_builder.with_oauth_client_cache(rt.cache.clone());
    }
    let pool = Arc::new(pool_builder);
    if !suppress_upstream_runtime {
        pool.seed_lazy_upstreams(&config.upstream).await;
        tracing::info!(
            subsystem = "gateway_client",
            phase = "discovery.lazy",
            upstream_count = config.upstream.len(),
            seeded_upstream_count = pool.upstream_count().await,
            "upstream gateway discovery deferred until first use"
        );
        gateway_runtime.swap(Some(pool)).await;
    } else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "discovery.skipped",
            spawn_depth,
            "upstream discovery skipped because stdio recursion guard is active"
        );
    }
    let (notify_tx, notify_rx) = mpsc::unbounded_channel();
    let client_registry = notifier.client_registry.clone();
    let _upstream_notifier_task = tokio::spawn(
        notifier
            .clone()
            .run_upstream_notifications(gateway_runtime.clone()),
    );
    let _catalog_notifier_task = tokio::spawn(notifier.clone().run(notify_rx));
    let config_path = config_toml_path()?;
    let live_config = Arc::new(std::sync::RwLock::new(config.clone()));
    let store: Arc<dyn labby_gateway::gateway::config_store::GatewayConfigStore> = Arc::new(
        LabConfigStore::new(Arc::clone(&live_config), config_path.clone())
            .with_service_clients(SharedServiceClients::from_env()),
    );
    let registry: Arc<dyn labby_gateway::gateway::service_registry::GatewayServiceRegistry> =
        Arc::new(registry);
    let gateway_manager = GatewayManager::from_config(
        GatewayManagerConfig {
            config_path,
            store,
            registry,
            in_process_connector: Some(crate::composition::in_process_connector()),
            oauth: upstream_oauth_runtime.map(|rt| GatewayOauthConfig {
                managers: rt.managers,
                cache: rt.cache,
                sqlite: rt.sqlite,
                key: rt.key,
                redirect_uri: rt.redirect_uri,
            }),
            resource_registry,
            usage_store: usage_store.clone(),
            code_mode_app_state: notifier.code_mode_app_state.clone(),
            execution_capability_provider: Some(
                crate::dispatch::execution_catalog::CanonicalExecutionCatalogProvider::production()
                    .context("open canonical ExecutionLoadout catalogs")?,
            ),
        },
        gateway_runtime,
    )?;

    // Code Mode `openapi` provider: config-parse errors DO fail boot (bad TOML),
    // but spec-LOAD failures never do — `OpenApiRegistry::load` degrades + WARNs
    // per spec. Build the hardened dispatch client + the registry (concurrent,
    // 8s per-spec timeout) and inject both into the gateway host.
    let openapi_provider_config =
        crate::config::load_openapi_provider_config(&config.openapi, &|k| std::env::var(k).ok())?;
    let openapi_http_client = labby_openapi::http::build_dispatch_client()?;
    let openapi_registry =
        labby_openapi::OpenApiRegistry::load(openapi_provider_config, Duration::from_secs(8)).await;
    if openapi_registry.is_empty() {
        tracing::info!(
            service = "openapi",
            "openapi code-mode provider: no specs configured/loaded"
        );
    } else {
        tracing::info!(
            service = "openapi",
            specs = ?openapi_registry.labels(),
            "openapi code-mode provider ready"
        );
    }
    let mut gateway_manager = gateway_manager
        .with_openapi(openapi_registry, openapi_http_client)
        .with_client_registry(client_registry);
    if integrated_trusted_host {
        let socket_path = std::env::var("LABBY_CORE_PROVIDER_SOCKET_PATH")
            .context("integrated trusted-host mode requires LABBY_CORE_PROVIDER_SOCKET_PATH")?;
        let provider = labby_gateway::core_provider::CoreProviderClient::new(socket_path)
            .map_err(|error| anyhow::anyhow!(error))?;
        gateway_manager = gateway_manager.with_core_provider_client(provider);
    }
    if let Some(store) = step_journal.clone() {
        gateway_manager = gateway_manager.with_step_journal(store);
    }

    gateway_manager.set_notifier(CatalogChangeNotifier::new(notify_tx));
    let gateway_manager = Arc::new(gateway_manager);
    // Seed config for both transports so MCP catalog visibility and code-mode
    // settings match the persisted config. Normal stdio follows the same gateway
    // runtime path as HTTP; only recursive stdio children suppress upstream
    // spawning.
    gateway_manager
        .try_seed_config(config.to_gateway_config())
        .await
        .context("loaded gateway config failed validation")?;
    install_gateway_manager(Arc::clone(&gateway_manager));
    if !suppress_upstream_runtime {
        match config.gateway_import_mode {
            crate::config::GatewayImportMode::Off => {
                tracing::info!(
                    subsystem = "gateway_client",
                    phase = "auto_import.skipped",
                    reason = "gateway_import_mode=off",
                    "external MCP config auto-import disabled"
                );
            }
            crate::config::GatewayImportMode::Pending => {
                match gateway_manager.discover_into_pending().await {
                    Ok(result) => {
                        tracing::info!(
                            subsystem = "gateway_client",
                            phase = "auto_import.pending",
                            queued = result.queued,
                            skipped = result.skipped,
                            "discovered servers queued for approval"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            subsystem = "gateway_client",
                            phase = "auto_import.pending_failed",
                            error = %error,
                            "pending-mode discovery failed"
                        );
                    }
                }
            }
            crate::config::GatewayImportMode::Auto => {
                match gateway_manager.auto_import_discovered_configs().await {
                    Ok(result) => {
                        tracing::info!(
                            subsystem = "gateway_client",
                            phase = "auto_import.finish",
                            imported = result.imported.len(),
                            skipped = result.skipped.len(),
                            errors = result.errors.len(),
                            "external MCP configs auto-imported"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            subsystem = "gateway_client",
                            phase = "auto_import.failed",
                            error = %error,
                            "external MCP config auto-import failed"
                        );
                    }
                }
            }
        }
        tracing::info!(
            subsystem = "gateway_client",
            phase = "manager.ready",
            transport = ?transport,
            upstream_count = gateway_manager.current_config().await.upstream.len(),
            "gateway manager installed"
        );
    } else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "manager.ready",
            transport = ?transport,
            spawn_depth,
            upstream_count = gateway_manager.current_config().await.upstream.len(),
            "gateway manager installed with upstream spawning suppressed"
        );
    }
    // Retention/cadence policy lives in `UsageStore::spawn_prune_loop`; this
    // is just configuration wiring for that one call.
    const USAGE_PRUNE_INTERVAL: Duration = Duration::from_hours(6);
    const USAGE_RETENTION_SECS: i64 = 30 * 24 * 60 * 60; // 30 days
    if let Some(store) = usage_store.clone() {
        store.spawn_prune_loop(USAGE_RETENTION_SECS, USAGE_PRUNE_INTERVAL);
    }
    // Retention/cadence policy for the durable Code Mode step journal; mirrors
    // the usage-store cadence above.
    const JOURNAL_PRUNE_INTERVAL: Duration = Duration::from_hours(6);
    const JOURNAL_RETENTION_SECS: i64 = 30 * 24 * 60 * 60; // 30 days
    if let Some(store) = step_journal.clone() {
        store.spawn_prune_loop(JOURNAL_RETENTION_SECS, JOURNAL_PRUNE_INTERVAL);
    }
    Ok(gateway_manager)
}

#[cfg(not(feature = "gateway"))]
fn reject_protected_routes_without_gateway(config: &LabConfig) -> Result<()> {
    if !config.protected_mcp_routes.is_empty() {
        anyhow::bail!(
            "protected MCP routes are configured but this labby build does not include the gateway feature"
        );
    }
    // Configured upstreams are harmless without the gateway client, but the
    // operator should know they're being ignored rather than silently dropped.
    if !config.upstream.is_empty() {
        tracing::warn!(
            subsystem = "startup",
            phase = "bootstrap.plan",
            upstream_count = config.upstream.len(),
            "gateway upstreams are configured but this build has no gateway support (gateway feature); values ignored"
        );
    }
    Ok(())
}

/// Run as a pure stdio<->streamable-HTTP bridge to an already-detected live
/// daemon. No `GatewayManager`, no upstream pool, no local OAuth state --
/// every request is forwarded to `live` and its response piped straight
/// back. See `crate::mcp::bridge` for what is and isn't forwarded.
#[cfg(feature = "gateway")]
async fn run_stdio_bridge(live: crate::live_gateway::LiveGateway) -> Result<ExitCode> {
    use crate::mcp::bridge::{BridgeClientHandler, BridgeServerHandler};

    let client_handler = BridgeClientHandler::new();
    let service = live
        .connect_service_bounded(client_handler)
        .await
        .context("connect to live labby serve daemon for stdio bridging")?;
    let handler = BridgeServerHandler::new(service);
    let running = handler.serve(rmcp::transport::stdio()).await?;
    tracing::info!(
        subsystem = "startup",
        phase = "ready",
        transport = "stdio-bridge",
        "labby serve ready (bridging to live daemon)"
    );
    running.waiting().await?;
    tracing::info!(
        subsystem = "startup",
        phase = "stop",
        transport = "stdio-bridge",
        "labby serve stdio bridge stopped"
    );
    Ok(ExitCode::SUCCESS)
}

fn run_stdio(
    registry: Arc<ToolRegistry>,
    #[cfg(feature = "gateway")] gateway_manager: Arc<GatewayManager>,
    access_runtime: Arc<AccessRuntime>,
    file_stash_runtime: Arc<crate::file_stash::FileStashRuntime>,
    notifier: PeerNotifier,
    spawn_depth: Option<u32>,
    suppress_upstream_runtime: bool,
) -> impl Future<Output = Result<ExitCode>> {
    // The server bootstrap stays on the stack throughout this session. Keep
    // the protocol future on the heap rather than copying it into that frame.
    Box::pin(async move {
        let file_stash_shutdown = Arc::clone(&file_stash_runtime);
        if suppress_upstream_runtime {
            tracing::warn!(
                surface = "mcp",
                service = "stdio",
                action = "recursion_guard.detected",
                subsystem = "mcp_server",
                phase = "stdio.recursion_guard",
                transport = "stdio",
                spawn_depth,
                "LABBY_SPAWN_DEPTH is set for stdio MCP serve; upstream spawning is disabled in this mode"
            );
        } else {
            tracing::info!(
                surface = "mcp",
                service = "stdio",
                action = "recursion_guard.clear",
                subsystem = "mcp_server",
                phase = "stdio.recursion_guard",
                transport = "stdio",
                spawn_depth,
                "stdio MCP recursion guard clear"
            );
        }
        tracing::info!(
            surface = "mcp",
            service = "stdio",
            action = "server.start",
            subsystem = "mcp_server",
            phase = "start",
            transport = "stdio",
            services = registry.services().len(),
            "starting stdio mcp server"
        );
        tracing::info!(
            subsystem = "startup",
            phase = "ready",
            transport = "stdio",
            services = registry.services().len(),
            "labby serve ready"
        );
        let service_count = registry.services().len();
        let server = LabMcpServer {
            registry,
            access_runtime,
            file_stash_runtime,
            #[cfg(feature = "gateway")]
            gateway_manager: Some(Arc::clone(&gateway_manager)),
            peers: Arc::clone(&notifier.peers),
            code_mode_app_state: notifier.code_mode_app_state.clone(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            #[cfg(feature = "gateway")]
            client_registry: notifier.client_registry.clone(),
            transport_label: "stdio",
            logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                crate::mcp::logging::logging_level_rank(crate::mcp::logging::LoggingLevel::Info),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: crate::mcp::server::next_relay_session_id(),
            #[cfg(test)]
            code_mode_widget_callbacks_enabled_for_test: false,
        };
        let running = server.serve(rmcp::transport::stdio()).await;
        let server_result: Result<()> = match running {
            Ok(running) => {
                tracing::info!(
                    surface = "mcp",
                    service = "stdio",
                    action = "server.ready",
                    subsystem = "mcp_server",
                    phase = "ready",
                    transport = "stdio",
                    services = service_count,
                    "stdio mcp server ready"
                );
                running.waiting().await.map(|_| ()).map_err(Into::into)
            }
            Err(error) => Err(error.into()),
        };
        #[cfg(feature = "gateway")]
        gateway_manager.shutdown_code_mode_runner_pool().await;
        file_stash_shutdown.shutdown().await;
        server_result?;
        tracing::info!(
            surface = "mcp",
            service = "stdio",
            action = "server.stop",
            subsystem = "mcp_server",
            phase = "stop",
            transport = "stdio",
            "stdio mcp server stopped"
        );
        Ok(ExitCode::SUCCESS)
    })
}

fn resolve_lab_spawn_depth(env: Option<String>) -> Option<u32> {
    env.as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u32>().ok())
}

fn stdio_recursion_guard_active(stdio_mode: bool, spawn_depth: Option<u32>) -> bool {
    stdio_mode && spawn_depth.unwrap_or_default() > 0
}

/// Build the MCP streamable HTTP service from app state.
///
/// The factory closure clones `Arc<ToolRegistry>` from `AppState` and constructs
/// a new `LabMcpServer`. Construction cost: two Arc increments.
///
/// **Per request, not per session.** This service is configured with
/// `NeverSessionManager` and `legacy_session_mode(false)`, so rmcp invokes the
/// factory on every POST rather than once per connection, and serves each one
/// over a `OneshotTransport` that ends with the response. There is no
/// connection-scoped `LabMcpServer` on this transport, and no `GET`/SSE stream
/// (rmcp routes `GET` only under `legacy_session_mode` or with an event store,
/// so it answers `405`). Cross-request continuity comes from the shared
/// `PeerNotifier` registry, not from the server instance.
///
/// Contrast `run_stdio`, where one instance does serve the whole process.
fn build_mcp_service(
    state: &AppState,
    mcp_config: &crate::config::McpPreferences,
    notifier: PeerNotifier,
) -> Result<StreamableHttpService<LabMcpServer, NeverSessionManager>> {
    build_mcp_service_with_scope(
        state,
        mcp_config,
        notifier,
        crate::mcp::route_scope::McpRouteScope::Root,
        &[],
    )
}

fn build_mcp_service_with_scope(
    state: &AppState,
    mcp_config: &crate::config::McpPreferences,
    notifier: PeerNotifier,
    route_scope: crate::mcp::route_scope::McpRouteScope,
    extra_allowed_hosts: &[String],
) -> Result<StreamableHttpService<LabMcpServer, NeverSessionManager>> {
    let registry = Arc::clone(&state.registry);
    let access_runtime = Arc::clone(&state.access_runtime);
    let file_stash_runtime = Arc::clone(&state.file_stash_runtime);
    #[cfg(feature = "gateway")]
    let gateway_manager = state.gateway_manager.clone();

    let session_manager = Arc::new(NeverSessionManager::default());

    let mut allowed_hosts = allowed_hosts(
        mcp_config.allowed_hosts.as_deref().unwrap_or(&[]),
        state
            .auth_config
            .as_ref()
            .and_then(|cfg| cfg.public_url.as_ref().map(url::Url::as_str)),
    );
    let mut seen_allowed_hosts: std::collections::HashSet<String> =
        allowed_hosts.iter().cloned().collect();
    for host in extra_allowed_hosts {
        if seen_allowed_hosts.insert(host.clone()) {
            allowed_hosts.push(host.clone());
        }
    }
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts.clone())
        .with_legacy_session_mode(false)
        .with_json_response(true);
    tracing::info!(
        surface = "mcp",
        service = "labby",
        action = "server.init",
        subsystem = "mcp_server",
        phase = "http.mount",
        transport = "http",
        protocol_version = "2026-07-28",
        lifecycle = "stateless",
        allowed_host_count = allowed_hosts.len(),
        "http mcp service configured"
    );

    // All HTTP sessions share the same PeerNotifier (and thus the same peers
    // vec) so that gateway reload notifications reach every connected session.
    let shared_peers = Arc::clone(&notifier.peers);
    let shared_code_mode_app_state = notifier.code_mode_app_state.clone();
    let shared_route_runtime: Arc<crate::mcp::runtime::McpRouteRuntime> = Default::default();
    #[cfg(feature = "gateway")]
    let shared_client_registry = notifier.client_registry.clone();
    let route_scope_label = route_scope.label();

    Ok(StreamableHttpService::new(
        move || {
            let reg = Arc::clone(&registry);
            let access_runtime = Arc::clone(&access_runtime);
            let file_stash_runtime = Arc::clone(&file_stash_runtime);
            #[cfg(feature = "gateway")]
            let manager = gateway_manager.clone();
            #[cfg(feature = "gateway")]
            let gateway_manager_configured = manager.is_some();
            #[cfg(not(feature = "gateway"))]
            let gateway_manager_configured = false;
            let peers = Arc::clone(&shared_peers);
            let code_mode_app_state = shared_code_mode_app_state.clone();
            #[cfg(feature = "gateway")]
            let client_registry = shared_client_registry.clone();
            let route_scope = route_scope.clone();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "session.init",
                subsystem = "mcp_server",
                phase = "session.init",
                transport = "http",
                services = reg.services().len(),
                gateway_manager_configured,
                route_scope = %route_scope_label,
                "initializing HTTP MCP session handler"
            );
            Ok(LabMcpServer {
                registry: reg,
                access_runtime,
                file_stash_runtime,
                #[cfg(feature = "gateway")]
                gateway_manager: manager,
                peers,
                code_mode_app_state,
                // Stateless HTTP requests cannot prove that a listen belongs
                // to any earlier tools/list request. This per-request store is
                // therefore intentionally empty; listen catches up
                // conservatively instead of inheriting another conversation.
                last_listed_tool_contract: Default::default(),
                route_runtime: Arc::clone(&shared_route_runtime),
                #[cfg(feature = "gateway")]
                client_registry,
                transport_label: "http",
                logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                    crate::mcp::logging::logging_level_rank(
                        crate::mcp::logging::LoggingLevel::Info,
                    ),
                )),
                route_scope,
                relay_session_id: crate::mcp::server::next_relay_session_id(),
                #[cfg(test)]
                code_mode_widget_callbacks_enabled_for_test: false,
            })
        },
        session_manager,
        config,
    ))
}

#[cfg(feature = "gateway")]
fn build_protected_mcp_routers(
    state: &AppState,
    mcp_config: &crate::config::McpPreferences,
    notifier: PeerNotifier,
) -> Result<Option<std::collections::HashMap<String, axum::Router>>> {
    let routes: Vec<_> = state
        .config
        .protected_mcp_routes
        .iter()
        .filter(|route| route.enabled && route.is_gateway_subset())
        .cloned()
        .collect();
    if routes.is_empty() {
        return Ok(None);
    }

    let mut routers = std::collections::HashMap::with_capacity(routes.len());
    for route in routes {
        let Some(scope) = crate::mcp::route_scope::McpRouteScope::from_protected_route(
            &route,
            &state.config.loadouts,
        )
        .map_err(anyhow::Error::msg)?
        else {
            continue;
        };
        let service = build_mcp_service_with_scope(
            state,
            mcp_config,
            notifier.clone(),
            scope,
            std::slice::from_ref(&route.public_host),
        )?;
        let router = axum::Router::new().nest_service(&route.public_path, service);
        routers.insert(route.name, router);
    }
    Ok(Some(routers))
}

/// Build the allowed hosts list for DNS rebinding protection.
///
/// Reads `LABBY_MCP_ALLOWED_HOSTS` (comma-separated) and the resolved resource
/// URL. Always includes loopback defaults. Rejects wildcard.
fn allowed_hosts(config_allowed_hosts: &[String], resource_url: Option<&str>) -> Vec<String> {
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];
    for h in config_allowed_hosts.iter().map(String::as_str) {
        let h = h.trim();
        if h.is_empty() || h == "*" {
            continue;
        }
        if !hosts.contains(&h.to_string()) {
            hosts.push(h.to_string());
        }
    }
    if let Ok(extra) = std::env::var("LABBY_MCP_ALLOWED_HOSTS") {
        for h in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            // Reject wildcard — would disable Host header validation entirely
            if h == "*" {
                tracing::warn!(
                    "ignoring wildcard '*' in LABBY_MCP_ALLOWED_HOSTS — \
                     would disable DNS rebinding protection"
                );
                continue;
            }
            if !hosts.contains(&h.to_string()) {
                hosts.push(h.to_string());
            }
        }
    }
    if let Some(url_str) = resource_url
        && let Ok(parsed) = url::Url::parse(url_str)
        && let Some(host) = parsed.host_str()
    {
        let h = host.to_string();
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    hosts
}

/// Bind a TCP listener on `addr`. If the port is already in use and the
/// holding process is `lab` (Linux only), send SIGTERM and retry.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
async fn bind_or_reclaim(addr: &str, port: u16) -> Result<tokio::net::TcpListener> {
    use std::io::ErrorKind;
    match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == ErrorKind::AddrInUse => {
            #[cfg(target_os = "linux")]
            {
                if let Some(reclaimed_pid) = reclaim_port_if_lab(addr, port) {
                    for attempt in 1u8..=5 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        match tokio::net::TcpListener::bind(addr).await {
                            Ok(l) => {
                                tracing::info!(
                                    subsystem = "api_server",
                                    phase = "listener.reclaimed",
                                    addr,
                                    attempt,
                                    reclaimed_pid,
                                    current_pid = std::process::id(),
                                    "port reclaimed after killing stale lab process; current serve process will continue startup"
                                );
                                return Ok(l);
                            }
                            Err(e2) if e2.kind() == ErrorKind::AddrInUse => continue,
                            Err(e2) => {
                                return Err(anyhow::Error::from(e2)
                                    .context(format!("failed to bind HTTP listener on `{addr}`")));
                            }
                        }
                    }
                }
            }
            Err(anyhow::Error::from(e).context(format!("failed to bind HTTP listener on `{addr}`")))
        }
        Err(e) => {
            Err(anyhow::Error::from(e).context(format!("failed to bind HTTP listener on `{addr}`")))
        }
    }
}

/// On Linux, find the PID holding `port`, confirm it's a `lab` process, and
/// send SIGTERM. Returns the reclaimed PID if a signal was sent.
#[cfg(target_os = "linux")]
fn reclaim_port_if_lab(addr: &str, port: u16) -> Option<u32> {
    if addr.contains(':') || !matches!(addr, "127.0.0.1" | "localhost") {
        tracing::debug!(
            subsystem = "api_server",
            phase = "listener.reclaim.lookup",
            addr,
            port,
            "port reclaim is scanning both IPv4 and IPv6 listener tables"
        );
    }
    let Some(pid) = find_pid_for_port(port) else {
        return None;
    };
    let Some(exe) = lab_executable_path(pid) else {
        return None;
    };
    let process_name = exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<unknown>");
    if !is_lab_executable(&exe) {
        tracing::warn!(
            subsystem = "api_server",
            phase = "listener.port_conflict",
            port,
            pid,
            process = process_name,
            executable = %exe.display(),
            "port in use by non-lab process — not killing"
        );
        return None;
    }
    tracing::warn!(
        subsystem = "api_server",
        phase = "listener.reclaim",
        port,
        pid,
        process = process_name,
        executable = %exe.display(),
        "port held by stale lab process — sending SIGTERM"
    );
    terminate_sigterm(pid).ok().map(|()| pid)
}

#[cfg(target_os = "linux")]
fn lab_executable_path(pid: u32) -> Option<PathBuf> {
    exe_path(pid)
}

#[cfg(target_os = "linux")]
fn is_lab_executable(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("lab" | "lab (deleted)")
    )
}

/// Walk `/proc/net/tcp` and `/proc/net/tcp6` to find the inode for a listening
/// port, then resolve it to a PID by scanning `/proc/*/fd/`.
#[cfg(target_os = "linux")]
fn find_pid_for_port(port: u16) -> Option<u32> {
    let hex_port = format!("{port:04X}");
    let inode = ["/proc/net/tcp", "/proc/net/tcp6"]
        .into_iter()
        .find_map(|path| find_listening_inode(path, &hex_port))?;

    let target = format!("socket:[{inode}]");
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let pid_str = entry.file_name();
        let Ok(pid) = pid_str.to_string_lossy().parse::<u32>() else {
            continue;
        };
        let fd_dir = format!("/proc/{pid}/fd");
        let Ok(fds) = std::fs::read_dir(&fd_dir) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(link) = std::fs::read_link(fd.path()) {
                if link.to_string_lossy() == target {
                    return Some(pid);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn find_listening_inode(path: &str, hex_port: &str) -> Option<u64> {
    let table = std::fs::read_to_string(path).ok()?;
    table.lines().skip(1).find_map(|line| {
        let cols: Vec<&str> = line.split_whitespace().collect();
        let local = cols.get(1)?;
        let state = cols.get(3)?;
        let inode_col = cols.get(9)?;
        let port_part = local.split(':').nth(1)?;
        if state.eq_ignore_ascii_case("0A") && port_part.eq_ignore_ascii_case(hex_port) {
            inode_col.parse::<u64>().ok()
        } else {
            None
        }
    })
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    #[cfg(feature = "fs")]
    use std::ffi::OsString;
    #[cfg(feature = "fs")]
    use std::path::PathBuf;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use futures::StreamExt;
    use tower::ServiceExt;

    #[cfg(feature = "fs")]
    use super::workspace_runtime_home_from_env_values;
    use super::{
        McpArgs, PeerNotifier, ServeCommand, Transport, allowed_hosts, bind_addr,
        build_http_router, filter_registry, initialize_selected_file_stash_runtime,
        is_loopback_host, resolve_lab_spawn_depth, resolve_port, resolve_transport,
        resolve_web_ui_auth_disabled, should_run_stdio, stdio_recursion_guard_active,
    };
    #[cfg(feature = "skills")]
    use super::{bootstrap_selected_skill_library_with, configure_skill_library_imports};
    use crate::api::AppState;
    use crate::cli::Cli;
    use crate::config::{LabConfig, McpPreferences, WebPreferences};
    use crate::registry::{build_default_registry, filter_built_in_upstream_apis};
    use clap::Parser;

    #[tokio::test]
    async fn excluding_stash_does_not_initialize_its_storage_root() {
        let registry = filter_registry(build_default_registry(), &["doctor".to_owned()])
            .expect("doctor is a registered service");
        let tempdir = tempfile::tempdir().expect("tempdir");
        let stash_root = tempdir.path().join("must-not-be-created");
        let mut config = LabConfig::default();
        config.file_stash.root = Some(stash_root.clone());

        let runtime = initialize_selected_file_stash_runtime(&registry, &config).await;

        assert!(!stash_root.exists());
        assert!(matches!(
            runtime.status().await,
            crate::file_stash::FileStashStatus::Blocked(_)
        ));
    }

    #[test]
    fn transport_resolution_prefers_explicit_stdio_then_cli_then_http_default() {
        let resolved = resolve_transport(
            Some(Transport::Http),
            Some(&ServeCommand::Mcp(McpArgs { stdio: true })),
            Some("http".into()),
            Some("http"),
        )
        .expect("mcp stdio command should win");
        assert!(matches!(resolved, Transport::Stdio));

        let resolved = resolve_transport(
            Some(Transport::Http),
            None,
            Some("stdio".into()),
            Some("stdio"),
        )
        .expect("cli value should win");
        assert!(matches!(resolved, Transport::Http));

        let resolved = resolve_transport(None, None, Some("http".into()), Some("stdio"))
            .expect("env value should win");
        assert!(matches!(resolved, Transport::Http));

        let resolved =
            resolve_transport(None, None, None, Some("stdio")).expect("config value should win");
        assert!(matches!(resolved, Transport::Stdio));

        let resolved =
            resolve_transport(None, None, None, None).expect("http should be the default");
        assert!(matches!(resolved, Transport::Http));

        let resolved = resolve_transport(
            Some(Transport::UnixSocket),
            None,
            Some("http".into()),
            Some("stdio"),
        )
        .expect("explicit Unix socket transport should win");
        assert!(matches!(resolved, Transport::UnixSocket));

        let resolved = resolve_transport(None, None, Some("unix_socket".into()), None)
            .expect("Unix socket env value should parse");
        assert!(matches!(resolved, Transport::UnixSocket));
        assert!(!should_run_stdio(Transport::UnixSocket, None));
    }

    #[test]
    fn port_resolution_prefers_cli_then_env_then_config() {
        assert_eq!(
            resolve_port(Some(9999), Some("8888".into()), Some(7777)).unwrap(),
            9999
        );
        assert_eq!(
            resolve_port(None, Some("8888".into()), Some(7777)).unwrap(),
            8888
        );
        assert_eq!(resolve_port(None, None, Some(7777)).unwrap(), 7777);
        assert_eq!(resolve_port(None, None, None).unwrap(), 8765);
    }

    #[test]
    fn services_allowlist_does_not_reenable_globally_disabled_upstreams() {
        let reg = filter_built_in_upstream_apis(build_default_registry(), false);
        let error = filter_registry(reg, &["gateway-alpha".to_string()])
            .expect_err("disabled gateway_alpha should be unknown to --services");
        assert!(error.to_string().contains("unknown service"));
    }

    #[cfg(feature = "skills")]
    #[test]
    fn excluded_artifacts_service_does_not_run_skill_library_bootstrap() {
        let registry = filter_registry(build_default_registry(), &["doctor".to_owned()]).unwrap();
        let result = bootstrap_selected_skill_library_with(&registry, || -> anyhow::Result<()> {
            panic!("excluded artifacts service must not touch Artifact Library storage")
        })
        .unwrap();
        assert!(result.is_none());
    }

    #[cfg(feature = "skills")]
    #[test]
    fn every_artifact_control_service_runs_skill_library_bootstrap_independently() {
        for service in ["artifacts", "bundles", "jobs", "sources", "uploads"] {
            let registry =
                filter_registry(build_default_registry(), &[service.to_owned()]).unwrap();
            let result = bootstrap_selected_skill_library_with(&registry, || Ok(41_u8)).unwrap();
            assert_eq!(
                result,
                Some(41),
                "{service} must initialize the shared runtime"
            );
        }
    }

    #[cfg(feature = "skills")]
    #[test]
    fn failed_import_construction_can_retry_before_runtime_publication() {
        use crate::config::{ArtifactPreferences, ArtifactSourceConfig, ArtifactSourceKind};

        let root = tempfile::tempdir().unwrap();
        let mut config = LabConfig {
            artifacts: ArtifactPreferences {
                sources: vec![ArtifactSourceConfig {
                    id: "depot".to_owned(),
                    kind: ArtifactSourceKind::Depot,
                    endpoint: "not a url".to_owned(),
                    control_plane_url: None,
                    pinned_addresses: Vec::new(),
                    bearer_token_env: None,
                }],
            },
            ..LabConfig::default()
        };
        assert!(configure_skill_library_imports(&config, root.path()).is_err());

        config.artifacts = ArtifactPreferences::default();
        assert!(configure_skill_library_imports(&config, root.path()).is_ok());
    }

    #[test]
    fn config_defaults_are_available_for_serve_resolution() {
        let cfg = LabConfig {
            mcp: McpPreferences {
                transport: Some("stdio".into()),
                host: Some("0.0.0.0".into()),
                port: Some(9000),
                allowed_hosts: Some(vec!["lab.internal".into()]),
                show_all: None,
                catalog_notification_timeout_ms: None,
                ..McpPreferences::default()
            },
            ..LabConfig::default()
        };
        assert_eq!(cfg.mcp.host.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn web_ui_auth_disabled_resolution_prefers_config_then_default() {
        assert!(
            resolve_web_ui_auth_disabled(
                &WebPreferences {
                    assets_dir: None,
                    disable_auth: Some(true),
                },
                false,
                false
            )
            .unwrap()
        );
        assert!(resolve_web_ui_auth_disabled(&WebPreferences::default(), true, false).unwrap());
        assert!(!resolve_web_ui_auth_disabled(&WebPreferences::default(), true, true).unwrap());
        assert!(!resolve_web_ui_auth_disabled(&WebPreferences::default(), false, false).unwrap());
    }

    #[cfg(feature = "fs")]
    #[test]
    fn workspace_runtime_home_uses_userprofile_when_home_is_absent() {
        let resolved = workspace_runtime_home_from_env_values(
            None,
            Some(OsString::from("/tmp/lab-userprofile")),
        );

        assert_eq!(resolved, Some(PathBuf::from("/tmp/lab-userprofile")));
    }

    #[cfg(feature = "fs")]
    #[test]
    fn workspace_runtime_home_uses_userprofile_when_home_is_empty() {
        let resolved = workspace_runtime_home_from_env_values(
            Some(OsString::from("")),
            Some(OsString::from("/tmp/lab-userprofile")),
        );

        assert_eq!(resolved, Some(PathBuf::from("/tmp/lab-userprofile")));
    }

    #[test]
    fn serve_subcommand_parses_stdio_helper() {
        let cli = Cli::try_parse_from(["lab", "serve", "mcp", "--stdio"])
            .expect("nested stdio helper should parse");

        match cli.command {
            crate::cli::Command::Serve(args) => {
                assert!(args.transport.is_none());
                match args.command {
                    Some(ServeCommand::Mcp(McpArgs { stdio })) => assert!(stdio),
                    other => panic!("unexpected serve subcommand: {other:?}"),
                }
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn stdio_mode_is_selected_from_resolved_transport() {
        assert!(should_run_stdio(Transport::Stdio, None));
        assert!(should_run_stdio(
            Transport::Http,
            Some(&ServeCommand::Mcp(McpArgs { stdio: true })),
        ));
        assert!(!should_run_stdio(Transport::Http, None));
    }

    #[test]
    fn lab_spawn_depth_resolution_tolerates_bad_env() {
        assert_eq!(resolve_lab_spawn_depth(Some("2".into())), Some(2));
        assert_eq!(resolve_lab_spawn_depth(Some(" 3 ".into())), Some(3));
        assert_eq!(resolve_lab_spawn_depth(Some(String::new())), None);
        assert_eq!(resolve_lab_spawn_depth(Some("not-a-number".into())), None);
        assert_eq!(resolve_lab_spawn_depth(None), None);
    }

    #[test]
    fn stdio_recursion_guard_only_suppresses_child_spawns() {
        assert!(!stdio_recursion_guard_active(false, Some(2)));
        assert!(!stdio_recursion_guard_active(true, None));
        assert!(!stdio_recursion_guard_active(true, Some(0)));
        assert!(stdio_recursion_guard_active(true, Some(1)));
    }

    #[test]
    fn loopback_host_detection() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.100"));
        assert!(!is_loopback_host("lab.example.com"));
    }

    #[test]
    fn bind_addr_brackets_bare_ipv6_hosts() {
        assert_eq!(bind_addr("::1", 8765), "[::1]:8765");
        assert_eq!(bind_addr("[::1]", 8765), "[::1]:8765");
        assert_eq!(bind_addr("127.0.0.1", 8765), "127.0.0.1:8765");
    }

    #[test]
    fn allowed_hosts_include_resource_url_host() {
        let hosts = allowed_hosts(&[], Some("https://lab.example.com/mcp"));
        assert!(hosts.contains(&"lab.example.com".to_string()));
    }

    #[test]
    fn allowed_hosts_include_configured_hosts() {
        let hosts = allowed_hosts(&["lab.internal".to_string()], None);
        assert!(hosts.contains(&"lab.internal".to_string()));
    }

    #[tokio::test]
    async fn hosted_http_without_http_mcp_keeps_v1_routes_but_not_mcp() {
        let state = AppState::new();
        let app = build_http_router(
            state,
            None,
            None,
            &McpPreferences::default(),
            &[],
            PeerNotifier::default(),
            false,
            false,
        )
        .expect("router without http mcp");

        let v1_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/setup/actions")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(v1_response.status(), StatusCode::OK);

        let mcp_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(mcp_response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn hosted_http_with_http_mcp_mounts_mcp_route() {
        let state = AppState::new();
        let app = build_http_router(
            state,
            None,
            None,
            &McpPreferences::default(),
            &[],
            PeerNotifier::default(),
            true,
            false,
        )
        .expect("router with http mcp");

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn http_mcp_adapts_every_declared_legacy_initialize_version() {
        let app = build_http_router(
            AppState::new(),
            None,
            None,
            &McpPreferences::default(),
            &[],
            PeerNotifier::default(),
            true,
            false,
        )
        .expect("router with HTTP MCP");
        for version in ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/mcp")
                        .header("host", "localhost")
                        .header("content-type", "application/json")
                        .header("accept", "application/json, text/event-stream")
                        .body(Body::from(
                            serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "method": "initialize",
                                "params": {
                                    "protocolVersion": version,
                                    "capabilities": {},
                                    "clientInfo": {"name": "legacy-test", "version": "1.0"}
                                }
                            })
                            .to_string(),
                        ))
                        .expect("request"),
                )
                .await
                .expect("response");

            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("response body");
            let body: serde_json::Value =
                serde_json::from_slice(&body).expect("legacy initialize response is JSON-RPC");
            assert!(body.get("error").is_none(), "version {version}: {body}");
            assert_eq!(body["result"]["protocolVersion"], version);
            assert_eq!(body["result"]["capabilities"]["tools"]["listChanged"], true);
        }
    }

    #[tokio::test]
    async fn stateless_http_list_mutation_listen_race_catches_up_conservatively() {
        let notifier = PeerNotifier::default();
        let code_mode_app_state = notifier.code_mode_app_state.clone();
        let app = build_http_router(
            AppState::new(),
            None,
            None,
            &McpPreferences::default(),
            &[],
            notifier,
            true,
            false,
        )
        .expect("router with HTTP MCP");
        let meta = serde_json::json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {
                "name": "stateless-baseline-test",
                "version": "1.0"
            },
            "io.modelcontextprotocol/clientCapabilities": {}
        });

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", "tools/list")
                    .body(Body::from(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "tools/list",
                            "params": {"_meta": meta.clone()}
                        })
                        .to_string(),
                    ))
                    .expect("tools/list request"),
            )
            .await
            .expect("tools/list response");
        assert_eq!(listed.status(), StatusCode::OK);

        // Mutate the advertised contract after the caller's completed list and
        // before its separate stateless listen request. Sampling the live
        // contract during listen would incorrectly treat this new state as the
        // caller's baseline and suppress the required catch-up signal.
        code_mode_app_state.set_enabled(false);

        let listening = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", "subscriptions/listen")
                    .body(Body::from(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 2,
                            "method": "subscriptions/listen",
                            "params": {
                                "_meta": meta,
                                "notifications": {"toolsListChanged": true}
                            }
                        })
                        .to_string(),
                    ))
                    .expect("subscriptions/listen request"),
            )
            .await
            .expect("subscriptions/listen response");
        assert_eq!(listening.status(), StatusCode::OK);
        let mut stream = listening.into_body().into_data_stream();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut observed = Vec::new();
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(chunk)) => {
                    observed.extend_from_slice(&chunk.expect("SSE chunk"));
                    if String::from_utf8_lossy(&observed)
                        .contains("notifications/tools/list_changed")
                    {
                        break;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        let observed = String::from_utf8_lossy(&observed);
        assert!(
            observed.contains("notifications/tools/list_changed"),
            "the list(A) -> mutate(B) -> listen race missed its conservative catch-up: {observed}"
        );
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn stateless_http_fresh_tools_list_observes_code_mode_app_state_changes() {
        async fn listed_tool_names(app: axum::Router) -> Vec<String> {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/mcp")
                        .header("host", "localhost")
                        .header("content-type", "application/json")
                        .header("accept", "application/json, text/event-stream")
                        .header("mcp-protocol-version", "2026-07-28")
                        .header("mcp-method", "tools/list")
                        .body(Body::from(
                            serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "method": "tools/list",
                                "params": {
                                    "_meta": {
                                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                        "io.modelcontextprotocol/clientInfo": {
                                            "name": "stateless-app-state-test",
                                            "version": "1.0"
                                        },
                                        "io.modelcontextprotocol/clientCapabilities": {}
                                    }
                                }
                            })
                            .to_string(),
                        ))
                        .expect("tools/list request"),
                )
                .await
                .expect("tools/list response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("tools/list response body");
            let body: serde_json::Value =
                serde_json::from_slice(&body).expect("tools/list response is JSON-RPC");
            body["result"]["tools"]
                .as_array()
                .expect("tools array")
                .iter()
                .map(|tool| tool["name"].as_str().expect("tool name").to_string())
                .collect()
        }

        let tempdir = tempfile::tempdir().expect("tempdir");
        let manager = std::sync::Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        manager
            .seed_config_unchecked_for_tests(
                LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    ..LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;
        let state = AppState::new().with_gateway_manager(std::sync::Arc::clone(&manager));

        // `mcp_ui_enabled` defaults to false (Labby-owned apps are opt-in), and
        // a manager-backed server reads the published config rather than the
        // mirrored session atomic, so the config is what a fresh listing must
        // observe.
        let notifier = PeerNotifier::default();
        let app = build_http_router(
            state,
            None,
            None,
            &McpPreferences::default(),
            &[],
            notifier.clone(),
            true,
            false,
        )
        .expect("router with HTTP MCP");

        let disabled = listed_tool_names(app.clone()).await;
        assert!(disabled.iter().any(|name| name == "codemode"));
        assert!(disabled.iter().any(|name| name == "mcp_app"));
        assert!(!disabled.iter().any(|name| name == "codemode_ui"));

        // A stateless HTTP server rebuilds per request, so the next listing
        // must observe the change rather than serve a cached catalog.
        manager
            .seed_config_unchecked_for_tests(
                LabConfig {
                    code_mode: crate::config::CodeModeConfig {
                        enabled: true,
                        mcp_ui_enabled: true,
                        ..crate::config::CodeModeConfig::default()
                    },
                    ..LabConfig::default()
                }
                .to_gateway_config(),
            )
            .await;
        let enabled = listed_tool_names(app).await;
        assert!(enabled.iter().any(|name| name == "codemode_ui"));
    }

    #[tokio::test]
    async fn http_mcp_discovers_all_supported_protocols() {
        let app = build_http_router(
            AppState::new(),
            None,
            None,
            &McpPreferences::default(),
            &[],
            PeerNotifier::default(),
            true,
            false,
        )
        .expect("router with HTTP MCP");
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", "server/discover")
                    .body(Body::from(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "server/discover",
                            "params": {
                                "_meta": {
                                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                    "io.modelcontextprotocol/clientInfo": {
                                        "name": "stateless-test",
                                        "version": "1.0"
                                    },
                                    "io.modelcontextprotocol/clientCapabilities": {}
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("mcp-session-id").is_none());
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 response");
        for version in rmcp::model::ProtocolVersion::KNOWN_VERSIONS {
            assert!(body.contains(version.as_str()), "missing {version}: {body}");
        }
        assert!(body.contains("\"resultType\":\"complete\""));
    }

    #[tokio::test]
    async fn http_mcp_rejects_mismatched_sep_2243_method_header() {
        let app = build_http_router(
            AppState::new(),
            None,
            None,
            &McpPreferences::default(),
            &[],
            PeerNotifier::default(),
            true,
            false,
        )
        .expect("router with HTTP MCP");
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", "resources/list")
                    .body(Body::from(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "tools/list",
                            "params": {
                                "_meta": {
                                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                    "io.modelcontextprotocol/clientInfo": {
                                        "name": "header-test",
                                        "version": "1.0"
                                    },
                                    "io.modelcontextprotocol/clientCapabilities": {}
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("header rejection is JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    #[tokio::test]
    async fn http_mcp_rejects_missing_sep_2243_name_header_for_tool_call() {
        let app = build_http_router(
            AppState::new(),
            None,
            None,
            &McpPreferences::default(),
            &[],
            PeerNotifier::default(),
            true,
            false,
        )
        .expect("router with HTTP MCP");
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header("host", "localhost")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", "tools/call")
                    .body(Body::from(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "method": "tools/call",
                            "params": {
                                "name": "setup",
                                "arguments": {
                                    "action": "help",
                                    "params": {}
                                },
                                "_meta": {
                                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                                    "io.modelcontextprotocol/clientInfo": {
                                        "name": "header-test",
                                        "version": "1.0"
                                    },
                                    "io.modelcontextprotocol/clientCapabilities": {}
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("header rejection is JSON-RPC");
        assert_eq!(body["error"]["code"], -32020);
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn protected_gateway_subset_builder_mounts_scoped_mcp_service() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let manager = std::sync::Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                tempdir.path().join("gateway.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let config = LabConfig {
            protected_mcp_routes: vec![crate::config::ProtectedMcpRouteConfig {
                name: "ops".to_string(),
                enabled: true,
                public_host: "mcp.example.com".to_string(),
                public_path: "/ops".to_string(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".to_string(),
                scopes: vec!["mcp:ops".to_string()],
                health_path: None,
                target: Some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
                    crate::config::ProtectedGatewaySubsetTarget {
                        project_id: None,
                        upstreams: vec!["gateway-alpha".to_string()],
                        services: vec!["gateway".to_string()],
                        expose_code_mode: false,
                        loadout: None,
                    },
                )),
            }],
            ..LabConfig::default()
        };
        manager.seed_config(config.to_gateway_config()).await;
        let state = AppState::new()
            .with_config(config)
            .with_gateway_manager(manager);
        let routers = super::build_protected_mcp_routers(
            &state,
            &McpPreferences::default(),
            PeerNotifier::default(),
        )
        .expect("protected mcp router")
        .expect("gateway subset router");
        let router = routers.get("ops").expect("ops scoped router").clone();

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ops")
                    .header("host", "mcp.example.com")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .header("mcp-protocol-version", "2026-07-28")
                    .header("mcp-method", "server/discover")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"server/discover","id":1,"params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"route-test","version":"1.0.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }
}
