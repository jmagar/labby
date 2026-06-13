//! Tests for tool-list/catalog visibility + upstream-pool resolution.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`). Duplicates the
//! small `completion_test_registry` fixture to keep this `tests.rs`
//! self-contained (per the test-distribution plan's minimal-duplication
//! guidance).

use crate::dispatch::error::ToolError;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::{
    ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};
use crate::mcp::catalog::{CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME};
use crate::mcp::handlers_resources::{
    CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI, CODE_MODE_EXECUTE_APP_URI,
    CODE_MODE_SEARCH_APP_SKYBRIDGE_URI, CODE_MODE_SEARCH_APP_URI,
};
use crate::mcp::handlers_tools::{code_mode_tool_meta, code_mode_trace_output_schema};
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};
use lab_apis::core::action::ActionSpec;
use rmcp::model::{CallToolRequestParams, Meta, Tool};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, atomic::AtomicU8};

const TEST_ACTIONS_ONE: &[ActionSpec] = &[
    ActionSpec {
        name: "queue.list",
        description: "List queue",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.search",
        description: "Search movies",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

const TEST_ACTIONS_TWO: &[ActionSpec] = &[
    ActionSpec {
        name: "calendar.list",
        description: "List calendar",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.lookup",
        description: "Look up movie",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

fn noop_dispatch(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async { Ok(Value::Null) })
}

fn completion_test_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredService {
        name: "radarr",
        description: "Movies",
        category: "media",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_ONE,
        dispatch: noop_dispatch,
    });
    registry.register(RegisteredService {
        name: "sonarr",
        description: "Shows",
        category: "media",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_TWO,
        dispatch: noop_dispatch,
    });
    registry
}

fn test_server(
    registry: ToolRegistry,
    gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
    route_scope: crate::mcp::route_scope::McpRouteScope,
    logging_level: rmcp::model::LoggingLevel,
) -> LabMcpServer {
    LabMcpServer {
        registry: Arc::new(registry),
        gateway_manager,
        node_role: None,
        peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(logging_level))),
        route_scope,
    }
}

async fn code_mode_manager(
    enabled: bool,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled,
                ..crate::config::CodeModeConfig::default()
            },
            ..crate::config::LabConfig::default()
        })
        .await;
    manager
}

async fn code_mode_manager_with_pool(
    enabled: bool,
    upstream: crate::config::UpstreamConfig,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    runtime.swap(Some(pool)).await;
    let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled,
                ..crate::config::CodeModeConfig::default()
            },
            upstream: vec![upstream],
            ..crate::config::LabConfig::default()
        })
        .await;
    manager
}

fn fixture_upstream_config(name: &str) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: Some("http://127.0.0.1:9/mcp".to_string()),
        bearer_token_env: None,
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: true,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

fn fixture_upstream_entry(upstream: &str, tools: HashMap<String, UpstreamTool>) -> UpstreamEntry {
    UpstreamEntry {
        name: Arc::from(upstream),
        tools,
        exposure_policy: ToolExposurePolicy::All,
        prompt_count: 0,
        resource_count: 1,
        prompt_names: Vec::new(),
        resource_uris: vec![format!("ui://{upstream}/app.html")],
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
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
        tool.meta = Some(Meta(serde_json::Map::from_iter([(
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

#[test]
fn code_mode_tool_meta_points_to_canonical_ui_resource() {
    let search = code_mode_tool_meta(CODE_MODE_SEARCH_TOOL_NAME);
    let execute = code_mode_tool_meta(TOOL_EXECUTE_TOOL_NAME);

    assert_eq!(
        search.0["ui"]["resourceUri"].as_str(),
        Some(CODE_MODE_SEARCH_APP_URI)
    );
    assert_eq!(
        execute.0["ui"]["resourceUri"].as_str(),
        Some(CODE_MODE_EXECUTE_APP_URI)
    );
    // OpenAI Apps hosts (ChatGPT / Codex) bind widgets via `openai/outputTemplate`
    // rather than `_meta.ui`. It points at the skybridge variant (same HTML, the
    // `text/html+skybridge` MIME those hosts expect) so the Claude resource is
    // untouched.
    assert_eq!(
        search
            .0
            .get("openai/outputTemplate")
            .and_then(|value| value.as_str()),
        Some(CODE_MODE_SEARCH_APP_SKYBRIDGE_URI),
        "search tool must expose the OpenAI Apps output template"
    );
    assert_eq!(
        execute
            .0
            .get("openai/outputTemplate")
            .and_then(|value| value.as_str()),
        Some(CODE_MODE_EXECUTE_APP_SKYBRIDGE_URI),
        "execute tool must expose the OpenAI Apps output template"
    );
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
    assert_eq!(
        kinds,
        vec!["code_mode_search_trace", "code_mode_execute_trace"]
    );
}

#[tokio::test]
async fn list_tools_advertises_code_mode_output_schemas() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
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
    let search = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_SEARCH_TOOL_NAME)
        .expect("search tool");
    let execute = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == TOOL_EXECUTE_TOOL_NAME)
        .expect("execute tool");

    for tool in [search, execute] {
        let schema = tool.output_schema.as_ref().expect("outputSchema");
        let kinds = schema["oneOf"]
            .as_array()
            .expect("oneOf variants")
            .iter()
            .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec!["code_mode_search_trace", "code_mode_execute_trace"]
        );
    }
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
        rmcp::model::LoggingLevel::Emergency,
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

    assert!(
        names.contains(&"youtube_search_ui"),
        "MCP App upstream tools must stay visible to the host"
    );
    assert!(
        !names.contains(&"youtube_probe"),
        "ordinary raw upstream tools stay hidden in Code Mode"
    );
    assert!(
        !names.contains(&"radarr"),
        "built-in raw tools stay hidden in Code Mode"
    );
}

#[tokio::test]
async fn snapshot_catalog_hides_builtin_tools_when_code_mode_is_enabled() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Info,
    );

    let snapshot = server.snapshot_catalog().await;

    // Code Mode mode: exactly `search` + `execute`. NO code_search, code_execute, or code.
    assert_eq!(
        snapshot.tools,
        ["execute".to_string(), "search".to_string()]
            .into_iter()
            .collect()
    );
    assert!(
        !snapshot.tools.contains("code_search"),
        "code_search must not appear in Code Mode mode"
    );
    assert!(
        !snapshot.tools.contains("code_execute"),
        "code_execute must not appear in Code Mode mode"
    );
    assert!(
        !snapshot.tools.contains("code"),
        "code must not appear in Code Mode mode"
    );
}

#[tokio::test]
async fn snapshot_catalog_shows_no_gateway_tools_when_surface_is_disabled() {
    // When code_mode.enabled=false, none of the gateway meta-tools
    // (search, execute, code, code_search, code_execute) should appear in
    // the snapshot.
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Info,
    );

    let snapshot = server.snapshot_catalog().await;

    // Raw mode — none of the five gateway meta-tools should appear.
    for meta_tool in ["search", "execute", "code", "code_search", "code_execute"] {
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
            "media",
            ["sonarr"],
            ["radarr"],
            false,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    for tool_name in [CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME] {
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
    let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime.clone(),
    ));
    let notifier = crate::mcp::peers::PeerNotifier::default();
    let server = LabMcpServer {
        registry: Arc::new(ToolRegistry::new()),
        gateway_manager: Some(Arc::clone(&manager)),
        node_role: None,
        peers: Arc::clone(&notifier.peers),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
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
    let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
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
        })
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Info,
    );

    let snapshot = server.snapshot_catalog().await;
    assert!(!snapshot.tools.contains("deploy"));
}

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_actions_json_filters_to_allowed_mcp_actions() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
            virtual_servers: vec![crate::config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: crate::config::VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                    allowed_actions: vec!["server.info".to_string()],
                }),
            }],
            ..crate::config::LabConfig::default()
        })
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Info,
    );

    let value = server
        .service_actions_json("deploy")
        .await
        .expect("service actions");
    let actions = value.as_array().expect("array");
    assert!(actions.iter().any(|action| action["name"] == "help"));
    assert!(actions.iter().any(|action| action["name"] == "schema"));
    assert!(actions.iter().any(|action| action["name"] == "server.info"));
    assert!(
        !actions
            .iter()
            .any(|action| action["name"] == "session.list")
    );
}
