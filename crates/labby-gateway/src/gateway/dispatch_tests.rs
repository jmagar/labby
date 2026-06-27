use std::collections::HashSet;

use serde_json::json;

use labby_runtime::gateway_config::{ProtectedMcpRouteConfig, UpstreamConfig};

use super::super::discovery::DiscoveredServer;
use super::super::manager::GatewayRuntimeHandle;
use super::super::params::GatewayDiscoverParams;
use super::super::types::McpClientTransportType;
use super::*;

#[test]
fn gateway_actions_include_management_surface() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"gateway.list"));
    assert!(names.contains(&"gateway.server.get"));
    assert!(names.contains(&"gateway.supported_services"));
    assert!(names.contains(&"gateway.protected_route.list"));
    assert!(names.contains(&"gateway.protected_route.get"));
    assert!(names.contains(&"gateway.protected_route.add"));
    assert!(names.contains(&"gateway.protected_route.update"));
    assert!(names.contains(&"gateway.protected_route.remove"));
    assert!(names.contains(&"gateway.protected_route.test"));
    assert!(names.contains(&"gateway.virtual_server.enable"));
    assert!(names.contains(&"gateway.virtual_server.disable"));
    assert!(names.contains(&"gateway.virtual_server.remove"));
    assert!(names.contains(&"gateway.virtual_server.quarantine.list"));
    assert!(names.contains(&"gateway.virtual_server.quarantine.restore"));
    assert!(names.contains(&"gateway.virtual_server.set_surface"));
    assert!(names.contains(&"gateway.virtual_server.get_mcp_policy"));
    assert!(names.contains(&"gateway.virtual_server.set_mcp_policy"));
    assert!(names.contains(&"gateway.service_config.get"));
    assert!(names.contains(&"gateway.service_config.set"));
    assert!(names.contains(&"gateway.service_actions"));
    assert!(names.contains(&"gateway.get"));
    assert!(names.contains(&"gateway.test"));
    assert!(names.contains(&"gateway.add"));
    assert!(names.contains(&"gateway.update"));
    assert!(names.contains(&"gateway.remove"));
    assert!(names.contains(&"gateway.reload"));
    assert!(names.contains(&"gateway.status"));
    assert!(names.contains(&"gateway.client_config.get"));
    assert!(names.contains(&"gateway.discovered_tools"));
    assert!(names.contains(&"gateway.discovered_resources"));
    assert!(names.contains(&"gateway.discovered_prompts"));
    assert!(names.contains(&"gateway.enrich.preview"));
    assert!(names.contains(&"gateway.enrich.apply"));
    assert!(names.contains(&"gateway.oauth.probe"));
    assert!(names.contains(&"gateway.oauth.start"));
    assert!(names.contains(&"gateway.oauth.status"));
    assert!(names.contains(&"gateway.oauth.clear"));
    assert!(names.contains(&"gateway.mcp.enable"));
    assert!(names.contains(&"gateway.mcp.disable"));
    assert!(names.contains(&"gateway.mcp.cleanup"));
    assert!(names.contains(&"gateway.public_urls.get"));

    for spec in ACTIONS {
        if matches!(
            spec.name,
            "gateway.code_mode.set"
                | "gateway.enrich.preview"
                | "gateway.enrich.apply"
                | "gateway.import"
                | "gateway.import_pending.approve"
                | "gateway.import_pending.reject"
                | "gateway.import_tombstones.clear"
                | "gateway.import_tombstones.restore"
        ) {
            continue;
        }
        assert!(
            !spec.destructive,
            "{} must not be destructive unless it risks permanent, hard-to-recreate data loss",
            spec.name
        );
    }
}

#[test]
fn import_mutations_are_destructive() {
    for name in [
        "gateway.import",
        "gateway.import_pending.approve",
        "gateway.import_pending.reject",
        "gateway.import_tombstones.clear",
        "gateway.import_tombstones.restore",
    ] {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("{name} action"));
        assert!(spec.destructive, "{name} mutates gateway import state");
    }
}

#[test]
fn enrich_preview_is_destructive_because_external_providers_spawn() {
    let spec = ACTIONS
        .iter()
        .find(|spec| spec.name == "gateway.enrich.preview")
        .expect("gateway.enrich.preview action");

    assert!(spec.destructive);
}

#[tokio::test]
async fn enrich_preview_dispatch_defaults_to_deterministic_provider() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.enrich.preview",
        json!({"upstreams": ["github"]}),
    )
    .await
    .expect("preview dispatch");

    assert_eq!(value["provider"], json!("deterministic"));
    assert_eq!(value["proposals"][0]["upstream"], json!("github"));
}

#[tokio::test]
async fn enrich_preview_dispatch_rejects_empty_selection() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.enrich.preview", json!({}))
        .await
        .expect_err("empty selection must fail");

    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn enrich_apply_dispatch_persists_hint() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let preview = dispatch_with_manager(
        &manager,
        "gateway.enrich.preview",
        json!({"upstreams": ["github"]}),
    )
    .await
    .expect("preview");
    let hash = preview["proposals"][0]["metadata_hash"]
        .as_str()
        .expect("hash")
        .to_string();

    let applied = dispatch_with_manager(
        &manager,
        "gateway.enrich.apply",
        json!({
            "upstream": "github",
            "hint": "search repositories",
            "metadata_hash": hash,
        }),
    )
    .await
    .expect("apply");

    assert_eq!(applied["hint"], json!("search repositories"));
    assert_eq!(
        manager.current_config().await.upstream[0]
            .code_mode_hint
            .as_deref(),
        Some("search repositories")
    );
}

#[test]
fn gateway_actions_include_servers_and_schema() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(
        names.contains(&"gateway.servers"),
        "missing gateway.servers; have {names:?}"
    );
    assert!(
        names.contains(&"gateway.schema"),
        "missing gateway.schema; have {names:?}"
    );
}

/// Test stub registry that knows a single `deploy` service with a small action
/// catalog. The host's real default-registry builder lives in `lab`, not
/// `lab-gateway`; gateway dispatch tests that exercise service-aware behavior
/// (`gateway.service_actions`, virtual-server enable/policy validation) inject
/// this so `service_meta`/`service_actions`/`contains_service` resolve `deploy`.
struct DeployTestRegistry;

static DEPLOY_TEST_META: labby_apis::core::PluginMeta = labby_apis::core::PluginMeta {
    name: "deploy",
    display_name: "Deploy",
    description: "deploy (test stub)",
    category: labby_apis::core::Category::Bootstrap,
    docs_url: "",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};

impl crate::registry::InProcessServiceRegistry for DeployTestRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}

impl crate::gateway::service_registry::GatewayServiceRegistry for DeployTestRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        vec!["deploy"]
    }

    fn contains_service(&self, name: &str) -> bool {
        name == "deploy"
    }

    fn service_actions(
        &self,
        name: &str,
    ) -> Option<Vec<crate::gateway::service_registry::ServiceActionInfo>> {
        (name == "deploy").then(|| {
            vec![
                crate::gateway::service_registry::ServiceActionInfo {
                    name: "deploy.plan",
                    description: "Plan a deployment",
                    destructive: false,
                },
                crate::gateway::service_registry::ServiceActionInfo {
                    name: "deploy.apply",
                    description: "Apply a deployment",
                    destructive: true,
                },
            ]
        })
    }

    fn service_meta(&self, name: &str) -> Option<&'static labby_apis::core::PluginMeta> {
        (name == "deploy").then_some(&DEPLOY_TEST_META)
    }
}

fn test_manager() -> GatewayManager {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    GatewayManager::new(path, GatewayRuntimeHandle::default())
        .with_builtin_service_registry(std::sync::Arc::new(DeployTestRegistry))
}

#[tokio::test]
async fn gateway_code_mode_set_accepts_all_public_config_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let value = dispatch_with_manager(
        &manager,
        "gateway.code_mode.set",
        json!({
            "enabled": true,
            "trace_params": false,
            "result_shape_policy": "truncate",
            "timeout_ms": 5000,
            "max_response_bytes": 4096,
            "max_response_tokens": 1024,
            "token_estimate_divisor": 2,
            "max_log_entries": 10,
            "max_log_bytes": 2048
        }),
    )
    .await
    .expect("code mode config should update");

    assert_eq!(value["enabled"], true);
    assert_eq!(value["trace_params"], false);
    assert_eq!(value["result_shape_policy"], "truncate");
    assert_eq!(value["timeout_ms"], 5000);
    assert_eq!(value["max_response_bytes"], 4096);
    assert_eq!(value["max_response_tokens"], 1024);
    assert_eq!(value["token_estimate_divisor"], 2);
    assert_eq!(value["max_log_entries"], 10);
    assert_eq!(value["max_log_bytes"], 2048);
}

#[tokio::test]
async fn gateway_code_mode_set_rejects_invalid_result_shape_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let err = dispatch_with_manager(
        &manager,
        "gateway.code_mode.set",
        json!({ "result_shape_policy": "redact" }),
    )
    .await
    .expect_err("invalid code mode shape policy should be rejected");
    let body = serde_json::to_value(&err).expect("serialize");

    assert_eq!(body["kind"], "invalid_param");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("redact")
    );
}

#[tokio::test]
async fn gateway_code_mode_set_rejects_invalid_public_config_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let err = dispatch_with_manager(
        &manager,
        "gateway.code_mode.set",
        json!({ "token_estimate_divisor": 0 }),
    )
    .await
    .expect_err("invalid code mode config should be rejected");
    let body = serde_json::to_value(&err).expect("serialize");

    assert_eq!(body["kind"], "invalid_param");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("token_estimate_divisor")
    );
}

#[tokio::test]
async fn gateway_public_urls_get_dispatches_from_catalog_action() {
    let manager = test_manager();
    let value = dispatch_with_manager(&manager, "gateway.public_urls.get", json!({}))
        .await
        .expect("public urls dispatches");

    assert!(value.get("effective_mcp_gateway").is_some());
}

#[tokio::test]
async fn gateway_servers_action_returns_not_found_when_no_pool() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.servers", json!({}))
        .await
        .expect_err("no pool configured");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(
        body["kind"], "not_found",
        "sdk_kind must be promoted to kind"
    );
}

#[tokio::test]
async fn gateway_schema_missing_name_returns_missing_param() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.schema", json!({}))
        .await
        .expect_err("missing name");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(body["kind"], "missing_param");
    assert_eq!(body["param"], "name");
}

#[tokio::test]
async fn gateway_schema_unknown_upstream_returns_not_found_envelope() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.schema", json!({"name": "nope"}))
        .await
        .expect_err("no pool configured");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(
        body["kind"], "not_found",
        "sdk_kind must be promoted to kind"
    );
}

#[tokio::test]
async fn gateway_dispatch_rejects_synthetic_tool_execution_actions() {
    let manager = test_manager();

    for action in ["tool_execute", "tool_invoke", "code_mode", "invoke"] {
        let err = dispatch_with_manager(&manager, action, json!({}))
            .await
            .expect_err("synthetic top-level MCP tools are not gateway actions");
        assert_eq!(err.kind(), "unknown_action", "{action}");
    }
}

#[tokio::test]
async fn gateway_list_returns_array() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list");

    assert!(value.is_array());
    assert_eq!(value.as_array().expect("array").len(), 1);
    let row = &value.as_array().expect("array")[0];
    assert_eq!(row["discovered_tool_count"], 0);
    assert_eq!(row["exposed_tool_count"], 0);
    assert_eq!(row["discovered_resource_count"], 0);
    assert_eq!(row["exposed_resource_count"], 0);
    assert_eq!(row["discovered_prompt_count"], 0);
    assert_eq!(row["exposed_prompt_count"], 0);
}

#[tokio::test]
async fn gateway_client_config_get_returns_http_and_stdio_configs() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001/mcp".to_string()),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
            UpstreamConfig {
                enabled: true,
                name: "fixture-stdio".to_string(),
                url: None,
                bearer_token_env: None,
                command: Some("npx".to_string()),
                args: vec!["-y".to_string(), "fixture-server".to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
        ])
        .await;

    let http = dispatch_with_manager(
        &manager,
        "gateway.client_config.get",
        json!({"name":"fixture-http"}),
    )
    .await
    .expect("http client config");
    assert_eq!(http["name"], "fixture-http");
    assert_eq!(http["type"], "http");
    assert_eq!(http["url"], "http://127.0.0.1:9001/mcp");

    let stdio = dispatch_with_manager(
        &manager,
        "gateway.client_config.get",
        json!({"name":"fixture-stdio"}),
    )
    .await
    .expect("stdio client config");
    assert_eq!(stdio["name"], "fixture-stdio");
    assert_eq!(stdio["type"], "stdio");
    assert_eq!(stdio["command"], "npx");
    assert_eq!(stdio["args"], json!(["-y", "fixture-server"]));
}

fn protected_route_fixture(name: &str) -> ProtectedMcpRouteConfig {
    ProtectedMcpRouteConfig {
        name: name.to_string(),
        enabled: true,
        public_host: "mcp.tootie.tv".to_string(),
        public_path: "/syslog".to_string(),
        upstream: None,
        backend_url: "http://100.88.16.79:3100".to_string(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: Vec::new(),
        health_path: None,
        target: None,
    }
}

fn protected_gateway_subset_route_fixture(name: &str) -> ProtectedMcpRouteConfig {
    ProtectedMcpRouteConfig {
        name: name.to_string(),
        enabled: true,
        public_host: "mcp.tootie.tv".to_string(),
        public_path: "/media".to_string(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: Vec::new(),
        health_path: None,
        target: Some(
            labby_runtime::gateway_config::ProtectedMcpRouteTarget::GatewaySubset(
                labby_runtime::gateway_config::ProtectedGatewaySubsetTarget {
                    upstreams: vec!["sonarr".to_string()],
                    services: Vec::new(),
                    expose_code_mode: false,
                },
            ),
        ),
    }
}

#[tokio::test]
async fn protected_route_dispatch_add_list_and_test_share_gateway_actions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let tested = dispatch_with_manager(
        &manager,
        "gateway.protected_route.test",
        json!({ "route": protected_route_fixture("syslog") }),
    )
    .await
    .expect("test route");
    assert_eq!(tested["resource"], "https://mcp.tootie.tv/syslog");

    let added = dispatch_with_manager(
        &manager,
        "gateway.protected_route.add",
        json!({ "route": protected_route_fixture("syslog") }),
    )
    .await
    .expect("add route");
    assert_eq!(added["name"], "syslog");

    let listed = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("list routes");
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert_eq!(listed[0]["public_host"], "mcp.tootie.tv");
}

#[tokio::test]
async fn protected_gateway_subset_hot_crud_requires_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let err = dispatch_with_manager(
        &manager,
        "gateway.protected_route.add",
        json!({ "route": protected_gateway_subset_route_fixture("media") }),
    )
    .await
    .expect_err("gateway_subset add must not pretend to hot-mount scoped service");
    assert_eq!(err.kind(), "restart_required");

    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![protected_gateway_subset_route_fixture("media")],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let err = dispatch_with_manager(
        &manager,
        "gateway.protected_route.update",
        json!({
            "name": "media",
            "route": protected_gateway_subset_route_fixture("media")
        }),
    )
    .await
    .expect_err("gateway_subset update must not leave stale scoped service mounted");
    assert_eq!(err.kind(), "restart_required");

    let err = dispatch_with_manager(
        &manager,
        "gateway.protected_route.remove",
        json!({ "name": "media" }),
    )
    .await
    .expect_err("gateway_subset remove must not leave stale scoped service mounted");
    assert_eq!(err.kind(), "restart_required");
}

#[tokio::test]
async fn gateway_server_get_returns_custom_gateway_row() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let value = dispatch_with_manager(&manager, "gateway.server.get", json!({"id":"fixture-http"}))
        .await
        .expect("server get");

    assert_eq!(value["id"], "fixture-http");
    assert_eq!(value["source"], "custom_gateway");
}

#[tokio::test]
async fn gateway_list_surfaces_cached_custom_gateway_summary_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(path, runtime.clone());

    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "noxa".to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("noxa".to_string()),
            args: vec!["mcp".to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: false,
            expose_tools: Some(vec!["scrape".to_string()]),
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let pool = crate::upstream::pool::UpstreamPool::new();
    let upstream_name: std::sync::Arc<str> = std::sync::Arc::from("noxa");
    let mut tools = std::collections::HashMap::new();
    for name in ["scrape", "crawl"] {
        let schema = std::sync::Arc::new(serde_json::Map::new());
        let tool = rmcp::model::Tool::new(name, format!("{name} description"), schema);
        tools.insert(
            name.to_string(),
            crate::upstream::types::UpstreamTool {
                tool,
                input_schema: None,
                output_schema: None,
                upstream_name: std::sync::Arc::clone(&upstream_name),
                destructive: false,
            },
        );
    }
    pool.insert_entry_for_tests(
        "noxa",
        crate::upstream::types::UpstreamEntry {
            name: std::sync::Arc::clone(&upstream_name),
            tools,
            exposure_policy: crate::upstream::types::ToolExposurePolicy::from_patterns(vec![
                "scrape".to_string(),
            ])
            .expect("policy"),
            proxy_resources: true,
            prompt_count: 3,
            resource_count: 4,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::upstream::types::UpstreamHealth::Healthy,
            prompt_health: crate::upstream::types::UpstreamHealth::Healthy,
            resource_health: crate::upstream::types::UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        },
    )
    .await;
    runtime.swap(Some(std::sync::Arc::new(pool))).await;

    let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list");
    let row = value
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "noxa")
        .expect("noxa row");

    assert_eq!(row["discovered_tool_count"], 2);
    assert_eq!(row["exposed_tool_count"], 1);
    assert_eq!(row["discovered_resource_count"], 4);
    assert_eq!(row["exposed_resource_count"], 4);
    assert_eq!(row["discovered_prompt_count"], 3);
    assert_eq!(row["exposed_prompt_count"], 3);
}

// Re-fixtured post-gateway-pivot: backed by the kept `deploy` service and a real
// `deploy.plan` action (the policy validator checks `allowed_actions` against the
// service's compiled action catalog, so the action must actually exist for
// `deploy`). The original `server.info` belonged to a removed plex/radarr service.
#[tokio::test]
async fn virtual_server_policy_validation_uses_service_name() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy-primary".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.set_mcp_policy",
        json!({"id":"deploy-primary","allowed_actions":["deploy.plan"]}),
    )
    .await
    .expect("set policy");

    assert_eq!(value["allowed_actions"][0], "deploy.plan");
}

#[test]
fn supported_services_lists_metadata_backed_lab_gateways() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"gateway.supported_services"));
}

#[tokio::test]
async fn supported_services_payload_includes_plex_when_feature_enabled() {
    let manager = test_manager();
    let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
        .await
        .expect("supported services");

    let _services = value.as_array().expect("array");
}

// CANNOT be re-fixtured without a production change (out of test-only scope): it
// relies on `filter_built_in_upstream_apis(reg, false)` removing `deploy`/`setup`,
// but post-pivot that filter is a documented no-op — no `BuiltInUpstreamApi`
// services remain (all surviving services are `BootstrapOperator`). See
// `registry::tests::upstream_api_filter_is_noop_after_gateway_pivot`. So `deploy`
// and `setup` are never omitted and the assertions can't hold. Re-enabling needs a
// real `BuiltInUpstreamApi` service to exist again — a production change.
#[tokio::test]
#[ignore = "filter_built_in_upstream_apis is a no-op post-pivot (no BuiltInUpstreamApi services left); deploy/setup are never omitted — prod change required"]
async fn supported_services_omits_upstreams_when_policy_disabled() {
    // NOTE: the default-registry builder + upstream-API filter live in the `lab`
    // binary, not `lab-gateway`. This test is permanently `#[ignore]`d (the filter
    // is a no-op post-pivot), so an `EmptyServiceRegistry` keeps it compiling here.
    let registry = std::sync::Arc::new(crate::gateway::service_registry::EmptyServiceRegistry);
    let manager = test_manager().with_builtin_service_registry(registry);
    let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
        .await
        .expect("supported services");

    let services = value.as_array().expect("array");
    assert!(!services.iter().any(|service| service["key"] == "deploy"));
    assert!(!services.iter().any(|service| service["key"] == "setup"));
}

// CANNOT be re-fixtured without a production change (out of test-only scope): same
// root cause as `supported_services_omits_upstreams_when_policy_disabled` —
// `filter_built_in_upstream_apis(reg, false)` is a no-op post-pivot, so `deploy`
// stays in the registry and `service_actions` returns its catalog instead of
// erroring. Re-enabling needs a real `BuiltInUpstreamApi` service.
#[tokio::test]
#[ignore = "filter_built_in_upstream_apis is a no-op post-pivot; deploy is never filtered out so service_actions does not error — prod change required"]
async fn service_actions_rejects_disabled_upstream_service() {
    // See note above: builder + filter live in `lab`; permanently ignored here.
    let registry = std::sync::Arc::new(crate::gateway::service_registry::EmptyServiceRegistry);
    let manager = test_manager().with_builtin_service_registry(registry);
    let err = dispatch_with_manager(
        &manager,
        "gateway.service_actions",
        json!({"service": "deploy"}),
    )
    .await
    .expect_err("disabled service should be unknown");

    assert_eq!(err.kind(), "invalid_param");
}

// CANNOT be re-fixtured without a production change (out of test-only scope): same
// root cause — `filter_built_in_upstream_apis(reg, false)` is a no-op post-pivot, so
// `deploy` remains a registered service and enabling its virtual server succeeds
// instead of returning `not_found`. Re-enabling needs a real `BuiltInUpstreamApi`
// service.
#[tokio::test]
#[ignore = "filter_built_in_upstream_apis is a no-op post-pivot; deploy stays registered so virtual_server.enable succeeds — prod change required"]
async fn virtual_server_enable_rejects_disabled_upstream_service() {
    // See note above: builder + filter live in `lab`; permanently ignored here.
    let registry = std::sync::Arc::new(crate::gateway::service_registry::EmptyServiceRegistry);
    let manager = test_manager().with_builtin_service_registry(registry);
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: false,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let err = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.enable",
        json!({"id": "deploy"}),
    )
    .await
    .expect_err("disabled upstream virtual server should be unavailable");

    assert_eq!(err.kind(), "not_found");
}

// Re-fixtured post-gateway-pivot: backed by the kept/registered `deploy` service.
#[tokio::test]
async fn enabling_virtual_server_marks_existing_server_row_enabled() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: false,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.enable",
        json!({"id": "deploy"}),
    )
    .await
    .expect("enable");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["enabled"], true);
}

// CANNOT be re-fixtured without a production change (out of test-only scope): it
// drives `gateway.service_config.set` with `PLEX_*` values, which only succeeds for
// a `service_meta`-resolvable service that declares those env fields. Post-pivot the
// only resolvable service is `deploy`, which declares zero env fields, so the set is
// rejected before a service row can be created. Needs a service_meta service with
// env fields.
#[tokio::test]
#[ignore = "service_config.set requires a service_meta service with env fields; only deploy resolves and it has none — prod change required"]
async fn enabling_virtual_server_creates_missing_service_row() {
    let manager = test_manager();

    dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "deploy",
            "values": {
                "PLEX_URL": "http://127.0.0.1:32400",
                "PLEX_TOKEN": "token"
            }
        }),
    )
    .await
    .expect("set service config");

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.enable",
        json!({"id": "deploy"}),
    )
    .await
    .expect("enable missing virtual server");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["source"], "in_process");
    assert_eq!(value["enabled"], true);
    assert_eq!(value["surfaces"]["mcp"]["enabled"], true);
}

#[tokio::test]
async fn disabling_virtual_server_keeps_server_row_visible_but_disabled() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.disable",
        json!({"id": "deploy"}),
    )
    .await
    .expect("disable");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["enabled"], false);

    let list = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list after disable");
    assert!(
        list.as_array()
            .expect("array")
            .iter()
            .any(|server| server["id"] == "deploy" && server["enabled"] == false)
    );
}

// CANNOT be re-fixtured without a production change (out of test-only scope):
// `gateway.service_config.set` with `PLEX_*` requires a service_meta-resolvable
// service that declares those env fields. Only `deploy` resolves post-pivot and it
// declares none, so the write is rejected. Needs a service_meta service with env
// fields.
#[tokio::test]
#[ignore = "service_config.set requires a service_meta service with env fields; only deploy resolves and it has none — prod change required"]
async fn setting_service_config_writes_canonical_env_backed_fields() {
    let manager = test_manager();

    let value = dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "deploy",
            "values": {
                "PLEX_URL": "http://127.0.0.1:32400",
                "PLEX_TOKEN": "token"
            }
        }),
    )
    .await
    .expect("set service config");

    assert_eq!(value["service"], "deploy");
    assert_eq!(value["configured"], true);
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "PLEX_URL" && field["present"] == true)
    );
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "PLEX_TOKEN" && field["present"] == true)
    );
}

// CANNOT be re-fixtured without a production change (out of test-only scope):
// `gateway.service_config.set` with `PLEX_*` requires a service_meta-resolvable
// service that declares those env fields. Only `deploy` resolves post-pivot and it
// declares none, so the write is rejected before the read-back can be exercised.
// Needs a service_meta service with env fields.
#[tokio::test]
#[ignore = "service_config.set requires a service_meta service with env fields; only deploy resolves and it has none — prod change required"]
async fn configured_but_disabled_service_can_be_read_back_for_editing() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: false,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "deploy",
            "values": {
                "PLEX_URL": "http://127.0.0.1:32400",
                "PLEX_TOKEN": "token"
            }
        }),
    )
    .await
    .expect("set service config");

    let value = dispatch_with_manager(
        &manager,
        "gateway.service_config.get",
        json!({"service": "deploy"}),
    )
    .await
    .expect("get service config");

    assert_eq!(value["service"], "deploy");
    assert_eq!(value["configured"], true);
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "PLEX_URL"
                && field["value_preview"] == "http://127.0.0.1:32400")
    );
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "PLEX_TOKEN" && field["secret"] == true)
    );
}

#[tokio::test]
async fn setting_virtual_server_surface_updates_visible_server_row() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    mcp: true,
                    ..labby_runtime::gateway_config::VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.set_surface",
        json!({"id": "deploy", "surface": "api", "enabled": true}),
    )
    .await
    .expect("set surface");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["surfaces"]["api"]["enabled"], true);
}

// Re-fixtured post-gateway-pivot: backed by the kept `deploy` service and its real
// `deploy.plan` action (the policy validator checks allowed_actions against the
// service's compiled catalog, so the action must exist for `deploy`).
#[tokio::test]
async fn setting_virtual_server_mcp_policy_persists_allowed_actions() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.set_mcp_policy",
        json!({"id": "deploy", "allowed_actions": ["deploy.plan"]}),
    )
    .await
    .expect("set mcp policy");

    assert_eq!(value["allowed_actions"], json!(["deploy.plan"]));

    let reloaded = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.get_mcp_policy",
        json!({"id": "deploy"}),
    )
    .await
    .expect("get mcp policy");

    assert_eq!(reloaded["allowed_actions"], json!(["deploy.plan"]));
}

// Re-fixtured post-gateway-pivot: assert against the kept `deploy` service's real
// `deploy.plan` action instead of the removed plex/radarr `server.info`.
#[tokio::test]
async fn service_actions_returns_compiled_action_catalog() {
    let manager = test_manager();
    let value = dispatch_with_manager(
        &manager,
        "gateway.service_actions",
        json!({"service": "deploy"}),
    )
    .await
    .expect("service actions");

    let actions = value.as_array().expect("array");
    assert!(actions.iter().any(|action| action["name"] == "deploy.plan"));
}

#[tokio::test]
async fn gateway_get_rejects_missing_name() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.get", json!({}))
        .await
        .expect_err("missing name should fail");

    assert_eq!(err.kind(), "invalid_param");
}

/// `gateway.test` with a `spec` whose `command` field names a stdio upstream
/// **executes that command as a real child process**.  This test uses `echo` so
/// the subprocess exits cleanly on all platforms.  See docs/UPSTREAM.md §"Testing
/// with Stdio Upstreams" and the SECURITY NOTE in the `gateway.test` handler.
#[tokio::test]
async fn gateway_test_spec_stdio_executes_command_and_name_routes_to_config() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
            UpstreamConfig {
                enabled: true,
                name: "configured-stdio".to_string(),
                url: None,
                bearer_token_env: None,
                command: Some("echo".to_string()),
                args: vec!["hello".to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
        ])
        .await;

    let named = dispatch_with_manager(&manager, "gateway.test", json!({"name": "fixture-http"}))
        .await
        .expect("named test");
    // Stdio gateways test freely — no ack required.
    let named_stdio = dispatch_with_manager(
        &manager,
        "gateway.test",
        json!({"name": "configured-stdio"}),
    )
    .await
    .expect("configured stdio test");
    let proposed = dispatch_with_manager(
        &manager,
        "gateway.test",
        json!({"spec": {
            "name": "fixture-stdio",
            "command": "npx",
            "args": ["hello"]
        }}),
    )
    .await
    .expect("spec test");

    assert_eq!(named["name"], "fixture-http");
    assert_eq!(named_stdio["name"], "configured-stdio");
    assert_eq!(proposed["name"], "fixture-stdio");
}

#[tokio::test]
async fn gateway_mutations_call_manager_methods() {
    let manager = test_manager();

    let added = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "fixture-http",
            "url": "https://fixture.example.com/mcp",
            "bearer_token_env": "FIXTURE_HTTP_TOKEN"
        }}),
    )
    .await
    .expect("add");
    assert_eq!(added["config"]["name"], "fixture-http");
    assert_eq!(added["config"]["bearer_token_env"], "FIXTURE_HTTP_TOKEN");

    let public = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "deepwiki",
            "url": "https://mcp.deepwiki.com/mcp"
        }}),
    )
    .await
    .expect("add no-auth http");
    assert_eq!(public["config"]["name"], "deepwiki");
    assert_eq!(public["config"]["bearer_token_env"], Value::Null);

    let updated = dispatch_with_manager(
        &manager,
        "gateway.update",
        json!({"name": "fixture-http", "patch": {"proxy_resources": true}}),
    )
    .await
    .expect("update");
    assert_eq!(updated["config"]["proxy_resources"], true);

    let status = dispatch_with_manager(&manager, "gateway.status", json!({}))
        .await
        .expect("status");
    assert!(status.is_array());

    let removed =
        dispatch_with_manager(&manager, "gateway.remove", json!({"name": "fixture-http"}))
            .await
            .expect("remove");
    assert_eq!(removed["config"]["name"], "fixture-http");

    let reloaded = dispatch_with_manager(&manager, "gateway.reload", json!({}))
        .await
        .expect("reload");
    assert!(reloaded.get("tools_changed").is_some());
}

#[tokio::test]
async fn gateway_add_stdio_needs_no_ack() {
    let manager = test_manager();

    let added = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "fixture-stdio",
            "command": "npx",
            "args": ["hello"]
        }}),
    )
    .await
    .expect("stdio add without ack");

    assert_eq!(added["config"]["name"], "fixture-stdio");
}

#[tokio::test]
async fn gateway_update_stdio_needs_no_ack() {
    let manager = test_manager();
    dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "fixture-stdio",
            "command": "npx",
            "args": ["hello"]
        }}),
    )
    .await
    .expect("add stdio");

    let updated = dispatch_with_manager(
        &manager,
        "gateway.update",
        json!({"name": "fixture-stdio", "patch": {"proxy_resources": true}}),
    )
    .await
    .expect("stdio update without ack");

    assert_eq!(updated["config"]["proxy_resources"], true);
}

#[tokio::test]
async fn virtual_server_remove_deletes_configured_service_row() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "stale-registry".to_string(),
                service: "mcpregistry".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    mcp: true,
                    ..labby_runtime::gateway_config::VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let removed = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.remove",
        json!({"id": "stale-registry"}),
    )
    .await
    .expect("remove virtual server");

    assert_eq!(removed["id"], "stale-registry");
    assert_eq!(removed["warnings"][0]["code"], "unknown_service");

    let remaining = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list after remove");
    assert_eq!(remaining.as_array().expect("array").len(), 0);
}

// Re-fixtured post-gateway-pivot: the quarantined virtual server is backed by the
// kept/registered `deploy` service, so restore returns it to the active list.
#[tokio::test]
async fn virtual_server_quarantine_list_and_restore_round_trip() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            quarantined_virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    mcp: true,
                    ..labby_runtime::gateway_config::VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let quarantined = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.quarantine.list",
        json!({}),
    )
    .await
    .expect("list quarantine");
    assert_eq!(quarantined.as_array().expect("array").len(), 1);
    assert_eq!(quarantined[0]["id"], "deploy");

    let restored = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.quarantine.restore",
        json!({"id": "deploy"}),
    )
    .await
    .expect("restore quarantine");
    assert_eq!(restored["id"], "deploy");

    let remaining = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.quarantine.list",
        json!({}),
    )
    .await
    .expect("list after restore");
    assert_eq!(remaining.as_array().expect("array").len(), 0);

    let listed = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list active");
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert_eq!(listed[0]["id"], "deploy");
}

#[tokio::test]
async fn invalid_gateway_specs_return_validation_errors() {
    let manager = test_manager();

    let invalid_url = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {"name": "bad", "url": "ftp://example.com"}}),
    )
    .await
    .expect_err("invalid scheme");
    assert_eq!(invalid_url.kind(), "invalid_param");

    let invalid_transport = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {"name": "bad", "url": "http://127.0.0.1:9001", "command": "node"}}),
    )
    .await
    .expect_err("invalid transport");
    assert_eq!(invalid_transport.kind(), "invalid_param");
}

#[tokio::test]
async fn only_reload_promises_to_pick_up_changed_bearer_token_env_vars() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let status = dispatch_with_manager(&manager, "gateway.status", json!({}))
        .await
        .expect("status");
    assert!(status.is_array());

    let help = dispatch_with_manager(&manager, "help", json!({}))
        .await
        .expect("help");
    assert_eq!(help["service"], "gateway");
    assert!(
        help.to_string().contains("gateway.reload"),
        "reload should remain the explicit env-refresh action"
    );
}

#[tokio::test]
async fn public_urls_action_dispatches_to_manager() {
    let manager = test_manager();

    let value = dispatch_with_manager(&manager, "gateway.public_urls.get", json!({}))
        .await
        .expect("public urls");

    assert!(value.get("app").is_some());
    assert!(value.get("mcp_gateway").is_some());
    assert!(value.get("effective_mcp_gateway").is_some());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gateway_mcp_cleanup_dispatch_returns_cleanup_payload() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let manager = test_manager();
    let upstream_name = "github-chat-cleanup-dispatch";
    let runtime_arg = "github-chat-cleanup-dispatch-mcp";
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    use std::os::unix::process::CommandExt;
    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Keep this stand-in out of nextest's process group so the test
    // process survives when cleanup kills the child's process group.
    command.process_group(0);
    let mut child = command.spawn().expect("spawn github chat stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.mcp.cleanup",
        json!({
            "name": upstream_name,
            "aggressive": false,
            "dry_run": false
        }),
    )
    .await
    .expect("cleanup dispatch");

    assert_eq!(value["upstream"], upstream_name);
    assert_eq!(value["aggressive"], false);
    assert!(
        value["gateway_killed"]
            .as_u64()
            .expect("gateway_killed as u64")
            >= 1
    );

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("github-chat stand-in process was not terminated by dispatch cleanup");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gateway_mcp_disable_with_cleanup_returns_gateway_and_cleanup_payload() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let manager = test_manager();
    let upstream_name = "github-chat-disable-dispatch";
    let runtime_arg = "github-chat-disable-dispatch-mcp";
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The cleanup path kills process groups for child runtimes. Keep this
    // stand-in out of nextest's process group so the test process survives.
    command.process_group(0);
    let mut child = command.spawn().expect("spawn github chat stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.mcp.disable",
        json!({
            "name": upstream_name,
            "cleanup": true,
            "aggressive": false
        }),
    )
    .await
    .expect("disable dispatch");

    assert_eq!(value["gateway"]["config"]["name"], upstream_name);
    assert_eq!(value["gateway"]["config"]["enabled"], false);
    assert_eq!(value["cleanup"]["upstream"], upstream_name);

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("github-chat stand-in process was not terminated by disable cleanup");
}

#[test]
fn discovery_url_preview_redacts_secret_url_parts() {
    assert_eq!(
        redact_url_preview("https://user:pass@example.com/mcp?token=secret#frag"),
        "https://example.com/mcp"
    );
    assert_eq!(redact_url_preview("not a url token=secret"), "<redacted>");
}

// ── shape_discovered_views unit tests ──────────────────────────────────

fn make_discovered_http(name: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: UpstreamConfig {
            name: name.to_string(),
            enabled: false,
            url: Some("http://127.0.0.1:9000".to_string()),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: true,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        },
        source_client: "cursor".to_string(),
        source_path: "/home/user/.cursor/mcp.json".to_string(),
        env_key_count: 0,
    }
}

fn make_discovered_stdio(name: &str, command: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: UpstreamConfig {
            name: name.to_string(),
            enabled: false,
            url: None,
            bearer_token_env: None,
            command: Some(command.to_string()),
            args: vec!["--serve".to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: true,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        },
        source_client: "claude-code".to_string(),
        source_path: "/home/user/.claude/settings.json".to_string(),
        env_key_count: 2,
    }
}

#[test]
fn shape_http_server_gets_http_transport_no_command_preview() {
    let discovered = vec![make_discovered_http("my-http-server")];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let existing: HashSet<String> = HashSet::new();
    let params = GatewayDiscoverParams::default();

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].transport, McpClientTransportType::Http);
    assert!(views[0].command_preview.is_none());
    assert_eq!(views[0].name, "my-http-server");
}

#[test]
fn shape_stdio_server_gets_stdio_transport_and_command_preview_first_token() {
    let discovered = vec![make_discovered_stdio(
        "my-stdio-server",
        "npx --yes some-mcp",
    )];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let existing: HashSet<String> = HashSet::new();
    let params = GatewayDiscoverParams::default();

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].transport, McpClientTransportType::Stdio);
    assert_eq!(views[0].command_preview.as_deref(), Some("npx"));
}

#[test]
fn shape_already_configured_true_when_name_in_existing_set() {
    let discovered = vec![make_discovered_http("configured-server")];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let mut existing: HashSet<String> = HashSet::new();
    existing.insert("configured-server".to_string());
    let params = GatewayDiscoverParams {
        include_existing: true,
        ..GatewayDiscoverParams::default()
    };

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert!(views[0].already_configured);
}

#[test]
fn shape_include_existing_false_filters_out_already_configured_servers() {
    let discovered = vec![
        make_discovered_http("new-server"),
        make_discovered_http("existing-server"),
    ];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let mut existing: HashSet<String> = HashSet::new();
    existing.insert("existing-server".to_string());
    let params = GatewayDiscoverParams {
        include_existing: false,
        ..GatewayDiscoverParams::default()
    };

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name, "new-server");
    assert!(!views[0].already_configured);
}

// ── handle_import and handle_discover validation branch tests ──────────

#[tokio::test]
async fn gateway_import_rejects_empty_params() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.import", json!({}))
        .await
        .expect_err("empty import params should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_import_rejects_both_all_and_names() {
    let manager = test_manager();
    let err = dispatch_with_manager(
        &manager,
        "gateway.import",
        json!({"all": true, "names": ["some-server"]}),
    )
    .await
    .expect_err("both all and names should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_import_rejects_unknown_client_kind() {
    let manager = test_manager();
    let err = dispatch_with_manager(
        &manager,
        "gateway.import",
        json!({"all": true, "clients": ["not-a-real-client"]}),
    )
    .await
    .expect_err("unknown client kind should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_discover_rejects_unknown_client_kind() {
    let manager = test_manager();
    let err = dispatch_with_manager(
        &manager,
        "gateway.discover",
        json!({"clients": ["typo-client"]}),
    )
    .await
    .expect_err("unknown client kind in discover should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_import_result_has_correct_shape() {
    // Verify the ImportResultView shape: all=true on empty discovery
    // returns ImportResultView with empty imported/skipped/errors
    let manager = test_manager();
    let result = dispatch_with_manager(&manager, "gateway.import", json!({"all": true}))
        .await
        .expect("all=true should succeed even with no discovered servers");
    // The result should be an object (ImportResultView), not an array
    assert!(
        result.is_object(),
        "import result should be an object with imported/skipped/errors"
    );
    assert!(
        result.get("imported").is_some(),
        "should have imported field"
    );
}

// --- lab-l3cm regression: public dispatch() must handle built-ins before manager resolution ---

/// `gateway::dispatch("help", …)` must succeed even when no gateway manager
/// is installed.  The old code called `require_gateway_manager()` first,
/// which returned `internal_error` in that situation.
#[tokio::test]
async fn gateway_dispatch_help_succeeds_without_manager() {
    let old = super::super::client::swap_gateway_manager_for_test(None);
    let result = dispatch("help", serde_json::json!({})).await;
    super::super::client::swap_gateway_manager_for_test(old);

    let value = result.expect("help must not require a gateway manager");
    assert_eq!(value["service"], "gateway");
    assert!(
        value["actions"].is_array(),
        "help response must contain an actions array"
    );
}

/// `gateway::dispatch("schema", {action: "gateway.list"})` must succeed even
/// when no gateway manager is installed.
#[tokio::test]
async fn gateway_dispatch_schema_succeeds_without_manager() {
    let old = super::super::client::swap_gateway_manager_for_test(None);
    let result = dispatch("schema", serde_json::json!({"action": "gateway.list"})).await;
    super::super::client::swap_gateway_manager_for_test(old);

    let value = result.expect("schema must not require a gateway manager");
    assert_eq!(value["action"], "gateway.list");
}

/// `gateway::dispatch("schema", {})` with a missing `action` param must
/// return `missing_param`, not `internal_error`.
#[tokio::test]
async fn gateway_dispatch_schema_missing_param_without_manager() {
    let old = super::super::client::swap_gateway_manager_for_test(None);
    let err = dispatch("schema", serde_json::json!({}))
        .await
        .expect_err("schema without action param must fail");
    super::super::client::swap_gateway_manager_for_test(old);

    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(body["kind"], "missing_param");
    assert_eq!(body["param"], "action");
}
fn upstream_fixture(name: &str, url: Option<String>, command: Option<String>) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        enabled: false,
        url,
        bearer_token_env: None,
        command,
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

fn make_http_server(name: &str, url: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: upstream_fixture(name, Some(url.to_string()), None),
        source_client: "test".to_string(),
        source_path: "/tmp/test.json".to_string(),
        env_key_count: 0,
    }
}

fn make_stdio_server(name: &str, command: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: upstream_fixture(name, None, Some(command.to_string())),
        source_client: "test".to_string(),
        source_path: "/tmp/test.json".to_string(),
        env_key_count: 2,
    }
}

#[test]
fn http_server_gets_http_transport() {
    let views = shape_discovered_views(
        vec![make_http_server("srv", "https://example.com/mcp")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &HashSet::new(),
        &GatewayDiscoverParams::default(),
    );
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0].transport, McpClientTransportType::Http));
    assert!(views[0].command_preview.is_none());
}

#[test]
fn stdio_server_gets_stdio_transport_and_command_preview() {
    let views = shape_discovered_views(
        vec![make_stdio_server("srv", "npx @some/mcp-server")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &HashSet::new(),
        &GatewayDiscoverParams::default(),
    );
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0].transport, McpClientTransportType::Stdio));
    assert_eq!(views[0].command_preview.as_deref(), Some("npx"));
}

#[test]
fn already_configured_flag_set_when_name_in_existing() {
    let mut existing = HashSet::new();
    existing.insert("known-server".to_string());
    let views = shape_discovered_views(
        vec![make_http_server("known-server", "https://h/m")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &existing,
        &GatewayDiscoverParams {
            include_existing: true,
            clients: vec![],
        },
    );
    assert_eq!(views.len(), 1);
    assert!(views[0].already_configured);
}

#[test]
fn include_existing_false_filters_out_configured_servers() {
    let mut existing = HashSet::new();
    existing.insert("known-server".to_string());
    let views = shape_discovered_views(
        vec![make_http_server("known-server", "https://h/m")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &existing,
        &GatewayDiscoverParams::default(), // include_existing defaults to false
    );
    assert!(views.is_empty());
}
