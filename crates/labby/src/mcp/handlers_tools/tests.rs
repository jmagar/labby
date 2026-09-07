#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
//! Tests for tool-list/catalog visibility + upstream-pool resolution.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`). Duplicates the
//! small `completion_test_registry` fixture to keep this `tests.rs`
//! self-contained (per the test-distribution plan's minimal-duplication
//! guidance).

use crate::config::{
    GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
    ProtectedMcpRouteTarget,
};
use crate::dispatch::error::ToolError;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::{
    SkillExposurePolicy, ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};
use crate::mcp::catalog::ToolCatalogSnapshot;
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, CODE_MODE_UI_TOOL_NAME,
    GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME, SERVER_LOGS_TOOL_NAME, SETTINGS_TOOL_NAME,
};
use crate::mcp::handlers_resources::{
    ADD_SERVER_APP_SKYBRIDGE_URI, ADD_SERVER_APP_URI, CODE_MODE_APP_SKYBRIDGE_URI,
    CODE_MODE_APP_URI, CODE_MODE_APP_URI_PREFIX, GATEWAY_STATUS_APP_SKYBRIDGE_URI,
    GATEWAY_STATUS_APP_URI, MCP_APPS_APP_SKYBRIDGE_URI, MCP_APPS_APP_URI,
    SERVER_LOGS_APP_SKYBRIDGE_URI, SERVER_LOGS_APP_URI, SERVER_LOGS_APP_URI_PREFIX,
    SETTINGS_APP_SKYBRIDGE_URI, SETTINGS_APP_URI,
};
use crate::mcp::handlers_tools::{
    add_server_tool_meta, add_server_tool_schema, code_mode_tool_meta,
    code_mode_trace_output_schema, gateway_status_tool_meta, gateway_status_tool_schema,
    mcp_app_tool_meta, mcp_app_tool_schema, server_logs_tool_meta, settings_tool_meta,
    settings_tool_schema, strip_resource_backed_ui_meta,
};
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};
use labby_primitives::action::ActionSpec;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, ElicitationCapability,
    FormElicitationCapability, Implementation, MetaObject, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, RequestMetaObject, Tool,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};

const TEST_ACTIONS_ONE: &[ActionSpec] = &[
    ActionSpec {
        name: "status.get",
        description: "Get status",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "health.get",
        description: "Get health",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

const TEST_ACTIONS_TWO: &[ActionSpec] = &[
    ActionSpec {
        name: "status.list",
        description: "List status entries",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "health.list",
        description: "List health entries",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

// One counter per test, never shared.
//
// These previously shared a single `DESTRUCTIVE_DISPATCH_COUNT`. Because
// `cargo test` runs test fns on parallel threads inside one process, the two
// destructive-dispatch tests raced on it: each does `store(0)` then asserts an
// exact count, so either test's `store`/`fetch_add` could land inside the
// other's window. All four interleavings were observed (`left 1 right 0`,
// `left 2 right 1`, `left 0 right 1`, `left 2 right 1`), reproducing 21/25
// runs under `--test-threads=2`. `DispatchFn` is a bare fn pointer and cannot
// capture per-test state, so isolation comes from giving each test its own
// static plus its own dispatch fn rather than from locking or serialization.
static DESTRUCTIVE_DISPATCH_COUNT_NO_ELICITATION: AtomicUsize = AtomicUsize::new(0);
static DESTRUCTIVE_DISPATCH_COUNT_MRTR: AtomicUsize = AtomicUsize::new(0);

const DESTRUCTIVE_ACTIONS: &[ActionSpec] = &[ActionSpec {
    name: "danger.delete",
    description: "Delete danger",
    destructive: true,
    requires_admin: false,
    params: &[],
    returns: "object",
}];

fn noop_dispatch(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async { Ok(Value::Null) })
}

fn destructive_counting_dispatch_no_elicitation(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async {
        DESTRUCTIVE_DISPATCH_COUNT_NO_ELICITATION.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"ok": true}))
    })
}

fn destructive_counting_dispatch_mrtr(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async {
        DESTRUCTIVE_DISPATCH_COUNT_MRTR.fetch_add(1, Ordering::SeqCst);
        Ok(serde_json::json!({"ok": true}))
    })
}

fn completion_test_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredService {
        name: "hidden-upstream",
        description: "Hidden upstream",
        category: "network",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_ONE,
        dispatch: noop_dispatch,
    });
    registry.register(RegisteredService {
        name: "gateway-alpha",
        description: "Gateway alpha",
        category: "network",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_TWO,
        dispatch: noop_dispatch,
    });
    registry
}

/// Takes the dispatch fn so each destructive test owns its own counter.
fn destructive_test_registry(dispatch: crate::registry::DispatchFn) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredService {
        name: "danger",
        description: "Danger",
        category: "test",
        kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
        status: "available",
        actions: DESTRUCTIVE_ACTIONS,
        dispatch,
    });
    registry
}

fn large_test_registry(service_count: usize) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for index in 0..service_count {
        let name = Box::leak(format!("service_{index:03}").into_boxed_str());
        registry.register(RegisteredService {
            name,
            description: "Synthetic service",
            category: "test",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: TEST_ACTIONS_ONE,
            dispatch: noop_dispatch,
        });
    }
    registry
}

fn reverse_large_test_registry(service_count: usize) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for index in (0..service_count).rev() {
        let name = Box::leak(format!("service_{index:03}").into_boxed_str());
        registry.register(RegisteredService {
            name,
            description: "Synthetic service",
            category: "test",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: TEST_ACTIONS_ONE,
            dispatch: noop_dispatch,
        });
    }
    registry
}

fn test_server(
    registry: ToolRegistry,
    gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
    route_scope: crate::mcp::route_scope::McpRouteScope,
    logging_level: crate::mcp::logging::LoggingLevel,
) -> LabMcpServer {
    let code_mode_app_state = gateway_manager
        .as_ref()
        .map(|manager| manager.code_mode_app_state())
        .unwrap_or_default();
    LabMcpServer {
        registry: Arc::new(registry),
        access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        gateway_manager,
        peers: Default::default(),
        code_mode_app_state,
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        client_registry: Default::default(),
        transport_label: "test",
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(logging_level))),
        route_scope,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    }
}

async fn authorized_test_access_runtime() -> Arc<crate::access::AccessRuntime> {
    let directory = tempfile::Builder::new()
        .prefix("labby-mcp-access-test-")
        .tempdir_in(std::env::current_dir().expect("test working directory"))
        .expect("access tempdir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("secure access tempdir");
    }
    let runtime = Arc::new(
        crate::access::AccessRuntime::initialize(directory.keep().join("access.db")).await,
    );
    let identity = labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "static-bearer:primary",
    )
    .expect("static bearer identity");
    runtime
        .bootstrap_owner(
            crate::access::BootstrapOwnerInput::new(identity, "Local", "Default")
                .expect("bootstrap input"),
        )
        .await
        .expect("bootstrap access authority");
    runtime
}

fn primary_static_bearer_identity() -> labby_auth::VerifiedIdentity {
    labby_auth::VerifiedIdentity::local_credential(
        labby_auth::Authenticator::StaticBearer,
        "static-bearer:primary",
    )
    .expect("static bearer identity")
}

async fn code_mode_manager(
    enabled: bool,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled,
                    mcp_ui_enabled: true,
                    ..crate::config::CodeModeConfig::default()
                },
                mcp_apps: crate::config::McpAppsConfig {
                    manager: true,
                    add_server: true,
                    server_logs: true,
                    gateway_status: true,
                    settings: true,
                },
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

fn test_labby_runner_spawn() -> crate::dispatch::gateway::code_mode::RunnerSpawn {
    let current = std::env::current_exe().expect("current test executable");
    let debug_dir = current
        .parent()
        .and_then(std::path::Path::parent)
        .expect("target debug directory");
    let program = debug_dir.join(format!("labby{}", std::env::consts::EXE_SUFFIX));
    assert!(
        program.is_file(),
        "test Labby binary missing: {}",
        program.display()
    );
    crate::dispatch::gateway::code_mode::RunnerSpawn {
        program,
        args: vec!["internal".to_string(), "code-mode-runner".to_string()],
    }
}

async fn code_mode_manager_with_test_runner(
    enabled: bool,
    upstreams: Vec<crate::config::UpstreamConfig>,
    pool: Option<Arc<UpstreamPool>>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    if let Some(pool) = pool {
        runtime.swap(Some(pool)).await;
    }
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_code_mode_runner_spawn(test_labby_runner_spawn()),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled,
                    mcp_ui_enabled: true,
                    ..crate::config::CodeModeConfig::default()
                },
                mcp_apps: crate::config::McpAppsConfig {
                    manager: true,
                    add_server: true,
                    server_logs: true,
                    gateway_status: true,
                    settings: true,
                },
                upstream: upstreams,
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

async fn restricted_gateway_manager(
    allowed_actions: &[&str],
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "gateway".to_string(),
                    service: "gateway".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: allowed_actions
                            .iter()
                            .map(|action| (*action).to_string())
                            .collect(),
                    }),
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

#[cfg(feature = "skills")]
async fn restricted_skills_gateway_manager(
    allowed_actions: &[&str],
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "artifacts-list-only".to_string(),
                    service: "artifacts".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: allowed_actions
                            .iter()
                            .map(|action| (*action).to_string())
                            .collect(),
                    }),
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

async fn code_mode_manager_with_pool(
    enabled: bool,
    upstream: crate::config::UpstreamConfig,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    code_mode_manager_with_pool_and_upstreams(enabled, vec![upstream], pool).await
}

async fn code_mode_manager_with_pool_multi(
    enabled: bool,
    upstreams: Vec<crate::config::UpstreamConfig>,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    code_mode_manager_with_pool_and_upstreams(enabled, upstreams, pool).await
}

async fn code_mode_manager_with_pool_and_upstreams(
    enabled: bool,
    upstreams: Vec<crate::config::UpstreamConfig>,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    runtime.swap(Some(pool)).await;
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled,
                    mcp_ui_enabled: true,
                    ..crate::config::CodeModeConfig::default()
                },
                mcp_apps: crate::config::McpAppsConfig {
                    manager: true,
                    add_server: true,
                    server_logs: true,
                    gateway_status: true,
                    settings: true,
                },
                upstream: upstreams,
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

fn fixture_upstream_config(name: &str) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: Some("http://127.0.0.1:9/mcp".to_string()),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: true,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

fn fixture_oauth_upstream_config(name: &str) -> crate::config::UpstreamConfig {
    let mut config = fixture_upstream_config(name);
    config.oauth = Some(crate::config::UpstreamOauthConfig {
        mode: crate::config::UpstreamOauthMode::AuthorizationCodePkce,
        registration: crate::config::UpstreamOauthRegistration::Preregistered {
            client_id: "client-id".to_string(),
            client_secret_env: None,
        },
        scopes: None,
        credential: Default::default(),
        prefer_client_metadata_document: None,
    });
    config
}

fn fixture_upstream_entry(upstream: &str, tools: HashMap<String, UpstreamTool>) -> UpstreamEntry {
    UpstreamEntry {
        name: Arc::from(upstream),
        tools,
        exposure_policy: ToolExposurePolicy::All,
        resource_exposure_policy: ToolExposurePolicy::All,
        prompt_exposure_policy: ToolExposurePolicy::All,
        skill_exposure_policy: SkillExposurePolicy::all(),
        proxy_skills: false,
        supports_skills: None,
        proxy_resources: true,
        prompt_count: 0,
        resource_count: 1,
        skill_count: 0,
        skill_names: Vec::new(),
        prompt_names: Vec::new(),
        resource_uris: vec![format!("ui://{upstream}/app.html")],
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        skill_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        skill_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
        skill_last_error: None,
    }
}

fn fixture_upstream_tool(
    upstream: &Arc<str>,
    name: &str,
    ui_resource: Option<&str>,
) -> UpstreamTool {
    let mut tool = Tool::new(
        name.to_string(),
        format!("{name} description"),
        Arc::new(serde_json::Map::new()),
    );
    if let Some(resource_uri) = ui_resource {
        tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": resource_uri }),
        )])));
    }
    UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::clone(upstream),
        destructive: false,
    }
}

fn fixture_destructive_upstream_tool(upstream: &Arc<str>, name: &str) -> UpstreamTool {
    let mut tool = fixture_upstream_tool(upstream, name, None);
    tool.destructive = true;
    tool
}

fn scoped_context(
    peer: rmcp::service::Peer<rmcp::RoleServer>,
    scopes: &[&str],
) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    let mut context =
        rmcp::service::RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
    let mut parts = axum::http::Request::new(()).into_parts().0;
    parts
        .extensions
        .insert(labby_auth::auth_context::AuthContext {
            sub: "reader".to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
            issuer: "https://lab.example.com".to_string(),
            via_session: true,
            csrf_token: None,
            email: None,
        });
    context.extensions.insert(parts);
    context
}

fn request_context_with_peer(
    peer: rmcp::service::Peer<rmcp::RoleServer>,
) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    rmcp::service::RequestContext::new(rmcp::model::NumberOrString::Number(1), peer)
}

async fn call_tool_error_text(server: LabMcpServer, tool_name: &str) -> String {
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new(tool_name.to_string()), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    result.content[0].as_text().expect("text").text.clone()
}

#[tokio::test]
async fn call_tool_server_logs_requires_admin_scope() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new(SERVER_LOGS_TOOL_NAME).with_arguments(
            serde_json::Map::from_iter([
                (
                    "action".to_string(),
                    Value::String("server_logs.query".to_string()),
                ),
                ("params".to_string(), serde_json::json!({})),
            ]),
        ),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(text.contains("\"kind\":\"forbidden\""));
    assert!(text.contains("lab:admin"));
}

#[tokio::test]
async fn call_tool_add_server_opens_for_admin_without_reserving_hidden_name() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let denied = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(ADD_SERVER_TOOL_NAME),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("denied Add Server result");
    assert!(denied.is_error.unwrap_or(false));
    assert!(
        !denied.content[0]
            .as_text()
            .expect("text")
            .text
            .contains("lab:admin"),
        "a hidden synthetic tool must fall through instead of intercepting the name"
    );

    let opened = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(ADD_SERVER_TOOL_NAME),
            scoped_context(running.peer().clone(), &["lab:admin"]),
        )
        .await
        .expect("opened Add Server result");
    assert!(!opened.is_error.unwrap_or(false));
    assert_eq!(
        opened.structured_content.as_ref().expect("structured")["data"]["kind"],
        "add_server"
    );
}

#[tokio::test]
async fn add_server_app_obeys_gateway_action_policy_for_discovery_and_callbacks() {
    let manager = restricted_gateway_manager(&["gateway.test"]).await;
    assert!(
        manager
            .mcp_action_allowed_for_service("gateway", "gateway.test")
            .await
    );
    assert!(
        !manager
            .mcp_action_allowed_for_service("gateway", "gateway.add")
            .await
    );
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("restricted tools");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != ADD_SERVER_TOOL_NAME),
        "the app must be hidden unless test and add are both exposed"
    );

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("restricted resources");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with(ADD_SERVER_APP_URI)),
        "hidden tools must not leave a readable app resource advertised"
    );

    let hidden = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(ADD_SERVER_TOOL_NAME).with_arguments(
                serde_json::Map::from_iter([
                    ("action".to_string(), Value::String("create".to_string())),
                    ("params".to_string(), serde_json::json!({ "spec": {} })),
                ]),
            ),
            scoped_context(running.peer().clone(), &["lab:admin"]),
        )
        .await
        .expect("restricted create result");
    assert!(hidden.is_error.unwrap_or(false));
    let text = hidden.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("gateway.add"),
        "a hidden synthetic tool must fall through instead of intercepting the name: {text}"
    );
}

#[tokio::test]
async fn add_server_app_handles_missing_gateway_registry_without_panicking() {
    let server = test_server(
        ToolRegistry::new(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("tools without gateway registry");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != ADD_SERVER_TOOL_NAME)
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(ADD_SERVER_TOOL_NAME).with_arguments(
                serde_json::Map::from_iter([
                    ("action".to_string(), Value::String("test".to_string())),
                    ("params".to_string(), serde_json::json!({ "spec": {} })),
                ]),
            ),
            scoped_context(running.peer().clone(), &["lab:admin"]),
        )
        .await
        .expect("missing gateway registry result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("gateway registry entry not wired"),
        "a non-advertised synthetic tool must not intercept a potentially upstream-owned name"
    );
}

#[tokio::test]
async fn add_server_app_is_hidden_without_gateway_manager() {
    let server = test_server(
        crate::registry::build_default_registry(),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("tools without gateway manager");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != ADD_SERVER_TOOL_NAME)
    );
}

#[tokio::test]
async fn hidden_add_server_name_is_reserved_from_discovery_but_legacy_call_still_routes() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let upstream_tool = fixture_upstream_tool(&upstream_name, ADD_SERVER_TOOL_NAME, None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([(ADD_SERVER_TOOL_NAME.to_string(), upstream_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(false, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = scoped_context(running.peer().clone(), &["lab:read"]);

    let tools = running
        .service()
        .list_tools_impl(None, context.clone())
        .await
        .expect("read-scope tools");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != ADD_SERVER_TOOL_NAME),
        "Labby-owned names stay conservatively reserved even when their synthetic app is hidden"
    );

    let result = running
        .service()
        .call_tool_impl(CallToolRequestParams::new(ADD_SERVER_TOOL_NAME), context)
        .await
        .expect("upstream Add Server result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("upstream_error") && !text.contains("lab:admin"),
        "Legacy calls retain their existing upstream fallback even though discovery is conservative: {text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_builtin_when_elicitation_is_not_supported() {
    DESTRUCTIVE_DISPATCH_COUNT_NO_ELICITATION.store(0, Ordering::SeqCst);
    let server = test_server(
        destructive_test_registry(destructive_counting_dispatch_no_elicitation),
        None,
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "danger-only",
            std::iter::empty::<&str>(),
            ["danger"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("danger").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_string(),
                Value::String("danger.delete".to_string()),
            ),
            ("params".to_string(), serde_json::json!({})),
        ])),
        request_context_with_peer(running.peer().clone()),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert_eq!(
        DESTRUCTIVE_DISPATCH_COUNT_NO_ELICITATION.load(Ordering::SeqCst),
        0,
        "unsupported elicitation must fail closed before destructive dispatch"
    );
}

fn destructive_request() -> CallToolRequestParams {
    CallToolRequestParams::new("danger").with_arguments(serde_json::Map::from_iter([
        (
            "action".to_string(),
            Value::String("danger.delete".to_string()),
        ),
        ("params".to_string(), serde_json::json!({})),
    ]))
}

fn request_context_with_elicitation(
    peer: rmcp::service::Peer<rmcp::RoleServer>,
) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    let capabilities = ClientCapabilities::builder()
        .enable_elicitation_with(
            ElicitationCapability::new().with_form(FormElicitationCapability::new()),
        )
        .build();
    let mut context = request_context_with_peer(peer);
    context.meta = RequestMetaObject::with_client_context(
        ProtocolVersion::V_2026_07_28,
        Implementation::new("test-client", "1.0.0"),
        capabilities,
    );
    context
}

#[tokio::test]
async fn destructive_builtin_uses_single_use_bound_mrtr_elicitation() {
    DESTRUCTIVE_DISPATCH_COUNT_MRTR.store(0, Ordering::SeqCst);
    let server = test_server(
        destructive_test_registry(destructive_counting_dispatch_mrtr),
        None,
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "danger-only",
            std::iter::empty::<&str>(),
            ["danger"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let first = running
        .service()
        .call_tool(
            destructive_request(),
            request_context_with_elicitation(running.peer().clone()),
        )
        .await
        .expect("input required");
    let CallToolResponse::InputRequired(input_required) = first else {
        panic!("destructive call must return input_required");
    };
    let request_state = input_required
        .request_state
        .expect("destructive challenge has server-owned state");
    assert_eq!(DESTRUCTIVE_DISPATCH_COUNT_MRTR.load(Ordering::SeqCst), 0);

    let mut retry = destructive_request();
    retry.request_state = Some(request_state);
    retry.input_responses = Some(BTreeMap::from([(
        "destructive_confirmation".to_string(),
        serde_json::json!({"action": "accept", "content": {"confirm": true}}),
    )]));
    let second = running
        .service()
        .call_tool(
            retry.clone(),
            request_context_with_elicitation(running.peer().clone()),
        )
        .await
        .expect("completed retry");
    assert!(matches!(second, CallToolResponse::Complete(_)));
    assert_eq!(DESTRUCTIVE_DISPATCH_COUNT_MRTR.load(Ordering::SeqCst), 1);

    let replay = running
        .service()
        .call_tool(
            retry,
            request_context_with_elicitation(running.peer().clone()),
        )
        .await
        .expect("replay receives a protocol result");
    let CallToolResponse::Complete(replay) = replay else {
        panic!("replay denial must be a complete error result");
    };
    assert_eq!(replay.is_error, Some(true));
    assert_eq!(DESTRUCTIVE_DISPATCH_COUNT_MRTR.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn destructive_confirmation_cannot_cross_mcp_sessions_and_is_burned() {
    DESTRUCTIVE_DISPATCH_COUNT_MRTR.store(0, Ordering::SeqCst);
    let first_server = test_server(
        destructive_test_registry(destructive_counting_dispatch_mrtr),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Info,
    );
    let mut second_server = test_server(
        destructive_test_registry(destructive_counting_dispatch_mrtr),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Info,
    );
    second_server.relay_session_id = 1;
    let (first_transport, _first_client) = tokio::io::duplex(64);
    let first = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        first_server,
        first_transport,
        None,
    );
    let (second_transport, _second_client) = tokio::io::duplex(64);
    let second = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        second_server,
        second_transport,
        None,
    );

    let CallToolResponse::InputRequired(challenge) = first
        .service()
        .call_tool(
            destructive_request(),
            request_context_with_elicitation(first.peer().clone()),
        )
        .await
        .expect("challenge")
    else {
        panic!("destructive call must return input_required");
    };
    let mut retry = destructive_request();
    retry.request_state = challenge.request_state;
    retry.input_responses = Some(BTreeMap::from([(
        "destructive_confirmation".to_string(),
        serde_json::json!({"action": "accept", "content": {"confirm": true}}),
    )]));

    let cross_session = second
        .service()
        .call_tool(
            retry.clone(),
            request_context_with_elicitation(second.peer().clone()),
        )
        .await
        .expect("cross-session denial");
    let CallToolResponse::Complete(cross_session) = cross_session else {
        panic!("cross-session retry must be denied");
    };
    assert_eq!(cross_session.is_error, Some(true));

    let burned = first
        .service()
        .call_tool(
            retry,
            request_context_with_elicitation(first.peer().clone()),
        )
        .await
        .expect("burned-state denial");
    let CallToolResponse::Complete(burned) = burned else {
        panic!("burned retry must be denied");
    };
    assert_eq!(burned.is_error, Some(true));
    assert_eq!(DESTRUCTIVE_DISPATCH_COUNT_MRTR.load(Ordering::SeqCst), 0);
}

#[test]
fn code_mode_ui_tool_meta_points_to_canonical_ui_resource() {
    let codemode = code_mode_tool_meta(CODE_MODE_UI_TOOL_NAME);

    // The binding URI carries a `?v=<hash>` cache-bust token (so a rebuilt widget
    // forces the host to refetch), but resolves to the canonical base URI.
    let codemode_ui = codemode.0["ui"]["resourceUri"]
        .as_str()
        .expect("codemode resourceUri");
    assert!(codemode_ui.starts_with(CODE_MODE_APP_URI));
    assert!(codemode_ui.contains("?v="));
    // OpenAI Apps hosts (ChatGPT / Codex) bind widgets via `openai/outputTemplate`
    // rather than `_meta.ui`. It points at the skybridge variant (same HTML, the
    // `text/html+skybridge` MIME those hosts expect) so the Claude resource is
    // untouched.
    let codemode_skybridge = codemode
        .0
        .get("openai/outputTemplate")
        .and_then(|value| value.as_str())
        .expect("codemode openai/outputTemplate");
    assert!(
        codemode_skybridge.starts_with(CODE_MODE_APP_SKYBRIDGE_URI),
        "codemode tool must expose the OpenAI Apps output template"
    );
    assert!(codemode_skybridge.contains("?v="));
}

#[test]
fn mcp_app_schema_and_meta_cover_managed_apps() {
    let schema = mcp_app_tool_schema();
    assert_eq!(
        schema["properties"]["action"]["enum"],
        serde_json::json!(["status", "enable", "disable"])
    );
    assert_eq!(
        schema["properties"]["target"]["enum"],
        serde_json::json!([
            "manager",
            "codemode",
            "gateway_status",
            "server_logs",
            "add_server",
            "settings",
            "all"
        ])
    );
    assert_eq!(schema["properties"]["target"]["default"], "codemode");
    assert_eq!(
        schema["properties"]["params"]["properties"]["target"]["enum"],
        schema["properties"]["target"]["enum"]
    );
    assert_eq!(
        schema["properties"]["params"]["additionalProperties"],
        false
    );
    assert_eq!(schema["additionalProperties"], false);

    let meta = mcp_app_tool_meta(MCP_APP_TOOL_NAME);
    assert!(
        meta.0["ui"]["resourceUri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(MCP_APPS_APP_URI) && uri.contains("?v="))
    );
    assert!(
        meta.0["openai/outputTemplate"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(MCP_APPS_APP_SKYBRIDGE_URI))
    );
}

#[test]
fn server_logs_tool_meta_points_to_log_viewer_ui_resource() {
    let server_logs = server_logs_tool_meta(SERVER_LOGS_TOOL_NAME);

    let ui = server_logs.0["ui"]["resourceUri"]
        .as_str()
        .expect("server logs resourceUri");
    assert!(ui.starts_with(SERVER_LOGS_APP_URI));
    assert!(ui.contains("?v="));

    let skybridge = server_logs
        .0
        .get("openai/outputTemplate")
        .and_then(|value| value.as_str())
        .expect("server logs openai/outputTemplate");
    assert!(
        skybridge.starts_with(SERVER_LOGS_APP_SKYBRIDGE_URI),
        "server_logs tool must expose the OpenAI Apps output template"
    );
    assert!(skybridge.contains("?v="));
}

#[test]
fn add_server_tool_meta_and_schema_bind_the_create_app() {
    let meta = add_server_tool_meta(ADD_SERVER_TOOL_NAME);
    assert!(
        meta.0["ui"]["resourceUri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(ADD_SERVER_APP_URI) && uri.contains("?v="))
    );
    assert!(
        meta.0["openai/outputTemplate"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(ADD_SERVER_APP_SKYBRIDGE_URI))
    );
    let schema = add_server_tool_schema();
    assert_eq!(
        schema["properties"]["action"]["enum"],
        serde_json::json!(["open", "test", "create"])
    );
    assert_eq!(schema["additionalProperties"], false);
    let spec = &schema["properties"]["params"]["properties"]["spec"];
    assert_eq!(spec["required"], serde_json::json!(["name"]));
    assert_eq!(spec["oneOf"].as_array().map(Vec::len), Some(2));
    for field in [
        "name",
        "url",
        "command",
        "args",
        "enabled",
        "proxy_resources",
        "proxy_prompts",
    ] {
        assert!(spec["properties"].get(field).is_some(), "missing {field}");
    }
}

#[test]
fn gateway_status_tool_meta_and_schema_bind_the_status_app() {
    let meta = gateway_status_tool_meta(GATEWAY_STATUS_TOOL_NAME);
    assert!(
        meta.0["ui"]["resourceUri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(GATEWAY_STATUS_APP_URI) && uri.contains("?v="))
    );
    assert!(
        meta.0["openai/outputTemplate"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(GATEWAY_STATUS_APP_SKYBRIDGE_URI))
    );
    let schema = gateway_status_tool_schema();
    assert_eq!(
        schema["properties"]["action"]["enum"],
        serde_json::json!(["open", "refresh"])
    );
    assert!(
        schema["properties"]["action"]["description"]
            .as_str()
            .is_some_and(|description| description.contains("reprobe"))
    );
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn settings_tool_meta_and_schema_bind_the_settings_app() {
    let meta = settings_tool_meta(SETTINGS_TOOL_NAME);
    assert!(
        meta.0["ui"]["resourceUri"]
            .as_str()
            .is_some_and(|uri| { uri.starts_with(SETTINGS_APP_URI) && uri.contains("?v=") })
    );
    assert!(
        meta.0["openai/outputTemplate"].as_str().is_some_and(|uri| {
            uri.starts_with(SETTINGS_APP_SKYBRIDGE_URI) && uri.contains("?v=")
        })
    );
    let schema = settings_tool_schema();
    let actions = schema["properties"]["action"]["enum"]
        .as_array()
        .expect("Settings actions");
    assert!(actions.contains(&serde_json::json!("state")));
    assert!(actions.contains(&serde_json::json!("config.update")));
}

#[test]
fn code_mode_trace_output_schema_advertises_structured_trace_kinds() {
    let schema = code_mode_trace_output_schema();
    assert_eq!(schema["type"].as_str(), Some("object"));

    let variants = schema["oneOf"].as_array().expect("oneOf variants");
    let kinds = variants
        .iter()
        .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["code_mode_execute_trace"]);
}

#[tokio::test]
async fn list_tools_advertises_code_mode_output_schemas() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let codemode = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME)
        .expect("codemode tool");
    let codemode_read = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_READ_TOOL_NAME)
        .expect("codemode_read tool");
    assert_eq!(
        codemode.input_schema["properties"]["code"]["minLength"],
        serde_json::json!(1),
        "codemode must advertise non-empty code"
    );
    assert!(
        codemode
            .meta
            .as_ref()
            .is_some_and(|meta| !meta.0.contains_key("ui")),
        "codemode must remain text-only"
    );
    let read_annotations = codemode_read
        .annotations
        .as_ref()
        .expect("read annotations");
    assert_eq!(read_annotations.read_only_hint, Some(true));
    assert_eq!(read_annotations.destructive_hint, Some(false));
    assert_eq!(read_annotations.idempotent_hint, Some(true));
    assert_eq!(read_annotations.open_world_hint, Some(true));
    let full_annotations = codemode.annotations.as_ref().expect("full annotations");
    assert_eq!(full_annotations.read_only_hint, Some(false));
    assert_eq!(full_annotations.destructive_hint, Some(true));
    assert_eq!(full_annotations.idempotent_hint, Some(false));
    assert_eq!(full_annotations.open_world_hint, Some(true));
    let schema = codemode.output_schema.as_ref().expect("outputSchema");
    let kinds = schema["oneOf"]
        .as_array()
        .expect("oneOf variants")
        .iter()
        .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["code_mode_execute_trace"]);

    let codemode_ui = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_UI_TOOL_NAME)
        .expect("codemode_ui tool");
    assert_eq!(codemode_ui.input_schema, codemode.input_schema);
    assert_eq!(codemode_ui.output_schema, codemode.output_schema);
    assert!(
        codemode_ui.meta.is_some(),
        "codemode_ui must own app metadata"
    );
    assert_eq!(codemode_ui.annotations, codemode.annotations);

    let control = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == MCP_APP_TOOL_NAME)
        .expect("mcp_app tool");
    let control_meta = control
        .meta
        .as_ref()
        .expect("mcp_app must own app metadata");
    assert!(
        control_meta.0["ui"]["resourceUri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(MCP_APPS_APP_URI))
    );
}

#[tokio::test]
async fn mcp_app_control_tool_survives_manager_ui_disable() {
    let manager = code_mode_manager(true).await;
    manager
        .set_mcp_app_visibility("manager", false, None)
        .await
        .expect("disable manager UI");
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("tools with manager UI disabled");
    let control = tools
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == MCP_APP_TOOL_NAME)
        .expect("mcp_app control tool");
    assert!(
        control
            .meta
            .as_ref()
            .is_some_and(|meta| !meta.0.contains_key("ui")),
        "manager UI metadata must be opt-in"
    );

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("resources with manager UI disabled");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with(MCP_APPS_APP_URI))
    );
    running
        .service()
        .read_resource_impl(
            ReadResourceRequestParams::new(MCP_APPS_APP_URI),
            scoped_context(peer, &["lab:admin"]),
        )
        .await
        .expect_err("disabled manager UI resource must be unreadable");
}

#[tokio::test]
async fn mcp_app_manager_stays_visible_when_code_mode_is_disabled() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("tools with code mode disabled");
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == MCP_APP_TOOL_NAME)
    );
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != CODE_MODE_UI_TOOL_NAME)
    );

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(peer, &["lab:admin"]))
        .await
        .expect("resources with code mode disabled");
    assert!(
        resources
            .resources
            .iter()
            .any(|resource| resource.uri.starts_with(MCP_APPS_APP_URI))
    );
}

#[tokio::test]
async fn manager_config_is_authoritative_over_a_stale_code_mode_app_mirror() {
    let manager = code_mode_manager(true).await;
    let server = test_server(
        completion_test_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state.set_enabled(false);
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    assert!(running.service().code_mode_app_enabled_on_mcp().await);
    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab"]))
        .await
        .expect("tools from manager-backed config");
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == CODE_MODE_UI_TOOL_NAME),
        "manager config must win over a stale disabled session mirror"
    );

    manager
        .set_mcp_app_visibility("codemode", false, None)
        .await
        .expect("disable Code Mode app");
    running.service().code_mode_app_state.set_enabled(true);
    assert!(!running.service().code_mode_app_enabled_on_mcp().await);
    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab"]))
        .await
        .expect("tools after disabling manager config");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != CODE_MODE_UI_TOOL_NAME),
        "stale enabled session mirror must not resurrect a disabled app"
    );
    let denied = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_UI_TOOL_NAME),
            scoped_context(peer, &["lab"]),
        )
        .await
        .expect("disabled UI call result");
    assert!(denied.is_error.unwrap_or(false));
    assert!(
        denied.content[0]
            .as_text()
            .expect("text")
            .text
            .contains("app_disabled"),
    );
}

#[tokio::test]
async fn mcp_app_status_reports_runtime_state() {
    let manager = code_mode_manager(true).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state = manager.code_mode_app_state();
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "status", "target": "codemode" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab"]),
        )
        .await
        .expect("mcp_app status result");

    assert!(!result.is_error.unwrap_or(false));
    let structured = result.structured_content.expect("structured status");
    assert_eq!(structured["kind"], "mcp_app_control");
    assert_eq!(
        structured["enabled"],
        running.service().code_mode_app_state.is_enabled()
    );
    assert_eq!(structured["text_tool"], CODE_MODE_TOOL_NAME);
    assert_eq!(structured["ui_tool"], CODE_MODE_UI_TOOL_NAME);
    assert_eq!(structured["changed"], false);
    assert!(
        result.meta.is_none(),
        "control result must not attach UI metadata"
    );
}

#[tokio::test]
async fn mcp_app_enable_is_idempotent_for_admin_scope() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "enable", "target": "codemode" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:admin"]),
        )
        .await
        .expect("mcp_app enable result");

    assert!(!result.is_error.unwrap_or(false));
    let structured = result.structured_content.expect("structured enable result");
    assert_eq!(structured["enabled"], true);
    assert_eq!(structured["changed"], false);
    assert_eq!(structured["notification_scheduled"], false);
    assert!(
        result.meta.is_none(),
        "control result must not attach UI metadata"
    );
}

#[tokio::test]
async fn mcp_app_disable_hides_ui_surface_and_enable_restores_it() {
    let manager = code_mode_manager(true).await;
    let shared_state = manager.code_mode_app_state();
    let mut server = test_server(
        completion_test_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state = shared_state.clone();

    let mut sibling_session = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    sibling_session.code_mode_app_state = shared_state;
    let independent_manager = code_mode_manager(true).await;
    let mut independent_gateway = test_server(
        completion_test_registry(),
        Some(Arc::clone(&independent_manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    independent_gateway.code_mode_app_state = independent_manager.code_mode_app_state();

    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    let disable = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "disable", "target": "codemode" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(peer.clone(), &["lab:admin"]),
        )
        .await
        .expect("mcp_app disable result");
    assert!(!disable.is_error.unwrap_or(false));
    let structured = disable
        .structured_content
        .expect("structured disable result");
    assert_eq!(structured["enabled"], false);
    assert_eq!(structured["changed"], true);
    assert_eq!(structured["scope"], "gateway");
    assert_eq!(structured["notification_scheduled"], true);
    assert!(!running.service().code_mode_app_state.is_enabled());
    assert!(!sibling_session.code_mode_app_state.is_enabled());
    assert!(independent_gateway.code_mode_app_state.is_enabled());
    assert!(!manager.code_mode_config().await.mcp_ui_enabled);

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("tools after disable");
    let names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
    assert!(names.contains(&MCP_APP_TOOL_NAME));
    assert!(!names.contains(&CODE_MODE_UI_TOOL_NAME));

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(peer.clone(), &["lab:read"]))
        .await
        .expect("resources after disable");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| { !resource.uri.starts_with("ui://lab/code-mode/") })
    );

    let stale_ui_call = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_UI_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(peer.clone(), &["lab"]),
        )
        .await
        .expect("stale codemode_ui call result");
    assert!(stale_ui_call.is_error.unwrap_or(false));
    let text = stale_ui_call.content[0]
        .as_text()
        .expect("text")
        .text
        .as_str();
    assert!(text.contains("app_disabled"), "{text}");

    let stale_resource = running
        .service()
        .read_resource_impl(
            ReadResourceRequestParams::new(CODE_MODE_APP_URI),
            scoped_context(peer.clone(), &["lab:read"]),
        )
        .await
        .expect_err("cached app resource must stay hidden while disabled");
    assert!(
        stale_resource.message.contains("unknown UI resource"),
        "cached resource should be hidden as unknown: {stale_resource:?}"
    );

    let enable = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "enable", "target": "codemode" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(peer.clone(), &["lab:admin"]),
        )
        .await
        .expect("mcp_app enable result");
    let structured = enable.structured_content.expect("structured enable result");
    assert_eq!(structured["enabled"], true);
    assert_eq!(structured["changed"], true);
    assert!(running.service().code_mode_app_state.is_enabled());
    assert!(sibling_session.code_mode_app_state.is_enabled());
    assert!(manager.code_mode_config().await.mcp_ui_enabled);

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer, &["lab:admin"]))
        .await
        .expect("tools after re-enable");
    assert!(
        tools
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == CODE_MODE_UI_TOOL_NAME)
    );
}

#[tokio::test]
async fn mcp_app_individual_disable_only_changes_selected_surface() {
    let manager = code_mode_manager(true).await;
    let shared_state = manager.code_mode_app_state();
    let mut server = test_server(
        crate::registry::build_default_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state = shared_state;
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({
                    "action": "disable",
                    "params": { "target": "server_logs" }
                })
                .as_object()
                .expect("object")
                .clone(),
            ),
            scoped_context(peer.clone(), &["lab:admin"]),
        )
        .await
        .expect("individual disable result");
    assert!(!result.is_error.unwrap_or(false));
    let structured = result
        .structured_content
        .expect("structured disable result");
    assert_eq!(structured["target"], "server_logs");
    assert_eq!(structured["enabled"], false);
    assert_eq!(structured["changed"], true);
    assert_eq!(structured["apps"]["server_logs"]["enabled"], false);
    for target in ["manager", "codemode", "gateway_status", "add_server"] {
        assert_eq!(structured["apps"][target]["enabled"], true, "{target}");
    }

    let cfg = manager.current_config().await;
    assert!(cfg.mcp_apps.manager);
    assert!(cfg.code_mode.mcp_ui_enabled);
    assert!(cfg.mcp_apps.gateway_status);
    assert!(!cfg.mcp_apps.server_logs);
    assert!(cfg.mcp_apps.add_server);
    assert!(cfg.mcp_apps.settings);
    assert!(running.service().code_mode_app_state.is_enabled());

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("tools after individual disable");
    for visible in [
        MCP_APP_TOOL_NAME,
        CODE_MODE_UI_TOOL_NAME,
        GATEWAY_STATUS_TOOL_NAME,
        ADD_SERVER_TOOL_NAME,
    ] {
        assert!(
            tools.tools.iter().any(|tool| tool.name.as_ref() == visible),
            "{visible} should stay visible"
        );
    }
    let logs = tools
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == SERVER_LOGS_TOOL_NAME)
        .expect("server_logs text service remains available");
    assert!(
        logs.meta
            .as_ref()
            .is_some_and(|meta| !meta.0.contains_key("ui")),
        "only server_logs app metadata should be hidden"
    );

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(peer, &["lab:admin"]))
        .await
        .expect("resources after individual disable");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with(SERVER_LOGS_APP_URI_PREFIX))
    );
    for visible_prefix in [
        MCP_APPS_APP_URI,
        CODE_MODE_APP_URI_PREFIX,
        GATEWAY_STATUS_APP_URI,
        ADD_SERVER_APP_URI,
        SETTINGS_APP_URI,
    ] {
        assert!(
            resources
                .resources
                .iter()
                .any(|resource| resource.uri.starts_with(visible_prefix)),
            "{visible_prefix} should stay visible"
        );
    }
}

#[tokio::test]
async fn mcp_app_manager_is_hidden_and_denied_on_protected_routes() {
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "ops",
        ["gateway-alpha"],
        ["gateway", "server_logs"],
        true,
    );
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        scope,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("protected tools");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != MCP_APP_TOOL_NAME)
    );

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("protected resources");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with(MCP_APPS_APP_URI))
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "status", "target": "all" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(peer, &["lab:admin"]),
        )
        .await
        .expect("protected manager denial");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(text.contains("root gateway route"), "{text}");
}

#[tokio::test]
async fn mcp_app_bulk_disable_hides_managed_apps_but_keeps_manager() {
    let manager = code_mode_manager(true).await;
    let shared_state = manager.code_mode_app_state();
    let mut server = test_server(
        crate::registry::build_default_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state = shared_state;
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let peer = running.peer().clone();

    // The shared app host wraps action params under `params`; exercise that shape
    // instead of only the direct top-level MCP schema used by text clients.
    let disable = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "disable", "params": { "target": "all" } })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(peer.clone(), &["lab:admin"]),
        )
        .await
        .expect("bulk disable result");
    assert!(!disable.is_error.unwrap_or(false));
    let structured = disable.structured_content.expect("structured bulk disable");
    assert_eq!(structured["target"], "all");
    assert_eq!(structured["enabled"], false);
    assert_eq!(structured["changed"], true);
    for target in [
        "manager",
        "codemode",
        "gateway_status",
        "server_logs",
        "add_server",
        "settings",
    ] {
        assert_eq!(structured["apps"][target]["enabled"], false, "{target}");
    }

    let cfg = manager.current_config().await;
    assert!(!cfg.mcp_apps.manager);
    assert!(!cfg.code_mode.mcp_ui_enabled);
    assert!(!cfg.mcp_apps.gateway_status);
    assert!(!cfg.mcp_apps.server_logs);
    assert!(!cfg.mcp_apps.add_server);
    assert!(!cfg.mcp_apps.settings);
    assert!(!running.service().code_mode_app_state.is_enabled());

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("tools after bulk disable");
    let names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
    assert!(names.contains(&MCP_APP_TOOL_NAME));
    assert!(!names.contains(&CODE_MODE_UI_TOOL_NAME));
    assert!(!names.contains(&GATEWAY_STATUS_TOOL_NAME));
    assert!(!names.contains(&ADD_SERVER_TOOL_NAME));
    assert!(!names.contains(&SETTINGS_TOOL_NAME));
    let manager_tool = tools
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == MCP_APP_TOOL_NAME)
        .expect("mcp_app control tool remains available");
    assert!(
        manager_tool
            .meta
            .as_ref()
            .is_some_and(|meta| !meta.0.contains_key("ui")),
        "disabled manager UI must leave the control tool text-only"
    );
    let logs = tools
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == SERVER_LOGS_TOOL_NAME)
        .expect("server_logs text service remains available");
    assert!(
        logs.meta
            .as_ref()
            .is_some_and(|meta| !meta.0.contains_key("ui")),
        "server_logs app metadata must be disabled"
    );

    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(peer.clone(), &["lab:admin"]))
        .await
        .expect("resources after bulk disable");
    for hidden_prefix in [
        MCP_APPS_APP_URI,
        CODE_MODE_APP_URI_PREFIX,
        GATEWAY_STATUS_APP_URI,
        SERVER_LOGS_APP_URI,
        ADD_SERVER_APP_URI,
        SETTINGS_APP_URI,
    ] {
        assert!(
            resources
                .resources
                .iter()
                .all(|resource| !resource.uri.starts_with(hidden_prefix)),
            "managed resource remained listed: {hidden_prefix}"
        );
    }
    for stale_uri in [
        MCP_APPS_APP_URI,
        CODE_MODE_APP_URI,
        GATEWAY_STATUS_APP_URI,
        SERVER_LOGS_APP_URI,
        ADD_SERVER_APP_URI,
        SETTINGS_APP_URI,
    ] {
        let stale = running
            .service()
            .read_resource_impl(
                ReadResourceRequestParams::new(stale_uri),
                scoped_context(peer.clone(), &["lab:admin"]),
            )
            .await
            .expect_err("disabled app resource must be unreadable");
        assert!(stale.message.contains("unknown UI resource"), "{stale:?}");
    }

    let enable = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "enable", "target": "all" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(peer, &["lab:admin"]),
        )
        .await
        .expect("bulk enable result");
    assert!(!enable.is_error.unwrap_or(false));
    let cfg = manager.current_config().await;
    assert!(cfg.mcp_apps.manager);
    assert!(cfg.code_mode.mcp_ui_enabled);
    assert!(cfg.mcp_apps.gateway_status);
    assert!(cfg.mcp_apps.server_logs);
    assert!(cfg.mcp_apps.add_server);
    assert!(cfg.mcp_apps.settings);
    assert!(running.service().code_mode_app_state.is_enabled());
}

#[tokio::test]
async fn mcp_app_mutation_requires_admin_scope() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(MCP_APP_TOOL_NAME).with_arguments(
                serde_json::json!({ "action": "disable", "target": "codemode" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab"]),
        )
        .await
        .expect("mcp_app forbidden result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(text.contains("lab:admin"), "{text}");
}

#[tokio::test]
async fn mcp_app_rejects_malformed_control_shape_without_mutation() {
    let manager = code_mode_manager(true).await;
    let server = test_server(
        completion_test_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    for (arguments, expected_param) in [
        (
            serde_json::json!({
                "action": "disable",
                "target": 7,
                "params": { "target": "manager" }
            }),
            "target",
        ),
        (
            serde_json::json!({ "action": "disable", "params": "manager" }),
            "params",
        ),
        (
            serde_json::json!({ "action": 7, "target": "manager" }),
            "action",
        ),
        (
            serde_json::json!({
                "action": "disable",
                "target": "manager",
                "params": { "target": "codemode" }
            }),
            "target",
        ),
    ] {
        let result = running
            .service()
            .call_tool_impl(
                CallToolRequestParams::new(MCP_APP_TOOL_NAME)
                    .with_arguments(arguments.as_object().expect("object").clone()),
                scoped_context(running.peer().clone(), &["lab:admin"]),
            )
            .await
            .expect("malformed mcp_app result");
        assert!(result.is_error.unwrap_or(false));
        let text = result.content[0].as_text().expect("text").text.as_str();
        assert!(text.contains("invalid_param"), "{text}");
        assert!(text.contains(expected_param), "{text}");

        let cfg = manager.current_config().await;
        assert!(cfg.mcp_apps.manager);
        assert!(cfg.code_mode.mcp_ui_enabled);
        assert!(cfg.mcp_apps.gateway_status);
        assert!(cfg.mcp_apps.server_logs);
        assert!(cfg.mcp_apps.add_server);
        assert!(cfg.mcp_apps.settings);
    }
}

#[tokio::test]
async fn list_tools_keeps_server_logs_visible_when_code_mode_hides_raw_tools() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .list_tools_impl(None, request_context_with_peer(running.peer().clone()))
        .await
        .expect("list tools");

    let tool = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == SERVER_LOGS_TOOL_NAME)
        .expect("server_logs should remain visible as an app-backed operator tool");
    let meta = tool.meta.as_ref().expect("server_logs meta");
    assert!(
        meta.0["ui"]["resourceUri"]
            .as_str()
            .is_some_and(|uri| uri.starts_with(SERVER_LOGS_APP_URI)),
        "server_logs should advertise MCP app metadata"
    );
    assert!(
        meta.0
            .get("openai/outputTemplate")
            .and_then(|value| value.as_str())
            .is_some_and(|uri| uri.starts_with(SERVER_LOGS_APP_SKYBRIDGE_URI)),
        "server_logs should advertise ChatGPT output template"
    );
}

#[tokio::test]
async fn list_tools_advertises_add_server_app_only_to_admins() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let denied = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
        .await
        .expect("read-scope tools");
    assert!(
        denied
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != ADD_SERVER_TOOL_NAME)
    );

    let allowed = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("admin tools");
    let tool = allowed
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == ADD_SERVER_TOOL_NAME)
        .expect("Add Server tool");
    assert!(
        tool.meta
            .as_ref()
            .and_then(|meta| meta.0["ui"]["resourceUri"].as_str())
            .is_some_and(|uri| uri.starts_with(ADD_SERVER_APP_URI))
    );
}

#[tokio::test]
async fn gateway_status_app_is_admin_only_and_returns_gateway_list() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let denied_tools = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
        .await
        .expect("read-scope tools");
    assert!(
        denied_tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != GATEWAY_STATUS_TOOL_NAME)
    );
    let denied_call = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(GATEWAY_STATUS_TOOL_NAME),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("hidden status call");
    assert!(denied_call.is_error.unwrap_or(false));

    let admin_tools = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("admin tools");
    let status_tool = admin_tools
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == GATEWAY_STATUS_TOOL_NAME)
        .expect("Gateway Status tool");
    assert!(
        status_tool
            .meta
            .as_ref()
            .and_then(|meta| meta.0["ui"]["resourceUri"].as_str())
            .is_some_and(|uri| uri.starts_with(GATEWAY_STATUS_APP_URI))
    );

    for action in ["open", "refresh"] {
        let result = running
            .service()
            .call_tool_impl(
                CallToolRequestParams::new(GATEWAY_STATUS_TOOL_NAME).with_arguments(
                    serde_json::Map::from_iter([
                        ("action".to_string(), Value::String(action.to_string())),
                        ("params".to_string(), serde_json::json!({})),
                    ]),
                ),
                scoped_context(running.peer().clone(), &["lab:admin"]),
            )
            .await
            .expect("admin status callback");
        assert!(!result.is_error.unwrap_or(false));
        assert!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value["data"].as_array())
                .is_some(),
            "{action} must return the route-scoped gateway list"
        );
    }
}

#[tokio::test]
async fn gateway_status_app_obeys_gateway_list_policy() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(restricted_gateway_manager(&["gateway.test"]).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let tools = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("restricted tools");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != GATEWAY_STATUS_TOOL_NAME)
    );
    let resources = running
        .service()
        .list_resources_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("restricted resources");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with(GATEWAY_STATUS_APP_URI))
    );
    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(GATEWAY_STATUS_TOOL_NAME),
            scoped_context(running.peer().clone(), &["lab:admin"]),
        )
        .await
        .expect("restricted status call");
    assert!(result.is_error.unwrap_or(false));
}

#[tokio::test]
async fn gateway_status_app_requires_manager_and_gateway_registry() {
    for server in [
        test_server(
            crate::registry::build_default_registry(),
            None,
            crate::mcp::route_scope::McpRouteScope::Root,
            crate::mcp::logging::LoggingLevel::Emergency,
        ),
        test_server(
            ToolRegistry::new(),
            Some(code_mode_manager(true).await),
            crate::mcp::route_scope::McpRouteScope::Root,
            crate::mcp::logging::LoggingLevel::Emergency,
        ),
    ] {
        let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let tools = running
            .service()
            .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
            .await
            .expect("tools without complete status wiring");
        assert!(
            tools
                .tools
                .iter()
                .all(|tool| tool.name.as_ref() != GATEWAY_STATUS_TOOL_NAME)
        );
    }
}

#[tokio::test]
async fn gateway_status_app_returns_only_route_visible_upstreams() {
    let pool = Arc::new(UpstreamPool::new());
    let manager = code_mode_manager_with_pool_multi(
        true,
        vec![
            fixture_upstream_config("visible"),
            fixture_upstream_config("hidden"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "visible-only",
            ["visible"],
            ["gateway"],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(128 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(GATEWAY_STATUS_TOOL_NAME),
            scoped_context(running.peer().clone(), &["lab:admin"]),
        )
        .await
        .expect("route-scoped status result");
    let rows = result
        .structured_content
        .as_ref()
        .and_then(|value| value["data"].as_array())
        .expect("status rows");
    let ids = rows
        .iter()
        .filter_map(|row| row["id"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["visible"]);
}

#[tokio::test]
async fn list_tools_does_not_advertise_unreadable_server_logs_ui_metadata() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
        .await
        .expect("list tools");

    let tool = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == SERVER_LOGS_TOOL_NAME)
        .expect("server_logs action tool remains discoverable");
    let meta = tool.meta.as_ref().map(|meta| &meta.0);
    assert!(
        meta.is_none_or(|meta| {
            !meta.contains_key("ui") && !meta.contains_key("openai/outputTemplate")
        }),
        "server_logs must not advertise MCP App metadata unless the caller can read the admin UI resource"
    );
}

#[cfg(feature = "skills")]
#[tokio::test]
async fn artifact_management_is_distinct_from_sep_skill_exposure() {
    let loadout = GatewayLoadoutConfig {
        name: "no-skills".to_string(),
        services: vec!["artifacts".to_string()],
        expose_tools: true,
        expose_resources: true,
        expose_prompts: true,
        expose_skills: false,
        expose_code_mode: false,
        ..GatewayLoadoutConfig::default()
    };
    let route = ProtectedMcpRouteConfig {
        name: "no-skills".to_string(),
        enabled: true,
        public_host: "mcp.example.com".to_string(),
        public_path: "/no-skills".to_string(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: vec![],
        health_path: None,
        target: Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                loadout: Some(loadout.name.clone()),
                ..Default::default()
            },
        )),
    };
    let scope = crate::mcp::route_scope::McpRouteScope::from_protected_route(
        &route,
        std::slice::from_ref(&loadout),
    )
    .expect("loadout scope resolves")
    .expect("gateway subset scope");
    assert!(
        scope.allows_service("artifacts"),
        "service allowlist includes artifacts"
    );
    assert!(!scope.exposes_skills(), "capability gate disables Skills");

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(false).await),
        scope,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = scoped_context(running.peer().clone(), &["lab:read"]);

    let listed = running
        .service()
        .list_tools_impl(None, context.clone())
        .await
        .expect("list tools for Skills-disabled loadout");
    assert!(
        listed
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == "artifacts")
    );
    assert!(
        listed
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != "skills")
    );

    let denied = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("skills").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_string(),
                Value::String("skills.list".to_string()),
            ),
            ("params".to_string(), serde_json::json!({})),
        ])),
        context,
    ))
    .await
    .expect("forged Skills call result");
    assert!(denied.is_error.unwrap_or(false));
    let text = denied.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("not_found") && text.contains("not enabled on the mcp surface"),
        "the retired Skills service must not dispatch: {text}"
    );
}

#[tokio::test]
async fn resource_disabled_loadout_hides_unreadable_mcp_app_bindings() {
    let loadout = GatewayLoadoutConfig {
        name: "text-only-code-mode".to_string(),
        expose_tools: true,
        expose_resources: false,
        expose_prompts: false,
        expose_skills: false,
        expose_code_mode: true,
        ..GatewayLoadoutConfig::default()
    };
    let route = ProtectedMcpRouteConfig {
        name: "text-only".to_string(),
        enabled: true,
        public_host: "mcp.example.com".to_string(),
        public_path: "/text-only".to_string(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: vec![],
        health_path: None,
        target: Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                loadout: Some(loadout.name.clone()),
                ..Default::default()
            },
        )),
    };
    let scope = crate::mcp::route_scope::McpRouteScope::from_protected_route(
        &route,
        std::slice::from_ref(&loadout),
    )
    .expect("loadout scope resolves")
    .expect("gateway subset scope");
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(true).await),
        scope,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state.set_enabled(true);
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:admin"]))
        .await
        .expect("list tools for resource-disabled loadout");

    assert!(
        result
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME),
        "text Code Mode remains available when the Loadout exposes Code Mode"
    );
    assert!(
        result
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != CODE_MODE_UI_TOOL_NAME),
        "codemode_ui must not be advertised when its backing resource is unreadable"
    );
    for tool in &result.tools {
        if let Some(meta) = tool.meta.as_ref() {
            assert!(
                !meta.0.contains_key("ui") && !meta.0.contains_key("openai/outputTemplate"),
                "{} advertised resource-backed UI metadata on a resource-disabled route",
                tool.name
            );
        }
    }
}

#[test]
fn resource_backed_ui_metadata_sanitizer_preserves_unrelated_meta() {
    let mut meta = Some(MetaObject(serde_json::Map::from_iter([
        (
            "ui".to_string(),
            serde_json::json!({"resourceUri": "ui://fixture/app.html"}),
        ),
        (
            "openai/outputTemplate".to_string(),
            serde_json::json!("ui://fixture/app.skybridge.html"),
        ),
        ("trace".to_string(), serde_json::json!("keep-me")),
    ])));

    strip_resource_backed_ui_meta(&mut meta);

    let meta = meta.expect("unrelated metadata remains");
    assert!(!meta.0.contains_key("ui"));
    assert!(!meta.0.contains_key("openai/outputTemplate"));
    assert_eq!(meta.0["trace"], serde_json::json!("keep-me"));
}

#[tokio::test]
async fn list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let mut app_callback = fixture_upstream_tool(&upstream_name, "youtube_app_callback", None);
    app_callback.tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
        "ui".to_string(),
        serde_json::json!({ "visibility": ["app"] }),
    )])));
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_app_callback".to_string(), app_callback),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    assert_eq!(
        result.tools, contract_tools,
        "tools/list and the notification contract must use identical descriptors"
    );
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(
        names.contains(&"youtube_search_ui"),
        "upstream MCP App tools must pass through while ordinary raw tools stay hidden"
    );
    assert!(names.contains(&"youtube_app_callback"));
    assert!(!names.contains(&"youtube_probe"));
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
    assert!(!names.contains(&"hidden-upstream"));
}

#[tokio::test]
async fn tool_catalog_snapshot_keeps_code_mode_contract_health_independent() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_app_state = manager.code_mode_app_state();

    let snapshot = server.snapshot_tool_catalog().await;

    assert!(snapshot.tools.contains(CODE_MODE_TOOL_NAME));
    assert!(snapshot.tools.contains(CODE_MODE_UI_TOOL_NAME));
    assert!(snapshot.tools.contains(MCP_APP_TOOL_NAME));
    assert!(snapshot.tools.contains("youtube_search_ui"));
    assert!(!snapshot.tools.contains("youtube_probe"));
}

#[tokio::test]
async fn list_tools_does_not_cold_connect_code_mode_catalog() {
    let pool = Arc::new(UpstreamPool::new());
    let manager = code_mode_manager_with_pool(
        true,
        fixture_upstream_config("cold-apps"),
        Arc::clone(&pool),
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    assert!(
        result
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME),
        "root list_tools must keep advertising Code Mode"
    );

    let summary = pool.cached_upstream_summary("cold-apps").await;
    assert!(
        summary.is_none(),
        "root list_tools must not cold-connect or populate a lazy upstream catalog"
    );
    assert!(
        pool.upstream_tool_last_error("cold-apps").await.is_none(),
        "skipping cold discovery should not mark the upstream failed"
    );
}

#[derive(Clone)]
struct ColdSubsetUpstream;

impl ServerHandler for ColdSubsetUpstream {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, rmcp::ErrorData> {
        Ok(rmcp::model::ListToolsResult::with_all_items(vec![
            Tool::new(
                "cold_subset_tool",
                "Discovered on the first scoped listing",
                Arc::new(serde_json::Map::new()),
            ),
        ]))
    }
}

#[tokio::test]
async fn protected_subset_first_list_tools_discovers_its_allowed_upstream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let service = StreamableHttpService::new(
        || Ok(ColdSubsetUpstream),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(vec![address.to_string()])
            .with_json_response(true),
    );
    let upstream_task = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
            .await
            .expect("upstream server");
    });

    let mut upstream = fixture_upstream_config("cold-subset");
    upstream.url = Some(format!("http://{address}/mcp"));
    upstream.proxy_resources = false;
    let pool = Arc::new(UpstreamPool::new());
    let manager = code_mode_manager_with_pool(false, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "connexin",
            ["cold-subset"],
            std::iter::empty::<&str>(),
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["mcp:read"]))
        .await
        .expect("list tools");

    assert_eq!(
        result
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        vec!["cold_subset_tool"]
    );
    upstream_task.abort();
}

#[tokio::test]
async fn raw_oauth_list_tools_uses_only_the_subject_cache() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let mut upstream = fixture_oauth_upstream_config("cold-oauth");
    upstream.url = Some(format!("http://{address}/mcp"));
    let pool = Arc::new(UpstreamPool::new());
    let manager = code_mode_manager_with_pool(false, upstream, Arc::clone(&pool)).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        running
            .service()
            .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"])),
    )
    .await
    .expect("cached tools/list must return promptly")
    .expect("list tools");

    assert!(
        result
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != "remote")
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err(),
        "root tools/list must not open an OAuth upstream connection"
    );
}

#[tokio::test]
#[cfg(feature = "proxy-testkit")]
async fn raw_oauth_list_tools_preserves_cached_subject_tools() {
    let mut upstream = fixture_oauth_upstream_config("warm-oauth");
    upstream.expose_tools = Some(vec!["visible".to_string()]);
    let pool = Arc::new(UpstreamPool::new());
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![
            Tool::new("visible", "visible", Arc::new(serde_json::Map::new())),
            Tool::new("hidden", "hidden", Arc::new(serde_json::Map::new())),
        ],
    )
    .await;
    let manager = code_mode_manager_with_pool(false, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
        .await
        .expect("list tools");

    assert!(
        result
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == "visible")
    );
    assert!(
        result
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != "hidden")
    );
}

#[tokio::test]
async fn list_tools_does_not_promote_upstream_mcp_app_tools_when_resources_are_not_proxied() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "github_pr_ui",
        Some("ui://apps/github-pr.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    let mut entry = fixture_upstream_entry(
        "apps",
        HashMap::from([("github_pr_ui".to_string(), ui_tool)]),
    );
    entry.proxy_resources = false;
    pool.insert_entry_for_test("apps", entry).await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.proxy_resources = false;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(!names.contains(&"github_pr_ui"));
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
}

#[tokio::test]
async fn list_tools_skips_upstream_ui_tools_that_collide_with_synthetic_names() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let synthetic_names = [
        CODE_MODE_TOOL_NAME,
        CODE_MODE_READ_TOOL_NAME,
        CODE_MODE_UI_TOOL_NAME,
        MCP_APP_TOOL_NAME,
        ADD_SERVER_TOOL_NAME,
        GATEWAY_STATUS_TOOL_NAME,
        SETTINGS_TOOL_NAME,
    ];
    let colliding_tools = synthetic_names
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                fixture_upstream_tool(&upstream_name, name, Some("ui://apps/collision.html")),
            )
        })
        .collect::<HashMap<_, _>>();
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test("apps", fixture_upstream_entry("apps", colliding_tools))
        .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");

    for synthetic_name in synthetic_names {
        let expected = usize::from(matches!(
            synthetic_name,
            CODE_MODE_TOOL_NAME
                | CODE_MODE_READ_TOOL_NAME
                | CODE_MODE_UI_TOOL_NAME
                | MCP_APP_TOOL_NAME
                | SETTINGS_TOOL_NAME
        ));
        let count = result
            .tools
            .iter()
            .filter(|tool| tool.name.as_ref() == synthetic_name)
            .count();
        assert_eq!(
            count, expected,
            "upstream UI tool must not duplicate synthetic tool {synthetic_name}"
        );
        assert_eq!(
            contract_tools
                .iter()
                .filter(|tool| tool.name.as_ref() == synthetic_name)
                .count(),
            expected,
            "peer contract must not duplicate synthetic tool {synthetic_name}"
        );
    }
}

#[tokio::test]
async fn protected_code_mode_list_tools_hides_raw_siblings_and_disallowed_builtins() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops",
            ["apps"],
            ["gateway-alpha"],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(!names.contains(&"gateway-alpha"));
    assert!(!names.contains(&"hidden-upstream"));
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
    assert!(
        names.contains(&"youtube_search_ui"),
        "upstream MCP App tools must pass through while ordinary raw tools stay hidden"
    );
    assert!(!names.contains(&"youtube_probe"));
}

#[tokio::test]
async fn protected_list_tools_hides_code_mode_when_route_disables_it() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops",
            ["apps"],
            ["gateway-alpha"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(names.contains(&"gateway-alpha"));
    assert!(
        !names.contains(&CODE_MODE_TOOL_NAME),
        "codemode must not be advertised when expose_code_mode=false: {names:?}"
    );
}

#[tokio::test]
async fn codemode_description_lists_route_scoped_enabled_upstreams_and_hints() {
    let mut apps = fixture_upstream_config("apps");
    apps.code_mode_hint = Some("Search connected application data".to_string());
    let mut hidden = fixture_upstream_config("hidden");
    hidden.enabled = false;
    let gateway_alpha = fixture_upstream_config("hidden-upstream");
    let pool = Arc::new(UpstreamPool::new());
    let manager =
        code_mode_manager_with_pool_multi(true, vec![apps, hidden, gateway_alpha], pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops",
            ["apps"],
            ["gateway-alpha"],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let codemode = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME)
        .expect("codemode tool");
    let description = codemode
        .description
        .as_ref()
        .expect("codemode description")
        .as_ref();

    assert!(description.contains("## Available upstream namespaces"));
    assert!(description.contains("- `apps` -- Search connected application data"));
    assert!(!description.contains("- `hidden`"));
    assert!(!description.contains("- `hidden-upstream`"));
    assert!(description.contains("Never guess helper or method names"));
}

#[tokio::test]
async fn protected_list_tools_filters_disallowed_builtins_when_code_mode_is_off() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops",
            ["apps"],
            ["gateway-alpha"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(names.contains(&"gateway-alpha"));
    assert!(!names.contains(&"hidden-upstream"));
    assert!(!names.contains(&CODE_MODE_TOOL_NAME));
}

#[tokio::test]
async fn call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("hidden while code_mode mode is enabled"),
        "MCP App sibling callbacks should reach upstream proxy routing, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "test fixture has no live peer, so allowed callbacks should fail at proxy call, got {text}"
    );
}

#[tokio::test]
async fn call_tool_allows_direct_mcp_app_ui_callbacks_with_read_scope() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_search_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_search_ui"),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "direct MCP App UI tools are render entry points and should not use the sibling execute-scope gate, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "test fixture has no live peer, so allowed UI callbacks should fail at proxy call, got {text}"
    );
}

#[tokio::test]
async fn destructive_direct_mcp_app_tools_require_execute_scope_and_elicitation() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_delete_ui",
        Some("ui://apps/youtube-delete.html"),
    );
    ui_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_delete_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let read_context = scoped_context(running.peer().clone(), &["lab:read"]);
    let tools = running
        .service()
        .list_tools_impl(None, read_context.clone())
        .await
        .expect("read-scope tools");
    assert!(
        tools
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != "youtube_delete_ui"),
        "read-only catalogs must not advertise destructive upstream MCP App tools"
    );

    let denied = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_delete_ui"),
        read_context,
    ))
    .await
    .expect("read-scope destructive call result");
    assert!(denied.is_error.unwrap_or(false));
    let denied_text = denied.content[0].as_text().expect("text").text.as_str();
    assert!(
        denied_text.contains("\"kind\":\"forbidden\"") && denied_text.contains("lab:admin"),
        "destructive app tool must require execute scope, got {denied_text}"
    );

    let allowed = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_delete_ui"),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("execute-scope destructive call result");
    let allowed_text = allowed.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(allowed_text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert!(envelope["error"]["upstream"].is_null());
    assert!(envelope["error"]["cause"].is_null());
}

#[tokio::test]
async fn app_callback_markers_without_ui_owner_stay_hidden_and_uncallable() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut standard = fixture_upstream_tool(&upstream_name, "standard_app_callback", None);
    standard.tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
        "ui".to_string(),
        serde_json::json!({ "visibility": ["app"] }),
    )])));
    let mut openai = fixture_upstream_tool(&upstream_name, "openai_app_callback", None);
    openai.tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
        "openai/widgetAccessible".to_string(),
        Value::Bool(true),
    )])));
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("standard_app_callback".to_string(), standard),
                ("openai_app_callback".to_string(), openai),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let advertised = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
        .await
        .expect("read-scope tools");
    for name in ["standard_app_callback", "openai_app_callback"] {
        assert!(
            advertised
                .tools
                .iter()
                .all(|tool| tool.name.as_ref() != name),
            "{name} must not escape raw-tool suppression without an exposed UI owner"
        );

        let result = Box::pin(running.service().call_tool_impl(
            CallToolRequestParams::new(name),
            scoped_context(running.peer().clone(), &["lab:read"]),
        ))
        .await
        .expect("call tool result");
        assert!(result.is_error.unwrap_or(false));
        let text = result.content[0].as_text().expect("text").text.as_str();
        let envelope: Value = serde_json::from_str(text).expect("error envelope");
        assert_eq!(envelope["error"]["kind"], "not_found");
        assert!(
            text.contains("hidden while code_mode mode is enabled"),
            "callback-only metadata must not create an app-call bypass, got {text}"
        );
    }
}

#[tokio::test]
async fn call_tool_rejects_priority_zero_direct_mcp_app_ui_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_search_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.priority = 0.0;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_search_ui"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "priority-zero upstream must not be callable through the UI callback bypass, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_priority_zero_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.priority = 0.0;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "priority-zero upstream must not be callable through the sibling callback bypass, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_disabled_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.enabled = false;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "disabled upstream must not be callable through the sibling callback bypass, got {text}"
    );
}

#[tokio::test]
async fn call_tool_preserves_selected_mcp_app_sibling_upstream() {
    let unrelated_name: Arc<str> = Arc::from("aaa_plain");
    let unrelated_probe = fixture_upstream_tool(&unrelated_name, "youtube_probe", None);

    let app_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &app_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let app_probe = fixture_upstream_tool(&app_name, "youtube_probe", None);

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "aaa_plain",
        fixture_upstream_entry(
            "aaa_plain",
            HashMap::from([("youtube_probe".to_string(), unrelated_probe)]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), app_probe),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("aaa_plain"),
            fixture_upstream_config("apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("structured agent error");
    assert_eq!(envelope["error"]["upstream"], "apps");
    assert_eq!(envelope["error"]["tool"], "apps::youtube_probe");
    assert_ne!(envelope["error"]["upstream"], "aaa_plain");
}

#[tokio::test]
async fn call_tool_requires_execute_scope_for_hidden_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "forbidden");
    assert_eq!(
        envelope["error"]["required_scopes"],
        serde_json::json!(["lab", "lab:admin"])
    );
}

/// The legacy `LABBY_CODE_MODE_WIDGET_CALLBACKS` bypass surfaces ANY exposed
/// non-destructive upstream tool — including one with no MCP App UI resource that
/// is therefore NOT advertised in `list_tools`. Calling such a hidden tool via
/// the bypass with an authenticated-but-insufficient scope must be rejected, not
/// silently allowed. This pins the `requires_scope_check` flag on the legacy
/// path (it was previously `false`, which let a `lab:read` caller through).
#[tokio::test]
async fn call_tool_requires_execute_scope_for_legacy_widget_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    // A plain tool with no UI sibling: only the legacy "any exposed tool" rule
    // makes it callable via the widget-callback gate.
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_probe".to_string(), plain_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "forbidden");
    assert_eq!(
        envelope["error"]["required_scopes"],
        serde_json::json!(["lab", "lab:admin"])
    );
}

#[tokio::test]
async fn codemode_requires_execute_scope_not_read_scope() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("call result");

    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(text.contains("\"kind\":\"forbidden\""), "{text}");
}

#[tokio::test]
async fn read_scope_lists_and_routes_only_codemode_read() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let listed = running
        .service()
        .list_tools_impl(None, scoped_context(running.peer().clone(), &["lab:read"]))
        .await
        .expect("list tools");
    let names = listed
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(names.contains(&CODE_MODE_READ_TOOL_NAME));
    assert!(!names.contains(&CODE_MODE_TOOL_NAME));
    assert!(!names.contains(&CODE_MODE_UI_TOOL_NAME));
    assert!(!names.contains(&MCP_APP_TOOL_NAME));

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_READ_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("call result");
    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(!text.contains("\"kind\":\"forbidden\""), "{text}");
    if result.is_error.unwrap_or(false) {
        assert!(text.contains("\"service\":\"codemode_read\""), "{text}");
    }

    let full_result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("full call result");
    let full_text: &str = full_result.content[0]
        .as_text()
        .expect("text")
        .text
        .as_ref();
    assert!(full_text.contains("\"kind\":\"forbidden\""), "{full_text}");
}

#[tokio::test]
async fn codemode_read_rejects_artifact_writes_through_the_mcp_surface() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager_with_test_runner(true, Vec::new(), None).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_READ_TOOL_NAME).with_arguments(
                serde_json::json!({
                    "code": "async () => await writeArtifact('blocked.txt', 'must not persist')"
                })
                .as_object()
                .expect("object")
                .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("call result");

    assert!(result.is_error.unwrap_or(false));
    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(text.contains("forbidden"), "{text}");
    assert!(text.contains("writeArtifact"), "{text}");
    assert!(text.contains("read-only"), "{text}");
}

#[tokio::test]
async fn codemode_read_rejects_mutating_upstream_before_dispatch() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut mutation = fixture_destructive_upstream_tool(&upstream_name, "records_delete");
    mutation.tool.annotations = Some(
        rmcp::model::ToolAnnotations::new()
            .read_only(false)
            .destructive(true),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("records_delete".to_string(), mutation)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_test_runner(
        true,
        vec![fixture_upstream_config("apps")],
        Some(Arc::clone(&pool)),
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_READ_TOOL_NAME).with_arguments(
                serde_json::json!({
                    "code": "async () => await callTool('apps::records_delete', {})"
                })
                .as_object()
                .expect("object")
                .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("call result");

    assert!(result.is_error.unwrap_or(false));
    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(text.contains("forbidden"), "{text}");
    assert!(text.contains("read-only"), "{text}");
    assert_eq!(
        pool.upstream_tool_last_error("apps").await,
        None,
        "a policy rejection must happen before any upstream dispatch attempt"
    );
}

#[tokio::test]
async fn codemode_allows_execute_scope_to_reach_runner_path() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab"]),
        )
        .await
        .expect("call result");

    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "lab scope must pass execute auth: {text}"
    );
    if result.is_error.unwrap_or(false) {
        assert!(
            text.contains("\"service\":\"codemode\""),
            "codemode should route through the execute branch with its service name: {text}"
        );
    } else {
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value["kind"].as_str()),
            Some("code_mode_execute_trace")
        );
    }
}

#[tokio::test]
async fn codemode_routes_to_code_mode_path() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab"]),
        )
        .await
        .expect("call result");

    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "codemode should pass execute auth: {text}"
    );
    if result.is_error.unwrap_or(false) {
        assert!(
            text.contains("\"service\":\"codemode\""),
            "codemode should preserve the called tool name in error envelopes: {text}"
        );
    } else {
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value["kind"].as_str()),
            Some("code_mode_execute_trace"),
            "codemode should return runtime trace structured content"
        );
    }
}

#[tokio::test]
async fn call_tool_allows_execute_scope_for_hidden_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("structured agent error");
    assert_ne!(envelope["error"]["kind"], "forbidden");
    assert_eq!(envelope["error"]["upstream"], "apps");
    assert_eq!(envelope["error"]["tool"], "apps::youtube_probe");
}

#[tokio::test]
async fn call_tool_honors_route_scope_for_mcp_app_sibling_callbacks() {
    let blocked_name: Arc<str> = Arc::from("blocked_apps");
    let ui_tool = fixture_upstream_tool(
        &blocked_name,
        "youtube_search_ui",
        Some("ui://blocked-apps/youtube-search.html"),
    );
    let blocked_probe = fixture_upstream_tool(&blocked_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "blocked_apps",
        fixture_upstream_entry(
            "blocked_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), blocked_probe),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("allowed_apps"),
            fixture_upstream_config("blocked_apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "allowed-only",
            ["allowed_apps"],
            ["gateway"],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );

    let text = call_tool_error_text(server, "youtube_probe").await;
    let envelope: Value = serde_json::from_str(&text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        !text.contains("blocked_apps"),
        "route-scope denial should not reach the blocked upstream, got {text}"
    );
}

#[tokio::test]
#[cfg(feature = "proxy-testkit")]
async fn list_tools_passes_through_subject_scoped_oauth_mcp_apps_in_code_mode() {
    let upstream_name: Arc<str> = Arc::from("oauth_apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://oauth-apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    let upstream = fixture_oauth_upstream_config("oauth_apps");
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![ui_tool.tool, plain_tool.tool],
    )
    .await;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = scoped_context(running.peer().clone(), &["lab"]);

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list subject-scoped OAuth tools");
    assert_eq!(result.tools, contract_tools);
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(names.contains(&"youtube_search_ui"));
    assert!(!names.contains(&"youtube_probe"));
}

#[tokio::test]
#[cfg(feature = "proxy-testkit")]
async fn list_tools_hides_subject_scoped_oauth_app_when_ui_resource_is_not_exposed() {
    let upstream_name: Arc<str> = Arc::from("oauth_apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://oauth-apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    let mut upstream = fixture_oauth_upstream_config("oauth_apps");
    upstream.expose_resources = Some(vec!["ui://oauth-apps/allowed-only.html".to_string()]);
    pool.install_test_subject_tools_for_upstream(&upstream, "reader", vec![ui_tool.tool])
        .await;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = scoped_context(running.peer().clone(), &["lab"]);

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list subject-scoped OAuth tools");
    assert_eq!(result.tools, contract_tools);
    assert!(
        result
            .tools
            .iter()
            .all(|tool| tool.name.as_ref() != "youtube_search_ui"),
        "a subject-scoped app tool must not advertise a UI resource blocked by expose_resources"
    );
}

#[tokio::test]
#[cfg(feature = "proxy-testkit")]
async fn call_tool_blocks_oauth_mcp_app_sibling_callback_before_subject_route_dispatch() {
    let upstream_name: Arc<str> = Arc::from("oauth_apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://oauth-apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    let upstream = fixture_oauth_upstream_config("oauth_apps");
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![ui_tool.tool, plain_tool.tool],
    )
    .await;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("structured agent error");
    assert!(
        envelope["error"]["upstream"].is_null(),
        "the redacted transport error must not leak a route name: {text}"
    );
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert_eq!(envelope["error"]["origin"], "policy");
    assert!(
        envelope["error"]["cause"].is_null(),
        "fail-closed policy must run before any subject-scoped upstream transport: {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_priority_zero_oauth_subject_scoped_callbacks() {
    let upstream_name: Arc<str> = Arc::from("oauth_apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://oauth-apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "oauth_apps",
        fixture_upstream_entry(
            "oauth_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let mut upstream = fixture_oauth_upstream_config("oauth_apps");
    upstream.priority = 0.0;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );

    let text = call_tool_error_text(server, "youtube_probe").await;
    let envelope: Value = serde_json::from_str(&text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        !text.contains("oauth_apps"),
        "non-routable OAuth upstream must not be selected or disclosed, got {text}"
    );
}

#[tokio::test]
async fn list_tools_paginates_large_builtin_catalog() {
    let manager = code_mode_manager(false).await;
    let server = test_server(
        large_test_registry(250),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let first = running
        .service()
        .list_tools_impl(None, request_context_with_peer(running.peer().clone()))
        .await
        .expect("first page");

    assert_eq!(
        first.tools.len(),
        crate::mcp::pagination::MCP_LIST_PAGE_SIZE
    );
    assert_eq!(first.tools[0].name.as_ref(), MCP_APP_TOOL_NAME);
    assert_eq!(first.tools[1].name.as_ref(), "service_000");
    assert_eq!(first.tools[99].name.as_ref(), "service_098");
    assert!(
        first
            .next_cursor
            .as_deref()
            .is_some_and(|cursor| cursor.starts_with("v1:100:"))
    );
    assert_eq!(first.ttl_ms, Some(0));
    assert_eq!(first.cache_scope, Some(rmcp::model::CacheScope::Private));
    assert!(
        running
            .service()
            .last_listed_tool_contract
            .read()
            .await
            .candidate_count(&None)
            == 0,
        "a partial first page must not publish a complete contract baseline"
    );
    let wire = serde_json::to_value(&first).expect("serialize tool list");
    assert_eq!(wire["resultType"], "complete");
    assert_eq!(wire["ttlMs"], 0);
    assert_eq!(wire["cacheScope"], "private");

    let second_request = PaginatedRequestParams::default().with_cursor(first.next_cursor.clone());
    let second = running
        .service()
        .list_tools_impl(
            Some(second_request),
            request_context_with_peer(running.peer().clone()),
        )
        .await
        .expect("second page");

    assert_eq!(
        second.tools.len(),
        crate::mcp::pagination::MCP_LIST_PAGE_SIZE
    );
    assert_eq!(second.tools[0].name.as_ref(), "service_099");
    assert_eq!(second.tools[99].name.as_ref(), "service_198");
    assert!(
        second
            .next_cursor
            .as_deref()
            .is_some_and(|cursor| cursor.starts_with("v1:200:"))
    );
    assert!(
        running
            .service()
            .last_listed_tool_contract
            .read()
            .await
            .candidate_count(&None)
            == 0,
        "an intermediate page must not publish a complete contract baseline"
    );
    let third_request = PaginatedRequestParams::default().with_cursor(second.next_cursor.clone());
    let third = running
        .service()
        .list_tools_impl(
            Some(third_request),
            request_context_with_peer(running.peer().clone()),
        )
        .await
        .expect("third page");
    assert_eq!(third.tools.len(), 52);
    assert_eq!(third.tools[0].name.as_ref(), "service_199");
    assert_eq!(third.tools[50].name.as_ref(), "service_249");
    assert_eq!(third.tools[51].name.as_ref(), SETTINGS_TOOL_NAME);
    assert!(third.next_cursor.is_none());
    assert!(
        running
            .service()
            .last_listed_tool_contract
            .read()
            .await
            .candidate_count(&None)
            == 1,
        "only the final revision-validated page publishes the baseline"
    );
    assert!(
        Arc::ptr_eq(&first.tools[1].input_schema, &second.tools[0].input_schema),
        "built-in tools should reuse the cached stable action schema across list pages"
    );
}

#[tokio::test]
async fn list_tools_pagination_is_independent_of_registry_insertion_order() {
    async fn collect_names(registry: ToolRegistry) -> Vec<String> {
        let manager = code_mode_manager(false).await;
        let server = test_server(
            registry,
            Some(manager),
            crate::mcp::route_scope::McpRouteScope::Root,
            crate::mcp::logging::LoggingLevel::Emergency,
        );
        let (transport, _client_transport) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let mut cursor = None;
        let mut names = Vec::new();
        loop {
            let request = cursor
                .take()
                .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let page = running
                .service()
                .list_tools_impl(request, request_context_with_peer(running.peer().clone()))
                .await
                .expect("tool page");
            names.extend(page.tools.into_iter().map(|tool| tool.name.to_string()));
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some(next);
        }
        names
    }

    let ascending = collect_names(large_test_registry(250)).await;
    let rebuilt = collect_names(reverse_large_test_registry(250)).await;

    assert_eq!(ascending, rebuilt);
    assert_eq!(ascending.len(), 252);
    assert_eq!(ascending[0], MCP_APP_TOOL_NAME);
    assert_eq!(ascending[251], SETTINGS_TOOL_NAME);
    assert!(ascending.is_sorted());
}

#[tokio::test]
async fn list_tools_rejects_cursor_after_catalog_revision_changes() {
    let original_manager = code_mode_manager(false).await;
    let original_server = test_server(
        large_test_registry(250),
        Some(original_manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (original_transport, _original_client_transport) = tokio::io::duplex(64);
    let original = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        original_server,
        original_transport,
        None,
    );
    let first = original
        .service()
        .list_tools_impl(None, request_context_with_peer(original.peer().clone()))
        .await
        .expect("first page");
    let changed_manager = code_mode_manager(false).await;
    let changed_server = test_server(
        large_test_registry(251),
        Some(changed_manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (changed_transport, _changed_client_transport) = tokio::io::duplex(64);
    let changed = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        changed_server,
        changed_transport,
        None,
    );
    let request = PaginatedRequestParams::default().with_cursor(first.next_cursor);

    let error = changed
        .service()
        .list_tools_impl(
            Some(request),
            request_context_with_peer(changed.peer().clone()),
        )
        .await
        .expect_err("cursor must not span catalog revisions");

    assert_eq!(
        error.data.as_ref().expect("error data")["kind"],
        serde_json::json!("invalid_cursor")
    );
}

#[tokio::test]
async fn list_tools_rejects_invalid_cursor() {
    let server = test_server(
        completion_test_registry(),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let request = PaginatedRequestParams::default().with_cursor(Some("bad".to_string()));

    let err = running
        .service()
        .list_tools_impl(
            Some(request),
            request_context_with_peer(running.peer().clone()),
        )
        .await
        .expect_err("invalid cursor");

    assert_eq!(
        err.data.as_ref().expect("error data")["kind"],
        serde_json::json!("invalid_cursor")
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_mcp_app_sibling_callbacks_without_elicitation() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let mut delete_tool = fixture_upstream_tool(&upstream_name, "youtube_delete", None);
    delete_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_delete".to_string(), delete_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert!(envelope["error"]["upstream"].is_null());
    assert!(envelope["error"]["cause"].is_null());
}

#[tokio::test]
async fn call_tool_blocks_destructive_direct_mcp_app_callbacks_without_elicitation() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_delete_ui",
        Some("ui://apps/youtube-delete.html"),
    );
    ui_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_delete_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );

    let text = call_tool_error_text(server, "youtube_delete_ui").await;
    let envelope: Value = serde_json::from_str(&text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert!(envelope["error"]["upstream"].is_null());
    assert!(envelope["error"]["cause"].is_null());
    assert!(
        !text.contains("confirm:true") && !text.contains("confirm\":true"),
        "destructive widget callbacks must not suggest confirm bypasses: {text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_legacy_widget_callbacks_without_elicitation() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut delete_tool = fixture_upstream_tool(&upstream_name, "youtube_delete", None);
    delete_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_delete".to_string(), delete_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert!(envelope["error"]["upstream"].is_null());
    assert!(envelope["error"]["cause"].is_null());
}

#[tokio::test]
async fn call_tool_allows_legacy_widget_callbacks_for_route_allowed_upstream() {
    let upstream_name: Arc<str> = Arc::from("allowed_apps");
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "allowed_apps",
        fixture_upstream_entry(
            "allowed_apps",
            HashMap::from([("youtube_probe".to_string(), plain_tool)]),
        ),
    )
    .await;
    let manager =
        code_mode_manager_with_pool(true, fixture_upstream_config("allowed_apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "allowed-only",
            ["allowed_apps"],
            ["gateway"],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("upstream_error"),
        "legacy callback should reach the route-allowed upstream proxy, got {text}"
    );
}

#[tokio::test]
async fn call_tool_honors_route_scope_for_legacy_widget_callbacks() {
    let blocked_name: Arc<str> = Arc::from("blocked_apps");
    let blocked_probe = fixture_upstream_tool(&blocked_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "blocked_apps",
        fixture_upstream_entry(
            "blocked_apps",
            HashMap::from([("youtube_probe".to_string(), blocked_probe)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("allowed_apps"),
            fixture_upstream_config("blocked_apps"),
        ],
        pool,
    )
    .await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "allowed-only",
            ["allowed_apps"],
            ["gateway"],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        !text.contains("blocked_apps"),
        "legacy callback should not reach a route-disallowed upstream, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_ambiguous_mcp_app_sibling_callbacks_when_one_candidate_is_destructive() {
    let safe_name: Arc<str> = Arc::from("safe_apps");
    let safe_ui_tool = fixture_upstream_tool(
        &safe_name,
        "youtube_search_ui",
        Some("ui://safe-apps/youtube-search.html"),
    );
    let safe_probe = fixture_upstream_tool(&safe_name, "youtube_probe", None);

    let destructive_name: Arc<str> = Arc::from("destructive_apps");
    let destructive_ui_tool = fixture_upstream_tool(
        &destructive_name,
        "youtube_search_ui",
        Some("ui://destructive-apps/youtube-search.html"),
    );
    let mut destructive_probe = fixture_upstream_tool(&destructive_name, "youtube_probe", None);
    destructive_probe.destructive = true;

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "safe_apps",
        fixture_upstream_entry(
            "safe_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), safe_ui_tool),
                ("youtube_probe".to_string(), safe_probe),
            ]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "destructive_apps",
        fixture_upstream_entry(
            "destructive_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), destructive_ui_tool),
                ("youtube_probe".to_string(), destructive_probe),
            ]),
        ),
    )
    .await;

    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("safe_apps"),
            fixture_upstream_config("destructive_apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "ambiguous_tool");
    assert_eq!(
        envelope["error"]["valid"],
        serde_json::json!([
            "destructive_apps::youtube_probe",
            "safe_apps::youtube_probe"
        ])
    );
}

#[tokio::test]
async fn call_tool_rejects_ambiguous_non_destructive_mcp_app_sibling_callbacks() {
    let alpha_name: Arc<str> = Arc::from("alpha_apps");
    let alpha_ui_tool = fixture_upstream_tool(
        &alpha_name,
        "youtube_search_ui",
        Some("ui://alpha-apps/youtube-search.html"),
    );
    let alpha_probe = fixture_upstream_tool(&alpha_name, "youtube_probe", None);

    let beta_name: Arc<str> = Arc::from("beta_apps");
    let beta_ui_tool = fixture_upstream_tool(
        &beta_name,
        "youtube_search_ui",
        Some("ui://beta-apps/youtube-search.html"),
    );
    let beta_probe = fixture_upstream_tool(&beta_name, "youtube_probe", None);

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "alpha_apps",
        fixture_upstream_entry(
            "alpha_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), alpha_ui_tool),
                ("youtube_probe".to_string(), alpha_probe),
            ]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "beta_apps",
        fixture_upstream_entry(
            "beta_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), beta_ui_tool),
                ("youtube_probe".to_string(), beta_probe),
            ]),
        ),
    )
    .await;

    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("alpha_apps"),
            fixture_upstream_config("beta_apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "ambiguous_tool");
    assert_eq!(
        envelope["error"]["valid"],
        serde_json::json!(["alpha_apps::youtube_probe", "beta_apps::youtube_probe"])
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_mcp_app_sibling_callback_without_elicitation() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let destructive = fixture_destructive_upstream_tool(&upstream_name, "youtube_purge");
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_purge".to_string(), destructive),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_purge"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
    assert!(envelope["error"]["upstream"].is_null());
    assert!(envelope["error"]["cause"].is_null());
}

#[tokio::test]
async fn call_tool_refuses_ambiguous_mcp_app_sibling_callback() {
    // Two UI-bearing upstreams expose the same destructive probe name. The old
    // code collapsed multi-candidate to `tool = None`, which skipped the
    // destructive gate and proxied an arbitrary upstream. The callback must now
    // fail closed with `ambiguous_tool` and never reach the proxy.
    let a: Arc<str> = Arc::from("apps_a");
    let b: Arc<str> = Arc::from("apps_b");
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps_a",
        fixture_upstream_entry(
            "apps_a",
            HashMap::from([
                (
                    "youtube_search_ui".to_string(),
                    fixture_upstream_tool(&a, "youtube_search_ui", Some("ui://apps_a/s.html")),
                ),
                (
                    "youtube_purge".to_string(),
                    fixture_destructive_upstream_tool(&a, "youtube_purge"),
                ),
            ]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "apps_b",
        fixture_upstream_entry(
            "apps_b",
            HashMap::from([
                (
                    "calendar_ui".to_string(),
                    fixture_upstream_tool(&b, "calendar_ui", Some("ui://apps_b/c.html")),
                ),
                (
                    "youtube_purge".to_string(),
                    fixture_destructive_upstream_tool(&b, "youtube_purge"),
                ),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_multi(
        true,
        vec![
            fixture_upstream_config("apps_a"),
            fixture_upstream_config("apps_b"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_purge"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("ambiguous_tool"),
        "multi-upstream sibling callback must fail closed, got {text}"
    );
    assert!(
        !text.contains("upstream_error"),
        "ambiguous destructive callback must not reach the upstream proxy, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_hidden_tool_without_ui_sibling_in_code_mode() {
    // A hidden raw tool whose upstream exposes no MCP App UI tool stays
    // unreachable — Code Mode's confinement guarantee.
    let upstream_name: Arc<str> = Arc::from("plain");
    let plain = fixture_upstream_tool(&upstream_name, "plain_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "plain",
        fixture_upstream_entry("plain", HashMap::from([("plain_probe".to_string(), plain)])),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("plain"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("plain_probe"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "hidden non-UI tool must be refused, got {text}"
    );
}

#[tokio::test]
async fn call_tool_allows_direct_mcp_app_ui_tool_in_code_mode() {
    // The requested tool itself carrying a UI resource is callable over the
    // bypass (the direct-UI route preserved by the refactor).
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_search_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_search_ui"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("hidden while code_mode mode is enabled"),
        "direct MCP App UI tool must be callable, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "direct UI callback should reach the proxy (no live peer), got {text}"
    );
}

#[tokio::test]
async fn snapshot_catalog_hides_builtin_tools_when_code_mode_is_enabled() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );

    let snapshot = server.snapshot_catalog().await;

    // Code Mode mode exposes the read-only and full text entry points, explicit
    // UI entry point, and text-only recovery control. No legacy aliases are permitted.
    assert_eq!(
        snapshot.tools,
        [
            CODE_MODE_READ_TOOL_NAME.to_string(),
            CODE_MODE_TOOL_NAME.to_string(),
            CODE_MODE_UI_TOOL_NAME.to_string(),
            MCP_APP_TOOL_NAME.to_string(),
        ]
        .into_iter()
        .collect()
    );
    assert!(
        !snapshot.tools.contains("code"),
        "code must not appear in Code Mode mode"
    );
}

#[tokio::test]
async fn snapshot_catalog_shows_no_gateway_tools_when_surface_is_disabled() {
    // When code_mode.enabled=false, none of the gateway Code Mode tool names
    // should appear in the snapshot.
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );

    let snapshot = server.snapshot_catalog().await;

    // Raw mode — none of the gateway meta-tools should appear.
    for meta_tool in [
        CODE_MODE_TOOL_NAME,
        CODE_MODE_UI_TOOL_NAME,
        MCP_APP_TOOL_NAME,
        "search",
        "execute",
        "code",
    ] {
        assert!(
            !snapshot.tools.contains(meta_tool),
            "gateway meta-tool '{meta_tool}' must not appear when neither mode is enabled"
        );
    }
}

#[tokio::test]
async fn protected_scope_denies_direct_code_mode_calls_when_hidden() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops",
            ["hidden-upstream"],
            ["gateway-alpha"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    for tool_name in [
        CODE_MODE_TOOL_NAME,
        CODE_MODE_UI_TOOL_NAME,
        MCP_APP_TOOL_NAME,
    ] {
        let result = Box::pin(
            running
                .service()
                .call_tool_impl(CallToolRequestParams::new(tool_name), context.clone()),
        )
        .await
        .expect("call tool result");
        assert!(result.is_error.unwrap_or(false));
        let text = result.content[0].as_text().expect("text").text.as_str();
        assert!(
            text.contains("route_scope_denied"),
            "{tool_name} should be denied, got {text}"
        );
    }
}

#[tokio::test]
async fn server_reads_current_pool_from_gateway_manager() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime.clone(),
        ),
    );
    let notifier = crate::mcp::peers::PeerNotifier::default();
    let server = LabMcpServer {
        registry: Arc::new(ToolRegistry::new()),
        access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        gateway_manager: Some(Arc::clone(&manager)),
        peers: Arc::clone(&notifier.peers),
        code_mode_app_state: notifier.code_mode_app_state.clone(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        client_registry: notifier.client_registry.clone(),
        transport_label: "test",
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(
            crate::mcp::logging::LoggingLevel::Info,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };

    assert!(server.current_upstream_pool().await.is_none());

    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;

    let current = server.current_upstream_pool().await.expect("pool");
    assert!(Arc::ptr_eq(&current, &pool));
}

#[tokio::test]
async fn snapshot_catalog_hides_mcp_disabled_virtual_services() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: false,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Info,
    );

    let snapshot = server.snapshot_catalog().await;
    assert!(!snapshot.tools.contains("deploy"));
}

#[tokio::test]
async fn gateway_add_through_mcp_protected_route_suppresses_hidden_enrichment_suggestion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            dir.path().join("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                upstream: vec![{
                    let mut upstream = fixture_upstream_config("gateway-alpha");
                    upstream.enabled = false;
                    upstream
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    pool.insert_entry_for_test(
        "github",
        fixture_upstream_entry(
            "github",
            HashMap::from([(
                "search_repos".to_string(),
                fixture_upstream_tool(&Arc::<str>::from("github"), "search_repos", None),
            )]),
        ),
    )
    .await;
    let mut hidden_spec = fixture_upstream_config("github");
    hidden_spec.enabled = false;

    let mut server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops-route",
            ["gateway-alpha".to_string()],
            ["gateway".to_string()],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.access_runtime = authorized_test_access_runtime().await;
    let peer_server = test_server(
        ToolRegistry::new(),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        peer_server,
        transport,
        None,
    );
    let mut context = request_context_with_peer(running.peer().clone());
    context.extensions.insert(primary_static_bearer_identity());

    let result = Box::pin(server.call_tool_impl(
        CallToolRequestParams::new("gateway").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_string(),
                Value::String("gateway.add".to_string()),
            ),
            (
                "params".to_string(),
                serde_json::json!({ "spec": hidden_spec }),
            ),
        ])),
        context,
    ))
    .await
    .expect("call tool result");

    assert!(!result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("gateway envelope");
    assert_eq!(envelope["ok"], true);
    let view = &envelope["data"];
    assert_eq!(view["config"]["name"], "github");
    assert_eq!(view["enrichment_suggestion"], Value::Null);
    assert!(
        view["enrichment_suggestion_error"]
            .as_str()
            .is_some_and(|message| message.contains("unknown gateway upstream `github`")),
        "hidden upstream suggestion should fail open with a scoped unknown_upstream error: {view}"
    );
}

#[tokio::test]
async fn gateway_pending_import_approve_through_mcp_protected_route_suppresses_hidden_enrichment_suggestion()
 {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            dir.path().join("config.toml"),
            runtime,
        ),
    );
    let mut pending = fixture_upstream_config("paperless");
    pending.enabled = false;
    let mut import_source =
        labby_runtime::gateway_config::ImportSource::new("claude", "/tmp/mcp.json", "now");
    import_source.server_name = Some("paperless".to_string());
    import_source.transport_fingerprint = Some("sha256:test".to_string());
    pending.imported_from = Some(import_source);
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                upstream: vec![{
                    let mut upstream = fixture_upstream_config("gateway-alpha");
                    upstream.enabled = false;
                    upstream
                }],
                upstream_pending: vec![pending],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;

    let mut server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops-route",
            ["gateway-alpha".to_string()],
            ["gateway".to_string()],
            true,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.access_runtime = authorized_test_access_runtime().await;
    let peer_server = test_server(
        ToolRegistry::new(),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        peer_server,
        transport,
        None,
    );
    let mut context = request_context_with_peer(running.peer().clone());
    context.extensions.insert(primary_static_bearer_identity());

    let result = Box::pin(server.call_tool_impl(
        CallToolRequestParams::new("gateway").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_string(),
                Value::String("gateway.import_pending.approve".to_string()),
            ),
            (
                "params".to_string(),
                serde_json::json!({ "name": "paperless", "confirm": true }),
            ),
        ])),
        context,
    ))
    .await
    .expect("call tool result");

    assert!(!result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("pending import envelope");
    assert_eq!(envelope["ok"], true);
    let view = &envelope["data"];
    assert_eq!(view["name"], "paperless");
    assert_eq!(view["enrichment_suggestion"], Value::Null);
    assert!(
        view["enrichment_suggestion_error"]
            .as_str()
            .is_some_and(|message| message.contains("unknown gateway upstream `paperless`")),
        "hidden pending import suggestion should fail open with a scoped unknown_upstream error: {view}"
    );
}

#[tokio::test]
async fn service_actions_json_filters_to_allowed_mcp_actions() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "doctor-readonly".to_string(),
                    service: "doctor".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: vec!["system.checks".to_string()],
                    }),
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Info,
    );

    let value = server
        .service_actions_json("doctor")
        .await
        .expect("service actions");
    let actions = value.as_array().expect("array");
    assert!(actions.iter().any(|action| action["name"] == "help"));
    assert!(actions.iter().any(|action| action["name"] == "schema"));
    assert!(
        actions
            .iter()
            .any(|action| action["name"] == "system.checks")
    );
    assert!(!actions.iter().any(|action| action["name"] == "audit.full"));
}

/// Regression: the Code Mode regime is per-route, so the notification fanout
/// must evaluate each peer's own contract.
///
/// With Code Mode enabled globally, a root session sees the constant `codemode`
/// tool and raw upstream churn is invisible to it. A protected route with
/// `expose_code_mode = false` sees the raw tools instead, so the *same* churn
/// is a real change for that session. Evaluating one global projection and
/// broadcasting the verdict told the raw-exposing session nothing.
#[tokio::test]
async fn peer_contracts_diverge_by_route_scope_under_global_code_mode() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    // One gateway, Code Mode on: the global regime both sessions share.
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;

    let code_mode_peer = test_server(
        completion_test_registry(),
        Some(Arc::clone(&manager)),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    )
    .peer_contract();
    let raw_peer = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "ops",
            ["apps"],
            ["gateway"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    )
    .peer_contract();

    let code_mode_contract = code_mode_peer.visible_contract().await;
    let raw_contract = raw_peer.visible_contract().await;

    // The Code Mode session sees the synthetic tool and the MCP-App UI tool,
    // never the plain upstream tool behind them.
    assert!(code_mode_contract.tools.contains(CODE_MODE_TOOL_NAME));
    assert!(code_mode_contract.tools.contains(CODE_MODE_UI_TOOL_NAME));
    assert!(code_mode_contract.tools.contains(MCP_APP_TOOL_NAME));
    assert!(
        code_mode_contract.tools.contains("youtube_search_ui"),
        "upstream MCP App tools pass through under Code Mode"
    );
    assert!(
        !code_mode_contract.tools.contains("youtube_probe"),
        "raw upstream tools stay hidden under Code Mode"
    );

    // The raw-exposing route sees the opposite: the plain tool is part of its
    // contract, and `codemode` is not.
    assert!(
        raw_contract.tools.contains("youtube_probe"),
        "a route with expose_code_mode = false sees raw upstream tools"
    );
    assert!(!raw_contract.tools.contains(CODE_MODE_TOOL_NAME));
    assert!(!raw_contract.tools.contains(CODE_MODE_UI_TOOL_NAME));
    assert!(!raw_contract.tools.contains(MCP_APP_TOOL_NAME));

    // The whole point: one global projection cannot stand in for both.
    assert_ne!(
        code_mode_contract, raw_contract,
        "per-peer evaluation is required; these sessions do not share a contract"
    );
}

#[tokio::test]
async fn peer_contract_removes_codemode_when_execute_scope_is_revoked() {
    let manager = code_mode_manager(true).await;
    let authorized = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    )
    .peer_contract();
    let mut unauthorized = authorized.clone();
    unauthorized.audience.code_mode_execute_allowed = false;

    let authorized = authorized.visible_contract().await;
    let unauthorized = unauthorized.visible_contract().await;

    assert!(authorized.tools.contains(CODE_MODE_TOOL_NAME));
    assert!(authorized.tools.contains(CODE_MODE_READ_TOOL_NAME));
    assert!(!unauthorized.tools.contains(CODE_MODE_TOOL_NAME));
    assert!(unauthorized.tools.contains(CODE_MODE_READ_TOOL_NAME));
    assert_ne!(
        authorized.contract_hash, unauthorized.contract_hash,
        "revoking execute scope changes the client-visible descriptor contract"
    );
}

#[tokio::test]
async fn peer_contract_removes_all_codemode_tools_when_read_and_execute_are_revoked() {
    let manager = code_mode_manager(true).await;
    let mut contract = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    )
    .peer_contract();
    contract.audience.code_mode_read_allowed = false;
    contract.audience.code_mode_execute_allowed = false;

    let contract = contract.visible_contract().await;
    assert!(!contract.tools.contains(CODE_MODE_READ_TOOL_NAME));
    assert!(!contract.tools.contains(CODE_MODE_TOOL_NAME));
    assert!(!contract.tools.contains(CODE_MODE_UI_TOOL_NAME));
}

// ── Issue #210: builtin outputSchema + descriptor drift (Raw mode) ──────────

/// AC-2 + AC-2a. The existing drift test above runs only under Code Mode,
/// where `hide_raw_tools` suppresses every builtin except `server_logs`, so it
/// never exercises the builtin descriptor loop. This sibling forces Raw mode.
#[tokio::test]
async fn raw_mode_builtin_descriptors_match_across_builders() {
    // HERMETIC: force Raw unconditionally. `Root` + `gateway_manager: None` is
    // NOT sufficient — `peer_contract.rs` returns InProcessPeer when
    // `gateway_manager.is_none() && config::process_code_mode_enabled()`, and
    // that backing store is a process-global `AtomicBool` any other test in
    // the binary can set. `expose_code_mode: false` forces Raw.
    //
    // The services list MUST name the registered services:
    // `service_visible_on_mcp` gates on `route_scope.allows_service`, so an
    // empty list advertises nothing and the test proves nothing.
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "raw-mode-drift",
        Vec::<String>::new(),
        ["hidden-upstream", "gateway-alpha"],
        /* expose_code_mode */ false,
    );
    let server = test_server(
        completion_test_registry(),
        None,
        scope,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let snapshot_for_request = running
        .service()
        .snapshot_tool_catalog_for_request(&context)
        .await;
    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");

    assert_eq!(
        result.tools, contract_tools,
        "raw-mode tools/list and the notification contract must use identical descriptors"
    );

    // AC-2a: `output_schema` participates in `descriptor_contract_hash`, which
    // drives tools/list_changed — one-sided drift makes change detection
    // wrong, not merely incomplete.
    assert_eq!(
        ToolCatalogSnapshot::from_descriptors(&result.tools),
        snapshot_for_request,
        "snapshot built from the handler's descriptors must equal the request snapshot"
    );

    // Positive name assertions: without these the test passes vacuously — the
    // exact failure mode of the code-mode sibling.
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"gateway-alpha"),
        "raw mode must advertise builtins"
    );
    assert!(
        names.contains(&"hidden-upstream"),
        "raw mode must NOT suppress builtins — the code-mode sibling asserts the inverse"
    );

    // AC-1: every registry service advertises the success-envelope schema.
    // Scoped to registry services: synthetic tools may legitimately carry no
    // schema (`mcp_app` returns `{"kind":"mcp_app_control", …}`).
    let service_names: Vec<&str> = completion_test_registry()
        .services()
        .iter()
        .map(|s| s.name)
        .collect();
    for tool in result
        .tools
        .iter()
        .filter(|t| service_names.contains(&t.name.as_ref()))
    {
        let schema = tool
            .output_schema
            .as_ref()
            .unwrap_or_else(|| panic!("{} advertises no outputSchema", tool.name));
        assert_eq!(schema["properties"]["ok"]["const"], serde_json::json!(true));
        assert_eq!(
            schema["required"],
            serde_json::json!(["ok", "service", "action", "data"])
        );
        assert_eq!(schema["additionalProperties"], serde_json::json!(true));
    }
}

#[cfg(feature = "skills")]
#[tokio::test]
async fn authenticated_http_gets_scoped_artifact_management_while_local_peers_do_not() {
    let mut server = test_server(
        crate::registry::build_docs_registry(),
        Some(restricted_skills_gateway_manager(&["artifacts.list"]).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    server.transport_label = "http";
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let mut context = request_context_with_peer(running.peer().clone());
    let request = axum::http::Request::builder()
        .header("x-labby-project-id", "project-one")
        .body(())
        .expect("HTTP request");
    let (mut parts, ()) = request.into_parts();
    parts
        .extensions
        .insert(labby_auth::auth_context::AuthContext {
            sub: "owner".to_string(),
            actor_key: None,
            scopes: vec!["lab:admin".to_string()],
            issuer: "static".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        });
    parts.extensions.insert(
        labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "owner",
        )
        .expect("verified fixture identity"),
    );
    context.extensions.insert(parts);

    let contract = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let listed = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("authenticated HTTP tools/list")
        .tools;
    assert_eq!(
        listed, contract,
        "HTTP descriptor paths must stay identical"
    );
    let managed = listed
        .iter()
        .find(|tool| tool.name.as_ref() == "artifacts")
        .expect("artifacts descriptor");
    assert_eq!(
        managed.input_schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum")
            .len(),
        1
    );
    let actions = managed.input_schema["properties"]["action"]["enum"]
        .as_array()
        .expect("action enum");
    assert!(actions.contains(&serde_json::json!("artifacts.list")));
    assert!(!actions.contains(&serde_json::json!("artifacts.create")));
    assert!(managed.meta.is_some());
    assert_eq!(
        managed
            .annotations
            .as_ref()
            .and_then(|annotations| annotations.destructive_hint),
        Some(true)
    );

    let local = running
        .service()
        .peer_contract()
        .visible_tool_descriptors()
        .await;
    assert!(
        local.iter().all(|tool| tool.name.as_ref() != "artifacts"),
        "Artifact management requires the authenticated project-bound HTTP context"
    );
}

#[tokio::test]
async fn raw_mode_preserves_upstream_annotations_verbatim_on_both_listing_paths() {
    let upstream_name: Arc<str> = Arc::from("annotated");
    let mut expected = rmcp::model::ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false);
    expected.title = Some("Reviewed upstream title".to_string());
    let mut upstream_tool = fixture_upstream_tool(&upstream_name, "annotated.read", None);
    upstream_tool.tool.annotations = Some(expected.clone());

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "annotated",
        fixture_upstream_entry(
            "annotated",
            HashMap::from([("annotated.read".to_string(), upstream_tool)]),
        ),
    )
    .await;
    let manager =
        code_mode_manager_with_pool(false, fixture_upstream_config("annotated"), pool).await;
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "annotation-passthrough",
        ["annotated"],
        Vec::<String>::new(),
        false,
    );
    let server = test_server(
        ToolRegistry::new(),
        Some(manager),
        scope,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let contract_tools = running
        .service()
        .peer_contract_for_request(&context)
        .visible_tool_descriptors()
        .await;
    let listed = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools")
        .tools;

    for tools in [&contract_tools, &listed] {
        let descriptor = tools
            .iter()
            .find(|tool| tool.name.as_ref() == "annotated.read")
            .expect("upstream downstream descriptor");
        let annotations = descriptor
            .annotations
            .as_ref()
            .expect("upstream annotations on downstream descriptor");
        assert_eq!(annotations, &expected);
        let serialized = serde_json::to_value(descriptor).unwrap();
        let expected_security = serde_json::json!([{"type": "oauth2", "scopes": ["lab:read"]}]);
        assert_eq!(serialized["_meta"]["securitySchemes"], expected_security);
    }
    assert_eq!(listed, contract_tools);
}

/// AC-3 both axes: the success envelope is always present as
/// `structuredContent`, carries exactly the four contract keys, and the text
/// block parses to the identical value.
#[tokio::test]
async fn builtin_help_success_sets_conformant_structured_content() {
    let server = test_server(
        completion_test_registry(),
        None,
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "raw-mode-envelope",
            Vec::<String>::new(),
            ["hidden-upstream", "gateway-alpha"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("gateway-alpha").with_arguments(serde_json::Map::from_iter([(
            "action".to_string(),
            Value::String("help".to_string()),
        )])),
        context,
    ))
    .await
    .expect("call tool result");

    assert_ne!(result.is_error, Some(true), "help must succeed");
    let structured = result
        .structured_content
        .as_ref()
        .expect("success results must always set structuredContent (FR-3 axis 2)");
    let envelope = structured.as_object().expect("envelope object");
    assert_eq!(
        envelope.len(),
        4,
        "success envelope must carry exactly ok/service/action/data: {envelope:?}"
    );
    assert_eq!(envelope["ok"], Value::Bool(true));
    assert_eq!(envelope["service"], Value::String("gateway-alpha".into()));
    assert_eq!(envelope["action"], Value::String("help".into()));
    assert!(envelope.contains_key("data"));

    let text = result.content[0].as_text().expect("compat text block");
    let reparsed: Value = serde_json::from_str(&text.text).expect("text block parses");
    assert_eq!(
        &reparsed, structured,
        "compat text block must serialize the identical envelope"
    );
}

/// Error exemption (CONTRACT §C3.2): an unknown action returns `isError` with
/// an `{ok: false}` envelope; the advertised success schema deliberately does
/// not describe it.
///
/// Needs its own dispatch fn: `noop_dispatch` succeeds for every action, so
/// the shared fixture cannot produce this path.
#[tokio::test]
async fn builtin_unknown_action_error_is_exempt_from_success_schema() {
    fn unknown_action_dispatch(
        action: String,
        _params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
        Box::pin(async move {
            Err(ToolError::UnknownAction {
                message: format!("unknown action `{action}`"),
                valid: vec!["demo.list".to_string()],
                hint: None,
            })
        })
    }
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredService {
        name: "gateway-alpha",
        description: "Gateway alpha",
        category: "network",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_TWO,
        dispatch: unknown_action_dispatch,
    });
    let server = test_server(
        registry,
        None,
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "raw-mode-error",
            Vec::<String>::new(),
            ["hidden-upstream", "gateway-alpha"],
            false,
        ),
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("gateway-alpha").with_arguments(serde_json::Map::from_iter([(
            "action".to_string(),
            Value::String("definitely.not.real".to_string()),
        )])),
        context,
    ))
    .await
    .expect("call tool result");

    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.as_ref().expect("error envelope");
    assert_eq!(structured["ok"], Value::Bool(false));
    assert!(
        structured.get("error").is_some(),
        "error envelope carries the agent error contract, not the success shape"
    );
}

/// FR-2a (issue #210, lab-41e7m.5): non-admin denial at the DISPATCH path.
/// The consolidated availability gate is audience-free; the admin check lives
/// at this call site. A non-admin caller's `add_server` call must fall
/// through to normal routing (unknown tool here — no upstream by that name),
/// never into the admin app handler; an admin caller is handled.
#[tokio::test]
async fn call_tool_add_server_denies_non_admin_scope_at_dispatch() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let denied = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new(ADD_SERVER_TOOL_NAME),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call result");
    assert_eq!(
        denied.is_error,
        Some(true),
        "non-admin call must fall through to normal routing (which errors here), not the admin app"
    );
    let envelope = denied.structured_content.as_ref().expect("error envelope");
    assert_eq!(envelope["ok"], Value::Bool(false));

    let handled = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new(ADD_SERVER_TOOL_NAME),
        scoped_context(running.peer().clone(), &["lab:admin"]),
    ))
    .await
    .expect("admin call result");
    let structured = handled
        .structured_content
        .as_ref()
        .expect("admin add_server dispatch formats an envelope");
    assert_eq!(
        structured["service"],
        Value::String(ADD_SERVER_TOOL_NAME.into())
    );
}

#[tokio::test]
async fn settings_mutations_use_the_setup_destructive_policy() {
    let server = test_server(
        crate::registry::build_default_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        crate::mcp::logging::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = scoped_context(running.peer().clone(), &["lab:admin"]);

    for action in ["config.update", "env.update"] {
        let request = CallToolRequestParams::new(SETTINGS_TOOL_NAME).with_arguments(
            serde_json::Map::from_iter([("action".to_string(), Value::String(action.to_string()))]),
        );
        assert!(
            running
                .service()
                .tool_request_is_destructive(&request, &context)
                .await,
            "Settings mutation `{action}` must inherit the destructive setup action policy"
        );
    }

    let read = CallToolRequestParams::new(SETTINGS_TOOL_NAME).with_arguments(
        serde_json::Map::from_iter([("action".to_string(), Value::String("state".to_string()))]),
    );
    assert!(
        !running
            .service()
            .tool_request_is_destructive(&read, &context)
            .await
    );
}
