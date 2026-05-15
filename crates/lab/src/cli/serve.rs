//! `labby serve` — start the MCP server.

#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use lab_auth::config::AuthMode;
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::{LocalSessionManager, SessionConfig},
};
use tokio::sync::mpsc;

use crate::api::AppState;
use crate::config::{LabConfig, config_toml_path, resolve_auth_for_config};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
use crate::dispatch::gateway::types::CatalogChangeNotifier;
use crate::dispatch::gateway::{SHARED_GATEWAY_OAUTH_SUBJECT, install_gateway_manager};
use crate::dispatch::logs::client::{
    bootstrap_running_log_system, resolve_queue_capacity, resolve_retention, resolve_store_path,
    resolve_subscriber_capacity,
};
use crate::mcp::peers::PeerNotifier;
use crate::mcp::server::LabMcpServer;
use crate::node::enrollment::store::EnrollmentStore;
use crate::node::identity::{resolve_local_hostname, resolve_runtime_role_from_config};
use crate::node::runtime::NodeRuntime;
use crate::node::store::NodeStore;
#[cfg(target_os = "linux")]
use crate::process::unix::{exe_path, terminate_sigterm};
use crate::registry::{ToolRegistry, build_default_registry};

/// Role override for `labby serve --role`.
///
/// Maps to [`crate::config::NodeRuntimeRole`] at startup; a separate type here
/// keeps the `clap` dependency out of `config.rs`.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ServeRole {
    Controller,
    Node,
}

impl From<ServeRole> for crate::config::NodeRuntimeRole {
    fn from(role: ServeRole) -> Self {
        match role {
            ServeRole::Controller => crate::config::NodeRuntimeRole::Controller,
            ServeRole::Node => crate::config::NodeRuntimeRole::Node,
        }
    }
}

/// Transport choices for `labby serve`.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Transport {
    /// stdin/stdout framing (available via `labby mcp`).
    Stdio,
    /// HTTP transport (default) — requires `LAB_MCP_HTTP_TOKEN` or OAuth when exposed remotely.
    Http,
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
    /// Sets `LAB_LOG=labby=<level>,warn` before tracing init.
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
    /// Sets `LAB_LOG=labby=<level>,warn` before tracing init.
    /// Example: `--log-level debug`
    #[arg(long)]
    pub log_level: Option<String>,
    /// Explicit runtime role override. Takes precedence over [node].role in config.toml
    /// and over hostname-based inference.
    /// `--role node` requires a controller host to be configured.
    #[arg(long, value_enum)]
    pub role: Option<ServeRole>,
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
            role: None,
            command: None,
        },
        config,
    )
    .await
}

/// Run the serve subcommand.
pub async fn run(args: ServeArgs, config: &LabConfig) -> Result<ExitCode> {
    let transport = resolve_transport(
        args.transport,
        args.command.as_ref(),
        std::env::var("LAB_MCP_TRANSPORT").ok(),
        config.mcp.transport.as_deref(),
    )?;
    tracing::info!(
        subsystem = "cli",
        phase = "serve.start",
        transport = ?transport,
        requested_service_count = args.services.len(),
        "starting serve command"
    );
    // Resolve host and port here for source-of-truth ordering, but defer
    // address parsing and validation until the actual bind call in run_http.
    // This way an invalid host string only errors when the hosted HTTP app path is chosen.
    let host = args
        .host
        .or_else(|| std::env::var("LAB_MCP_HTTP_HOST").ok())
        .or_else(|| config.mcp.host.clone())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = resolve_port(
        args.port,
        std::env::var("LAB_MCP_HTTP_PORT").ok(),
        config.mcp.port,
    )?;
    let config_path = config_toml_path().unwrap_or_else(|| "config.toml".into());
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

    // ── Role resolution ── must happen BEFORE build_default_registry() so that
    // node-mode processes exit early without building the full controller registry.
    let local_host = resolve_local_hostname().context("resolve local hostname")?;
    let resolved_runtime =
        resolve_runtime_role_from_config(&local_host, config, args.role.map(Into::into))
            .context("resolve device runtime role")?;
    tracing::info!(
        subsystem = "startup",
        phase = "bootstrap.device-runtime",
        node_role = ?resolved_runtime.role,
        local_host = %resolved_runtime.local_host,
        master_host = %resolved_runtime.master_host,
        "node runtime resolved"
    );
    let node_runtime = NodeRuntime::from_config(resolved_runtime, config, Some(port))?;
    let node_role = node_runtime.role();

    // Early return for node (non-controller) processes: skip the full
    // controller startup (registry build, OAuth, gateway, logs system, web UI, etc.).
    if matches!(node_role, crate::config::NodeRole::NonMaster) {
        return run_node_mode(transport, args.command.as_ref(), config, node_runtime, port).await;
    }

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
    let log_retention_days = config
        .node
        .as_ref()
        .and_then(|n| n.log_retention_days)
        .unwrap_or(crate::node::log_store::DEFAULT_RETENTION_DAYS);
    let node_log_db_path = node_runtime.home_dir().join(".lab/node-logs.sqlite");
    let node_store = match crate::node::log_store::SqliteNodeLogStore::open(
        node_log_db_path.clone(),
        log_retention_days,
    )
    .await
    {
        Ok(log_store) => {
            tracing::info!(
                path = %node_log_db_path.display(),
                retention_days = log_retention_days,
                "node log store opened"
            );
            Arc::new(NodeStore::with_log_store(log_store))
        }
        Err(err) => {
            tracing::warn!(
                path = %node_log_db_path.display(),
                error = %err,
                "node log store unavailable; falling back to in-memory store"
            );
            Arc::new(NodeStore::default())
        }
    };
    let enrollment_store = Arc::new(
        EnrollmentStore::open(node_runtime.home_dir().join(".lab/node-enrollments.json"))
            .await
            .context("open node enrollment store")?,
    );

    let stdio_mode = should_run_stdio(transport, args.command.as_ref());
    let gateway_runtime = GatewayRuntimeHandle::default();
    let bearer_token = http_token();
    let auth_config =
        resolve_auth_for_config(&config).context("invalid HTTP auth configuration")?;
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
    let upstream_oauth_runtime = if stdio_mode {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime skipped for stdio transport"
        );
        None
    } else {
        build_upstream_oauth_runtime(config, &auth_config).await?
    };
    tracing::info!(
        subsystem = "gateway_client",
        phase = "discovery.start",
        upstream_count = config.upstream.len(),
        oauth_upstream_count = config
            .upstream
            .iter()
            .filter(|upstream| upstream.oauth.is_some())
            .count(),
        in_process_peer_count = registry
            .services()
            .iter()
            .filter(|service| !service.actions.is_empty())
            .count(),
        "starting upstream gateway discovery"
    );
    crate::config::set_process_tool_search_enabled(config.tool_search.enabled);
    let mut pool_builder = crate::dispatch::upstream::pool::UpstreamPool::new();
    if let Some(rt) = &upstream_oauth_runtime {
        pool_builder = pool_builder.with_oauth_client_cache(rt.cache.clone());
    }
    let pool = Arc::new(pool_builder);
    // In MCP-only (stdio) mode skip upstream discovery entirely — no child processes
    // should be spawned, making the axon↔lab recursion cycle physically impossible.
    if !stdio_mode {
        if upstream_oauth_runtime.is_some() {
            pool.discover_all_for_subject_with_in_process_peers(
                &config.upstream,
                SHARED_GATEWAY_OAUTH_SUBJECT,
                &registry,
            )
            .await;
        } else {
            pool.discover_all_with_in_process_peers(&config.upstream, &registry)
                .await;
        }
        tracing::info!(
            subsystem = "gateway_client",
            phase = "discovery.finish",
            upstream_count = config.upstream.len(),
            discovered_upstream_count = pool.upstream_count().await,
            "upstream gateway discovery complete"
        );
        gateway_runtime.swap(Some(pool)).await;
    } else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "discovery.skipped",
            "upstream discovery skipped for MCP-only stdio mode — no upstream processes spawned"
        );
    }
    let notifier = PeerNotifier::default();
    let (notify_tx, notify_rx) = mpsc::unbounded_channel();
    let _catalog_notifier_task = tokio::spawn(notifier.clone().run(notify_rx));
    let service_clients = SharedServiceClients::from_env();
    let mut gateway_manager = GatewayManager::new(
        config_toml_path().unwrap_or_else(|| "config.toml".into()),
        gateway_runtime.clone(),
    )
    .with_builtin_service_registry(registry.clone())
    .with_service_clients(service_clients);
    if let Some(rt) = upstream_oauth_runtime {
        gateway_manager = gateway_manager
            .with_upstream_oauth_managers(rt.managers)
            .with_oauth_client_cache(rt.cache)
            .with_oauth_resources(rt.sqlite, rt.key, rt.redirect_uri);
    }
    gateway_manager.set_notifier(CatalogChangeNotifier::new(notify_tx));
    let gateway_manager = Arc::new(gateway_manager);
    // Seed config for both transports so MCP catalog visibility and tool-search
    // settings match the persisted config. MCP-only stdio still skips installing
    // the process-global gateway manager and upstream discovery, which are the
    // paths that can create recursive upstream connections.
    gateway_manager.seed_config(config.clone()).await;
    if !stdio_mode {
        install_gateway_manager(Arc::clone(&gateway_manager));
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
        tracing::info!(
            subsystem = "gateway_client",
            phase = "manager.ready",
            upstream_count = gateway_manager.current_config().await.upstream.len(),
            "gateway manager installed"
        );
    } else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "manager.skipped",
            "gateway manager install skipped for MCP-only stdio mode"
        );
    }
    let logs_system = bootstrap_running_log_system(
        resolve_store_path(Some(config)),
        resolve_retention(Some(config)),
        resolve_queue_capacity(Some(config)),
        resolve_subscriber_capacity(Some(config)),
    )
    .await?;

    // Create the ACP session registry before the HTTP/stdio split so both transports
    // share the same process-global dispatch slot (intra-process only — stdio and
    // HTTP modes are mutually exclusive within one process).
    let acp_registry = Arc::new(crate::acp::registry::AcpSessionRegistry::new());
    crate::dispatch::acp::install_registry(Arc::clone(&acp_registry));
    acp_registry.restore_from_db().await;
    tracing::info!(
        subsystem = "acp",
        phase = "ready",
        "ACP session registry installed"
    );

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
        return run_stdio(
            Arc::new(registry),
            Arc::clone(&gateway_manager),
            node_role,
            notifier,
        )
        .await;
    }

    if host.is_empty() {
        anyhow::bail!("HTTP host cannot be empty — set LAB_MCP_HTTP_HOST or mcp.host in config");
    }

    crate::mcp::server::verify_upstream_subject_resolution_support()
        .context("verify upstream OAuth subject-resolution wiring")?;
    let auth_configured = bearer_token.is_some() || matches!(auth_config.mode, AuthMode::OAuth);

    // Safety gate: refuse to bind on a non-localhost address without
    // any auth configured (lab-319g). This prevents accidental
    // unauthenticated deployment on a LAN-accessible address.
    if !auth_configured && !is_loopback_host(&host) {
        anyhow::bail!(
            "refusing to bind HTTP on {host}:{port} without authentication. \
             Set LAB_MCP_HTTP_TOKEN or LAB_AUTH_MODE=oauth, or bind to \
             127.0.0.1 for local-only access."
        );
    }

    let oauth_state = if matches!(auth_config.mode, AuthMode::OAuth) {
        Some(
            lab_auth::state::AuthState::new(auth_config.clone())
                .await
                .context("initialize lab-auth oauth state")?,
        )
    } else {
        None
    };

    let web_assets_dir = resolve_web_assets_dir(&config.web);
    let embedded_web_assets_enabled =
        web_assets_dir.is_none() && crate::api::web::embedded_web_assets_available();

    let oauth_enabled = matches!(auth_config.mode, AuthMode::OAuth);

    let mut state = AppState::from_registry(registry)
        .with_config(config.clone())
        .with_http_bind_host(host.clone())
        .with_acp_registry(Arc::clone(&acp_registry));
    if auth_configured {
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
    state = state.with_gateway_manager(Arc::clone(&gateway_manager));
    state = state.with_auth_config(auth_config);
    let web_ui_auth_disabled = resolve_web_ui_auth_disabled(
        &config.web,
        web_assets_dir.is_some() || embedded_web_assets_enabled,
        oauth_enabled,
    )?;
    state = state.with_web_ui_auth_disabled(web_ui_auth_disabled);

    // lab-bg3e.3 Q5 reframe: prominent startup banner whenever the web UI
    // is reachable without authentication. v1 ships unsecured by design;
    // operators must understand any local process can write ~/.lab/.env.
    if web_ui_auth_disabled {
        let banner = "==================================================================\n\
                      ⚠  Lab web UI is running WITHOUT authentication.\n\
                      ⚠  Any local process can read or modify your configuration.\n\
                      ⚠  Set up OAuth (LAB_AUTH_MODE=oauth) to secure the API.\n\
                      ==================================================================";
        eprintln!("\n{banner}\n");
        tracing::warn!(
            subsystem = "web_server",
            phase = "startup.banner",
            "lab web UI started without authentication; any local process can write ~/.lab/.env"
        );
    }

    state = state.with_node_store(Arc::clone(&node_store));
    state = state.with_enrollment_store(Arc::clone(&enrollment_store));
    state = state.with_log_system(logs_system);
    #[cfg(feature = "mcpregistry")]
    let _registry_sync_keepalive = {
        let db_path = crate::config::registry_db_path();
        match crate::dispatch::marketplace::store::RegistryStore::open(&db_path).await {
            Ok(store) => {
                let store = Arc::new(store);
                state = state.with_registry_store(Arc::clone(&store));
                let sync_store = Arc::clone(&store);
                match crate::dispatch::marketplace::mcp_client::require_mcp_client() {
                    Ok(sync_client) => Some(tokio::spawn(async move {
                        // Fire immediately at startup — do not wait for the first interval tick.
                        if let Err(e) = crate::dispatch::marketplace::sync::perform_sync(
                            &sync_store,
                            &sync_client,
                            false,
                            "startup",
                        )
                        .await
                        {
                            tracing::warn!(
                                service = "mcpregistry",
                                event = "sync.failed",
                                error = %e,
                                "initial sync failed; will retry next hour"
                            );
                        }
                        let mut interval = tokio::time::interval(Duration::from_secs(3600));
                        // Skip missed ticks — if a sync takes >1h, Burst would fire again immediately.
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        // Consume the immediate tick so the first loop iteration is at T+1h.
                        interval.tick().await;
                        loop {
                            interval.tick().await;
                            if let Err(e) = crate::dispatch::marketplace::sync::perform_sync(
                                &sync_store,
                                &sync_client,
                                false,
                                "hourly",
                            )
                            .await
                            {
                                tracing::warn!(
                                    service = "mcpregistry",
                                    event = "sync.failed",
                                    error = %e,
                                    "hourly sync failed; will retry next hour"
                                );
                            }
                        }
                    })),
                    Err(e) => {
                        tracing::warn!(
                            service = "mcpregistry",
                            event = "sync.client.unavailable",
                            error = %e,
                            "mcpregistry client unavailable; registry background sync disabled"
                        );
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    service = "mcpregistry",
                    event = "store.open.failed",
                    error = %e,
                    "registry store unavailable; /v0.1 will return 503"
                );
                None
            }
        }
    };
    // `_registry_sync_keepalive` keeps the background sync task alive for the
    // duration of `serve`; binding it by name preserves the JoinHandle.
    state = state.with_node_role(node_role);

    // Wire the configured workspace root into AppState so the fs
    // service serves `fs.list` / `fs.preview` without re-reading config
    // per request. Failure is non-fatal: invalid root keeps fs calls on the
    // structured `workspace_not_configured` path.
    //
    // Guarded by `feature = "fs"` so a build without fs cannot report the
    // service as enabled at startup just because a `[workspace].root` is
    // configured.
    #[cfg(feature = "fs")]
    match crate::dispatch::fs::resolve_workspace_root(config) {
        Ok(root) => {
            tracing::info!(
                subsystem = "startup",
                phase = "fs.workspace_root",
                path = %root.display(),
                "workspace filesystem browser enabled"
            );
            state = state.with_workspace_root(root);
        }
        Err(e) => {
            tracing::warn!(
                subsystem = "startup",
                phase = "fs.workspace_root",
                error = %e,
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
        mcp_server_enabled = matches!(transport, Transport::Http),
        gateway_client_enabled = !config.upstream.is_empty(),
        oauth_upstream_enabled = config
            .upstream
            .iter()
            .any(|upstream| upstream.oauth.is_some()),
        web_ui_auth_disabled = state.web_ui_auth_disabled,
        "startup plan resolved"
    );

    node_runtime.start_background_tasks();

    run_http(
        &host,
        port,
        bearer_token,
        state,
        oauth_state,
        &config.mcp,
        &config.api.cors_origins,
        notifier,
        matches!(transport, Transport::Http),
    )
    .await
}

struct UpstreamOauthRuntime {
    managers: Arc<dashmap::DashMap<String, crate::oauth::upstream::manager::UpstreamOauthManager>>,
    cache: crate::oauth::upstream::cache::OauthClientCache,
    sqlite: lab_auth::sqlite::SqliteStore,
    key: crate::oauth::upstream::encryption::EncryptionKey,
    redirect_uri: String,
}

async fn build_upstream_oauth_runtime(
    config: &LabConfig,
    auth_config: &lab_auth::config::AuthConfig,
) -> Result<Option<UpstreamOauthRuntime>> {
    let Some(public_url) = auth_config.public_url.as_ref() else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime disabled because no public url is configured"
        );
        return Ok(None);
    };
    let Ok(encryption_key_raw) = std::env::var("LAB_OAUTH_ENCRYPTION_KEY") else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime disabled because LAB_OAUTH_ENCRYPTION_KEY is unset"
        );
        return Ok(None);
    };
    anyhow::ensure!(
        public_url.scheme() == "https",
        "LAB_PUBLIC_URL must be absolute https:// when upstream oauth is configured"
    );
    let key = crate::oauth::upstream::encryption::load_key(&encryption_key_raw)
        .map_err(|error| anyhow::anyhow!("invalid LAB_OAUTH_ENCRYPTION_KEY: {error}"))?;
    let sqlite = lab_auth::sqlite::SqliteStore::open(auth_config.sqlite_path.clone())
        .await
        .context("open sqlite store for upstream oauth")?;
    let redirect_uri = build_upstream_oauth_callback_uri(public_url)?;

    let managers = Arc::new(dashmap::DashMap::new());
    for upstream in config
        .upstream
        .iter()
        .filter(|upstream| upstream.oauth.is_some())
    {
        managers.insert(
            upstream.name.clone(),
            crate::oauth::upstream::manager::UpstreamOauthManager::new(
                sqlite.clone(),
                key.clone(),
                upstream.clone(),
                redirect_uri.clone(),
            ),
        );
    }
    let cache = crate::oauth::upstream::cache::OauthClientCache::new(Arc::clone(&managers));
    tracing::info!(
        subsystem = "gateway_client",
        phase = "oauth.runtime.ready",
        oauth_upstream_count = managers.len(),
        "upstream oauth runtime initialized"
    );
    Ok(Some(UpstreamOauthRuntime {
        managers,
        cache,
        sqlite,
        key,
        redirect_uri,
    }))
}

fn build_upstream_oauth_callback_uri(public_url: &url::Url) -> Result<String> {
    let mut redirect_uri = public_url.clone();
    let base_path = redirect_uri.path().trim_end_matches('/');
    let next_path = if base_path.is_empty() {
        "/auth/upstream/callback".to_string()
    } else {
        format!("{base_path}/auth/upstream/callback")
    };
    redirect_uri.set_path(&next_path);
    redirect_uri.set_query(None);
    redirect_uri.set_fragment(None);
    Ok(redirect_uri.to_string())
}

fn resolve_web_assets_dir(web: &crate::config::WebPreferences) -> Option<PathBuf> {
    let from_env = std::env::var("LAB_WEB_ASSETS_DIR")
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

    Ok(web_assets_enabled && !oauth_enabled)
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
            .map_err(|err| anyhow::anyhow!("invalid LAB_MCP_TRANSPORT value `{value}`: {err}"));
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
            .with_context(|| format!("invalid LAB_MCP_HTTP_PORT value `{value}`"));
    }
    Ok(config.unwrap_or(8765))
}

/// Return the bearer token if configured, or `None` for auth-free operation.
fn http_token() -> Option<String> {
    std::env::var("LAB_MCP_HTTP_TOKEN")
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
    auth_state: Option<lab_auth::state::AuthState>,
    mcp_config: &crate::config::McpPreferences,
    config_cors_origins: &[String],
    notifier: PeerNotifier,
    mount_http_mcp: bool,
) -> Result<ExitCode> {
    // ── Single-master lock ────────────────────────────────────────────────────
    // Only one HTTP master instance may run per device at a time. Exits
    // immediately with a clear error if the lock is already held by another
    // process. This guard is NOT applied in stdio/MCP-only mode — `labby serve
    // mcp --stdio` may run freely alongside a running master.
    let _master_lock: std::fs::File = {
        let lock_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".local/state/lab");
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
                    "lab: another master instance is already running on this device \
                     (lock: {}). Use 'labby mcp' for node/MCP-only mode.",
                    lock_path.display()
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

    let web_assets_enabled = state.web_assets_dir.is_some();
    let bearer_token_configured = bearer_token.is_some();
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
    let router = build_http_router(
        state,
        bearer_token,
        auth_state,
        mcp_config,
        config_cors_origins,
        notifier,
        mount_http_mcp,
    )?;
    tracing::info!(
        subsystem = "api_server",
        phase = "router.build.finish",
        http_mcp_enabled = mount_http_mcp,
        "http router ready"
    );
    // Parse and validate the address at bind time, not at CLI parse time.
    let addr = bind_addr(host, port);
    tracing::info!(
        subsystem = "api_server",
        phase = "listener.bind.start",
        addr,
        "binding http listener"
    );
    let listener = bind_or_reclaim(&addr, port).await?;
    // Notify systemd that the socket is ready (sd_notify READY=1).
    #[cfg(all(feature = "systemd", unix))]
    {
        if std::env::var_os("NOTIFY_SOCKET").is_some() {
            if let Err(e) = sd_notify::notify(&[sd_notify::NotifyState::Ready]) {
                tracing::warn!(
                    surface = "api", service = "http", action = "sd_notify.error",
                    error = %e, "sd_notify failed"
                );
            } else {
                tracing::info!(
                    surface = "api",
                    service = "http",
                    action = "sd_notify.ready",
                    "systemd READY=1 sent"
                );
            }
        }
    }
    tracing::info!(
        subsystem = "api_server",
        phase = "ready",
        addr,
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
        pid = std::process::id(),
        web_server_enabled = web_assets_enabled,
        mcp_server_enabled = mount_http_mcp,
        "labby serve ready"
    );
    // SIGUSR1 → config reload signal handler (unix only).
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigusr1 =
            signal(SignalKind::user_defined1()).context("failed to register SIGUSR1 handler")?;
        tokio::select! {
            result = axum::serve(listener, router) => { result?; }
            _ = async {
                loop {
                    sigusr1.recv().await;
                    tracing::info!(
                        surface = "api", service = "http", action = "config.reload",
                        "SIGUSR1 received — config reload triggered",
                    );
                    // Future: re-read config.toml and apply diffs here.
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        axum::serve(listener, router).await?;
    }
    Ok(ExitCode::SUCCESS)
}

fn build_http_router(
    state: AppState,
    bearer_token: Option<String>,
    auth_state: Option<lab_auth::state::AuthState>,
    mcp_config: &crate::config::McpPreferences,
    config_cors_origins: &[String],
    notifier: PeerNotifier,
    mount_http_mcp: bool,
) -> Result<axum::Router> {
    let mcp_router = if mount_http_mcp {
        // Build the MCP streamable HTTP service in the serve path (not in the
        // router module) to avoid an api->mcp dependency.
        let mcp_service = build_mcp_service(&state, mcp_config, notifier)?;
        Some(axum::Router::new().nest_service("/mcp", mcp_service))
    } else {
        None
    };

    Ok(crate::api::router::build_router(
        state,
        bearer_token,
        auth_state,
        mcp_router,
        config_cors_origins,
    ))
}

/// Run the minimal node-mode startup path.
///
/// Called when role resolution determines the process is a non-controller node.
/// Skips the full controller startup (registry build, OAuth, gateway, logs system,
/// web UI, marketplace sync, EnrollmentStore, etc.) and runs only what a node needs:
/// background tasks (metadata upload, bootstrap logs, WebSocket flush) and a
/// loopback health server to keep the process alive and signal systemd readiness.
async fn run_node_mode(
    transport: Transport,
    command: Option<&ServeCommand>,
    config: &LabConfig,
    node_runtime: NodeRuntime,
    port: u16,
) -> Result<ExitCode> {
    // Stdio mode is not designed for node-mode operation.
    if should_run_stdio(transport, command) {
        anyhow::bail!("labby serve mcp --stdio is not supported in node mode");
    }

    tracing::info!(
        surface = "node",
        service = "runtime",
        action = "node_mode.start",
        node_id = %node_runtime.local_host(),
        port,
        controller_host = %config.node.as_ref().and_then(|n| n.controller.as_deref()).unwrap_or("<none>"),
        "starting node runtime (non-controller mode)"
    );

    // Start background tasks: metadata upload, bootstrap log collection, WebSocket flush.
    // These are fire-and-forget — start_background_tasks spawns a detached task and returns
    // immediately so the health server loop starts without being blocked by network timeouts.
    node_runtime.start_background_tasks();

    // Run the loopback health server as the process keep-alive.
    crate::node::health::run_loopback_health_server(port).await
}

async fn run_stdio(
    registry: Arc<ToolRegistry>,
    gateway_manager: Arc<GatewayManager>,
    node_role: crate::config::NodeRole,
    notifier: PeerNotifier,
) -> Result<ExitCode> {
    let spawn_depth = resolve_lab_spawn_depth(std::env::var("LAB_SPAWN_DEPTH").ok());
    // MCP-only mode never spawns upstream processes, so a positive
    // LAB_SPAWN_DEPTH is logged as lifecycle evidence instead of changing
    // behavior in this process.
    if spawn_depth.unwrap_or_default() > 0 {
        tracing::warn!(
            surface = "mcp",
            service = "stdio",
            action = "recursion_guard.detected",
            subsystem = "mcp_server",
            phase = "stdio.recursion_guard",
            transport = "stdio",
            spawn_depth,
            "LAB_SPAWN_DEPTH is set for stdio MCP serve; upstream spawning is disabled in this mode"
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
        node_role = ?node_role,
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
        gateway_manager: Some(Arc::clone(&gateway_manager)),
        node_role: Some(node_role),
        peers: Arc::clone(&notifier.peers),
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
            crate::mcp::logging::logging_level_rank(rmcp::model::LoggingLevel::Info),
        )),
    };
    let running = server.serve(rmcp::transport::stdio()).await?;
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
    running.waiting().await?;
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
}

fn resolve_lab_spawn_depth(env: Option<String>) -> Option<u32> {
    env.as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u32>().ok())
}

/// Build the MCP streamable HTTP service from app state.
///
/// The factory closure clones `Arc<ToolRegistry>` from `AppState` and constructs
/// a new `LabMcpServer` per session. Construction cost: two Arc increments.
fn build_mcp_service(
    state: &AppState,
    mcp_config: &crate::config::McpPreferences,
    notifier: PeerNotifier,
) -> Result<StreamableHttpService<LabMcpServer, LocalSessionManager>> {
    let registry = Arc::clone(&state.registry);
    let gateway_manager = state.gateway_manager.clone();

    let session_ttl_secs = resolve_session_ttl_secs(
        std::env::var("LAB_MCP_SESSION_TTL_SECS").ok(),
        mcp_config.session_ttl_secs,
    )?;

    let mut session_config = SessionConfig::default();
    session_config.keep_alive = Some(Duration::from_secs(session_ttl_secs));

    let mut session_manager = LocalSessionManager::default();
    session_manager.session_config = session_config;
    let session_manager = Arc::new(session_manager);

    let stateful =
        resolve_stateful_mode(std::env::var("LAB_MCP_STATEFUL").ok(), mcp_config.stateful)?;

    let allowed_hosts = allowed_hosts(
        mcp_config.allowed_hosts.as_deref().unwrap_or(&[]),
        state
            .auth_config
            .as_ref()
            .and_then(|cfg| cfg.public_url.as_ref().map(url::Url::as_str)),
    );
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts.clone())
        .with_stateful_mode(stateful);
    tracing::info!(
        surface = "mcp",
        service = "labby",
        action = "server.init",
        subsystem = "mcp_server",
        phase = "http.mount",
        transport = "http",
        stateful,
        session_ttl_secs,
        allowed_host_count = allowed_hosts.len(),
        "http mcp service configured"
    );

    // All HTTP sessions share the same PeerNotifier (and thus the same peers
    // vec) so that gateway reload notifications reach every connected session.
    let shared_peers = Arc::clone(&notifier.peers);
    let node_role = state.node_role;

    Ok(StreamableHttpService::new(
        move || {
            let reg = Arc::clone(&registry);
            let manager = gateway_manager.clone();
            let peers = Arc::clone(&shared_peers);
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "session.init",
                subsystem = "mcp_server",
                phase = "session.init",
                transport = "http",
                services = reg.services().len(),
                gateway_manager_configured = manager.is_some(),
                node_role = ?node_role,
                "initializing HTTP MCP session handler"
            );
            Ok(LabMcpServer {
                registry: reg,
                gateway_manager: manager,
                node_role,
                peers,
                logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                    crate::mcp::logging::logging_level_rank(rmcp::model::LoggingLevel::Info),
                )),
            })
        },
        session_manager,
        config,
    ))
}

/// Build the allowed hosts list for DNS rebinding protection.
///
/// Reads `LAB_MCP_ALLOWED_HOSTS` (comma-separated) and the resolved resource
/// URL. Always includes loopback defaults. Rejects wildcard.
fn resolve_session_ttl_secs(env: Option<String>, config: Option<u64>) -> Result<u64> {
    if let Some(value) = env {
        return value
            .parse::<u64>()
            .with_context(|| format!("invalid LAB_MCP_SESSION_TTL_SECS value `{value}`"));
    }
    Ok(config.unwrap_or(300))
}

fn resolve_stateful_mode(env: Option<String>, config: Option<bool>) -> Result<bool> {
    if let Some(value) = env {
        return value
            .parse::<bool>()
            .with_context(|| format!("invalid LAB_MCP_STATEFUL value `{value}`"));
    }
    Ok(config.unwrap_or(true))
}

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
    if let Ok(extra) = std::env::var("LAB_MCP_ALLOWED_HOSTS") {
        for h in extra.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            // Reject wildcard — would disable Host header validation entirely
            if h == "*" {
                tracing::warn!(
                    "ignoring wildcard '*' in LAB_MCP_ALLOWED_HOSTS — \
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
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::{
        McpArgs, PeerNotifier, ServeCommand, Transport, allowed_hosts, bind_addr,
        build_http_router, filter_registry, is_loopback_host, resolve_lab_spawn_depth,
        resolve_port, resolve_session_ttl_secs, resolve_stateful_mode, resolve_transport,
        resolve_web_ui_auth_disabled, should_run_stdio,
    };
    use crate::api::AppState;
    use crate::cli::Cli;
    use crate::config::{LabConfig, McpPreferences, WebPreferences};
    use crate::registry::{build_default_registry, filter_built_in_upstream_apis};
    use clap::Parser;

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
        let error = filter_registry(reg, &["radarr".to_string()])
            .expect_err("disabled radarr should be unknown to --services");
        assert!(error.to_string().contains("unknown service"));
    }

    #[test]
    fn config_defaults_are_available_for_serve_resolution() {
        let cfg = LabConfig {
            mcp: McpPreferences {
                transport: Some("stdio".into()),
                host: Some("0.0.0.0".into()),
                port: Some(9000),
                session_ttl_secs: Some(120),
                stateful: Some(false),
                allowed_hosts: Some(vec!["lab.internal".into()]),
            },
            ..LabConfig::default()
        };
        assert_eq!(cfg.mcp.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cfg.mcp.session_ttl_secs, Some(120));
        assert_eq!(cfg.mcp.stateful, Some(false));
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
    fn lab_spawn_depth_resolution_is_logging_only_and_tolerates_bad_env() {
        assert_eq!(resolve_lab_spawn_depth(Some("2".into())), Some(2));
        assert_eq!(resolve_lab_spawn_depth(Some(" 3 ".into())), Some(3));
        assert_eq!(resolve_lab_spawn_depth(Some("".into())), None);
        assert_eq!(resolve_lab_spawn_depth(Some("not-a-number".into())), None);
        assert_eq!(resolve_lab_spawn_depth(None), None);
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
    fn session_ttl_resolution_prefers_env_then_config_then_default() {
        assert_eq!(
            resolve_session_ttl_secs(Some("120".into()), Some(90)).unwrap(),
            120
        );
        assert_eq!(resolve_session_ttl_secs(None, Some(90)).unwrap(), 90);
        assert_eq!(resolve_session_ttl_secs(None, None).unwrap(), 300);
    }

    #[test]
    fn stateful_resolution_prefers_env_then_config_then_default() {
        assert!(!resolve_stateful_mode(Some("false".into()), Some(true)).unwrap());
        assert!(!resolve_stateful_mode(None, Some(false)).unwrap());
        assert!(resolve_stateful_mode(None, None).unwrap());
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
        )
        .expect("router without http mcp");

        let v1_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/extract/actions")
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
}
