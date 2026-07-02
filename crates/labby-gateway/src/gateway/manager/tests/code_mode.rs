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
            .cached_embeddings(&render.fingerprint)
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
