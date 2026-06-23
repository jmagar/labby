use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;

mod args;
mod code;
mod dispatch;
mod list;
mod oauth;

pub use args::*;

use dispatch::dispatch_command;

use crate::config::{LabConfig, config_toml_path, resolve_auth_for_config};
use crate::dispatch::clients::SharedServiceClients;
use crate::dispatch::gateway::config_store::LabConfigStore;
use crate::dispatch::gateway::install_gateway_manager;
use crate::dispatch::gateway::manager::{
    GatewayManager, GatewayManagerConfig, GatewayOauthConfig, GatewayRuntimeHandle,
};
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::output::OutputFormat;
use crate::registry::ToolRegistry;

pub(crate) async fn build_manager(
    config: &LabConfig,
    discover_upstreams: bool,
) -> Result<Arc<GatewayManager>> {
    let auth_config = resolve_auth_for_config(config)?;
    let upstream_oauth_runtime = crate::oauth::upstream::runtime::build_upstream_oauth_runtime(
        &config.upstream,
        &auth_config,
    )
    .await?;
    Ok(build_manager_with_upstream_oauth_runtime(
        config,
        discover_upstreams,
        upstream_oauth_runtime,
    )
    .await)
}

async fn build_manager_with_upstream_oauth_runtime(
    config: &LabConfig,
    discover_upstreams: bool,
    upstream_oauth_runtime: Option<crate::oauth::upstream::runtime::UpstreamOauthRuntime>,
) -> Arc<GatewayManager> {
    let runtime = GatewayRuntimeHandle::default();
    if discover_upstreams {
        // Seed lazily (mirroring `serve`): catalog entries come from config
        // without spawning any upstream processes. Connections are made on
        // demand via the manager's `ensure_*_runtime_ready` paths, so one-shot
        // CLI commands only spawn the upstreams they actually touch.
        let mut pool_builder = UpstreamPool::new()
            .with_request_timeout(config.upstream_request_timeout())
            .with_relay_timeout(config.upstream_relay_timeout())
            .with_in_process_connector(crate::mcp::in_process_peer::connector());
        if let Some(rt) = &upstream_oauth_runtime {
            pool_builder = pool_builder.with_oauth_client_cache(rt.cache.clone());
        }
        let pool = Arc::new(pool_builder);
        pool.seed_lazy_upstreams(&config.upstream).await;
        runtime.swap(Some(pool)).await;
    }

    let config_path = config_toml_path().unwrap_or_else(|| "config.toml".into());
    let live_config = Arc::new(std::sync::RwLock::new(config.clone()));
    let store: Arc<dyn lab_gateway::gateway::config_store::GatewayConfigStore> = Arc::new(
        LabConfigStore::new(Arc::clone(&live_config), config_path.clone())
            .with_service_clients(SharedServiceClients::from_env()),
    );
    let registry: Arc<dyn lab_gateway::gateway::service_registry::GatewayServiceRegistry> =
        Arc::new(filtered_builtin_service_registry(config));

    let manager = GatewayManager::from_config(
        GatewayManagerConfig {
            config_path,
            store,
            registry,
            in_process_connector: None,
            oauth: upstream_oauth_runtime.map(|rt| GatewayOauthConfig {
                managers: rt.managers,
                cache: rt.cache,
                sqlite: rt.sqlite,
                key: rt.key,
                redirect_uri: rt.redirect_uri,
            }),
        },
        runtime,
    );
    let manager = Arc::new(manager);
    manager.seed_config(config.to_gateway_config()).await;
    install_gateway_manager(Arc::clone(&manager));
    manager
}

fn filtered_builtin_service_registry(config: &LabConfig) -> ToolRegistry {
    crate::registry::filter_built_in_upstream_apis(
        crate::registry::build_default_registry(),
        config.services.built_in_upstream_apis_enabled,
    )
}

pub async fn run(args: GatewayArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    let discover_upstreams = !(matches!(
        &args.command,
        GatewayCommand::Mcp(GatewayMcpArgs {
            command: GatewayMcpCommand::List
                | GatewayMcpCommand::Enable(_)
                | GatewayMcpCommand::Disable(_)
                | GatewayMcpCommand::Cleanup(_)
                | GatewayMcpCommand::Auth(GatewayMcpAuthArgs {
                    command: GatewayMcpAuthCommand::Status(_) | GatewayMcpAuthCommand::Clear(_),
                }),
        })
    ) || matches!(&args.command, GatewayCommand::ProtectedRoute(_)));
    let manager = build_manager(config, discover_upstreams).await?;
    // Race the command against SIGINT/SIGTERM so the drain below also runs
    // when the invocation is killed externally (e.g. `timeout 100s labby
    // gateway code exec ...` SIGTERMs at the deadline). Without this the
    // default signal disposition kills the process before the drain and
    // orphans spawned stdio upstream children.
    let result = tokio::select! {
        result = dispatch_command(Arc::clone(&manager), args, format) => result,
        code = shutdown_signal() => Ok(ExitCode::from(code)),
    };
    // INVARIANT: drain the upstream pool before the one-shot CLI exits. The
    // manager is installed into a process-global (`install_gateway_manager`),
    // so `UpstreamConnection` Drop never runs at process exit and spawned
    // stdio upstream process groups (npx/uvx trees) would be orphaned —
    // repeated invocations leak hundreds of child processes.
    if let Some(pool) = manager.current_pool().await {
        pool.drain_for_swap("gateway.cli.exit").await;
    }
    result
}

/// Resolve with the conventional exit code (128 + signum) when SIGINT or
/// SIGTERM arrives.
async fn shutdown_signal() -> u8 {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        let sigterm_recv = async {
            match sigterm.as_mut() {
                Some(sig) => {
                    sig.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => 130,
            () = sigterm_recv => 143,
        }
    }
    #[cfg(not(unix))]
    {
        let _unused = tokio::signal::ctrl_c().await;
        130
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use clap::CommandFactory;
    use clap::Parser;

    use crate::cli::{Cli, Command};
    use crate::config::{
        LabConfig, UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode,
        UpstreamOauthRegistration,
    };
    use crate::oauth::upstream::encryption::load_key;
    use crate::oauth::upstream::runtime::build_upstream_oauth_runtime_from_parts;

    use super::{GatewayCommand, build_manager_with_upstream_oauth_runtime};

    #[test]
    fn gateway_cli_parser_accepts_expected_commands() {
        Cli::command().debug_assert();

        assert!(Cli::try_parse_from(["lab", "gateway", "list"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "get", "fixture-http"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "test", "--name", "fixture-http"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "add",
                "--name",
                "fixture-http",
                "--url",
                "http://127.0.0.1:8791",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "update",
                "fixture-http",
                "--proxy-resources",
                "true",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["lab", "gateway", "remove", "fixture-http"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "import", "--all", "--yes"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "quarantine", "list"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "quarantine", "restore", "plex"]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "reload"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "auth",
                "start",
                "fixture-http",
                "--open",
                "--wait",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "auth",
                "open",
                "fixture-http",
                "--wait",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "mcp", "auth", "status", "fixture-http",])
                .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "mcp", "auth", "clear", "fixture-http",])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["lab", "gateway", "mcp", "list",]).is_ok());
        assert!(Cli::try_parse_from(["lab", "gateway", "mcp", "enable", "fixture-http",]).is_ok());
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "disable",
                "fixture-http",
                "--cleanup",
                "--aggressive",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "mcp",
                "cleanup",
                "fixture-http",
                "--aggressive",
            ])
            .is_ok()
        );
        // Cloudflare-parity: only `gateway code exec` survives. Discovery stays
        // inside the Code Mode runtime.
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "code",
                "search",
                "--code",
                "async () => tools.slice(0, 3)",
            ])
            .is_err(),
            "`gateway code search` was removed per spec — only `gateway code exec` is supported"
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "code", "schema", "github::search_issues"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "lab",
                "gateway",
                "code",
                "exec",
                "--code",
                "await callTool(\"github::search_issues\", {query:\"repo\"})",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["lab", "gateway", "code", "exec", "--file", "snippet.js",])
                .is_ok()
        );
    }

    #[test]
    fn gateway_add_defaults_resource_proxying_on() {
        let cli = Cli::try_parse_from([
            "lab",
            "gateway",
            "add",
            "--name",
            "fixture-http",
            "--url",
            "http://127.0.0.1:8791",
        ])
        .expect("gateway add parses");

        let Command::Gateway(args) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::Add(args) = args.command else {
            panic!("expected gateway add command");
        };

        assert!(args.proxy_resources);
    }

    #[test]
    fn gateway_add_allows_resource_proxying_opt_out() {
        let cli = Cli::try_parse_from([
            "lab",
            "gateway",
            "add",
            "--name",
            "fixture-http",
            "--url",
            "http://127.0.0.1:8791",
            "--proxy-resources",
            "false",
        ])
        .expect("gateway add parses");

        let Command::Gateway(args) = cli.command else {
            panic!("expected gateway command");
        };
        let GatewayCommand::Add(args) = args.command else {
            panic!("expected gateway add command");
        };

        assert!(!args.proxy_resources);
    }

    #[tokio::test]
    async fn gateway_cli_manager_wires_upstream_oauth_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = LabConfig {
            upstream: vec![UpstreamConfig {
                name: "axon".to_string(),
                enabled: true,
                priority: 1.0,
                url: Some("https://axon.example.com/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: true,
                proxy_prompts: true,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: Some(UpstreamOauthConfig {
                    mode: UpstreamOauthMode::AuthorizationCodePkce,
                    registration: UpstreamOauthRegistration::Dynamic,
                    scopes: None,
                    prefer_client_metadata_document: None,
                }),
                imported_from: None,
            }],
            ..LabConfig::default()
        };
        let sqlite = lab_auth::sqlite::SqliteStore::open(dir.path().join("auth.sqlite"))
            .await
            .expect("sqlite store");
        let key_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [9_u8; 32]);
        let key = load_key(&key_b64).expect("encryption key");
        let oauth_runtime = build_upstream_oauth_runtime_from_parts(
            &config.upstream,
            sqlite,
            key,
            "https://lab.example.com/auth/upstream/callback".to_string(),
        );

        let manager =
            build_manager_with_upstream_oauth_runtime(&config, true, Some(oauth_runtime)).await;

        assert!(
            manager.upstream_oauth_manager("axon").is_some(),
            "gateway CLI manager must register OAuth managers for OAuth upstreams"
        );
        assert!(
            manager.oauth_client_cache().is_some(),
            "gateway CLI manager must install an OAuth client cache for Code Mode and upstream calls"
        );
    }
}
