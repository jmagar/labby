#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
//! Code Mode runtime readiness + tool resolution tests.
#![allow(clippy::panic)]

use labby_codemode::{CodeModeCaller, CodeModeHost, CodeModeSurface, ToolScope};
use labby_runtime::error::ToolError;
use serde_json::json;
use tracing_subscriber::layer::SubscriberExt;

use super::*;

#[tokio::test]
async fn code_mode_host_resource_read_connects_a_cold_upstream() {
    let mut upstream = fixture_http_upstream("alpha");
    upstream.proxy_resources = true;
    let (manager, _) = code_mode_manager_with_pool(upstream).await;
    let error = CodeModeHost::read_resource(
        &manager,
        "lab://upstream/alpha/fixture://skill".to_string(),
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
    )
    .await
    .expect_err("unreachable fixture must fail connection, not report a missing resource");
    assert!(
        matches!(error, ToolError::Sdk { sdk_kind, .. } if sdk_kind == "upstream_connect_error")
    );
}

#[tokio::test]
async fn code_mode_host_resource_read_checks_scope_before_connecting() {
    let mut upstream = fixture_http_upstream("alpha");
    upstream.proxy_resources = true;
    let (manager, pool) = code_mode_manager_with_pool(upstream).await;
    let error = CodeModeHost::read_resource(
        &manager,
        "lab://upstream/alpha/fixture://skill".to_string(),
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::scoped_namespaces(vec!["beta".to_string()], Vec::new()),
    )
    .await
    .expect_err("out-of-scope resource must be refused");
    assert!(matches!(error, ToolError::Sdk { sdk_kind, .. } if sdk_kind == "forbidden"));
    assert_eq!(pool.connection_count_for_tests().await, 0);
}

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
async fn unauthenticated_code_mode_readiness_never_discovers_oauth_catalog() {
    let (manager, pool) =
        code_mode_manager_with_pool(fixture_oauth_upstream("private", "http://127.0.0.1:9/mcp"))
            .await;

    let tools = manager
        .code_mode_catalog_tools(true, None, None)
        .await
        .expect("OAuth upstreams are skipped without a subject");

    assert!(tools.is_empty());
    assert!(pool.healthy_tools().await.is_empty());
    assert_eq!(pool.connection_count_for_tests().await, 0);
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

#[tokio::test]
async fn admin_tool_browser_search_and_describe_use_the_live_manager_catalog() {
    let (manager, pool) = code_mode_manager_with_pool(fixture_http_upstream("alpha")).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let searched = manager
        .search_admin_tools(Some("admin".to_string()), "alpha ping", 50)
        .await
        .expect("search live manager catalog");
    assert_eq!(searched.results.len(), 1);
    assert_eq!(searched.results[0].id, "alpha::ping");

    let described = manager
        .describe_admin_tool(Some("admin".to_string()), "alpha::ping")
        .await
        .expect("describe live manager catalog");
    assert_eq!(described.id, "alpha::ping");
    assert!(described.typescript.is_some());
    assert_eq!(pool.connection_count_for_tests().await, 0);
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
async fn advertised_subject_scoped_oauth_tool_normalizes_metadata_and_executes() {
    let upstream = fixture_oauth_upstream("private", "http://unused.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "alice",
        vec![rmcp::model::Tool::new(
            "private_ping".to_string(),
            "### Private ping \u{2066}documentation\u{2069}",
            Arc::new(serde_json::Map::from_iter([
                ("type".to_string(), json!("object")),
                ("required".to_string(), json!(["query"])),
                (
                    "properties".to_string(),
                    json!({
                        "query": {
                            "type": "string",
                            "description": "### Query \u{2066}documentation\u{2069}",
                            "enum": ["\u{2066}exact\u{2069}"]
                        }
                    }),
                ),
            ])),
        )],
    )
    .await;
    let caller = CodeModeCaller::Scoped {
        capabilities: labby_codemode::CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some("alice".to_string()),
    };

    let advertised = CodeModeHost::list_tools(
        &manager,
        &caller,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
        false,
        false,
    )
    .await
    .expect("subject catalog is advertised");
    assert!(
        advertised
            .entries
            .iter()
            .any(|entry| entry.id == "private::private_ping")
    );

    let resolved = manager
        .resolve_code_mode_upstream_tool("private", "private_ping", None, Some("alice"))
        .await
        .expect("advertised Code Mode tool resolves");
    assert_eq!(resolved.tool.name.as_ref(), "private_ping");
    let description = resolved.tool.description.as_deref().expect("description");
    assert!(!description.contains("###"));
    assert!(!description.contains('\u{2066}'));
    assert!(!description.contains('\u{2069}'));
    let query_schema = &resolved.tool.input_schema["properties"]["query"];
    let query_description = query_schema["description"].as_str().unwrap();
    assert!(!query_description.contains("###"));
    assert!(!query_description.contains('\u{2066}'));
    assert!(!query_description.contains('\u{2069}'));
    // Documentation is normalized, but schema values remain exact. Both the
    // discovery contract and the final peer check must use this representation.
    assert_eq!(query_schema["enum"], json!(["\u{2066}exact\u{2069}"]));
    for selector in ["private_ping", "private::private_ping"] {
        let (owner, resolved) = manager
            .resolve_raw_upstream_tool(selector, None, Some("alice"))
            .await
            .expect("advertised raw tool resolves");
        assert_eq!(owner, "private");
        assert_eq!(resolved.tool.name.as_ref(), "private_ping");
    }

    CodeModeHost::call_tool(
        &manager,
        "private::private_ping",
        json!({"query": "\u{2066}exact\u{2069}"}),
        &caller,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect("advertised subject-scoped tool executes through its subject peer");
}

#[tokio::test]
async fn annotated_read_only_fixture_is_searchable_describable_and_callable() {
    let upstream = fixture_oauth_upstream("fixture", "http://unused.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    let annotated = |name: &str, annotations: rmcp::model::ToolAnnotations| {
        let mut tool = rmcp::model::Tool::new(
            name.to_string(),
            format!("{name} fixture"),
            Arc::new(serde_json::Map::new()),
        );
        tool.annotations = Some(annotations);
        tool
    };
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![
            annotated(
                "provider_status",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            ),
            rmcp::model::Tool::new(
                "unannotated".to_string(),
                "unannotated fixture",
                Arc::new(serde_json::Map::new()),
            ),
            annotated(
                "contradictory",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(true),
            ),
        ],
    )
    .await;
    let caller = CodeModeCaller::Scoped {
        capabilities: labby_codemode::CodeModeCallerCapabilities {
            can_read: true,
            can_execute: false,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some("reader".to_string()),
    };
    let scope = ToolScope::default().read_only();

    let render = CodeModeHost::list_tools(
        &manager,
        &caller,
        CodeModeSurface::Mcp,
        &scope,
        false,
        false,
    )
    .await
    .expect("read-only fixture catalog");
    assert_eq!(
        render
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture::provider_status"]
    );

    let searched = labby_codemode::search_visible_tools(&render.entries, &scope, "provider", 10)
        .expect("search annotated fixture");
    assert_eq!(searched.results[0].id, "fixture::provider_status");
    let described =
        labby_codemode::describe_visible_tool(&render.entries, &scope, "fixture::provider_status")
            .expect("describe annotated fixture");
    assert_eq!(described.id, "fixture::provider_status");

    CodeModeHost::call_tool(
        &manager,
        "fixture::provider_status",
        json!({}),
        &caller,
        CodeModeSurface::Mcp,
        &scope,
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect("call annotated read-only fixture");
    let resource = CodeModeHost::read_resource(
        &manager,
        "lab://upstream/fixture/fixture://skill".to_string(),
        &caller,
        CodeModeSurface::Mcp,
        &scope,
    )
    .await
    .expect("read proxied fixture resource");
    assert_eq!(resource["contents"], json!([]));

    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![
            annotated(
                "provider_status",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            ),
            annotated(
                "operation_status",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            ),
        ],
    )
    .await;
    let refreshed = CodeModeHost::list_tools(
        &manager,
        &caller,
        CodeModeSurface::Mcp,
        &scope,
        false,
        false,
    )
    .await
    .expect("refreshed read-only fixture catalog");
    assert_eq!(
        refreshed
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture::operation_status", "fixture::provider_status"]
    );
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
async fn code_mode_catalog_preserves_upstream_output_schema_for_describe_types() {
    let output_schema = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "message": { "type": "string" }
        },
        "required": ["ok"],
        "additionalProperties": false
    });
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    pool.insert_entry_for_tests(
        "alpha",
        healthy_entry_with_typed_tool("alpha", "typed", output_schema.clone()),
    )
    .await;

    let render = CodeModeHost::list_tools(
        &manager,
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
        false,
        false,
    )
    .await
    .expect("Code Mode host catalog");
    let entry = render
        .entries
        .iter()
        .find(|entry| entry.id == "alpha::typed")
        .expect("typed tool entry");

    assert_eq!(entry.output_schema, Some(output_schema));
    assert!(
        entry.dts.contains("type AlphaTypedOutput = {"),
        "dts must define a concrete output type, got: {}",
        entry.dts
    );
    assert!(
        entry.dts.contains("ok: boolean;"),
        "dts must render output properties, got: {}",
        entry.dts
    );
    assert!(
        !entry.signature.contains("Promise<unknown>"),
        "signature must not degrade typed output to unknown: {}",
        entry.signature
    );
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
                    can_read: true,
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
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect_err("read-only caller must not execute destructive tool");

    assert_eq!(err.kind(), "forbidden");
    assert!(err.user_message().contains("alpha::delete"));
}

/// The read-only Code Mode gate rests entirely on the upstream's own
/// `readOnlyHint`; the operator-held `trusted_read_only_tools` allowlist that
/// used to be a second required conjunct is retired. These two tests pin both
/// directions of what is now the only gate, so a future change to it cannot
/// widen read-only execution unnoticed.
#[tokio::test]
async fn code_mode_host_blocks_unannotated_tools_for_read_only_callers() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    // Not destructive, and carrying no annotations at all — the ordinary shape
    // of an upstream tool that never declared its safety.
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let err = CodeModeHost::call_tool(
        &manager,
        "alpha::ping",
        json!({}),
        &CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
            sub: Some("user-1".to_string()),
        },
        CodeModeSurface::Mcp,
        &ToolScope::new(Vec::new(), Vec::new()),
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect_err("a read-only caller must not execute an unannotated tool");

    // Two layers deny this, and the catalog is the first: an unannotated tool is
    // not admitted to a read-only caller's catalog at all, so resolution fails
    // before the execution gate is consulted. `forbidden` would mean the catalog
    // admitted it and only the gate caught it; either is a denial, and asserting
    // both keeps this test honest if the layering ever shifts.
    assert!(
        matches!(err.kind(), "not_found" | "forbidden"),
        "a read-only caller must be denied an unannotated tool, got kind {}: {}",
        err.kind(),
        err.user_message()
    );
}

#[tokio::test]
async fn code_mode_host_admits_annotated_read_only_tools_without_an_operator_allowlist() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let mut entry = healthy_entry_with_tool("alpha", "ping");
    entry
        .tools
        .get_mut("ping")
        .expect("fixture tool")
        .tool
        .annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
    pool.insert_entry_for_tests("alpha", entry).await;

    // The fixture upstream is not reachable, so the call still fails — but it
    // must fail past the policy gate, not at it. Nothing here configures a
    // `trusted_read_only_tools` allowlist, which is the point: the annotation
    // alone is now sufficient.
    let outcome = CodeModeHost::call_tool(
        &manager,
        "alpha::ping",
        json!({}),
        &CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
            sub: Some("user-1".to_string()),
        },
        CodeModeSurface::Mcp,
        &ToolScope::new(Vec::new(), Vec::new()),
        labby_codemode::ExecCtx::none(),
    )
    .await;

    if let Err(err) = outcome {
        assert!(
            !err.user_message().contains("not explicitly annotated"),
            "an annotated read-only tool must clear the read-only gate, got: {}",
            err.user_message()
        );
    }
}

#[tokio::test]
async fn mcp_tool_error_result_does_not_poison_upstream_connection_health() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("unifi")]).await;
    pool.insert_tool_error_server_for_tests("unifi", "controller rejected the request")
        .await;

    let err = manager
        .execute_upstream_tool("unifi", "unifi", json!({}))
        .await
        .expect_err("is_error=true must remain a Code Mode tool error");

    assert_eq!(err.kind(), "tool_error");
    assert_eq!(
        pool.upstream_tool_last_error("unifi").await,
        None,
        "a successful MCP response must not count as a connection-health failure"
    );
}

#[tokio::test]
async fn cortex_exact_schema_rejects_bad_fields_before_upstream_dispatch() {
    // Minimal composed-schema fixture. This test proves the *wiring* facts:
    // a cached upstream inputSchema is enforced before dispatch (invalid_param)
    // and a pre-dispatch failure never touches upstream health. Full keyword
    // regression coverage (the original ~65-line Cortex schema) lives in
    // `labby-codemode`'s `tests_ids_schema.rs`.
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("cortex")]).await;
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action"],
        "properties": {
            "action": { "type": "string" },
            "project": { "type": "string" }
        },
        "oneOf": [
            {
                "properties": { "action": { "const": "project_context" } },
                "required": ["action", "project"]
            }
        ]
    });
    let upstream_name: Arc<str> = Arc::from("cortex");
    let tool = rmcp::model::Tool::new(
        "cortex".to_string(),
        "Cortex action dispatcher",
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "cortex",
        fixture_upstream_entry(
            "cortex",
            HashMap::from([(
                "cortex".to_string(),
                UpstreamTool {
                    tool,
                    input_schema: Some(schema),
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;

    for params in [
        // additionalProperties: false rejects the unknown field.
        json!({"action": "project_context", "project": "/repo", "since": "x"}),
        // oneOf requires `project` alongside `project_context`.
        json!({"action": "project_context"}),
    ] {
        let error = CodeModeHost::call_tool(
            &manager,
            "cortex::cortex",
            params,
            &CodeModeCaller::Scoped {
                capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
                sub: Some("user-1".to_string()),
            },
            CodeModeSurface::Mcp,
            &ToolScope::default(),
            labby_codemode::ExecCtx::none(),
        )
        .await
        .expect_err("schema mismatch must fail before the upstream call");
        assert_eq!(error.kind(), "invalid_param");
    }

    assert_eq!(
        pool.upstream_tool_last_error("cortex").await,
        None,
        "pre-dispatch schema failures must not affect upstream health"
    );
}

#[tokio::test]
async fn mcp_invalid_params_map_to_code_mode_invalid_param_without_health_failure() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("cortex")]).await;
    let message =
        "invalid project_context arguments: unknown field `since`, expected project, tool, limit";
    pool.insert_mcp_error_server_for_tests(
        "cortex",
        rmcp::model::ErrorData::invalid_params(message, None),
    )
    .await;

    let err = manager
        .execute_upstream_tool("cortex", "cortex", json!({}))
        .await
        .expect_err("invalid params must remain a Code Mode caller error");

    match err {
        ToolError::Sdk {
            sdk_kind,
            message: actual,
        } => {
            assert_eq!(sdk_kind, "invalid_param");
            assert_eq!(actual, message);
        }
        other => panic!("expected invalid_param sdk error, got {other:?}"),
    }
    assert_eq!(
        pool.upstream_tool_last_error("cortex").await,
        None,
        "valid MCP errors must not poison upstream health"
    );
    assert!(
        pool.upstream_tool_health("cortex")
            .await
            .expect("health entry")
            .is_routable(),
        "upstream must remain routable after invalid params"
    );
}

#[tokio::test]
async fn palette_catalog_discovers_configured_upstream_tools() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let tools = Arc::new(tokio::sync::RwLock::new(vec!["ping".to_string()]));
    assert!(
        pool.healthy_tools_for_upstream("alpha").await.is_empty(),
        "fixture starts as a lazy-seeded upstream without cached tools"
    );
    pool.insert_live_tool_server_for_tests("alpha", tools).await;

    let catalog = manager
        .palette_catalog(&crate::gateway::palette::PaletteCaller::admin(
            Some("admin"),
            Some("req-1"),
        ))
        .await
        .expect("catalog builds");

    assert_eq!(catalog.entries.len(), 1);
    let crate::gateway::palette::LauncherEntryView::McpTool(entry) = &catalog.entries[0] else {
        panic!("expected mcp tool entry");
    };
    assert_eq!(entry.id, "mcp:alpha::ping");
    assert_eq!(entry.source, "alpha");
    assert_eq!(entry.tool, "ping");
    assert!(
        entry.input_schema.is_none(),
        "catalog rows must not retain exact schemas"
    );
    assert!(!catalog.truncated);
}

#[tokio::test]
async fn palette_catalog_caps_cross_upstream_projection_but_exact_lookup_remains_available() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    for upstream in ["alpha", "beta"] {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tools = (0..600)
            .map(|index| {
                let name = format!("tool_{index:04}");
                let tool = rmcp::model::Tool::new(
                    name.clone(),
                    "bounded palette fixture",
                    Arc::new(serde_json::Map::new()),
                );
                (
                    name,
                    UpstreamTool {
                        tool,
                        input_schema: Some(json!({"type": "object"})),
                        output_schema: None,
                        upstream_name: Arc::clone(&upstream_name),
                        destructive: false,
                    },
                )
            })
            .collect();
        pool.insert_entry_for_tests(upstream, fixture_upstream_entry(upstream, tools))
            .await;
    }

    let caller = crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1"));
    let catalog = manager
        .palette_catalog_snapshot(&caller)
        .await
        .expect("bounded catalog");
    assert_eq!(catalog.entries.len(), 1_000);
    assert!(catalog.truncated);
    assert!(catalog.entries.iter().all(|entry| match entry {
        crate::gateway::palette::LauncherEntryView::McpTool(entry) => entry.input_schema.is_none(),
        crate::gateway::palette::LauncherEntryView::LabbyAction(_) => false,
    }));

    let searched = manager
        .palette_catalog_snapshot_matching(
            &caller,
            &crate::gateway::palette::PaletteSearchQuery::new("mcp:beta").expect("valid query"),
        )
        .await
        .expect("query is applied before the cross-upstream cap");
    assert_eq!(searched.entries.len(), 600);
    assert!(!searched.truncated);
    assert!(searched.entries.iter().any(|entry| match entry {
        crate::gateway::palette::LauncherEntryView::McpTool(entry) => {
            entry.id == "mcp:beta::tool_0599"
        }
        crate::gateway::palette::LauncherEntryView::LabbyAction(_) => false,
    }));

    let exact = manager
        .palette_catalog_snapshot_for_tool(&caller, "mcp:beta::tool_0599")
        .await
        .expect("exact lookup outside bounded catalog");
    assert_eq!(exact.entries.len(), 1);
    assert!(!exact.truncated);
}

#[tokio::test]
async fn palette_search_filters_before_single_upstream_catalog_cap() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let mut tools = (0..1_100)
        .map(|index| {
            let name = format!("tool_{index:04}");
            let tool = rmcp::model::Tool::new(
                name.clone(),
                "ordinary fixture",
                Arc::new(serde_json::Map::new()),
            );
            (
                name,
                UpstreamTool {
                    tool,
                    input_schema: None,
                    output_schema: None,
                    upstream_name: Arc::clone(&upstream_name),
                    destructive: false,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let name = "zzzz_unique_match".to_string();
    tools.insert(
        name.clone(),
        UpstreamTool {
            tool: rmcp::model::Tool::new(
                name,
                "ordinary fixture",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: None,
            output_schema: None,
            upstream_name,
            destructive: false,
        },
    );
    pool.insert_entry_for_tests("alpha", fixture_upstream_entry("alpha", tools))
        .await;

    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("unique_match").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert_eq!(searched.entries.len(), 1);
    assert!(matches!(
        &searched.entries[0],
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:alpha::zzzz_unique_match"
    ));
}

#[tokio::test]
async fn palette_search_global_cap_keeps_later_exact_match_over_weak_matches() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let weak = (0..1_000)
        .map(|index| {
            let name = format!("weak_{index:04}");
            (
                name.clone(),
                UpstreamTool {
                    tool: rmcp::model::Tool::new(
                        name,
                        "contains needle somewhere",
                        Arc::new(serde_json::Map::new()),
                    ),
                    input_schema: None,
                    output_schema: None,
                    upstream_name: Arc::clone(&upstream_name),
                    destructive: false,
                },
            )
        })
        .collect();
    pool.insert_entry_for_tests("alpha", fixture_upstream_entry("alpha", weak))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "needle"))
        .await;

    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("needle").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert!(searched.entries.iter().any(|entry| matches!(
        entry,
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:beta::needle"
    )));
}

#[tokio::test]
async fn palette_search_reports_truncation_when_global_inspection_budget_is_exhausted() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    for upstream in ["alpha", "beta"] {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tools = (0..6_000)
            .map(|index| {
                let name = format!("tool_{index:04}");
                (
                    name.clone(),
                    UpstreamTool {
                        tool: rmcp::model::Tool::new(
                            name,
                            "ordinary fixture",
                            Arc::new(serde_json::Map::new()),
                        ),
                        input_schema: None,
                        output_schema: None,
                        upstream_name: Arc::clone(&upstream_name),
                        destructive: false,
                    },
                )
            })
            .collect();
        pool.insert_entry_for_tests(upstream, fixture_upstream_entry(upstream, tools))
            .await;
    }
    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("absent").expect("valid query"),
        )
        .await
        .expect("bounded search");
    assert!(searched.entries.is_empty());
    assert!(searched.truncated);
}

#[tokio::test]
async fn palette_search_matches_description_subsequences_before_catalog_cap() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let tool = rmcp::model::Tool::new(
        "otherwise_hidden",
        "Deploy Production Safely",
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([(
                "otherwise_hidden".to_string(),
                UpstreamTool {
                    tool,
                    input_schema: None,
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;

    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("dps").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert_eq!(searched.entries.len(), 1);
}

#[tokio::test]
async fn palette_search_scores_only_the_visible_sanitized_description() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let hidden = rmcp::model::Tool::new(
        "hidden",
        format!("{}needle", "x".repeat(512)),
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([(
                "hidden".to_string(),
                UpstreamTool {
                    tool: hidden,
                    input_schema: None,
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "needle"))
        .await;
    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("needle").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert_eq!(searched.entries.len(), 1);
    assert!(matches!(
        &searched.entries[0],
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:beta::needle"
    ));
}

#[tokio::test]
async fn palette_search_many_delayed_oauth_upstreams_has_one_bounded_deadline() {
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::manager::UpstreamOauthManager;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed OAuth fixture");
    let addr = listener.local_addr().expect("listener address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_for_server = Arc::clone(&accepted);
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            accepted_for_server.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let _socket = socket;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });
    let upstreams = (0..32)
        .map(|index| {
            fixture_oauth_upstream(&format!("oauth_{index:02}"), &format!("http://{addr}/mcp"))
        })
        .collect::<Vec<_>>();
    let dir = tempfile::tempdir().expect("tempdir");
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let managers = Arc::new(dashmap::DashMap::new());
    for upstream in &upstreams {
        managers.insert(
            upstream.name.clone(),
            UpstreamOauthManager::new(
                sqlite.clone(),
                key.clone(),
                upstream.clone(),
                redirect_uri.clone(),
            ),
        );
    }
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new().with_oauth_client_cache(cache.clone()));
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime)
        .with_upstream_oauth_managers(managers)
        .with_oauth_client_cache(cache)
        .with_oauth_resources(sqlite, key, redirect_uri);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: upstreams.clone(),
            ..GatewayConfig::default()
        })
        .await;
    pool.install_test_subject_tools_for_upstream(
        &upstreams[0],
        "admin",
        vec![rmcp::model::Tool::new(
            "needle",
            "fast OAuth match",
            Arc::new(serde_json::Map::new()),
        )],
    )
    .await;
    let started = std::time::Instant::now();
    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("needle").expect("valid query"),
        )
        .await
        .expect("deadline degrades to a partial catalog");
    let elapsed = started.elapsed();
    server.abort();

    assert!(searched.entries.iter().any(|entry| matches!(
        entry,
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:oauth_00::needle"
    )));
    assert!(
        elapsed < Duration::from_secs(3),
        "many delayed upstreams must share one deadline: {elapsed:?}"
    );
    assert!(
        accepted.load(Ordering::SeqCst) < 32,
        "catalog fanout must bound simultaneous delayed connection work"
    );
}

#[tokio::test]
async fn palette_catalog_scoped_caller_only_sees_allowed_upstreams() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "search"))
        .await;

    let caller = crate::gateway::palette::PaletteCaller::scoped_read_only(
        Some("user"),
        Some("request-1"),
        vec!["beta".to_string()],
    );
    let catalog = manager
        .palette_catalog(&caller)
        .await
        .expect("scoped palette catalog should build for allowed upstream");

    let ids = catalog
        .entries
        .iter()
        .map(|entry| match entry {
            crate::gateway::palette::LauncherEntryView::McpTool(entry) => entry.id.as_str(),
            crate::gateway::palette::LauncherEntryView::LabbyAction(entry) => entry.id.as_str(),
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["mcp:beta::search"]);
}

#[tokio::test]
async fn palette_catalog_scope_and_fingerprint_follow_visible_schema() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "pong"))
        .await;

    let admin = crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1"));
    let scoped = crate::gateway::palette::PaletteCaller::scoped_read_only(
        Some("user"),
        Some("req-2"),
        vec!["alpha".to_string()],
    );

    let admin_catalog = manager
        .palette_catalog(&admin)
        .await
        .expect("admin catalog");
    let scoped_catalog = manager
        .palette_catalog(&scoped)
        .await
        .expect("scoped catalog");
    assert_eq!(admin_catalog.entries.len(), 2);
    assert_eq!(scoped_catalog.entries.len(), 1);
    assert_ne!(admin_catalog.fingerprint, scoped_catalog.fingerprint);

    let upstream_name: Arc<str> = Arc::from("alpha");
    let tool = rmcp::model::Tool::new(
        "ping".to_string(),
        "changed schema",
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([(
                "ping".to_string(),
                UpstreamTool {
                    tool,
                    input_schema: Some(json!({"type": "object", "required": ["q"]})),
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;

    let changed = manager
        .palette_catalog(&scoped)
        .await
        .expect("changed catalog");
    assert_ne!(scoped_catalog.fingerprint, changed.fingerprint);
}

fn palette_contract_hash(
    catalog: &crate::gateway::palette::LauncherCatalogView,
    id: &str,
) -> String {
    catalog
        .entries
        .iter()
        .find_map(|entry| match entry {
            crate::gateway::palette::LauncherEntryView::McpTool(entry) if entry.id == id => {
                Some(entry.contract_hash.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing palette entry {id}"))
}

#[tokio::test]
async fn palette_execute_binds_oauth_catalog_and_call_to_the_same_subject() {
    let upstream = fixture_oauth_upstream("private", "http://unused.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    let subject_tool = |property: &str| {
        let properties =
            serde_json::Map::from_iter([(property.to_string(), json!({"type": "string"}))]);
        let mut tool = rmcp::model::Tool::new(
            "private_ping".to_string(),
            "private ping",
            Arc::new(serde_json::Map::from_iter([(
                "properties".to_string(),
                serde_json::Value::Object(properties),
            )])),
        );
        tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
        tool
    };
    pool.install_test_subject_tools_for_upstream(&upstream, "alice", vec![subject_tool("alice")])
        .await;
    pool.install_test_subject_tools_for_upstream(&upstream, "bob", vec![subject_tool("bob")])
        .await;

    let alice = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-123"));
    let bob = crate::gateway::palette::PaletteCaller::admin(Some("bob"), Some("req-bob"));
    let alice_catalog = manager
        .palette_catalog(&alice)
        .await
        .expect("alice catalog");
    let bob_catalog = manager.palette_catalog(&bob).await.expect("bob catalog");
    let id = "mcp:private::private_ping";
    let alice_hash = palette_contract_hash(&alice_catalog, id);
    let bob_hash = palette_contract_hash(&bob_catalog, id);
    assert_ne!(
        alice_hash, bob_hash,
        "subject-specific schemas must not cross"
    );

    let response = manager
        .palette_execute(
            &alice,
            crate::gateway::palette::PaletteExecuteRequest {
                id: id.to_string(),
                params: json!({"token": "TOKEN-CANARY"}),
                confirm_destructive: false,
                expected_contract_hash: alice_hash.clone(),
            },
        )
        .await
        .expect("Alice executes against Alice's subject connection");

    assert_eq!(response.receipt.request_id, "req-123");
    assert_eq!(
        serde_json::to_value(&response.receipt).unwrap()["executionMode"],
        "exact"
    );
    assert_eq!(response.receipt.tool_id, id);
    assert_eq!(response.receipt.contract_hash, alice_hash);
    let receipt = serde_json::to_string(&response.receipt).expect("receipt serializes");
    for forbidden in [
        "alice",
        "TOKEN-CANARY",
        "oauth",
        "params",
        "result",
        "llmInvocations",
        "auditId",
    ] {
        assert!(
            !receipt.contains(forbidden),
            "receipt leaked {forbidden}: {receipt}"
        );
    }
}

#[tokio::test]
async fn palette_execute_does_not_reuse_an_invalidated_oauth_subject_connection() {
    let upstream = fixture_oauth_upstream("private", "http://127.0.0.1:9/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    let mut tool = rmcp::model::Tool::new(
        "private_ping".to_string(),
        "private ping",
        Arc::new(serde_json::Map::new()),
    );
    tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
    pool.install_test_subject_tools_for_upstream(&upstream, "alice", vec![tool])
        .await;
    let alice = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-revoked"));
    let catalog = manager
        .palette_catalog(&alice)
        .await
        .expect("catalog before revocation");
    let contract_hash = palette_contract_hash(&catalog, "mcp:private::private_ping");

    pool.invalidate_oauth_subject_sessions("private", "alice", "credential revoked")
        .await;
    let error = manager
        .palette_execute(
            &alice,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:private::private_ping".to_string(),
                params: json!({}),
                expected_contract_hash: contract_hash,
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("revoked subject connection must not be reused");
    assert!(
        matches!(
            error.kind(),
            "upstream_connect_error" | "network_error" | "auth_failed"
        ),
        "unexpected revocation error: {error:?}"
    );
}

#[tokio::test]
async fn palette_execute_rechecks_the_published_config_after_catalog_preview() {
    let mut upstream = fixture_http_upstream("alpha");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    let caller = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-reload"));
    let catalog = manager
        .palette_catalog(&caller)
        .await
        .expect("catalog before reload");
    let contract_hash = palette_contract_hash(&catalog, "mcp:alpha::ping");

    upstream.priority = 0.0;
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![upstream],
            ..GatewayConfig::default()
        })
        .await;
    let error = manager
        .palette_execute(
            &caller,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::ping".to_string(),
                params: json!({}),
                expected_contract_hash: contract_hash,
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("disabled published revision must win over the preview");
    assert_eq!(error.kind(), "not_found");
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "Serialize tracing capture across awaits on this current-thread test runtime"
)]
async fn palette_execute_fails_closed_when_the_previewed_contract_changes() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("github")]).await;
    pool.insert_entry_for_tests("github", healthy_entry_with_tool("github", "search_issues"))
        .await;
    let alice = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-drift"));
    let catalog = manager
        .palette_catalog(&alice)
        .await
        .expect("preview catalog");
    let id = "mcp:github::search_issues";
    let old_hash = palette_contract_hash(&catalog, id);

    let mut changed = pool.healthy_tools_for_upstream("github").await;
    changed[0].input_schema = Some(json!({
        "type": "object",
        "properties": {"query": {"type": "string"}}
    }));
    pool.insert_entry_for_tests(
        "github",
        fixture_upstream_entry(
            "github",
            HashMap::from([("search_issues".to_string(), changed.remove(0))]),
        ),
    )
    .await;

    let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let buffer = crate::test_support::SharedBuf::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .without_time(),
    );
    let tracing_guard = tracing::subscriber::set_default(subscriber);
    let error = manager
        .palette_execute(
            &alice,
            crate::gateway::palette::PaletteExecuteRequest {
                id: id.to_string(),
                params: json!({"query": "bug"}),
                expected_contract_hash: old_hash,
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("changed contract must fail closed");
    assert_eq!(error.kind(), "contract_changed");
    drop(tracing_guard);
    let logs = crate::test_support::captured_logs(&buffer);
    assert!(
        !logs.contains("upstream.request"),
        "contract drift dispatched an upstream request: {logs}"
    );
}

#[tokio::test]
async fn palette_execute_rejects_cross_upstream_scope_and_destructive_reclassification() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "safe"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "other"))
        .await;
    let caller = crate::gateway::palette::PaletteCaller {
        caller: CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: false,
                is_admin: false,
            },
            sub: Some("alice".to_string()),
        },
        caller_auth: labby_runtime::caller_auth::PropagatedCallerAuth {
            sub: Some("alice".to_string()),
            scopes: vec![
                "mcp:read".to_string(),
                "mcp:write".to_string(),
                "gateway:alpha".to_string(),
            ],
            trusted_local: false,
            access_principal_id: None,
            private_context_token: None,
        },
        scope: ToolScope::scoped_namespaces(vec!["alpha".to_string()], Vec::new()),
        owner: crate::gateway::shared::make_api_runtime_owner(Some("alice"), Some("req-scope")),
        oauth_subject: "alice".to_string(),
    };

    let error = manager
        .palette_execute(
            &caller,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:beta::other".to_string(),
                params: json!({}),
                expected_contract_hash: "a".repeat(64),
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("cross-upstream call denied");
    assert_eq!(error.kind(), "forbidden");

    let catalog = manager
        .palette_catalog(&caller)
        .await
        .expect("scoped catalog");
    let old_hash = palette_contract_hash(&catalog, "mcp:alpha::safe");
    let mut reclassified = pool.healthy_tools_for_upstream("alpha").await;
    reclassified[0].destructive = true;
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([("safe".to_string(), reclassified.remove(0))]),
        ),
    )
    .await;
    let error = manager
        .palette_execute(
            &caller,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::safe".to_string(),
                params: json!({}),
                expected_contract_hash: old_hash,
                confirm_destructive: true,
            },
        )
        .await
        .expect_err("destructive reclassification is contract drift");
    assert_eq!(error.kind(), "contract_changed");
}

#[tokio::test]
async fn palette_execute_rejects_unknown_hidden_destructive_and_read_only_calls() {
    let mut suppressed = fixture_http_upstream("suppressed");
    suppressed.priority = 0.0;
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha"), suppressed]).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "delete"))
        .await;
    pool.insert_entry_for_tests(
        "suppressed",
        healthy_entry_with_tool("suppressed", "secret"),
    )
    .await;

    let mut destructive = pool.healthy_tools_for_upstream("alpha").await;
    destructive[0].destructive = true;
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([("delete".to_string(), destructive.remove(0))]),
        ),
    )
    .await;

    let admin = crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1"));
    let read_only = crate::gateway::palette::PaletteCaller::scoped_read_only(
        Some("user"),
        Some("req-2"),
        vec!["alpha".to_string()],
    );
    let catalog = manager
        .palette_catalog(&admin)
        .await
        .expect("admin catalog");
    let destructive_hash = palette_contract_hash(&catalog, "mcp:alpha::delete");

    let err = manager
        .palette_execute(
            &admin,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:missing::tool".to_string(),
                params: json!({}),
                confirm_destructive: false,
                expected_contract_hash: "a".repeat(64),
            },
        )
        .await
        .expect_err("unknown id rejected");
    assert_eq!(err.kind(), "not_found");

    let err = manager
        .palette_execute(
            &admin,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:suppressed::secret".to_string(),
                params: json!({}),
                confirm_destructive: false,
                expected_contract_hash: "a".repeat(64),
            },
        )
        .await
        .expect_err("priority zero hidden");
    assert_eq!(err.kind(), "not_found");

    let err = manager
        .palette_execute(
            &read_only,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::delete".to_string(),
                params: json!({}),
                confirm_destructive: true,
                expected_contract_hash: destructive_hash.clone(),
            },
        )
        .await
        .expect_err("read-only rejected");
    assert_eq!(err.kind(), "forbidden");

    let err = manager
        .palette_execute(
            &admin,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::delete".to_string(),
                params: json!({}),
                confirm_destructive: false,
                expected_contract_hash: destructive_hash,
            },
        )
        .await
        .expect_err("destructive confirmation required");
    assert_eq!(err.kind(), "confirmation_required");
}

// ── Semantic search (fail-open embedding blend) ──────────────────────────────

#[tokio::test]
async fn semantic_rank_returns_empty_when_unconfigured() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let result = manager
        .semantic_rank(
            "hello".to_string(),
            5,
            &CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
        )
        .await
        .unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn semantic_search_cooldown_blocks_immediate_retry_after_failure() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    manager.record_semantic_search_failure("test failure").await;
    assert!(!manager.semantic_search_available().await);
}

#[tokio::test]
async fn semantic_search_recovery_clears_cooldown() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    manager.record_semantic_search_failure("test failure").await;
    assert!(!manager.semantic_search_available().await);
    manager.record_semantic_search_recovery().await;
    assert!(manager.semantic_search_available().await);
}

#[tokio::test]
async fn ensure_embeddings_for_fingerprint_is_noop_when_unconfigured() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let entries = Vec::new(); // empty catalog — also exercises the cold-start-empty-catalog path
    let result = manager
        .ensure_embeddings_for_fingerprint("some-fingerprint", &entries)
        .await;
    assert!(result.is_empty());
    assert!(
        manager
            .cached_embeddings("some-fingerprint")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn catalog_embeddings_stay_cold_when_semantic_search_unconfigured() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    // Default config has semantic_search.tei_url = None.
    let render = manager
        .list_tools(
            &CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            false,
            false,
        )
        .await
        .unwrap();
    // The embedding cache must remain empty — ensure_embeddings_for_fingerprint
    // returns immediately for an unconfigured host.
    assert!(
        manager
            .cached_embeddings(&render.embedding_fingerprint)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn semantic_rank_never_returns_ids_outside_scope_filtered_catalog() {
    // semantic_rank's own internal build_tools_render call uses the SAME
    // `scope` parameter it was given, and its ranking set is additionally
    // filtered with the same `kind == Snippet || scope.allows(...)` test the
    // sandbox's own discovery catalog uses — so an id excluded by that scope
    // is structurally never present in the vectors handed to
    // `rank_by_similarity` in the first place.
    //
    // This unit test exercises the unconfigured (no TEI) path, which
    // already proves semantic_rank cannot fabricate ids independent of
    // build_tools_render's scope-filtered output regardless of scope — a
    // live, multi-upstream, TEI-backed confirmation of the same invariant
    // is covered by the plan's manual smoke test (Task 7 Step 6).
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let restrictive_scope = ToolScope::scoped_namespaces(vec![], vec![]);
    let result = manager
        .semantic_rank(
            "anything".to_string(),
            5,
            &CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &restrictive_scope,
        )
        .await
        .unwrap();
    assert!(result.is_empty());
}
#[tokio::test]
async fn ensure_embeddings_unreachable_tei_fails_open_and_records_cooldown() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let mut cfg = manager.code_mode_config().await;
    cfg.semantic_search.tei_url = Some("http://127.0.0.1:1".to_string());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: cfg,
            ..GatewayConfig::default()
        })
        .await;
    let entries = vec![labby_codemode::ToolDescriptor::tool(
        "alpha",
        "ping",
        "Ping the alpha upstream",
        None,
        None,
    )];
    assert!(manager.semantic_search_available().await);
    let result = manager
        .ensure_embeddings_for_fingerprint("fp-test", &entries)
        .await;
    assert!(result.is_empty(), "fail-open returns empty vectors");
    assert!(
        !manager.semantic_search_available().await,
        "failure must start the cooldown"
    );
}
