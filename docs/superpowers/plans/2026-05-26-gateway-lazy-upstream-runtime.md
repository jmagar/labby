# Gateway Lazy Upstream Runtime Implementation Plan

> **Status:** Implemented in PR #77. The unchecked task boxes and
> “Expected: FAIL” notes below are the original TDD execution record, not open
> follow-up work. Current behavior is verified by the PR tests and service docs.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `labby serve` from eagerly starting every configured upstream MCP server while preserving full gateway `scout`, `invoke`, `code_search`, and `code_execute` access.

**Architecture:** Startup installs an empty shared `UpstreamPool` and `GatewayManager`; upstream processes connect lazily from the shared dispatch layer. `dispatch/gateway/manager.rs` owns runtime warm-up decisions for search and exact tool execution, while `dispatch/upstream/pool.rs` exposes focused primitives for registering configured upstream metadata and connecting one upstream on demand. `mcp/server.rs` remains a thin protocol adapter.

**Tech Stack:** Rust 2024, tokio, rmcp, existing `GatewayManager`, `UpstreamPool`, `ToolIndex`, and `cargo nextest`.

---

## File Structure

- Modify `crates/lab/src/dispatch/upstream/pool.rs`
  - Add a lazy catalog seeding API that records enabled upstreams without connecting.
  - Add a focused `ensure_tools_for_upstream(...)` API that connects/reprobes one upstream and updates catalog/connection state.
  - Keep connection details and runtime ownership inside the upstream dispatch layer.
- Modify `crates/lab/src/dispatch/gateway/manager.rs`
  - Add shared dispatch helpers for lazy search warm-up and exact upstream tool warm-up.
  - Call those helpers from `search_tools`, Code Mode catalog construction support, `resolve_tool_execute_with_upstream`, and `resolve_code_mode_upstream_tool`.
  - Keep MCP unaware of process lifecycle.
- Modify `crates/lab/src/dispatch/gateway/code_mode.rs`
  - Call a `GatewayManager` method that returns a ready Code Mode catalog, instead of directly reading `current_pool().healthy_tools()`.
  - This is still shared dispatch, not MCP.
- Modify `crates/lab/src/cli/serve.rs`
  - Replace startup eager discovery with lazy pool creation plus catalog seeding.
  - Keep recursive stdio suppression behavior intact.
  - Keep explicit auto-import behavior unchanged unless it intentionally calls gateway refresh/test.
- Test in `crates/lab/src/dispatch/upstream/pool.rs`
  - Unit tests for lazy catalog seeding and single-upstream connect-on-demand using existing in-process connector test hooks.
- Test in `crates/lab/src/dispatch/gateway/manager.rs`
  - Unit tests that `search_tools` warms a cold runtime, and exact tool resolution connects only the requested upstream.
- Test in `crates/lab/src/dispatch/gateway/code_mode.rs`
  - Unit test that `code_search` asks the manager for a warmed catalog instead of returning empty on a cold pool.
- Optional docs update in `docs/GATEWAY.md` or the closest gateway runtime doc if existing docs claim startup eagerly connects upstreams.

## Task 1: Add Lazy Upstream Pool Primitives

**Files:**
- Modify: `crates/lab/src/dispatch/upstream/pool.rs`

- [ ] **Step 1: Write failing tests for cold seeded upstreams and one-upstream connection**

Add tests near the existing `register_in_process_service_list_with_connector` tests in `pool.rs`:

```rust
#[tokio::test]
async fn seed_lazy_upstreams_records_enabled_names_without_connections() {
    let pool = UpstreamPool::new();
    let configs = vec![
        test_upstream_config("alpha"),
        test_upstream_config("beta"),
        test_upstream_config_disabled("disabled"),
    ];

    pool.seed_lazy_upstreams(&configs).await;

    assert_eq!(pool.upstream_count().await, 2);
    assert_eq!(pool.connection_count_for_tests().await, 0);
    assert!(pool.cached_upstream_summary("alpha").await.is_some());
    assert!(pool.cached_upstream_summary("beta").await.is_some());
    assert!(pool.cached_upstream_summary("disabled").await.is_none());
}

#[tokio::test]
async fn ensure_tools_for_upstream_connects_only_requested_upstream() {
    let pool = UpstreamPool::new();
    let configs = vec![test_upstream_config("slow"), test_upstream_config("fast")];
    pool.seed_lazy_upstreams(&configs).await;

    let fast_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let slow_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let connector: TestUpstreamConnector = {
        let fast_seen = Arc::clone(&fast_seen);
        let slow_seen = Arc::clone(&slow_seen);
        Arc::new(move |config| {
            let fast_seen = Arc::clone(&fast_seen);
            let slow_seen = Arc::clone(&slow_seen);
            Box::pin(async move {
                match config.name.as_str() {
                    "fast" => fast_seen.store(true, Ordering::Relaxed),
                    "slow" => slow_seen.store(true, Ordering::Relaxed),
                    other => panic!("unexpected upstream {other}"),
                }
                Ok(test_connected_upstream(&config.name, vec![test_tool("ping")]))
            })
        })
    };

    pool.ensure_tools_for_upstream_with_connector(&configs[1], None, connector)
        .await
        .expect("fast connects");

    assert!(fast_seen.load(Ordering::Relaxed));
    assert!(!slow_seen.load(Ordering::Relaxed));
    assert_eq!(pool.connection_count_for_tests().await, 1);
    assert_eq!(pool.healthy_tools_for_upstream("fast").await.len(), 1);
    assert!(pool.healthy_tools_for_upstream("slow").await.is_empty());
}
```

If helper names already exist, reuse the local test helpers instead of duplicating them. If they do not exist, add small test-only helpers in the test module:

```rust
fn test_upstream_config(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        enabled: true,
        url: Some(format!("http://127.0.0.1/{name}")),
        ..UpstreamConfig::default()
    }
}

fn test_upstream_config_disabled(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        enabled: false,
        ..test_upstream_config(name)
    }
}
```

- [ ] **Step 2: Run the new pool tests and verify they fail**

Run:

```bash
cargo nextest run -p labby dispatch::upstream::pool::seed_lazy_upstreams_records_enabled_names_without_connections dispatch::upstream::pool::ensure_tools_for_upstream_connects_only_requested_upstream --all-features
```

Expected: FAIL because `seed_lazy_upstreams`, `connection_count_for_tests`, and/or `ensure_tools_for_upstream_with_connector` do not exist.

- [ ] **Step 3: Implement lazy catalog seeding**

In `impl UpstreamPool`, add:

```rust
pub async fn seed_lazy_upstreams(&self, configs: &[UpstreamConfig]) {
    let mut catalog = self.catalog.write().await;
    let mut resource_names = Vec::new();
    let mut processed_names = std::collections::HashSet::new();

    for config in configs {
        if !config.enabled {
            continue;
        }
        if !processed_names.insert(&config.name) {
            continue;
        }
        if config.name.contains('/') || config.name.contains('?') || config.name.contains('#') {
            continue;
        }
        if validate_upstream_config(config).is_err() {
            continue;
        }

        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        catalog.entry(config.name.clone()).or_insert_with(|| UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: HashMap::new(),
            tool_health: UpstreamHealth::Unhealthy {
                consecutive_failures: 0,
            },
            tool_last_error: None,
            resource_count: 0,
            resource_health: UpstreamHealth::Healthy,
            resource_last_error: None,
            prompt_count: 0,
            prompt_health: UpstreamHealth::Healthy,
            prompt_last_error: None,
            exposure_policy: resolve_exposure_policy(&config.name, config.expose_tools.clone()),
        });

        if config.proxy_resources {
            resource_names.push(config.name.clone());
        }
    }

    resource_names.sort_unstable();
    resource_names.dedup();
    *self.resource_upstreams.write().await = resource_names;
}
```

Adjust fields to match the actual `UpstreamEntry` definition in `types.rs`; do not add synthetic fields.

- [ ] **Step 4: Implement single-upstream ensure API**

Add a production method:

```rust
pub async fn ensure_tools_for_upstream(
    &self,
    config: &UpstreamConfig,
    oauth_subject: Option<&str>,
) -> anyhow::Result<bool> {
    if !config.enabled {
        return Ok(false);
    }

    if !self.healthy_tools_for_upstream(&config.name).await.is_empty() {
        return Ok(false);
    }

    let stale_connection = {
        let mut connections = self.connections.write().await;
        connections.remove(&config.name)
    };
    if let Some(connection) = stale_connection {
        connection
            .shutdown(&config.name, "upstream.lazy.ensure.before_connect")
            .await;
    }

    let started = Instant::now();
    let (conn, tools) = connect_upstream(
        config,
        oauth_subject,
        self.oauth_client_cache.as_ref(),
        self.runtime_origin.as_deref(),
        self.runtime_owner.as_ref(),
    )
    .await?;

    let tool_count = tools.len();
    self.connections
        .write()
        .await
        .insert(config.name.clone(), conn);
    self.replace_catalog_tools(config, tools).await;
    self.record_success_for(&config.name, UpstreamCapability::Tools)
        .await;
    tracing::info!(
        surface = "dispatch",
        service = "upstream.pool",
        action = "upstream.lazy.ensure",
        event = "finish",
        upstream = %config.name,
        tool_count,
        elapsed_ms = started.elapsed().as_millis(),
        "lazy upstream tools connected"
    );
    Ok(true)
}
```

Keep a test-only connector variant if needed for deterministic tests:

```rust
#[cfg(test)]
type TestUpstreamConnector = Arc<
    dyn Fn(UpstreamConfig) -> BoxFuture<'static, anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)>>
        + Send
        + Sync,
>;
```

- [ ] **Step 5: Run the pool tests and verify they pass**

Run:

```bash
cargo nextest run -p labby dispatch::upstream::pool::seed_lazy_upstreams_records_enabled_names_without_connections dispatch::upstream::pool::ensure_tools_for_upstream_connects_only_requested_upstream --all-features
```

Expected: PASS.

## Task 2: Move Lazy Search Warm-Up Into Gateway Manager

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write failing gateway manager tests**

Add tests near existing `search_tools` tests:

```rust
#[tokio::test]
async fn search_tools_warms_cold_lazy_runtime_before_searching() {
    let manager = test_manager_with_tool_search_enabled();
    let pool = Arc::new(UpstreamPool::new());
    let cfg = LabConfig {
        upstream: vec![test_upstream_config("alpha")],
        tool_search: ToolSearchConfig {
            enabled: true,
            ..ToolSearchConfig::default()
        },
        ..LabConfig::default()
    };
    pool.seed_lazy_upstreams(&cfg.upstream).await;
    manager.seed_config(cfg).await;
    manager.runtime.swap(Some(Arc::clone(&pool))).await;

    manager
        .ensure_search_runtime_ready(true)
        .await
        .expect("search runtime warms");

    assert!(
        manager.current_pool().await.is_some(),
        "manager keeps a shared lazy pool installed"
    );
}
```

If `GatewayManager::runtime` is private to tests, use existing test constructors in the same module. If no constructor exists, add a test-only helper following local patterns.

- [ ] **Step 2: Run the new manager test and verify it fails**

Run:

```bash
cargo nextest run -p labby dispatch::gateway::manager::search_tools_warms_cold_lazy_runtime_before_searching --all-features
```

Expected: FAIL because `ensure_search_runtime_ready` does not exist.

- [ ] **Step 3: Add shared dispatch warm-up helpers**

In `impl GatewayManager`, add:

```rust
pub async fn ensure_search_runtime_ready(&self, wait_for_refresh: bool) -> Result<(), ToolError> {
    let cfg = self.config.read().await.clone();
    if !cfg.tool_search.enabled {
        return Ok(());
    }

    let pool = match self.runtime.current_pool().await {
        Some(pool) => pool,
        None => {
            let base_pool = match &self.oauth_client_cache {
                Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
                None => UpstreamPool::new(),
            };
            let pool = Arc::new(base_pool);
            pool.seed_lazy_upstreams(&cfg.upstream).await;
            self.runtime.swap(Some(Arc::clone(&pool))).await;
            pool
        }
    };

    pool.seed_lazy_upstreams(&cfg.upstream).await;
    drop(pool);
    self.refresh_tool_search_indexes_if_stale(wait_for_refresh).await;
    Ok(())
}

pub async fn ensure_upstream_tool_runtime_ready(
    &self,
    upstream_name: &str,
) -> Result<(), ToolError> {
    let cfg = self.config.read().await.clone();
    let Some(upstream) = cfg.upstream.iter().find(|candidate| candidate.name == upstream_name) else {
        return Err(ToolError::Sdk {
            sdk_kind: "unknown_upstream".to_string(),
            message: format!("unknown upstream `{upstream_name}`"),
        });
    };

    let pool = match self.runtime.current_pool().await {
        Some(pool) => pool,
        None => {
            let base_pool = match &self.oauth_client_cache {
                Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
                None => UpstreamPool::new(),
            };
            let pool = Arc::new(base_pool);
            pool.seed_lazy_upstreams(&cfg.upstream).await;
            self.runtime.swap(Some(Arc::clone(&pool))).await;
            pool
        }
    };

    pool.seed_lazy_upstreams(&cfg.upstream).await;
    pool.ensure_tools_for_upstream(upstream, Some(SHARED_GATEWAY_OAUTH_SUBJECT))
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "upstream_connect_error".to_string(),
            message: format!("failed to connect upstream `{upstream_name}`: {err}"),
        })?;
    Ok(())
}
```

If OAuth subject handling needs to distinguish OAuth and non-OAuth upstreams, pass `Some(SHARED_GATEWAY_OAUTH_SUBJECT)` only when `upstream.oauth.is_some()`.

- [ ] **Step 4: Route `search_tools` through the helper**

Replace:

```rust
let has_cached_index = self.has_cached_tool_search_index();
self.refresh_tool_search_indexes_if_stale(!has_cached_index)
    .await;
```

with:

```rust
let has_cached_index = self.has_cached_tool_search_index();
self.ensure_search_runtime_ready(!has_cached_index).await?;
```

- [ ] **Step 5: Run manager tests**

Run:

```bash
cargo nextest run -p labby dispatch::gateway::manager --all-features
```

Expected: PASS.

## Task 3: Make Code Mode Catalog Use Gateway Manager Warm-Up

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`

- [ ] **Step 1: Add a manager-owned Code Mode catalog API**

In `GatewayManager`, add:

```rust
pub async fn code_mode_catalog_tools(&self) -> Result<Vec<UpstreamTool>, ToolError> {
    self.ensure_search_runtime_ready(true).await?;
    let Some(pool) = self.current_pool().await else {
        return Ok(Vec::new());
    };
    Ok(pool.healthy_tools().await)
}
```

- [ ] **Step 2: Update Code Mode broker to use the manager API**

In `code_mode.rs`, replace direct pool access:

```rust
let Some(pool) = manager.current_pool().await else {
    return Ok((Vec::new(), 2, false));
};

let mut entries = pool
    .healthy_tools()
    .await
    .into_iter()
```

with:

```rust
let mut entries = manager
    .code_mode_catalog_tools()
    .await?
    .into_iter()
```

Keep the rest of the mapping unchanged.

- [ ] **Step 3: Add or update a Code Mode test**

Update the existing cold-manager test to assert the manager path is used. If full upstream connect mocking is too heavy in `code_mode.rs`, add a focused test for empty manager behavior and rely on `manager.rs` tests for warm-up:

```rust
#[tokio::test]
async fn code_search_uses_gateway_manager_catalog_api() {
    let manager = test_manager_with_empty_lazy_pool_and_tool_search_enabled();
    let broker = CodeModeBroker::new(&ToolRegistry::new(), Some(&manager));

    let result = broker
        .search(
            "async () => tools.length",
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
        )
        .await
        .expect("code search succeeds");

    assert_eq!(result, serde_json::json!(0));
}
```

- [ ] **Step 4: Run Code Mode tests**

Run:

```bash
cargo nextest run -p labby dispatch::gateway::code_mode --all-features
```

Expected: PASS.

## Task 4: Lazy Exact Tool Execution

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write failing exact-tool tests**

Add tests for `resolve_tool_execute_with_upstream` and `resolve_code_mode_upstream_tool` cold-pool behavior:

```rust
#[tokio::test]
async fn resolve_tool_execute_with_upstream_warms_requested_upstream() {
    let manager = test_manager_with_tool_search_enabled();
    manager.seed_config(test_config_with_upstreams(["alpha", "beta"])).await;
    manager
        .install_test_lazy_pool_with_tool("alpha", "ping")
        .await;

    let result = manager
        .resolve_tool_execute_with_upstream("ping", Some("alpha"))
        .await;

    assert!(result.is_ok());
}
```

Use existing local test helpers where available. If deterministic connector injection from Task 1 is not exposed to manager tests, assert that the helper is called by testing `ensure_upstream_tool_runtime_ready` directly with a test connector hook.

- [ ] **Step 2: Run the exact-tool test and verify it fails**

Run:

```bash
cargo nextest run -p labby dispatch::gateway::manager::resolve_tool_execute_with_upstream_warms_requested_upstream --all-features
```

Expected: FAIL because exact resolution reads only the current cached pool.

- [ ] **Step 3: Update exact upstream paths**

In `resolve_tool_execute_with_upstream`, before reading `pool.healthy_tools_for_upstream(upstream_name)`, add:

```rust
if let Some(upstream_name) = selector.upstream.as_deref() {
    self.ensure_upstream_tool_runtime_ready(upstream_name).await?;
}
```

For the no-upstream selector path, keep using the search-warm pool:

```rust
if selector.upstream.is_none() {
    self.ensure_search_runtime_ready(true).await?;
}
```

In `resolve_code_mode_upstream_tool`, add:

```rust
self.ensure_upstream_tool_runtime_ready(upstream).await?;
```

before reading `pool.healthy_tools_for_upstream(upstream)`.

- [ ] **Step 4: Run manager tests**

Run:

```bash
cargo nextest run -p labby dispatch::gateway::manager --all-features
```

Expected: PASS.

## Task 5: Change Serve Startup Policy

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`

- [ ] **Step 1: Replace eager discovery with lazy seeding**

In `run`, replace the eager block:

```rust
if !suppress_upstream_runtime {
    if upstream_oauth_runtime.is_some() {
        pool.discover_all_for_subject_with_in_process_peers(
            &config.upstream,
            SHARED_GATEWAY_OAUTH_SUBJECT,
            &registry,
        )
        .await;
    } else {
        pool.discover_all_with_in_process_peers(&config.upstream, &registry)
            .await;
    }
    tracing::info!(... "upstream gateway discovery complete");
    gateway_runtime.swap(Some(pool)).await;
} else {
    tracing::info!(... "upstream discovery skipped because stdio recursion guard is active");
}
```

with:

```rust
if !suppress_upstream_runtime {
    pool.seed_lazy_upstreams(&config.upstream).await;
    tracing::info!(
        subsystem = "gateway_client",
        phase = "discovery.lazy",
        upstream_count = config.upstream.len(),
        seeded_upstream_count = pool.upstream_count().await,
        "upstream gateway discovery deferred until first use"
    );
    gateway_runtime.swap(Some(pool)).await;
} else {
    tracing::info!(
        subsystem = "gateway_client",
        phase = "discovery.skipped",
        spawn_depth,
        "upstream discovery skipped because stdio recursion guard is active"
    );
}
```

Keep `register_in_process_service_peers` out of startup. Built-in Lab service tools remain visible through normal MCP service registration and built-in search, not as upstream processes.

- [ ] **Step 2: Verify no eager discovery call remains in serve startup**

Run:

```bash
rg -n "discover_all.*with_in_process_peers|register_in_process_service_peers" crates/lab/src/cli/serve.rs
```

Expected: no matches in startup code.

- [ ] **Step 3: Run compile check**

Run:

```bash
cargo check -p labby --all-features
```

Expected: PASS.

## Task 6: Verification

**Files:**
- No new files unless docs are updated.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo nextest run -p labby dispatch::upstream::pool dispatch::gateway::manager dispatch::gateway::code_mode --all-features
```

Expected: PASS.

- [ ] **Step 2: Run all labby tests**

Run:

```bash
cargo nextest run -p labby --all-features
```

Expected: PASS.

- [ ] **Step 3: Run formatting**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS.

- [ ] **Step 4: Manual startup smoke**

Run with debug logging and configured upstreams:

```bash
LAB_LOG=labby=debug cargo run -p labby --all-features -- serve --host 127.0.0.1 --port 8765
```

Expected:

- startup logs include `phase = "discovery.lazy"`
- startup logs do not include `starting upstream capability discovery`
- configured stdio upstream processes are not spawned until first `scout`, `code_search`, `invoke`, or `code_execute`

- [ ] **Step 5: Manual lazy-use smoke**

Call `scout` from an MCP client or equivalent test harness.

Expected:

- first call logs `tool_search.reprobe` and upstream connect events
- later calls use cached indexes until the existing TTL says they are stale
- `invoke` with `upstream::tool` connects only that upstream when cold

## Self-Review

- Spec coverage: The plan removes eager startup discovery, keeps tools available through gateway search/execution, and places lifecycle logic in shared dispatch.
- Placeholder scan: No placeholder markers or unspecified "add tests" steps remain.
- Type consistency: New public methods are consistently named `seed_lazy_upstreams`, `ensure_tools_for_upstream`, `ensure_search_runtime_ready`, `ensure_upstream_tool_runtime_ready`, and `code_mode_catalog_tools`.
- Scope check: The plan avoids changing MCP tool registration semantics. MCP remains a protocol adapter.
