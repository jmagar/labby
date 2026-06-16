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
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
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
            upstream: upstreams,
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

fn fixture_oauth_upstream_config(name: &str) -> crate::config::UpstreamConfig {
    let mut config = fixture_upstream_config(name);
    config.oauth = Some(crate::config::UpstreamOauthConfig {
        mode: crate::config::UpstreamOauthMode::AuthorizationCodePkce,
        registration: crate::config::UpstreamOauthRegistration::Preregistered {
            client_id: "client-id".to_string(),
            client_secret_env: None,
        },
        scopes: None,
        prefer_client_metadata_document: None,
    });
    config
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
    parts.extensions.insert(crate::api::oauth::AuthContext {
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

    assert!(names.contains(&"youtube_search_ui"));
    assert!(!names.contains(&"youtube_probe"));
    assert!(names.contains(&CODE_MODE_SEARCH_TOOL_NAME));
    assert!(names.contains(&TOOL_EXECUTE_TOOL_NAME));
    assert!(!names.contains(&"radarr"));
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
            "media",
            ["apps"],
            ["radarr"],
            true,
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

    assert!(!names.contains(&"radarr"));
    assert!(!names.contains(&"sonarr"));
    assert!(names.contains(&CODE_MODE_SEARCH_TOOL_NAME));
    assert!(names.contains(&TOOL_EXECUTE_TOOL_NAME));
    assert!(names.contains(&"youtube_search_ui"));
    assert!(!names.contains(&"youtube_probe"));
}

#[tokio::test]
async fn protected_list_tools_filters_disallowed_builtins_when_code_mode_is_off() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media",
            ["apps"],
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

    assert!(names.contains(&"radarr"));
    assert!(!names.contains(&"sonarr"));
    assert!(!names.contains(&CODE_MODE_SEARCH_TOOL_NAME));
    assert!(!names.contains(&TOOL_EXECUTE_TOOL_NAME));
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
        rmcp::model::LoggingLevel::Emergency,
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
        text.contains("upstream `apps` is not connected"),
        "MCP App sibling callbacks should dispatch to the UI sibling upstream, got {text}"
    );
    assert!(
        !text.contains("upstream `aaa_plain` is not connected"),
        "callback dispatch must not fall through to an unrelated same-name tool, got {text}"
    );
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
        rmcp::model::LoggingLevel::Emergency,
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
        rmcp::model::LoggingLevel::Emergency,
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
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "lab scope should pass the callback execute-scope gate, got {text}"
    );
    assert!(
        text.contains("upstream `apps` is not connected"),
        "allowed callback should reach selected upstream proxy routing, got {text}"
    );
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
        "route-scope denial should not reach the blocked upstream, got {text}"
    );
}

#[tokio::test]
async fn call_tool_uses_subject_scoped_route_for_oauth_mcp_app_sibling_callbacks() {
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
    let manager =
        code_mode_manager_with_pool(true, fixture_oauth_upstream_config("oauth_apps"), pool).await;
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

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("upstream `oauth_apps` call failed"),
        "OAuth callback should use subject-scoped call routing, got {text}"
    );
    assert!(
        !text.contains("upstream `oauth_apps` is not connected"),
        "OAuth callback must not use shared raw-pool routing, got {text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_mcp_app_sibling_callbacks() {
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

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("\"kind\":\"confirmation_required\""),
        "{text}"
    );
    assert!(
        text.contains("not callable via the widget callback bypass"),
        "{text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_direct_mcp_app_callbacks() {
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

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete_ui"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
}

#[tokio::test]
async fn call_tool_blocks_destructive_legacy_widget_callbacks() {
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
        rmcp::model::LoggingLevel::Emergency,
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
        rmcp::model::LoggingLevel::Emergency,
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
        rmcp::model::LoggingLevel::Emergency,
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
async fn call_tool_blocks_destructive_mcp_app_sibling_callback() {
    // A destructive sibling of a UI tool must be refused with
    // `confirmation_required` — the callback bypass has no confirmation channel.
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
        text.contains("confirmation_required"),
        "destructive sibling callback must be gated, got {text}"
    );
    assert!(
        !text.contains("upstream_error"),
        "destructive sibling callback must not reach the upstream proxy, got {text}"
    );
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
