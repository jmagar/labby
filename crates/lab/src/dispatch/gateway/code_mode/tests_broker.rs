//! Tests: broker search/execute/call_tool over a live upstream catalog.
#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

fn fixture_upstream_entry(
    upstream: &str,
    tools: HashMap<String, crate::dispatch::upstream::types::UpstreamTool>,
) -> crate::dispatch::upstream::types::UpstreamEntry {
    crate::dispatch::upstream::types::UpstreamEntry {
        name: Arc::from(upstream),
        tools,
        exposure_policy: crate::dispatch::upstream::types::ToolExposurePolicy::All,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
        prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
        resource_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

fn fixture_catalog_tool(
    upstream: &str,
    tool_name: &str,
) -> crate::dispatch::upstream::types::UpstreamTool {
    let upstream_name: Arc<str> = Arc::from(upstream);
    crate::dispatch::upstream::types::UpstreamTool {
        tool: rmcp::model::Tool::new(
            tool_name.to_string(),
            format!("{tool_name} description"),
            Arc::new(serde_json::Map::new()),
        ),
        input_schema: None,
        output_schema: None,
        upstream_name,
        destructive: false,
    }
}

#[tokio::test]
async fn search_without_manager_returns_empty_array() {
    // No gateway manager → no upstream catalog → search returns an empty
    // array regardless of the supplied JS (it never runs the script).
    let registry = super::ToolRegistry::new();
    let broker = super::CodeModeBroker::new(&registry, None);

    let result = broker
        .search(
            "async () => tools",
            super::CodeModeCaller::TrustedLocal,
            super::CodeModeSurface::Cli,
        )
        .await
        .expect("search ok without manager");

    assert_eq!(result, serde_json::json!([]));
}

#[tokio::test]
async fn broker_search_exposes_typed_schema_metadata_from_live_catalog() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = super::super::runtime::GatewayRuntimeHandle::default();
    let pool = Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = super::GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: true,
                ..crate::config::CodeModeConfig::default()
            },
            ..crate::config::LabConfig::default()
        })
        .await;
    let upstream_name: Arc<str> = Arc::from("typed");
    let upstream_tool = crate::dispatch::upstream::types::UpstreamTool {
        tool: rmcp::model::Tool::new(
            "lookup".to_string(),
            "Lookup typed data",
            Arc::new(serde_json::Map::new()),
        ),
        input_schema: Some(json!({
            "type": "object",
            "properties": {"q": {"type": "string"}},
            "required": ["q"]
        })),
        output_schema: Some(json!({
            "type": "object",
            "properties": {"answer": {"type": "integer"}}
        })),
        upstream_name: Arc::clone(&upstream_name),
        destructive: false,
    };
    pool.insert_entry_for_tests(
        "typed",
        fixture_upstream_entry(
            "typed",
            HashMap::from([("lookup".to_string(), upstream_tool)]),
        ),
    )
    .await;

    let registry = super::ToolRegistry::new();
    let broker = super::CodeModeBroker::new(&registry, Some(&manager));
    // The JS evaluation step now runs in a subprocess (Javy runner) that lib
    // unit tests cannot spawn (`current_exe()` is the test harness, not
    // labby). The catalog projection (`schema`/`signature`/`dts`) is what this
    // test actually covers, so assert directly on `code_search_catalog` — the
    // same source `search` serializes into the runner as `const tools`.
    let caller = super::CodeModeCaller::Scoped {
        scopes: vec!["lab:read".to_string()],
        sub: None,
    };
    let surface = super::CodeModeSurface::Mcp;
    let owner = caller.runtime_owner(surface);
    let oauth_subject = caller.oauth_subject();
    let (entries, _catalog_json, _size) = broker
        .code_search_catalog(&manager, true, &owner, oauth_subject)
        .await
        .expect("catalog builds over live catalog");
    let result = serde_json::to_value(&entries).expect("serialize catalog");

    let entries = result.as_array().expect("array");
    let entry = entries
        .iter()
        .find(|entry| entry["id"] == "typed::lookup")
        .expect("typed lookup entry");
    assert_eq!(entry["schema"]["required"], json!(["q"]));
    assert_eq!(
        entry["output_schema"]["properties"]["answer"]["type"],
        "integer"
    );
    assert!(
        entry["signature"]
            .as_str()
            .is_some_and(|signature| signature.contains("Promise<"))
    );
    assert!(
        entry["dts"]
            .as_str()
            .is_some_and(|dts| dts.contains("interface Codemode"))
    );
}

#[tokio::test]
async fn broker_search_refreshes_read_only_catalog_after_upstream_tool_expansion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = super::super::runtime::GatewayRuntimeHandle::default();
    let pool = Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = super::GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: true,
                ..crate::config::CodeModeConfig::default()
            },
            upstream: vec![crate::config::UpstreamConfig {
                enabled: true,
                name: "agent-os_windows-mcp".to_string(),
                url: Some("http://127.0.0.1:9/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            ..crate::config::LabConfig::default()
        })
        .await;

    pool.insert_entry_for_tests(
        "agent-os_windows-mcp",
        fixture_upstream_entry(
            "agent-os_windows-mcp",
            HashMap::from([(
                "Wait".to_string(),
                fixture_catalog_tool("agent-os_windows-mcp", "Wait"),
            )]),
        ),
    )
    .await;

    let live_tools = Arc::new(tokio::sync::RwLock::new(vec!["Wait".to_string()]));
    pool.insert_live_tool_server_for_tests("agent-os_windows-mcp", Arc::clone(&live_tools))
        .await;

    let registry = super::ToolRegistry::new();
    let broker = super::CodeModeBroker::new(&registry, Some(&manager));
    let read_only = super::CodeModeCaller::Scoped {
        scopes: vec!["lab:read".to_string()],
        sub: None,
    };
    let surface = super::CodeModeSurface::Mcp;
    // The catalog-refresh behavior under test is in-process (`code_search_catalog`
    // re-resolves the live catalog on each call). The JS name-filter `search`
    // previously applied is now an in-test projection: collect agent-os tool
    // names from the freshly built catalog. (The runner that runs the JS filter
    // cannot be spawned from lib unit tests; see the sibling test.)
    let owner = read_only.runtime_owner(surface);
    let oauth_subject = read_only.oauth_subject();
    let agent_os_names = |entries: &[super::CodeModeCatalogEntry]| -> Vec<String> {
        let mut names: Vec<String> = entries
            .iter()
            .filter(|entry| entry.upstream == "agent-os_windows-mcp")
            .map(|entry| entry.name.clone())
            .collect();
        names.sort();
        names
    };

    let (initial, _catalog_json, _size) = broker
        .code_search_catalog(&manager, true, &owner, oauth_subject)
        .await
        .expect("initial read-only catalog builds over partial catalog");
    assert_eq!(agent_os_names(&initial), vec!["Wait".to_string()]);

    *live_tools.write().await = vec![
        "FileSystem".to_string(),
        "PowerShell".to_string(),
        "Snapshot".to_string(),
        "Wait".to_string(),
    ];

    let (refreshed, _catalog_json, _size) = broker
        .code_search_catalog(&manager, true, &owner, oauth_subject)
        .await
        .expect("read-only catalog refreshes expanded live catalog");
    assert_eq!(
        agent_os_names(&refreshed),
        vec![
            "FileSystem".to_string(),
            "PowerShell".to_string(),
            "Snapshot".to_string(),
            "Wait".to_string(),
        ]
    );

    let tool = manager
        .resolve_code_mode_upstream_tool("agent-os_windows-mcp", "PowerShell", None, None)
        .await
        .expect("execute resolution sees the same refreshed live catalog");
    assert_eq!(tool.tool.name.as_ref(), "PowerShell");
}

#[tokio::test]
async fn broker_call_tool_validates_schema_before_upstream_dispatch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = super::super::runtime::GatewayRuntimeHandle::default();
    let pool = Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = super::GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: true,
                ..crate::config::CodeModeConfig::default()
            },
            upstream: vec![crate::config::UpstreamConfig {
                enabled: true,
                name: "fixture".to_string(),
                url: Some("http://127.0.0.1:9/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            ..crate::config::LabConfig::default()
        })
        .await;
    let upstream_name: Arc<str> = Arc::from("fixture");
    let upstream_tool = crate::dispatch::upstream::types::UpstreamTool {
        tool: rmcp::model::Tool::new(
            "needs_action".to_string(),
            "Needs action",
            Arc::new(serde_json::Map::new()),
        ),
        input_schema: Some(json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {"type": "string"}
            }
        })),
        output_schema: None,
        upstream_name: Arc::clone(&upstream_name),
        destructive: false,
    };
    pool.insert_entry_for_tests(
        "fixture",
        fixture_upstream_entry(
            "fixture",
            HashMap::from([("needs_action".to_string(), upstream_tool)]),
        ),
    )
    .await;
    let registry = super::ToolRegistry::new();
    let broker = super::CodeModeBroker::new(&registry, Some(&manager));
    let tool_id = "fixture::needs_action";

    let err = broker
        .call_tool_id(
            tool_id,
            json!({}),
            super::CodeModeCaller::TrustedLocal,
            super::CodeModeSurface::Cli,
            &super::CodeModeCapabilityFilter::default(),
        )
        .await
        .expect_err("missing action must fail before dispatch");
    assert_eq!(err.kind(), "missing_param");
}

#[tokio::test]
async fn code_execute_call_tool_lab_id_returns_unknown_tool() {
    let registry = super::ToolRegistry::new();
    let broker = super::CodeModeBroker::new(&registry, None);

    let err = broker
        .call_tool_id(
            "lab::radarr.movie.search",
            json!({"query": "Matrix"}),
            super::CodeModeCaller::TrustedLocal,
            super::CodeModeSurface::Cli,
            &super::CodeModeCapabilityFilter::default(),
        )
        .await
        .expect_err("lab:: callTool id should return unknown_tool");

    match err {
        super::ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "unknown_tool");
            // Message references canonical tool name "execute" (Cloudflare-parity rename).
            assert!(message.contains("execute"));
            assert!(message.contains("\"radarr\""));
        }
        other => panic!("expected unknown_tool, got {other:?}"),
    }
}

/// When the search/execute surface is enabled (`code_mode.enabled=true`),
/// `resolve_code_mode_upstream_tool` must NOT reject calls with a surface guard.
/// With a live (healthy) `testup` entry seeded in the pool, requesting a tool
/// that entry does not expose must surface a genuine `unknown_tool` lookup miss
/// — not an `upstream_connect_error` that fires before the lookup ever runs.
#[tokio::test]
async fn resolve_upstream_tool_returns_unknown_tool_for_absent_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = super::super::runtime::GatewayRuntimeHandle::default();
    let pool = Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = super::GatewayManager::new(dir.path().join("config.toml"), runtime);
    manager
        .seed_config(crate::config::LabConfig {
            code_mode: crate::config::CodeModeConfig {
                enabled: true,
                ..crate::config::CodeModeConfig::default()
            },
            upstream: vec![crate::config::UpstreamConfig {
                enabled: true,
                name: "testup".to_string(),
                url: Some("http://127.0.0.1:9/mcp".to_string()),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            }],
            ..crate::config::LabConfig::default()
        })
        .await;

    // Seed a HEALTHY testup entry that exposes `present_tool` but not the tool
    // we will ask for. Because the entry is already healthy, the runtime-ready
    // step short-circuits (no cold connect), so the request reaches the actual
    // tool lookup and misses — proving a real `unknown_tool`, not a connect error.
    pool.insert_entry_for_tests(
        "testup",
        fixture_upstream_entry(
            "testup",
            HashMap::from([(
                "present_tool".to_string(),
                fixture_catalog_tool("testup", "present_tool"),
            )]),
        ),
    )
    .await;

    let err = manager
        .resolve_code_mode_upstream_tool("testup", "some_tool", None, None)
        .await
        .expect_err("tool not present — expect unknown_tool from a real lookup miss");

    match err {
        super::ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(
                sdk_kind, "unknown_tool",
                "absent tool on a healthy upstream must be a real unknown_tool lookup miss: {message}"
            );
            assert!(
                message.contains("testup::some_tool"),
                "error must name the missing tool, got: {message}"
            );
        }
        other => panic!("expected Sdk error, got {other:?}"),
    }
}
