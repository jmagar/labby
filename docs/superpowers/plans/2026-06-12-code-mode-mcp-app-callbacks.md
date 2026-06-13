# Code Mode MCP App Callbacks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make MCP Apps rendered through Lab's Code Mode surface able to call exposed sibling tools on the same upstream without re-exposing ordinary raw tools to the model-facing `tools/list`.

**Architecture:** Keep Code Mode's collapsed model surface intact: `list_tools` continues to show only synthetic `search`/`execute` plus widget-bearing MCP App tools. Add a narrow `tools/call` bypass at the existing raw-tool hidden gate that allows callbacks only when the requested upstream tool is exposed, routable, non-destructive, route-scope-allowed, and belongs to an upstream that also exposes an MCP App UI tool. Preserve the existing opt-in `LAB_CODE_MODE_WIDGET_CALLBACKS=1` legacy bypass as the broader operator escape hatch.

**Tech Stack:** Rust 2024, rmcp, Tokio, serde_json, Lab `UpstreamPool`, Lab MCP `call_tool` and `list_tools` handlers, `cargo test`.

---

## File Structure

- Modify `crates/lab/src/dispatch/upstream/pool/tools.rs`
  - Owns upstream tool catalog lookup helpers. Add or verify a helper that finds same-upstream MCP App sibling callback candidates without importing `mcp/` types.
- Modify `crates/lab/src/mcp/call_tool.rs`
  - Owns the Code Mode raw-tool hidden gate in MCP `tools/call`. Add or verify the narrow callback bypass here, before returning `tool hidden while code_mode mode is enabled`.
- Modify `crates/lab/src/mcp/handlers_tools/tests.rs`
  - Owns MCP tool-list and tool-call visibility tests. Add regression coverage for direct failure, same-upstream sibling success path, ambiguity, destructive denial, and continued hidden `tools/list` behavior.
- Modify `docs/dev/CODE_MODE.md`
  - Document the default safe MCP App callback behavior and clarify that `LAB_CODE_MODE_WIDGET_CALLBACKS=1` is now only the broad legacy bypass.
- Modify `docs/surfaces/MCP.md`
  - Document host-visible behavior: widget-bearing tools may stay visible, sibling callback tools stay hidden from `tools/list`, and callback calls are scoped.

## Current Evidence To Recheck

Before editing, inspect the live checkout because parts of this fix may already be present:

```bash
rg -n "find_mcp_app_sibling|tool_has_mcp_app_ui_resource|widget_callback|hidden while code_mode" crates/lab/src
sed -n '210,305p' crates/lab/src/mcp/call_tool.rs
sed -n '150,185p' crates/lab/src/dispatch/upstream/pool/tools.rs
sed -n '304,425p' crates/lab/src/mcp/handlers_tools/tests.rs
```

Expected: either no sibling callback support exists yet, or the current branch already contains a partial implementation that must be hardened against the tests below.

### Task 1: Add Upstream-Pool MCP App Sibling Lookup

**Files:**
- Modify: `crates/lab/src/dispatch/upstream/pool/tools.rs`
- Test: `crates/lab/src/dispatch/upstream/pool/tools.rs`

- [ ] **Step 1: Write the failing pool tests**

Append these tests inside the existing `#[cfg(test)] mod tests` in `crates/lab/src/dispatch/upstream/pool/tools.rs`. If a similarly named test already exists, replace it with this fuller version so it covers exposure policy, route scope, and ambiguous same-name tools.

```rust
#[tokio::test]
async fn mcp_app_sibling_lookup_requires_exposed_ui_tool_on_same_upstream() {
    let pool = UpstreamPool::new();

    let apps_name: Arc<str> = Arc::from("apps");
    let mut apps_tools =
        test_upstream_tools(&apps_name, &["youtube_search_ui", "youtube_probe"]);
    let ui_meta = Meta(serde_json::Map::from_iter([(
        "ui".to_string(),
        serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
    )]));
    apps_tools
        .get_mut("youtube_search_ui")
        .expect("ui tool")
        .tool
        .meta = Some(ui_meta);
    let apps_entry = healthy_in_process_entry(Arc::clone(&apps_name), apps_tools);
    pool.catalog
        .write()
        .await
        .insert("apps".to_string(), apps_entry);

    let plain_name: Arc<str> = Arc::from("plain");
    let plain_tools = test_upstream_tools(&plain_name, &["youtube_probe"]);
    let plain_entry = healthy_in_process_entry(Arc::clone(&plain_name), plain_tools);
    pool.catalog
        .write()
        .await
        .insert("plain".to_string(), plain_entry);

    let candidates = pool
        .find_mcp_app_sibling_tool_candidates("youtube_probe", None)
        .await;
    let upstreams = candidates
        .iter()
        .map(|(upstream, _)| upstream.as_str())
        .collect::<Vec<_>>();

    assert_eq!(upstreams, vec!["apps"]);

    let allowed = BTreeSet::from(["plain".to_string()]);
    assert!(
        pool.find_mcp_app_sibling_tool_candidates("youtube_probe", Some(&allowed))
            .await
            .is_empty(),
        "route scope must still constrain MCP App callback siblings"
    );
}

#[tokio::test]
async fn mcp_app_sibling_lookup_respects_exposure_policy() {
    let pool = UpstreamPool::new();
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut tools = test_upstream_tools(
        &upstream_name,
        &["youtube_search_ui", "youtube_probe", "internal_delete"],
    );
    tools
        .get_mut("youtube_search_ui")
        .expect("ui tool")
        .tool
        .meta = Some(Meta(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
        )])));
    let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
    entry.exposure_policy = ToolExposurePolicy::from_patterns(vec![
        "youtube_search_ui".to_string(),
        "youtube_probe".to_string(),
    ])
    .expect("policy");
    pool.catalog
        .write()
        .await
        .insert("apps".to_string(), entry);

    assert_eq!(
        pool.find_mcp_app_sibling_tool_candidates("youtube_probe", None)
            .await
            .len(),
        1
    );
    assert!(
        pool.find_mcp_app_sibling_tool_candidates("internal_delete", None)
            .await
            .is_empty(),
        "unexposed sibling tools must remain uncallable"
    );
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p lab mcp_app_sibling_lookup --all-features
```

Expected before implementation: compile failure for missing `find_mcp_app_sibling_tool_candidates`, or assertion failure showing the helper does not filter correctly.

- [ ] **Step 3: Implement the lookup helper**

In `crates/lab/src/dispatch/upstream/pool/tools.rs`, add this helper near `find_tool_candidates`. Keep it in the dispatch layer and do not import anything from `crate::mcp`.

```rust
/// Return exposed tools whose upstream also exposes at least one MCP App UI tool.
///
/// Code Mode keeps ordinary raw tools out of `list_tools`, but a rendered MCP
/// App can only talk back to its server through host `callServerTool`
/// callbacks. This lookup is the narrow callback allowlist: the requested
/// tool must still be exposed by its upstream, and that same upstream must
/// expose an MCP App UI tool.
pub async fn find_mcp_app_sibling_tool_candidates(
    &self,
    tool_name: &str,
    allowed: Option<&BTreeSet<String>>,
) -> Vec<(String, UpstreamTool)> {
    let catalog = self.catalog.read().await;
    let mut matches = Vec::new();
    for (upstream_name, entry) in catalog.iter() {
        if !upstream_allowed(allowed, upstream_name) || !entry.tool_health.is_routable() {
            continue;
        }
        let Some(tool) = entry.tools.get(tool_name) else {
            continue;
        };
        if !entry.exposure_policy.matches(tool.tool.name.as_ref()) {
            continue;
        }
        let has_ui_sibling = entry.tools.values().any(|candidate| {
            entry.exposure_policy.matches(candidate.tool.name.as_ref())
                && tool_has_mcp_app_ui_resource(candidate)
        });
        if has_ui_sibling {
            matches.push((upstream_name.clone(), tool.clone()));
        }
    }
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches
}
```

Also ensure `tool_has_mcp_app_ui_resource` exists in the same file:

```rust
pub(crate) fn tool_has_mcp_app_ui_resource(tool: &UpstreamTool) -> bool {
    tool.tool
        .meta
        .as_ref()
        .and_then(|meta| meta.0.get("ui"))
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .is_some_and(|uri| uri.starts_with("ui://"))
}
```

If `BTreeSet` or `Value` is not already in scope, add:

```rust
use std::collections::BTreeSet;
use serde_json::Value;
```

- [ ] **Step 4: Run tests to verify pass**

Run:

```bash
cargo test -p lab mcp_app_sibling_lookup --all-features
```

Expected: both `mcp_app_sibling_lookup_*` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/upstream/pool/tools.rs
git commit -m "test: cover mcp app sibling lookup"
```

### Task 2: Allow Narrow MCP App Sibling Callbacks Through Code Mode Gate

**Files:**
- Modify: `crates/lab/src/mcp/call_tool.rs`
- Test: `crates/lab/src/mcp/handlers_tools/tests.rs`

- [ ] **Step 1: Write failing MCP call-tool tests**

Add these tests to `crates/lab/src/mcp/handlers_tools/tests.rs`, near the existing Code Mode visibility tests. If `call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden` already exists, replace it with this version and add the additional destructive denial test.

```rust
#[tokio::test]
async fn call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
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

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("hidden while code_mode mode is enabled"),
        "MCP App sibling callbacks should reach upstream proxy routing, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "test fixture has no live peer, so allowed callbacks should fail at proxy call, got {text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let mut delete_tool = fixture_upstream_tool(&upstream_name, "youtube_delete", None);
    delete_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_delete".to_string(), delete_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
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

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(text.contains("\"kind\":\"confirmation_required\""), "{text}");
    assert!(
        text.contains("not callable via the widget callback bypass"),
        "{text}"
    );
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p lab call_tool_ --all-features
```

Expected before implementation: `call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden` fails with `hidden while code_mode mode is enabled`, and the destructive callback test either reaches the proxy or returns the wrong error kind.

- [ ] **Step 3: Implement the narrow bypass**

In `crates/lab/src/mcp/call_tool.rs`, replace the `if self.code_mode_visibility().await.hides_raw_tools()` block with this logic. Keep it after the built-in service visibility/action checks and before admin-scope/destructive built-in checks.

```rust
if self.code_mode_visibility().await.hides_raw_tools() {
    let widget_callback = if svc.is_none()
        && let Some(pool) = self.current_upstream_pool().await
    {
        let allowed = self.route_scope.allowed_upstreams();
        if crate::config::code_mode_widget_callbacks_enabled() {
            pool.find_tool_allowed(&service, allowed)
                .await
                .map(|(_, tool)| ("upstream_widget_callback_legacy", Some(tool)))
        } else if let Some((_, tool)) = pool
            .find_tool_allowed(&service, allowed)
            .await
            .filter(|(_, tool)| {
                crate::dispatch::upstream::pool::tool_has_mcp_app_ui_resource(tool)
            })
        {
            Some(("upstream_widget_callback", Some(tool)))
        } else {
            let candidates = pool
                .find_mcp_app_sibling_tool_candidates(&service, allowed)
                .await;
            if candidates.is_empty() {
                None
            } else {
                let tool = (candidates.len() == 1)
                    .then(|| candidates.into_iter().next().expect("checked len").1);
                Some(("upstream_widget_sibling_callback", tool))
            }
        }
    } else {
        None
    };
    match widget_callback {
        Some((_, Some(tool))) if tool.destructive => {
            let envelope = build_error(
                &service,
                &action,
                "confirmation_required",
                &format!(
                    "destructive upstream tool `{service}` is not callable via the \
                     widget callback bypass — use the `execute` tool with confirm:true"
                ),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }
        Some((route, _)) => {
            tracing::info!(
                surface = "mcp",
                service = %service,
                action = %action,
                route,
                "code_mode raw-tool gate bypassed for MCP App widget callback"
            );
        }
        None => {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("tool `{service}` is hidden while code_mode mode is enabled"),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }
    }
}
```

Why `candidates.len() == 1`: if two allowed upstreams expose the same sibling tool name and both have UI apps, Lab cannot prove which rendered app originated the callback from `tools/call` alone. In that ambiguous case, the helper returns multiple candidates, this block leaves `tool` as `None`, and the call is allowed to reach normal upstream routing only if that existing routing can disambiguate; otherwise it should fail safely with the existing upstream ambiguity/error path.

- [ ] **Step 4: Run tests to verify pass**

Run:

```bash
cargo test -p lab call_tool_ --all-features
```

Expected: sibling callback and destructive denial tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/mcp/call_tool.rs crates/lab/src/mcp/handlers_tools/tests.rs
git commit -m "fix: allow safe mcp app callbacks in code mode"
```

### Task 3: Prove Tool Visibility Stays Collapsed

**Files:**
- Modify: `crates/lab/src/mcp/handlers_tools/tests.rs`

- [ ] **Step 1: Write the visibility regression test**

Ensure this test exists in `crates/lab/src/mcp/handlers_tools/tests.rs`; replace any weaker version with this exact assertion set.

```rust
#[tokio::test]
async fn list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
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

    assert!(names.contains(&"youtube_search_ui"));
    assert!(!names.contains(&"youtube_probe"));
    assert!(names.contains(&CODE_MODE_SEARCH_TOOL_NAME));
    assert!(names.contains(&TOOL_EXECUTE_TOOL_NAME));
    assert!(!names.contains(&"radarr"));
}
```

- [ ] **Step 2: Run test to verify behavior**

Run:

```bash
cargo test -p lab list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden --all-features
```

Expected: PASS. If `youtube_probe` appears in `names`, the implementation accidentally widened model visibility and must be fixed before continuing.

- [ ] **Step 3: Commit**

```bash
git add crates/lab/src/mcp/handlers_tools/tests.rs
git commit -m "test: keep code mode app siblings hidden from listings"
```

### Task 4: Update Code Mode and MCP Surface Docs

**Files:**
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `docs/surfaces/MCP.md`

- [ ] **Step 1: Update `docs/dev/CODE_MODE.md` widget callback section**

Replace the `### Widget → host callbacks (opt-in)` section with:

```markdown
### Widget → host callbacks

While the synthetic `search`/`execute` surface is active, raw upstream tools stay
hidden from `list_tools`. MCP App tools that carry `_meta.ui.resourceUri` may
still be advertised so the host can render the widget.

A rendered MCP App can call back to its server only through host
`callServerTool` / `tools/call`. Lab allows those callback calls through Code
Mode's raw-tool gate only when all of these are true:

- the requested tool is an exposed upstream tool, not a Lab built-in service;
- the upstream is routable and allowed by the current protected route scope;
- the same upstream exposes at least one MCP App UI tool;
- the requested tool is not destructive.

The callback exemption changes callability only. It does not put sibling tools
back into `list_tools`, so the model-facing surface remains collapsed.
Destructive sibling callbacks return `confirmation_required`; callers should use
the `execute` tool with `confirm:true` for destructive upstream actions.

`LAB_CODE_MODE_WIDGET_CALLBACKS=1` remains as a broader legacy operator bypass.
With that variable set, any known exposed upstream tool may pass the raw-tool
gate while Code Mode is enabled. Leave it off unless a legacy widget depends on
callbacks that cannot be represented by the same-upstream MCP App sibling rule.
```

- [ ] **Step 2: Update `docs/surfaces/MCP.md` Code Mode MCP Apps section**

Add this paragraph after the Code Mode MCP Apps resource description:

```markdown
For upstream MCP Apps, Code Mode keeps widget-bearing tools host-visible so the
host can render their `_meta.ui.resourceUri` resources. Ordinary sibling tools
remain hidden from `tools/list`, but a rendered app's `callServerTool` callback
may call an exposed, non-destructive sibling tool on the same routable upstream.
The exemption is scoped to callback callability and route scope; it does not
expand the model-facing catalog.
```

- [ ] **Step 3: Run doc sanity checks**

Run:

```bash
rg -n "Widget → host callbacks|same-upstream MCP App sibling|LAB_CODE_MODE_WIDGET_CALLBACKS|callServerTool" docs/dev/CODE_MODE.md docs/surfaces/MCP.md
```

Expected: output includes the new default callback behavior and the legacy env-var note.

- [ ] **Step 4: Commit**

```bash
git add docs/dev/CODE_MODE.md docs/surfaces/MCP.md
git commit -m "docs: document code mode mcp app callbacks"
```

### Task 5: Full Verification and Issue Closure Evidence

**Files:**
- No code files unless earlier tasks exposed a bug.

- [ ] **Step 1: Run focused regression tests**

Run:

```bash
cargo test -p lab mcp_app_sibling_lookup --all-features
cargo test -p lab list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden --all-features
cargo test -p lab call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden --all-features
cargo test -p lab call_tool_blocks_destructive_mcp_app_sibling_callbacks --all-features
```

Expected: all focused tests pass.

- [ ] **Step 2: Run all MCP handler tests**

Run:

```bash
cargo test -p lab mcp::handlers_tools --all-features
cargo test -p lab mcp::call_tool_codemode --all-features
```

Expected: all tests pass. If the module path filter does not match in this workspace, run:

```bash
cargo test -p lab handlers_tools --all-features
cargo test -p lab call_tool_codemode --all-features
```

Expected: all tests pass.

- [ ] **Step 3: Run default all-features verification**

Run:

```bash
cargo fmt --all --check
cargo nextest run --workspace --all-features
cargo clippy --workspace --all-features --all-targets -- -D warnings
```

Expected: formatting is clean, nextest passes, and clippy emits no warnings.

- [ ] **Step 4: Capture live-style reproduction notes**

Use this issue comment body after tests pass:

```markdown
Implemented and verified in Lab:

- Code Mode still hides ordinary raw upstream tools from `tools/list`.
- MCP App tools with `_meta.ui.resourceUri` remain host-visible so widgets render.
- A rendered app can call an exposed, non-destructive sibling tool on the same allowed upstream through `callServerTool`.
- Destructive sibling callbacks are rejected with `confirmation_required`.
- `LAB_CODE_MODE_WIDGET_CALLBACKS=1` remains as the broad legacy escape hatch, but is no longer required for normal same-upstream MCP App callbacks.

Verification:

- `cargo test -p lab mcp_app_sibling_lookup --all-features`
- `cargo test -p lab list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden --all-features`
- `cargo test -p lab call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden --all-features`
- `cargo test -p lab call_tool_blocks_destructive_mcp_app_sibling_callbacks --all-features`
- `cargo nextest run --workspace --all-features`
```

- [ ] **Step 5: Final commit if any verification-only docs changed**

```bash
git status --short
git add docs/dev/CODE_MODE.md docs/surfaces/MCP.md crates/lab/src/dispatch/upstream/pool/tools.rs crates/lab/src/mcp/call_tool.rs crates/lab/src/mcp/handlers_tools/tests.rs
git commit -m "fix: restore mcp app callbacks under code mode"
```

Expected: if all prior task commits were already made and `git status --short` is clean, skip this commit.

## Self-Review

Spec coverage:
- Reproduces the ytdl-mcp failure mode: `youtube_search_ui` is visible, `youtube_probe`/`youtube_download` are hidden from `tools/list`, and callback `tools/call` can reach exposed same-upstream siblings.
- Preserves Code Mode's model-facing collapse to `search`/`execute` plus MCP App UI tools.
- Keeps route scope, exposure policy, health/routability, and destructive confirmation boundaries intact.
- Documents the new default callback behavior and the remaining `LAB_CODE_MODE_WIDGET_CALLBACKS=1` escape hatch.

Placeholder scan:
- No task contains `TBD`, `TODO`, "implement later", or "write tests for the above".
- Every code-changing step includes concrete code blocks and commands.

Type consistency:
- `find_mcp_app_sibling_tool_candidates` returns `Vec<(String, UpstreamTool)>` in Task 1 and is called with `allowed: Option<&BTreeSet<String>>` in Task 2.
- The MCP tests use existing fixture names from `handlers_tools/tests.rs`: `fixture_upstream_tool`, `fixture_upstream_entry`, `code_mode_manager_with_pool`, `fixture_upstream_config`, and `test_server`.
- Error assertions use the existing JSON envelope string path through `CallToolResult::error`.
