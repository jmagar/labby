//! Tests for tool-list/catalog visibility + upstream-pool resolution.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`). Duplicates the
//! small `completion_test_registry` fixture to keep this `tests.rs`
//! self-contained (per the test-distribution plan's minimal-duplication
//! guidance).

use crate::dispatch::error::ToolError;
use crate::mcp::catalog::{CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME};
use crate::mcp::handlers_resources::{CODE_MODE_EXECUTE_APP_URI, CODE_MODE_SEARCH_APP_URI};
use crate::mcp::handlers_tools::{code_mode_tool_meta, code_mode_trace_output_schema};
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};
use lab_apis::core::action::ActionSpec;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

const TEST_ACTIONS_ONE: &[ActionSpec] = &[
    ActionSpec {
        name: "queue.list",
        description: "List queue",
        destructive: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.search",
        description: "Search movies",
        destructive: false,
        params: &[],
        returns: "object",
    },
];

const TEST_ACTIONS_TWO: &[ActionSpec] = &[
    ActionSpec {
        name: "calendar.list",
        description: "List calendar",
        destructive: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.lookup",
        description: "Look up movie",
        destructive: false,
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
    assert!(
        search.0.get("openai/outputTemplate").is_none(),
        "compat aliases should not be added without host evidence"
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
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: true,
                ..crate::config::CodeModeConfig::default()
            },
            ..crate::config::LabConfig::default()
        })
        .await;
    let server = LabMcpServer {
        registry: std::sync::Arc::new(completion_test_registry()),
        gateway_manager: Some(manager),
        node_role: None,
        peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Emergency,
        ))),
    };
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
async fn snapshot_catalog_hides_builtin_tools_when_code_mode_is_enabled() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: true,
                ..crate::config::CodeModeConfig::default()
            },
            ..crate::config::LabConfig::default()
        })
        .await;
    let server = LabMcpServer {
        registry: std::sync::Arc::new(completion_test_registry()),
        gateway_manager: Some(manager),
        node_role: None,
        peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
    };

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
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime,
    ));
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: false,
                ..crate::config::CodeModeConfig::default()
            },
            ..crate::config::LabConfig::default()
        })
        .await;
    let server = LabMcpServer {
        registry: std::sync::Arc::new(completion_test_registry()),
        gateway_manager: Some(manager),
        node_role: None,
        peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
    };

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
async fn server_reads_current_pool_from_gateway_manager() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
        std::path::PathBuf::from("config.toml"),
        runtime.clone(),
    ));
    let notifier = crate::mcp::peers::PeerNotifier::default();
    let server = LabMcpServer {
        registry: std::sync::Arc::new(ToolRegistry::new()),
        gateway_manager: Some(std::sync::Arc::clone(&manager)),
        node_role: None,
        peers: std::sync::Arc::clone(&notifier.peers),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
    };

    assert!(server.current_upstream_pool().await.is_none());

    let pool = std::sync::Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
    runtime.swap(Some(std::sync::Arc::clone(&pool))).await;

    let current = server.current_upstream_pool().await.expect("pool");
    assert!(std::sync::Arc::ptr_eq(&current, &pool));
}

#[tokio::test]
async fn snapshot_catalog_hides_mcp_disabled_virtual_services() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
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

    let server = LabMcpServer {
        registry: std::sync::Arc::new(crate::registry::build_default_registry()),
        gateway_manager: Some(manager),
        node_role: None,
        peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
    };

    let snapshot = server.snapshot_catalog().await;
    assert!(!snapshot.tools.contains("deploy"));
}

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_actions_json_filters_to_allowed_mcp_actions() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = std::sync::Arc::new(crate::dispatch::gateway::manager::GatewayManager::new(
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

    let server = LabMcpServer {
        registry: std::sync::Arc::new(crate::registry::build_default_registry()),
        gateway_manager: Some(manager),
        node_role: None,
        peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
    };

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
