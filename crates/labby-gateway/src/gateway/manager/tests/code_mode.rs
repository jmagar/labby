//! Code Mode runtime readiness + tool resolution tests.
#![allow(clippy::panic)]

use labby_codemode::{CodeModeCaller, CodeModeHost, CodeModeSurface, ToolScope};
use labby_runtime::error::ToolError;
use serde_json::json;

use super::*;

#[tokio::test]
async fn search_tools_seeds_cold_lazy_runtime_before_searching() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        })
        .await;

    manager
        .ensure_search_runtime_ready(true, None, None)
        .await
        .expect_err("failed live discovery returns an actionable error");

    let pool = manager
        .current_pool()
        .await
        .expect("manager keeps a shared lazy pool installed");
    assert!(pool.cached_upstream_summary("alpha").await.is_some());
}

#[tokio::test]
async fn scoped_code_mode_catalog_fails_when_allowed_upstream_is_unhealthy() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "ping"))
        .await;
    let allowed = std::collections::BTreeSet::from(["alpha".to_string()]);

    let err = manager
        .code_mode_catalog_tools_allowed(true, None, None, Some(&allowed))
        .await
        .expect_err("healthy disallowed upstreams must not mask scoped connect failures");

    match err {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "upstream_connect_error");
            assert!(message.contains("alpha"));
            assert!(!message.contains("beta"));
        }
        other => panic!("expected upstream_connect_error sdk error, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_code_mode_upstream_tool_hides_priority_zero_upstreams() {
    let mut upstream = fixture_http_upstream("suppressed");
    upstream.priority = 0.0;
    let (manager, pool) = code_mode_manager_with_pool(upstream).await;
    pool.insert_entry_for_tests(
        "suppressed",
        healthy_entry_with_tool("suppressed", "secret-tool"),
    )
    .await;

    let err = manager
        .resolve_code_mode_upstream_tool("suppressed", "secret-tool", None, None)
        .await
        .expect_err("priority=0 upstream tools must not be invokable by code mode id");

    match err {
        ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "unknown_tool"),
        other => panic!("expected unknown_tool sdk error, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_code_mode_upstream_tool_resolves_requested_upstream() {
    // resolve_code_mode_upstream_tool requires the codemode surface, gated
    // solely by code_mode.enabled, to be active.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        })
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let tool = manager
        .resolve_code_mode_upstream_tool("alpha", "ping", None, None)
        .await
        .expect("code mode should resolve requested upstream");

    assert_eq!(tool.tool.name.as_ref(), "ping");
}

// Regression: the Cloudflare-parity surface exposes search+execute under
// `code_mode.enabled` (RootSynthetic). `execute`'s callTool must resolve
// upstream tools when `code_mode.enabled` is the active flag — the single
// toggle that exposes the surface. A prior merge gated resolution on a
// separate flag, so execute could never call a tool when the surface was
// exposed via code_mode (the only way it is exposed). The test suite did
// not cover this path, so it passed while the live server rejected callTool.
#[tokio::test]
async fn resolve_upstream_tool_works_with_code_mode_enabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        })
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let tool = manager
        .resolve_code_mode_upstream_tool("alpha", "ping", None, None)
        .await
        .expect("execute callTool must resolve when code_mode surface is enabled");

    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_resolves_cached_tool_without_code_mode() {
    let upstream = fixture_http_upstream("alpha");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: false,
                ..CodeModeConfig::default()
            },
            upstream: vec![upstream],
            ..GatewayConfig::default()
        })
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let (upstream, tool) = manager
        .resolve_raw_upstream_tool("ping", None, None)
        .await
        .expect("raw proxy resolution should not require code_mode");

    assert_eq!(upstream, "alpha");
    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_honors_qualified_upstream_name() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "ping"))
        .await;

    let (upstream, tool) = manager
        .resolve_raw_upstream_tool("beta::ping", None, None)
        .await
        .expect("qualified raw tool should resolve requested upstream");

    assert_eq!(upstream, "beta");
    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_scoped_hides_priority_zero_upstreams() {
    let mut upstream = fixture_http_upstream("suppressed");
    upstream.priority = 0.0;
    let (manager, pool) = code_mode_manager_with_upstreams(vec![upstream]).await;
    pool.insert_entry_for_tests("suppressed", healthy_entry_with_tool("suppressed", "ping"))
        .await;
    let allowed = std::collections::BTreeSet::from(["suppressed".to_string()]);

    let err = manager
        .resolve_raw_upstream_tool_scoped("suppressed::ping", Some(&allowed), None, None)
        .await
        .expect_err("priority=0 upstream tools must not be invokable through scoped raw proxy");

    match err {
        ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "unknown_tool"),
        other => panic!("expected unknown_tool sdk error, got {other:?}"),
    }
}

#[tokio::test]
async fn code_mode_enabled_reads_code_mode_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            ..GatewayConfig::default()
        })
        .await;

    // PRESENCE: code_mode_enabled() reflects code_mode.enabled = true
    assert!(
        manager.code_mode_enabled().await,
        "code_mode_enabled() must return true when code_mode.enabled = true"
    );
}

#[tokio::test]
async fn code_mode_host_list_tools_honors_scoped_namespaces() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "pong"))
        .await;

    let render = CodeModeHost::list_tools(
        &manager,
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::scoped_namespaces(vec!["alpha".to_string()], Vec::new()),
        false,
        false,
    )
    .await
    .expect("scoped Code Mode host catalog");

    let ids = render
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["alpha::ping"]);
}

#[tokio::test]
async fn code_mode_host_list_tools_for_mcp_does_not_block_on_cold_unhealthy_upstreams() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind hanging upstream fixture");
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _socket = socket;
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
        }
    });

    let mut hanging = fixture_http_upstream("alpha");
    hanging.url = Some(format!("http://{addr}/mcp"));
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![hanging, fixture_http_upstream("beta")]).await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "ping"))
        .await;

    let render = tokio::time::timeout(
        Duration::from_millis(100),
        CodeModeHost::list_tools(
            &manager,
            &CodeModeCaller::Scoped {
                capabilities: labby_codemode::CodeModeCallerCapabilities {
                    can_execute: true,
                    can_use_snippets: false,
                    is_admin: false,
                },
                sub: Some("user-1".to_string()),
            },
            CodeModeSurface::Mcp,
            &ToolScope::default(),
            false,
            false,
        ),
    )
    .await
    .expect("MCP proxy generation must not wait for cold upstream refresh")
    .expect("MCP Code Mode proxy generation should use current healthy tools");

    let ids = render
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["beta::ping"]);
}

#[tokio::test]
async fn code_mode_host_blocks_destructive_calls_for_read_only_callers() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let mut entry = healthy_entry_with_tool("alpha", "delete");
    entry
        .tools
        .get_mut("delete")
        .expect("fixture tool")
        .destructive = true;
    pool.insert_entry_for_tests("alpha", entry).await;

    let err = CodeModeHost::call_tool(
        &manager,
        "alpha::delete",
        json!({}),
        &CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
            sub: Some("user-1".to_string()),
        },
        CodeModeSurface::Mcp,
        &ToolScope::new(Vec::new(), Vec::new()),
    )
    .await
    .expect_err("read-only caller must not execute destructive tool");

    match err {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "forbidden");
            assert!(message.contains("alpha::delete"));
        }
        other => panic!("expected forbidden sdk error, got {other:?}"),
    }
}
