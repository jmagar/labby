//! Projection / view tests: redaction, runtime views, protected-route index.

use crate::gateway::projection::{runtime_view, server_view_from_upstream};

use super::*;

#[tokio::test]
async fn protected_route_add_updates_live_resolver_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .protected_route_add(fixture_protected_route("syslog"))
        .await
        .expect("add protected route");

    assert_eq!(
        manager
            .resolve_protected_route("mcp.tootie.tv", "/syslog")
            .await
            .expect("route should be live")
            .name,
        "syslog"
    );
    assert_eq!(
        manager
            .resolve_protected_route_metadata(
                "mcp.tootie.tv",
                "/.well-known/oauth-protected-resource/syslog",
            )
            .await
            .expect("metadata route should be live")
            .name,
        "syslog"
    );
}

#[tokio::test]
async fn manager_get_preserves_bearer_token_env_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
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

    let gateway = manager.get("fixture-http").await.expect("gateway");
    assert_eq!(
        gateway.config.bearer_token_env.as_deref(),
        Some("FIXTURE_HTTP_TOKEN")
    );
}

#[tokio::test]
async fn manager_get_redacts_sensitive_stdio_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-stdio".to_string(),
            url: None,
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: Some("env".to_string()),
            args: vec![
                "OPENAI_API_KEY=super-secret".to_string(),
                "npx".to_string(),
                "--access_token=abc123".to_string(),
                "--api-key=super-secret".to_string(),
            ],
            env: BTreeMap::new(),
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

    let gateway = manager.get("fixture-stdio").await.expect("gateway");
    assert_eq!(gateway.config.command.as_deref(), Some("env"));
    assert_eq!(
        gateway.config.args,
        vec![
            "OPENAI_API_KEY=[redacted]".to_string(),
            "npx".to_string(),
            "--access_token=[redacted]".to_string(),
            "--api-key=[redacted]".to_string(),
        ]
    );
}

#[tokio::test]
async fn server_view_redacts_sensitive_target_url_components() {
    let upstream = UpstreamConfig {
        enabled: true,
        name: "fixture-http".to_string(),
        url: Some("http://user:pass@127.0.0.1:9001/callback?token=secret&mode=1".to_string()),
        bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };

    let view = server_view_from_upstream(None, &upstream).await;

    assert_eq!(
        view.config_summary.target.as_deref(),
        Some("http://127.0.0.1:9001/callback?token=[redacted]&mode=1")
    );
}

#[tokio::test]
async fn server_view_redacts_invalid_target_urls() {
    let upstream = UpstreamConfig {
        enabled: true,
        name: "fixture-http".to_string(),
        url: Some("http://user:pass@[::1".to_string()),
        bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };

    let view = server_view_from_upstream(None, &upstream).await;

    assert_eq!(
        view.config_summary.target.as_deref(),
        Some("[invalid-url-redacted]")
    );
}

#[tokio::test]
async fn server_view_redacts_stdio_env_targets() {
    let upstream = UpstreamConfig {
        enabled: true,
        name: "fixture-stdio".to_string(),
        url: None,
        bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
        command: Some("env".to_string()),
        args: vec![
            "OPENAI_API_KEY=super-secret".to_string(),
            "npx".to_string(),
            "--access_token=abc123".to_string(),
        ],
        env: BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    };

    let view = server_view_from_upstream(None, &upstream).await;

    assert_eq!(view.config_summary.target.as_deref(), Some("env"));
}

#[tokio::test]
async fn runtime_view_includes_last_upstream_error() {
    let pool = UpstreamPool::new();
    let now = std::time::Instant::now();
    let mut entry = fixture_upstream_entry("broken-upstream", HashMap::new());
    entry.tool_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.prompt_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.resource_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.tool_unhealthy_since = Some(now);
    entry.prompt_unhealthy_since = Some(now);
    entry.resource_unhealthy_since = Some(now);
    entry.tool_last_error = Some("stdio handshake failed".to_string());

    pool.insert_entry_for_tests("broken-upstream", entry).await;

    let runtime = runtime_view(Some(&pool), "broken-upstream", None).await;
    assert_eq!(
        runtime.last_error.as_deref(),
        Some("stdio handshake failed")
    );
}

#[tokio::test]
async fn runtime_view_preserves_non_benign_prompt_and_resource_errors() {
    let pool = UpstreamPool::new();
    let now = std::time::Instant::now();
    let mut entry = fixture_upstream_entry("partial-upstream", HashMap::new());
    entry.prompt_count = 3;
    entry.resource_count = 2;
    entry.prompt_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.resource_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.prompt_unhealthy_since = Some(now);
    entry.resource_unhealthy_since = Some(now);
    entry.prompt_last_error = Some("prompt listing unsupported".to_string());
    entry.resource_last_error = Some("resource listing unsupported".to_string());

    pool.insert_entry_for_tests("partial-upstream", entry).await;

    let runtime = runtime_view(Some(&pool), "partial-upstream", None).await;
    assert_eq!(
        runtime.last_error.as_deref(),
        Some("resource listing unsupported")
    );

    let mut upstream = fixture_http_upstream("partial-upstream");
    upstream.proxy_resources = true;
    let server = server_view_from_upstream(Some(&pool), &upstream).await;

    assert_eq!(server.warnings.len(), 1);
    assert_eq!(server.warnings[0].message, "resource listing unsupported");
}

#[tokio::test]
async fn runtime_view_ignores_method_not_found_capability_errors() {
    let pool = UpstreamPool::new();
    let now = std::time::Instant::now();
    let mut entry = fixture_upstream_entry("partial-upstream", HashMap::new());
    entry.prompt_count = 1;
    entry.resource_count = 1;
    entry.prompt_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.resource_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.prompt_unhealthy_since = Some(now);
    entry.resource_unhealthy_since = Some(now);
    entry.prompt_last_error = Some(
        "failed to list prompts from upstream: Mcp error: -32601: Method not found".to_string(),
    );
    entry.resource_last_error = Some(
        "failed to list resources from upstream: Mcp error: -32601: Method not found".to_string(),
    );

    pool.insert_entry_for_tests("partial-upstream", entry).await;

    let runtime = runtime_view(Some(&pool), "partial-upstream", None).await;
    assert_eq!(runtime.last_error, None);

    let mut upstream = fixture_http_upstream("partial-upstream");
    upstream.proxy_resources = true;
    let server = server_view_from_upstream(Some(&pool), &upstream).await;

    assert!(server.warnings.is_empty());
}

#[tokio::test]
async fn custom_gateway_connected_includes_resources_and_prompts() {
    let pool = UpstreamPool::new();
    let mut upstream = fixture_http_upstream("partial-upstream");
    upstream.url = Some("http://127.0.0.1:9001/mcp".to_string());
    upstream.proxy_resources = true;
    let mut entry = fixture_upstream_entry("partial-upstream", HashMap::new());
    entry.prompt_count = 4;
    entry.resource_count = 2;

    pool.insert_entry_for_tests("partial-upstream", entry).await;

    let view = server_view_from_upstream(Some(&pool), &upstream).await;
    assert!(view.connected);
    assert!(view.warnings.is_empty());
    assert_eq!(view.exposed_resource_count, 2);
    assert_eq!(view.exposed_prompt_count, 4);
}

#[tokio::test]
async fn lazily_seeded_healthy_upstream_reports_connected_before_first_use() {
    // Regression: with lazy discovery the catalog is empty (0 tools) until an
    // upstream's first use. A seeded-but-healthy upstream must not render as
    // "Disconnected" just because no tools are exposed yet.
    let pool = UpstreamPool::new();
    let upstream = fixture_http_upstream("lazy-upstream");
    pool.seed_lazy_upstreams(std::slice::from_ref(&upstream))
        .await;

    let view = server_view_from_upstream(Some(&pool), &upstream).await;
    assert!(
        view.connected,
        "seeded healthy upstream should be connected"
    );
    assert!(view.surfaces.mcp.connected);
    assert_eq!(view.discovered_tool_count, 0);
    assert!(view.warnings.is_empty());
}

#[tokio::test]
async fn errored_upstream_reports_disconnected_even_when_circuit_closed() {
    // An upstream with a recorded operator-visible error must surface as down
    // regardless of the optimistic seeded health default.
    let pool = UpstreamPool::new();
    let mut entry = fixture_upstream_entry("broken-upstream", HashMap::new());
    entry.tool_health = UpstreamHealth::Unhealthy {
        consecutive_failures: 1,
    };
    entry.tool_last_error = Some("auth required: 401 Unauthorized".to_string());
    pool.insert_entry_for_tests("broken-upstream", entry).await;

    let upstream = fixture_http_upstream("broken-upstream");
    let view = server_view_from_upstream(Some(&pool), &upstream).await;
    assert!(!view.connected, "errored upstream should be disconnected");
    assert!(!view.surfaces.mcp.connected);
    assert_eq!(
        view.warnings.first().map(|warning| warning.code.as_str()),
        Some("auth_failed")
    );
}
