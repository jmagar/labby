# Nonblocking Root List Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Labby's root MCP `list_tools` response fast and stable even when one upstream MCP server is slow, unreachable, or timing out during initialization.

**Architecture:** Root MCP `list_tools` must not synchronously warm or refresh the Code Mode upstream catalog. In hidden-raw-tools / MCP App mode, it should advertise Labby's synthetic Code Mode tool and merge only already-cached healthy MCP App UI siblings. Cold upstream discovery remains owned by Code Mode execution/search and direct tool resolution paths, where the user actually asked to inspect or call upstream tools.

**Tech Stack:** Rust 2024, Tokio, rmcp, Labby gateway dispatch layer, existing `cargo test -p labby --all-features` test style.

## Global Constraints

- `CLAUDE.md` is the source of truth for agent memory; do not edit `AGENTS.md` or `GEMINI.md` directly.
- Rust module style is modern sibling files only; do not add `mod.rs`.
- Business/shared gateway behavior belongs in `crates/lab/src/dispatch/`; MCP surface adapters stay thin.
- Do not import `mcp/` from `dispatch/upstream/`.
- Default verification targets all-features builds/tests.
- Keep changes narrowly scoped to root `list_tools` behavior and Code Mode catalog warming.

---

## File Structure

- Modify: `crates/lab/src/mcp/handlers_tools.rs`
  - Responsibility: MCP `list_tools` adapter. Remove synchronous Code Mode catalog warmup from the root tool-list path. Continue reading already-healthy UI tools from the current upstream pool.
- Modify: `crates/lab/src/mcp/handlers_tools/tests.rs`
  - Responsibility: regression coverage for root tool-list behavior. Add a test proving a lazy/cold upstream does not get connected during `list_tools`.
- No new runtime files.
- No docs changes beyond this implementation plan unless implementation reveals generated docs need refresh.

### Task 1: Regression Test for Nonblocking Root `list_tools`

**Files:**
- Modify: `crates/lab/src/mcp/handlers_tools/tests.rs`

**Interfaces:**
- Consumes: `code_mode_manager_with_pool(enabled, upstream, pool) -> Arc<GatewayManager>` from the same test module.
- Consumes: `fixture_upstream_config(name: &str) -> UpstreamConfig` from the same test module.
- Produces: test `list_tools_does_not_cold_connect_code_mode_catalog()` that fails while `list_tools_impl()` calls `manager.code_mode_catalog_tools_allowed(true, ...)`.

- [ ] **Step 1: Add the failing test**

Add this test after `list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden`:

```rust
#[tokio::test]
async fn list_tools_does_not_cold_connect_code_mode_catalog() {
    let pool = Arc::new(UpstreamPool::new());
    let manager =
        code_mode_manager_with_pool(true, fixture_upstream_config("cold-apps"), Arc::clone(&pool))
            .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(
        names.contains(&CODE_MODE_TOOL_NAME),
        "root list_tools must keep advertising Code Mode"
    );
    let summary = pool.cached_upstream_summary("cold-apps").await;
    assert!(
        summary.is_none(),
        "root list_tools must not cold-connect or populate a lazy upstream catalog"
    );
    assert!(
        pool.upstream_tool_last_error("cold-apps").await.is_none(),
        "skipping cold discovery should not mark the upstream failed"
    );
}
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run:

```bash
cargo test -p labby --all-features list_tools_does_not_cold_connect_code_mode_catalog -- --nocapture
```

Expected: FAIL. The failure should show the cached upstream summary exists because `list_tools_impl()` warmed the Code Mode catalog.

- [ ] **Step 3: Commit the failing test**

```bash
git add crates/lab/src/mcp/handlers_tools/tests.rs
git commit -m "test: cover nonblocking root list tools"
```

### Task 2: Remove Synchronous Catalog Warm from Root `list_tools`

**Files:**
- Modify: `crates/lab/src/mcp/handlers_tools.rs`

**Interfaces:**
- Consumes: `LabMcpServer::current_upstream_pool().await -> Option<Arc<UpstreamPool>>`.
- Consumes: `UpstreamPool::healthy_ui_tools_allowed(allowed) -> Vec<UpstreamTool>`.
- Produces: root `list_tools_impl()` that never calls `GatewayManager::code_mode_catalog_tools_allowed(true, ...)`.
- Produces: warning-free all-features compile.

- [ ] **Step 1: Remove the warmup block**

In `crates/lab/src/mcp/handlers_tools.rs`, delete this whole block:

```rust
        #[cfg(feature = "gateway")]
        if hide_raw_tools
            && let Some(manager) = self.gateway_manager.as_ref()
            && let Err(error) = manager
                .code_mode_catalog_tools_allowed(
                    true,
                    None,
                    oauth_subject.as_deref(),
                    self.route_scope.allowed_upstreams(),
                )
                .await
        {
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "list_tools",
                error = %error,
                "failed to warm upstream catalog before listing MCP App tools"
            );
        }
```

Do not replace it with another cold-connect path. The next block that reads `self.current_upstream_pool().await` stays in place.

- [ ] **Step 2: Add an explanatory comment above the pool read**

Change the comment immediately before `if let Some(pool) = self.current_upstream_pool().await` to this:

```rust
        // Merge upstream tools from the already-healthy catalog only. Root
        // `list_tools` must never cold-connect upstreams: a single slow or
        // unhealthy server can otherwise stall the host's tool refresh and make
        // Labby's synthetic Code Mode tool appear to disappear. Code Mode
        // execution/search still performs cold discovery through the gateway
        // manager when the caller actually asks for upstream catalog data.
```

- [ ] **Step 3: Run the focused test to verify it passes**

Run:

```bash
cargo test -p labby --all-features list_tools_does_not_cold_connect_code_mode_catalog -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Run existing nearby list-tools tests**

Run:

```bash
cargo test -p labby --all-features list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden list_tools_advertises_code_mode_output_schemas -- --nocapture
```

Expected: PASS. If Cargo rejects multiple exact test filters, run the two commands separately:

```bash
cargo test -p labby --all-features list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden -- --nocapture
cargo test -p labby --all-features list_tools_advertises_code_mode_output_schemas -- --nocapture
```

- [ ] **Step 5: Commit the implementation**

```bash
git add crates/lab/src/mcp/handlers_tools.rs
git commit -m "fix: keep root list tools nonblocking"
```

### Task 3: Prove Code Mode Discovery Still Works Outside Root `list_tools`

**Files:**
- Test-only task; no source edits expected.

**Interfaces:**
- Consumes: `GatewayManager::code_mode_catalog_tools_allowed(true, ...)` through `CodeModeBroker::build_code_mode_proxy_for_tests(...)`.
- Produces: focused proof that removing root warmup did not break Code Mode search/proxy catalog generation.

- [ ] **Step 1: Run Code Mode broker catalog tests**

Run:

```bash
cargo test -p labby --all-features execute_proxy_embeds_local_discovery_helpers_without_host_callbacks execute_proxy_embeds_only_reduced_discovery_catalog -- --nocapture
```

Expected: PASS. If Cargo rejects multiple exact test filters, run the two commands separately:

```bash
cargo test -p labby --all-features execute_proxy_embeds_local_discovery_helpers_without_host_callbacks -- --nocapture
cargo test -p labby --all-features execute_proxy_embeds_only_reduced_discovery_catalog -- --nocapture
```

- [ ] **Step 2: Run the current live ytdl catalog smoke test**

Run:

```bash
labby gateway code exec --json --code 'async () => await codemode.search({ query: "ytdl-mcp youtube download audio", limit: 10 })'
```

Expected: JSON contains these tool IDs:

```json
[
  "ytdl-mcp::youtube_download",
  "ytdl-mcp::youtube_search",
  "ytdl-mcp::youtube_probe"
]
```

Warnings for unrelated OAuth upstreams such as `google-drive`, `google-gmail`, `google-calendar`, `google-people`, or `globalping` are acceptable only if the command exits `0` and returns the ytdl results.

- [ ] **Step 3: Run the gateway status sanity check**

Run:

```bash
time labby gateway code status --json
```

Expected: exits `0` and includes:

```json
"enabled": true
```

The full JSON will include additional Code Mode limits. This is a low-friction local sanity check that the gateway is up before MCP-level validation.

- [ ] **Step 4: Run focused Rust formatting and diff checks**

Run:

```bash
cargo fmt --all --check
git diff --check
```

Expected: both commands exit `0`.

- [ ] **Step 5: Run the default compile gate**

Run:

```bash
cargo check --workspace --all-features
```

Expected: PASS with no new warnings from the touched files.

- [ ] **Step 6: Commit verification-only changes if any**

If no files changed during verification, do not commit. If `cargo fmt` changed formatting, commit only those touched files:

```bash
git add crates/lab/src/mcp/handlers_tools.rs crates/lab/src/mcp/handlers_tools/tests.rs
git commit -m "chore: format nonblocking list tools changes"
```

## Self-Review

**Spec coverage:** The plan removes the root cause identified in logs: root `list_tools` synchronously warming Code Mode catalog and getting delayed by a slow upstream initialize. Task 1 covers the regression, Task 2 implements the behavior, Task 3 proves Code Mode/ytdl discovery still works through its intended path.

**Placeholder scan:** No `TBD`, `TODO`, "similar to", or unspecified implementation steps remain.

**Type consistency:** The plan uses existing names verified in the codebase: `list_tools_impl`, `code_mode_catalog_tools_allowed`, `healthy_ui_tools_allowed`, `cached_upstream_summary`, `upstream_tool_last_error`, `CODE_MODE_TOOL_NAME`, and the existing test helpers in `handlers_tools/tests.rs`.
