//! Reload / pool-lifecycle tests: lazy seeding, catalog diffing, runtime
//! handle swaps, and virtual-server quarantine migration.

use std::collections::BTreeSet;

use crate::config::{VirtualServerConfig, VirtualServerSurfacesConfig};
use crate::dispatch::gateway::config::{load_gateway_config, write_gateway_config};
use crate::dispatch::gateway::manager::pool_lifecycle::quarantine_unregistered_virtual_servers;
use crate::dispatch::gateway::manager::{GatewayCatalogSnapshot, diff_catalogs};

use super::*;

#[tokio::test]
async fn reload_seeds_lazy_upstreams_without_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &LabConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..LabConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let pool = manager.current_pool().await.expect("pool installed");
    assert!(pool.cached_upstream_summary("alpha").await.is_some());
    assert_eq!(pool.connection_count_for_tests().await, 0);
    assert!(pool.healthy_tools_for_upstream("alpha").await.is_empty());
}

#[tokio::test]
async fn reload_applies_configured_upstream_request_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &LabConfig {
            upstream_request_timeout_ms: Some(60_000),
            upstream: vec![fixture_http_upstream("alpha")],
            ..LabConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let pool = manager.current_pool().await.expect("pool installed");
    assert_eq!(
        pool.request_timeout(),
        std::time::Duration::from_millis(60_000)
    );
}

#[tokio::test]
async fn gateway_test_does_not_schedule_background_reprobes() {
    UpstreamPool::reset_probe_task_schedule_count_for_tests("ephemeral-stdio");
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let upstream = fixture_stdio_upstream("ephemeral-stdio");

    let _runtime = manager
        .test(Ok::<&UpstreamConfig, &str>(&upstream))
        .await
        .expect("gateway test returns a runtime view");

    assert_eq!(
        UpstreamPool::probe_task_schedule_count_for_tests("ephemeral-stdio"),
        0
    );
}

#[test]
fn catalog_diff_detects_removed_tool_provider() {
    let before = GatewayCatalogSnapshot {
        tools: ["fixture-http-echo".to_string()].into_iter().collect(),
        resources: BTreeSet::new(),
        prompts: BTreeSet::new(),
    };
    let after = GatewayCatalogSnapshot::default();

    let diff = diff_catalogs(&before, &after);
    assert!(diff.tools_changed);
    assert!(!diff.resources_changed);
    assert!(!diff.prompts_changed);
}

#[tokio::test]
async fn runtime_handle_starts_without_pool() {
    let handle = GatewayRuntimeHandle::default();
    assert!(handle.current_pool().await.is_none());
}

#[tokio::test]
async fn runtime_handle_swaps_pool_atomically() {
    let handle = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());

    handle.swap(Some(Arc::clone(&pool))).await;

    let current = handle.current_pool().await.expect("pool present");
    assert!(Arc::ptr_eq(&current, &pool));
}

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn reload_quarantines_virtual_servers_for_unregistered_services() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &LabConfig {
            virtual_servers: vec![
                VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                },
                VirtualServerConfig {
                    id: "stale-registry".to_string(),
                    service: "mcpregistry".to_string(),
                    enabled: true,
                    surfaces: VirtualServerSurfacesConfig {
                        mcp: true,
                        ..VirtualServerSurfacesConfig::default()
                    },
                    mcp_policy: None,
                },
            ],
            ..LabConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let listed = manager.list().await.expect("list");
    assert!(listed.iter().any(|server| server.id == "deploy"));
    assert!(!listed.iter().any(|server| server.id == "stale-registry"));

    let migrated = load_gateway_config(&path).expect("load migrated config");
    assert_eq!(migrated.virtual_servers.len(), 1);
    assert_eq!(migrated.virtual_servers[0].id, "deploy");
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
    assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-registry");
}

#[tokio::test]
async fn reload_does_not_duplicate_existing_quarantined_virtual_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let stale = VirtualServerConfig {
        id: "stale-registry".to_string(),
        service: "mcpregistry".to_string(),
        enabled: true,
        surfaces: VirtualServerSurfacesConfig {
            mcp: true,
            ..VirtualServerSurfacesConfig::default()
        },
        mcp_policy: None,
    };
    write_gateway_config(
        &path,
        &LabConfig {
            virtual_servers: vec![stale.clone()],
            quarantined_virtual_servers: vec![stale],
            ..LabConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("reload");

    let migrated = load_gateway_config(&path).expect("load migrated config");
    assert!(migrated.virtual_servers.is_empty());
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
    assert_eq!(migrated.quarantined_virtual_servers[0].id, "stale-registry");
}

#[test]
fn quarantine_migration_is_noop_when_only_existing_quarantine_remains() {
    let stale = VirtualServerConfig {
        id: "stale-registry".to_string(),
        service: "mcpregistry".to_string(),
        enabled: true,
        surfaces: VirtualServerSurfacesConfig::default(),
        mcp_policy: None,
    };

    let registry = crate::registry::build_default_registry();
    let (migrated, migration) = quarantine_unregistered_virtual_servers(
        LabConfig {
            quarantined_virtual_servers: vec![stale],
            ..LabConfig::default()
        },
        &registry,
    );

    assert!(!migration.changed());
    assert!(migrated.virtual_servers.is_empty());
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
}

// T7 — reload availability: unaffected upstream catalog entry survives a
// single-upstream config change.
//
// The reconciliation property: after a reload that only adds one new upstream,
// the catalog entry for the *unchanged* upstream is still present in the fresh
// pool.  We assert connection-object identity (by checking both pool snapshots
// contain the same upstream name) rather than running concurrent calls, which
// would require live MCP servers and be inherently flaky in CI.
//
// Why this captures the intent: `pool_lifecycle.rs` seeds ALL upstreams lazily
// into the new pool.  If the reconciliation regresses to a selective rebuild
// that drops entries for unchanged upstreams, this test will fail.
#[tokio::test]
async fn reload_unaffected_upstream_catalog_entry_survives_single_upstream_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    // Start with two upstreams.
    write_gateway_config(
        &path,
        &LabConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..LabConfig::default()
        },
    )
    .expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    // Capture the pool pointer after the first reload.
    let pool_after_first = manager
        .current_pool()
        .await
        .expect("pool after first reload");
    assert!(
        pool_after_first
            .cached_upstream_summary("alpha")
            .await
            .is_some(),
        "alpha must be seeded after initial reload"
    );
    assert!(
        pool_after_first
            .cached_upstream_summary("bravo")
            .await
            .is_some(),
        "bravo must be seeded after initial reload"
    );

    // Write a new config that adds a third upstream (charlie) — alpha and bravo
    // are unchanged.
    write_gateway_config(
        &path,
        &LabConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
                fixture_http_upstream("charlie"),
            ],
            ..LabConfig::default()
        },
    )
    .expect("write updated config");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("second reload");

    let pool_after_second = manager
        .current_pool()
        .await
        .expect("pool after second reload");

    // The reconciliation property: alpha and bravo are still present in the
    // freshly built pool even though only charlie was added.
    assert!(
        pool_after_second
            .cached_upstream_summary("alpha")
            .await
            .is_some(),
        "alpha catalog entry must survive reload of unaffected upstream"
    );
    assert!(
        pool_after_second
            .cached_upstream_summary("bravo")
            .await
            .is_some(),
        "bravo catalog entry must survive reload of unaffected upstream"
    );
    assert!(
        pool_after_second
            .cached_upstream_summary("charlie")
            .await
            .is_some(),
        "charlie must be seeded in the new pool"
    );

    // The new pool is a fresh Arc — not the same object.  Confirm the swap
    // happened so we know we tested the post-reload pool, not a stale one.
    assert!(
        !Arc::ptr_eq(&pool_after_first, &pool_after_second),
        "reload must swap the pool (new Arc expected)"
    );
}
