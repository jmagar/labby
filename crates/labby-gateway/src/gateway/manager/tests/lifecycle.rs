//! Reload / pool-lifecycle tests: lazy seeding, catalog diffing, runtime
//! handle swaps, and virtual-server quarantine migration.

use std::collections::BTreeSet;

use crate::gateway::config::{load_gateway_config, write_gateway_config};
use crate::gateway::manager::pool_lifecycle::quarantine_unregistered_virtual_servers;
use crate::gateway::manager::{GatewayCatalogSnapshot, diff_catalogs};
use labby_runtime::gateway_config::{VirtualServerConfig, VirtualServerSurfacesConfig};

use super::*;

#[tokio::test]
async fn reload_seeds_lazy_upstreams_without_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
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
        &GatewayConfig {
            upstream_request_timeout_ms: Some(60_000),
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
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
        tools: std::iter::once("fixture-http-echo".to_string()).collect(),
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

// Re-fixtured post-gateway-pivot: `deploy` is a kept/registered service and must
// survive reload; `mcpregistry` is unregistered and must be quarantined.
#[tokio::test]
async fn reload_quarantines_virtual_servers_for_unregistered_services() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    write_gateway_config(
        &path,
        &GatewayConfig {
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
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default())
        .with_builtin_service_registry(deploy_known_registry());
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
        &GatewayConfig {
            virtual_servers: vec![stale.clone()],
            quarantined_virtual_servers: vec![stale],
            ..GatewayConfig::default()
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

    // The default-registry builder lives in `lab`; this test only exercises the
    // already-quarantined branch (no active virtual servers), so an empty registry
    // is sufficient — nothing is looked up.
    let registry = crate::gateway::service_registry::EmptyServiceRegistry;
    let (migrated, migration) = quarantine_unregistered_virtual_servers(
        GatewayConfig {
            quarantined_virtual_servers: vec![stale],
            ..GatewayConfig::default()
        },
        &registry,
    );

    assert!(!migration.changed());
    assert!(migrated.virtual_servers.is_empty());
    assert_eq!(migrated.quarantined_virtual_servers.len(), 1);
}

// T7 — reload availability: unaffected upstream catalog entries and their live
// pool survive a single-upstream config change.
//
// The reconciliation property: after a reload that only adds one new upstream,
// the catalog entries for unchanged upstreams remain in the same live pool
// instead of forcing a full swap-and-drain.
//
// Why this captures the intent: `pool_lifecycle.rs` now evicts only changed
// upstream names, then lazy-seeds the updated config into the existing pool.
// If reconciliation regresses to dropping unchanged entries or rebuilding the
// whole pool, this test will fail.
#[tokio::test]
async fn reload_unaffected_upstream_catalog_entry_survives_single_upstream_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    // Start with two upstreams.
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
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
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
                fixture_http_upstream("charlie"),
            ],
            ..GatewayConfig::default()
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
    // preserved pool even though only charlie was added.
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

    assert!(
        Arc::ptr_eq(&pool_after_first, &pool_after_second),
        "single-upstream changes must selectively reconcile without swapping the pool"
    );
}

#[tokio::test]
async fn reload_changed_upstream_rebuilds_pool_instead_of_reusing_stale_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");
    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    let mut changed_alpha = fixture_http_upstream("alpha");
    changed_alpha.url = Some("http://127.0.0.1:9100".to_string());
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![changed_alpha, fixture_http_upstream("bravo")],
            ..GatewayConfig::default()
        },
    )
    .expect("write updated config");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("second reload");
    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after second reload");

    assert!(
        !Arc::ptr_eq(&pool_before, &pool_after),
        "modified upstreams must rebuild the pool to avoid stale runtime state"
    );
}

#[tokio::test]
async fn reload_removed_upstream_rebuilds_pool_instead_of_reusing_stale_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write initial config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");
    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![fixture_http_upstream("bravo")],
            ..GatewayConfig::default()
        },
    )
    .expect("write updated config");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("second reload");
    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after second reload");

    assert!(
        !Arc::ptr_eq(&pool_before, &pool_after),
        "removed upstreams must rebuild the pool to avoid stale runtime state"
    );
}

// Perf C1 regression: a true no-op reload (the on-disk config is byte-identical
// to the live in-memory config) MUST preserve the live `Arc<UpstreamPool>`. The
// fingerprint-gated short-circuit in `pool_lifecycle.rs`
// (`upstream_runtime_fingerprint` + the `pool_inputs_unchanged` branch that logs
// `pool_rebuild_skipped=true`) is what keeps lazily-spawned stdio children alive
// across unrelated reloads. Without this, every reload would tear down and
// rebuild the pool, forcing a re-handshake on next use.
//
// This is the no-op counterpart to
// `reload_unaffected_upstream_catalog_entry_survives_single_upstream_change`.
// We have no log-capture infra wired in this tree, so the `Arc::ptr_eq`
// identity assertion is the core contract: same Arc means the rebuild was
// skipped.
#[tokio::test]
async fn reload_noop_preserves_live_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    // Reload again WITHOUT changing the config on disk — the upstream set, gateway
    // spawn prefs, and code-mode settings are byte-identical, so the fingerprint
    // matches and the live pool must be preserved.
    manager
        .reload_with_origin(None, None)
        .await
        .expect("no-op reload");

    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after no-op reload");

    assert!(
        Arc::ptr_eq(&pool_before, &pool_after),
        "a no-op reload must preserve the SAME live pool (fingerprint unchanged ⇒ \
         pool_rebuild_skipped); a rebuilt pool here re-regresses Perf C1"
    );
}

// Perf C1 regression: a reload that changes ONLY fields the fingerprint
// deliberately EXCLUDES (here: `protected_mcp_routes`) must also preserve the live
// pool. `upstream_runtime_fingerprint` hashes only the upstream set, gateway spawn
// prefs, code-mode config, and request timeout — protected routes, virtual servers,
// tombstones, and public URLs are reconciled separately and must NOT force a pool
// rebuild. If a future edit folds protected routes into the fingerprint, this test
// fails (correctly flagging the C1 regression).
#[tokio::test]
async fn reload_protected_routes_only_change_preserves_live_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");

    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            ..GatewayConfig::default()
        },
    )
    .expect("write config");

    let manager = GatewayManager::new(path.clone(), GatewayRuntimeHandle::default());
    manager
        .reload_with_origin(None, None)
        .await
        .expect("initial reload");

    let pool_before = manager
        .current_pool()
        .await
        .expect("pool after first reload");

    // Rewrite the config adding ONLY a protected MCP route — every upstream and all
    // pool-shaping config is untouched, so the runtime fingerprint is unchanged.
    write_gateway_config(
        &path,
        &GatewayConfig {
            upstream: vec![
                fixture_http_upstream("alpha"),
                fixture_http_upstream("bravo"),
            ],
            protected_mcp_routes: vec![fixture_protected_route("syslog")],
            ..GatewayConfig::default()
        },
    )
    .expect("write config with protected route");

    manager
        .reload_with_origin(None, None)
        .await
        .expect("protected-routes-only reload");

    let pool_after = manager
        .current_pool()
        .await
        .expect("pool after protected-routes reload");

    assert!(
        Arc::ptr_eq(&pool_before, &pool_after),
        "a reload that changes only fingerprint-excluded fields (protected routes) \
         must preserve the SAME live pool; a rebuilt pool here re-regresses Perf C1"
    );

    // The protected-routes reconciliation still happened — the in-memory config now
    // carries the new route even though the pool was preserved.
    let cfg = manager.current_config().await;
    assert_eq!(
        cfg.protected_mcp_routes.len(),
        1,
        "protected route must be applied even on the pool-preserving path"
    );
}
