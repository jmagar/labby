//! Observability-source pinning + cached inspection-surface tests (lab-tad5).

use super::*;

#[test]
fn observability_source_covers_gateway_manager_reconcile_events() {
    // The gateway reconcile events were split across manager/ sub-modules:
    // add/remove live in config_ops.rs, reload in pool_lifecycle.rs.
    let source = [
        include_str!("../config_ops.rs"),
        include_str!("../pool_lifecycle.rs"),
    ]
    .concat();
    for expected in [
        "event = \"install.start\"",
        "event = \"remove.finish\"",
        "event = \"catalog.refresh.finish\"",
        "before_tool_count",
        "after_tool_count",
        "event = \"old_pool.drain.start\"",
        "event = \"pool.seed.start\"",
        "operation = \"lazy_runtime_seed\"",
    ] {
        assert!(
            source.contains(expected),
            "missing gateway manager observability field `{expected}`"
        );
    }
}

// --- lab-tad5: gateway inspection churn reduction regression tests ---

/// `discovered_resources` must not issue live RPC calls; it must serve from
/// the cached resource URI snapshot populated during the last pool connection.
/// With the lab-mzm2 fix, calling it repeatedly on an empty pool must not
/// trigger any live fan-out.
#[tokio::test]
async fn gateway_discovered_resources_serves_from_cache_not_live_rpc() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    // With no pool or no cached URIs, results must be empty — not an error.
    let result = manager.discovered_resources("nonexistent-upstream").await;
    assert!(
        result.is_ok(),
        "discovered_resources must not error on empty pool"
    );
    assert!(
        result.unwrap().is_empty(),
        "discovered_resources must return empty vec when no pool is present"
    );
}

/// `discovered_prompts` must not issue live RPC calls; it must serve from
/// the cached prompt name snapshot.
#[tokio::test]
async fn gateway_discovered_prompts_serves_from_cache_not_live_rpc() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let result = manager.discovered_prompts("nonexistent-upstream").await;
    assert!(
        result.is_ok(),
        "discovered_prompts must not error on empty pool"
    );
    assert!(
        result.unwrap().is_empty(),
        "discovered_prompts must return empty vec when no pool is present"
    );
}

/// `discovered_tools` must serve from the cached tool exposure rows — this
/// already worked pre-fix, but we pin it here alongside the resource/prompt
/// tests so the full inspection surface is regression-covered (lab-tad5).
#[tokio::test]
async fn gateway_discovered_tools_serves_from_cache() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let result = manager.discovered_tools("nonexistent-upstream").await;
    assert!(
        result.is_ok(),
        "discovered_tools must not error on empty pool"
    );
    assert!(
        result.unwrap().is_empty(),
        "discovered_tools must return empty vec when no pool is present"
    );
}
