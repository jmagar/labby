# Schema-Aware Snippets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make snippets declare upstream MCP tools in frontmatter and validate those declarations against the gateway's typed tool schemas before validation, testing, or execution.

**Architecture:** Snippet files stay Markdown/JS and add a compact `tools: [upstream::tool]` frontmatter field. The snippets shared dispatch layer parses those declarations, resolves them through the existing `GatewayManager` and Code Mode catalog, and returns a structured schema report that CLI, MCP, HTTP, and the admin UI can all consume. Individual tool-call payload validation remains in the existing Code Mode broker; snippets add preflight dependency validation and schema visibility.

**Tech Stack:** Rust 2024, serde/serde_json, existing Labby dispatch layer, existing Gateway Code Mode schema/cache types, Next.js 16 admin UI, React 19, Aurora tokens, lucide-react.

---

## File Structure

- Modify `crates/lab/src/dispatch/snippets/store.rs`: parse and serialize `tools: [upstream::tool]` frontmatter declarations.
- Modify `crates/lab/src/dispatch/snippets.rs`: add store-level tests for tool declarations.
- Create `crates/lab/src/dispatch/snippets/schema.rs`: resolve declared snippet tools against the live gateway catalog and build schema reports.
- Modify `crates/lab/src/dispatch/snippets.rs`: export the new schema module.
- Modify `crates/lab/src/dispatch/snippets/dispatch.rs`: include schema reports in `snippets.validate`, preflight declared tools in `snippets.exec` and `snippets.test`, and keep `snippets.list` cheap.
- Modify `crates/lab/src/dispatch/snippets/catalog.rs`: document the schema-aware response and tool declaration contract.
- Modify `crates/lab/src/cli/snippets.rs`: print validation schema-report summaries in human output.
- Modify `crates/lab/src/api/services/snippets.rs`: no new business logic; add tests that HTTP validation returns tool schema reports.
- Modify `apps/gateway-admin/lib/types/snippets.ts`: add typed schema-report fields.
- Modify `apps/gateway-admin/components/snippets/snippets-page-content.tsx`: show declared tools, schema availability, missing tools, and destructive flags using Aurora tokens.
- Modify `apps/gateway-admin/components/snippets/snippets-page-content.test.tsx`: verify the schema report renders.
- Modify `docs/snippets/README.md` and built-in snippets under `docs/snippets/*.md`: document and add `tools` declarations.
- Regenerate `docs/generated/*` with `target/debug/labby docs generate`.

---

### Task 1: Parse Tool Declarations In Snippet Frontmatter

**Files:**
- Modify: `crates/lab/src/dispatch/snippets/store.rs`
- Modify: `crates/lab/src/dispatch/snippets.rs`

- [ ] **Step 1: Write the failing store tests**

Add these tests to `crates/lab/src/dispatch/snippets.rs` inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn frontmatter_tools_parse_code_mode_tool_ids() {
    let temp = tempfile::tempdir().unwrap();
    let lab_home = temp.path().join("lab-home");
    let body = r#"---
name: schema-demo
description: Schema demo
tags: [demo]
tools: [synapse2::scout, qdrant::query]
---

```js
async (input) => ({ ok: true, input })
```
"#;

    create_user_snippet(&lab_home, "schema-demo", body, None, false).unwrap();
    let resolved =
        resolve_snippet(&lab_home, &temp.path().join("builtins"), "schema-demo").unwrap();

    let ids: Vec<_> = resolved.tools.iter().map(|tool| tool.id.as_str()).collect();
    assert_eq!(ids, vec!["synapse2::scout", "qdrant::query"]);
}

#[test]
fn frontmatter_tools_reject_invalid_code_mode_tool_ids() {
    let body = r#"---
name: bad-tools
description: Bad tools
tags: []
tools: [synapse2__scout]
---

```js
async () => ({ ok: true })
```
"#;

    let err = validate_snippet_body("bad-tools", body)
        .expect_err("tool ids must use the Code Mode upstream::tool form");
    assert_eq!(err.kind(), "invalid_param");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p labby --lib --all-features snippets::tests::frontmatter_tools -- --nocapture
```

Expected: FAIL because `ResolvedSnippet` has no `tools` field and the parser ignores `tools:`.

- [ ] **Step 3: Add the frontmatter data model**

In `crates/lab/src/dispatch/snippets/store.rs`, add this type near `SnippetInputSpec`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetToolRef {
    pub id: String,
}
```

Add `pub tools: Vec<SnippetToolRef>,` to `SnippetInfo`, `ResolvedSnippet`, and `SnippetFrontmatter` immediately after `tags`.

- [ ] **Step 4: Populate the new field everywhere snippets are built**

In every `SnippetInfo` and `ResolvedSnippet` constructor in `store.rs`, set:

```rust
tools: metadata
    .as_ref()
    .map(|m| m.tools.clone())
    .unwrap_or_default(),
```

When `metadata` is consumed rather than borrowed, use:

```rust
tools: metadata
    .as_ref()
    .map(|m| m.tools.clone())
    .unwrap_or_default(),
```

before moving `metadata` for `inputs`.

- [ ] **Step 5: Parse `tools: [...]`**

In `frontmatter()`, add `let mut tools = Vec::new();` near `tags`.

In the top-level `match key.trim()` block, add:

```rust
"tools" => tools = parse_tool_refs(value)?,
```

Return it:

```rust
Ok(Some(SnippetFrontmatter {
    name,
    description,
    tags,
    tools,
    inputs,
}))
```

Add this helper near `parse_tags`:

```rust
fn parse_tool_refs(value: &str) -> Result<Vec<SnippetToolRef>, ToolError> {
    let value = value.trim();
    if value.is_empty() || value == "[]" {
        return Ok(Vec::new());
    }
    let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) else {
        return Err(ToolError::InvalidParam {
            message: "frontmatter `tools` must be an inline array".to_string(),
            param: "body".to_string(),
        });
    };
    inner
        .split(',')
        .map(|raw| {
            let id = raw.trim().trim_matches('"').to_string();
            crate::dispatch::gateway::code_mode::CodeModeToolId::parse(&id)?;
            Ok(SnippetToolRef { id })
        })
        .collect()
}
```

- [ ] **Step 6: Update generated user-snippet frontmatter**

In `render_user_snippet_body`, include an empty tools array:

```rust
Ok(format!(
    "---\nname: {name}\ndescription: {description}\ntags: []\ntools: []\n---\n\n```js\n{code}\n```\n"
))
```

- [ ] **Step 7: Run the store tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets::tests::frontmatter_tools -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/snippets.rs crates/lab/src/dispatch/snippets/store.rs
git commit -m "feat(snippets): parse declared tool dependencies"
```

---

### Task 2: Resolve Declared Tools Against Gateway Schemas

**Files:**
- Create: `crates/lab/src/dispatch/snippets/schema.rs`
- Modify: `crates/lab/src/dispatch/snippets.rs`

- [ ] **Step 1: Write failing schema resolver tests**

Add this test module to the bottom of the new file `crates/lab/src/dispatch/snippets/schema.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rmcp::model::Tool;
    use serde_json::json;

    use super::*;
    use crate::dispatch::gateway::manager::GatewayManager;
    use crate::dispatch::gateway::runtime::GatewayRuntimeHandle;
    use crate::dispatch::upstream::pool::UpstreamPool;
    use crate::dispatch::upstream::types::{
        ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
    };

    fn upstream_entry(upstream: &str, tool: UpstreamTool) -> UpstreamEntry {
        UpstreamEntry {
            name: Arc::from(upstream),
            tools: HashMap::from([(tool.tool.name.to_string(), tool)]),
            exposure_policy: ToolExposurePolicy::All,
            prompt_count: 0,
            resource_count: 0,
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: UpstreamHealth::Healthy,
            prompt_health: UpstreamHealth::Healthy,
            resource_health: UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
        }
    }

    async fn manager_with_tool() -> GatewayManager {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = GatewayRuntimeHandle::default();
        let pool = Arc::new(UpstreamPool::new());
        runtime.swap(Some(Arc::clone(&pool))).await;
        let manager = GatewayManager::new(dir.path().join("config.toml"), runtime);
        let schema = Arc::new(serde_json::Map::from_iter([
            ("type".to_string(), json!("object")),
            (
                "required".to_string(),
                json!(["query"]),
            ),
            (
                "properties".to_string(),
                json!({"query": {"type": "string"}}),
            ),
        ]));
        let upstream_tool = UpstreamTool {
            tool: Tool::new("search", "Search docs", schema),
            input_schema: Some(json!({
                "type": "object",
                "required": ["query"],
                "properties": {"query": {"type": "string"}}
            })),
            output_schema: Some(json!({"type": "object"})),
            upstream_name: Arc::from("axon"),
            destructive: false,
        };
        pool.insert_entry_for_tests("axon", upstream_entry("axon", upstream_tool))
            .await;
        manager
    }

    #[tokio::test]
    async fn resolves_declared_tool_schemas() {
        let manager = manager_with_tool().await;
        let refs = vec![SnippetToolRef {
            id: "axon::search".to_string(),
        }];

        let report = resolve_tool_schema_report(Some(&manager), &refs).await.unwrap();

        assert!(report.valid);
        assert_eq!(report.resolved.len(), 1);
        assert_eq!(report.resolved[0].id, "axon::search");
        assert!(report.resolved[0].input_schema.is_some());
        assert!(report.missing.is_empty());
    }

    #[tokio::test]
    async fn reports_missing_declared_tools() {
        let manager = manager_with_tool().await;
        let refs = vec![SnippetToolRef {
            id: "axon::missing".to_string(),
        }];

        let report = resolve_tool_schema_report(Some(&manager), &refs).await.unwrap();

        assert!(!report.valid);
        assert_eq!(report.missing, vec!["axon::missing"]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p labby --lib --all-features snippets::schema::tests -- --nocapture
```

Expected: FAIL because `schema.rs` is not implemented.

- [ ] **Step 3: Implement the schema report types**

Create `crates/lab/src/dispatch/snippets/schema.rs` with:

```rust
use serde::Serialize;
use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::code_mode::{CodeModeToolId, CodeModeToolRef};
use crate::dispatch::gateway::manager::GatewayManager;

use super::store::SnippetToolRef;

#[derive(Debug, Clone, Serialize)]
pub struct SnippetToolSchema {
    pub id: String,
    pub upstream: String,
    pub name: String,
    pub description: String,
    pub destructive: bool,
    pub has_input_schema: bool,
    pub has_output_schema: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SnippetToolSchemaReport {
    pub valid: bool,
    pub declared: Vec<String>,
    pub resolved: Vec<SnippetToolSchema>,
    pub missing: Vec<String>,
}
```

- [ ] **Step 4: Implement live resolution**

Add below the types:

```rust
pub async fn resolve_tool_schema_report(
    manager: Option<&GatewayManager>,
    refs: &[SnippetToolRef],
) -> Result<SnippetToolSchemaReport, ToolError> {
    let declared: Vec<String> = refs.iter().map(|tool| tool.id.clone()).collect();
    let Some(manager) = manager else {
        return Ok(SnippetToolSchemaReport {
            valid: refs.is_empty(),
            declared,
            resolved: Vec::new(),
            missing: refs.iter().map(|tool| tool.id.clone()).collect(),
        });
    };

    let mut resolved = Vec::new();
    let mut missing = Vec::new();
    for tool_ref in refs {
        let id = CodeModeToolId::parse(&tool_ref.id)?;
        let CodeModeToolRef::UpstreamTool { upstream, tool } = id.reference;
        match manager
            .resolve_code_mode_upstream_tool(&upstream, &tool, None, None)
            .await
        {
            Ok(upstream_tool) => {
                let description = upstream_tool
                    .tool
                    .description
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                let input_schema = upstream_tool.input_schema.clone();
                let output_schema = upstream_tool.output_schema.clone();
                resolved.push(SnippetToolSchema {
                    id: tool_ref.id.clone(),
                    upstream,
                    name: tool,
                    description,
                    destructive: upstream_tool.destructive,
                    has_input_schema: input_schema.is_some(),
                    has_output_schema: output_schema.is_some(),
                    input_schema,
                    output_schema,
                });
            }
            Err(_) => missing.push(tool_ref.id.clone()),
        }
    }

    Ok(SnippetToolSchemaReport {
        valid: missing.is_empty(),
        declared,
        resolved,
        missing,
    })
}
```

- [ ] **Step 5: Export the schema module**

In `crates/lab/src/dispatch/snippets.rs`, add:

```rust
pub mod schema;
```

- [ ] **Step 6: Run the schema resolver tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets::schema::tests -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/snippets.rs crates/lab/src/dispatch/snippets/schema.rs
git commit -m "feat(snippets): resolve declared tool schemas"
```

---

### Task 3: Add Schema Reports To Validate, Test, And Exec

**Files:**
- Modify: `crates/lab/src/dispatch/snippets/dispatch.rs`
- Modify: `crates/lab/src/dispatch/snippets/catalog.rs`
- Modify: `crates/lab/src/api/services/snippets.rs`

- [ ] **Step 1: Write failing dispatch tests**

Add tests to `crates/lab/src/dispatch/snippets/dispatch.rs` under the existing test module or create one if absent:

```rust
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

fn dispatch_test_upstream_entry(
    upstream: &str,
    tool: crate::dispatch::upstream::types::UpstreamTool,
) -> crate::dispatch::upstream::types::UpstreamEntry {
    crate::dispatch::upstream::types::UpstreamEntry {
        name: Arc::from(upstream),
        tools: HashMap::from([(tool.tool.name.to_string(), tool)]),
        exposure_policy: crate::dispatch::upstream::types::ToolExposurePolicy::All,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
        prompt_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
        resource_health: crate::dispatch::upstream::types::UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

async fn test_manager_with_code_mode_tool(
    upstream: &str,
    tool_name: &str,
) -> crate::dispatch::gateway::manager::GatewayManager {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = crate::dispatch::gateway::runtime::GatewayRuntimeHandle::default();
    let pool = Arc::new(crate::dispatch::upstream::pool::UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = crate::dispatch::gateway::manager::GatewayManager::new(
        dir.path().join("config.toml"),
        runtime,
    );
    let upstream_tool = crate::dispatch::upstream::types::UpstreamTool {
        tool: rmcp::model::Tool::new(
            tool_name.to_string(),
            format!("{tool_name} description"),
            Arc::new(serde_json::Map::new()),
        ),
        input_schema: Some(json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        })),
        output_schema: Some(json!({"type": "object"})),
        upstream_name: Arc::from(upstream),
        destructive: false,
    };
    pool.insert_entry_for_tests(
        upstream,
        dispatch_test_upstream_entry(upstream, upstream_tool),
    )
    .await;
    manager
}

#[tokio::test]
async fn validate_existing_snippet_reports_declared_tool_schemas() {
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("LAB_HOME", temp.path().join("lab-home"));
    let body = r#"---
name: schema-demo
description: Schema demo
tags: []
tools: [axon::search]
---

```js
async () => ({ ok: true })
```
"#;
    crate::dispatch::snippets::store::create_user_snippet(
        &crate::dispatch::helpers::lab_home(),
        "schema-demo",
        body,
        None,
        false,
    )
    .unwrap();

    let manager = test_manager_with_code_mode_tool("axon", "search").await;
    let value = dispatch_with_manager(&manager, "snippets.validate", json!({"name": "schema-demo"}))
        .await
        .unwrap();

    assert_eq!(value["valid"], true);
    assert_eq!(value["tools"]["valid"], true);
    assert_eq!(value["tools"]["resolved"][0]["id"], "axon::search");
}

#[tokio::test]
async fn exec_fails_before_code_mode_when_declared_tool_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    std::env::set_var("LAB_HOME", temp.path().join("lab-home"));
    let body = r#"---
name: missing-tool
description: Missing tool
tags: []
tools: [axon::missing]
---

```js
async () => ({ ok: true })
```
"#;
    crate::dispatch::snippets::store::create_user_snippet(
        &crate::dispatch::helpers::lab_home(),
        "missing-tool",
        body,
        None,
        false,
    )
    .unwrap();

    let manager = test_manager_with_code_mode_tool("axon", "search").await;
    let err = dispatch_with_manager(&manager, "snippets.exec", json!({"name": "missing-tool"}))
        .await
        .unwrap_err();

    assert_eq!(err.kind(), "missing_dependency");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p labby --lib --all-features snippets::dispatch -- --nocapture
```

Expected: FAIL because validation does not include `tools` and exec does not preflight dependencies.

- [ ] **Step 3: Include schema reports in `validate_snippet`**

Change `validate_snippet` in `dispatch.rs` to accept the manager:

```rust
async fn validate_snippet(
    manager: Option<&crate::dispatch::gateway::manager::GatewayManager>,
    name: Option<&str>,
    body: Option<&str>,
) -> Result<Value, ToolError>
```

For body mode, parse frontmatter and include:

```rust
let metadata = super::store::frontmatter(body)?.ok_or_else(|| ToolError::InvalidParam {
    message: "schema-aware validation requires snippet frontmatter".to_string(),
    param: "body".to_string(),
})?;
let tools = super::schema::resolve_tool_schema_report(manager, &metadata.tools).await?;
```

For existing mode, after resolving the snippet, include:

```rust
let tools = super::schema::resolve_tool_schema_report(manager, &snippet.tools).await?;
```

Return JSON with:

```rust
to_json(json!({
    "valid": tools.valid,
    "name": snippet.name,
    "mode": "existing",
    "source": snippet.source,
    "path": snippet.path,
    "tools": tools,
}))
```

- [ ] **Step 4: Preflight declared tools before execution**

In `execute_snippet`, after `let snippet = resolve_snippet(...) ?;`, add:

```rust
let tool_report = super::schema::resolve_tool_schema_report(Some(manager), &snippet.tools).await?;
if !tool_report.valid {
    return Err(ToolError::Sdk {
        sdk_kind: "missing_dependency".to_string(),
        message: format!(
            "snippet `{name}` declares unavailable tools: {}",
            tool_report.missing.join(", ")
        ),
    });
}
```

Keep the existing Code Mode broker validation in place; it still validates actual `callTool` params against each tool's input schema at call time.

- [ ] **Step 5: Update catalog descriptions**

In `crates/lab/src/dispatch/snippets/catalog.rs`, update:

```rust
description: "Validate a snippet body or existing snippet without executing it",
returns: "SnippetValidation",
```

to:

```rust
description: "Validate a snippet body or existing snippet, including declared upstream tool schemas, without executing it",
returns: "SnippetValidationWithToolSchemaReport",
```

Update `snippets.exec` description to:

```rust
description: "Validate declared upstream tool schemas, then execute a snippet through gateway Code Mode",
```

- [ ] **Step 6: Add HTTP test coverage**

In `crates/lab/src/api/services/snippets.rs`, add a test that calls the route for `snippets.validate` and asserts `tools.declared` is present:

```rust
assert_eq!(body["tools"]["declared"][0], "axon::search");
assert_eq!(body["tools"]["valid"], true);
```

- [ ] **Step 7: Run Rust tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/snippets/dispatch.rs crates/lab/src/dispatch/snippets/catalog.rs crates/lab/src/api/services/snippets.rs
git commit -m "feat(snippets): validate declared tool schemas"
```

---

### Task 4: Surface Schema Reports In The Admin UI

**Files:**
- Modify: `apps/gateway-admin/lib/types/snippets.ts`
- Modify: `apps/gateway-admin/components/snippets/snippets-page-content.tsx`
- Modify: `apps/gateway-admin/components/snippets/snippets-page-content.test.tsx`

- [ ] **Step 1: Write the failing UI test**

In `apps/gateway-admin/components/snippets/snippets-page-content.test.tsx`, update the mocked `snippets.list` item:

```ts
tools: [{ id: 'axon::search' }],
```

Update the mocked `snippets.validate` response:

```ts
tools: {
  valid: true,
  declared: ['axon::search'],
  missing: [],
  resolved: [
    {
      id: 'axon::search',
      upstream: 'axon',
      name: 'search',
      description: 'Search docs',
      destructive: false,
      has_input_schema: true,
      has_output_schema: true,
      input_schema: { type: 'object', required: ['query'] },
      output_schema: { type: 'object' },
    },
  ],
},
```

Add assertions:

```ts
assert.match(document.body.textContent ?? '', /axon::search/)
assert.match(document.body.textContent ?? '', /input schema/)
```

- [ ] **Step 2: Run the UI test to verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/snippets/snippets-page-content.test.tsx
```

Expected: FAIL because the frontend types and component do not render `tools`.

- [ ] **Step 3: Add frontend schema-report types**

In `apps/gateway-admin/lib/types/snippets.ts`, add:

```ts
export interface SnippetToolRef {
  id: string
}

export interface SnippetToolSchema {
  id: string
  upstream: string
  name: string
  description: string
  destructive: boolean
  has_input_schema: boolean
  has_output_schema: boolean
  input_schema?: unknown
  output_schema?: unknown
}

export interface SnippetToolSchemaReport {
  valid: boolean
  declared: string[]
  resolved: SnippetToolSchema[]
  missing: string[]
}
```

Add `tools?: SnippetToolRef[]` to `SnippetInfo`, and add `tools?: SnippetToolSchemaReport` to `SnippetValidation`.

- [ ] **Step 4: Render declared tools on the selected snippet**

In `apps/gateway-admin/components/snippets/snippets-page-content.tsx`, derive:

```tsx
const declaredTools = selected?.tools ?? []
const validationTools =
  actionState.kind === 'success' &&
  typeof actionState.detail === 'string' &&
  actionState.detail.includes('"tools"')
    ? null
    : null
```

Replace that temporary `validationTools` with an explicit state:

```tsx
const [toolReport, setToolReport] = React.useState<SnippetToolSchemaReport | null>(null)
```

When running validate:

```tsx
const validation = await snippetsApi.validate(selected.name)
setToolReport(validation.tools ?? null)
return validation
```

Render under the selected snippet metadata:

```tsx
<div className="space-y-2">
  <p className={AURORA_DENSE_META}>Declared tools</p>
  {declaredTools.length === 0 ? (
    <p className="text-xs text-aurora-text-muted">No declared upstream tools</p>
  ) : (
    <div className="flex flex-wrap gap-2">
      {declaredTools.map((tool) => (
        <Badge key={tool.id} variant="outline" className="border-aurora-border-default text-aurora-text-secondary">
          {tool.id}
        </Badge>
      ))}
    </div>
  )}
</div>
```

Render the validation report:

```tsx
{toolReport ? (
  <div className="rounded-aurora-1 border border-aurora-border-default bg-aurora-panel-subtle p-3">
    <div className="flex items-center justify-between gap-3">
      <p className="text-sm font-medium text-aurora-text-primary">Tool schemas</p>
      <Badge variant={toolReport.valid ? 'secondary' : 'destructive'}>
        {toolReport.valid ? 'ready' : 'missing tools'}
      </Badge>
    </div>
    <div className="mt-3 space-y-2">
      {toolReport.resolved.map((tool) => (
        <div key={tool.id} className="flex items-center justify-between gap-3 text-xs">
          <span className="font-mono text-aurora-text-secondary">{tool.id}</span>
          <span className="text-aurora-text-muted">
            {tool.has_input_schema ? 'input schema' : 'no input schema'}
            {tool.destructive ? ' / destructive' : ''}
          </span>
        </div>
      ))}
      {toolReport.missing.map((tool) => (
        <div key={tool} className="font-mono text-xs text-aurora-status-error-text">
          {tool}
        </div>
      ))}
    </div>
  </div>
) : null}
```

- [ ] **Step 5: Run targeted UI tests and lint**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/snippets/snippets-page-content.test.tsx lib/api/snippets-client.test.ts
pnpm --dir apps/gateway-admin exec eslint components/snippets/snippets-page-content.tsx components/snippets/snippets-page-content.test.tsx lib/types/snippets.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib/types/snippets.ts apps/gateway-admin/components/snippets/snippets-page-content.tsx apps/gateway-admin/components/snippets/snippets-page-content.test.tsx
git commit -m "feat(ui): show snippet tool schemas"
```

---

### Task 5: Update Built-In Snippets And Docs

**Files:**
- Modify: `docs/snippets/README.md`
- Modify: `docs/snippets/axon-fanout.md`
- Modify: `docs/snippets/cross-server-docs-brief.md`
- Modify: `docs/snippets/homelab-readonly-pulse.md`
- Modify: `docs/snippets/repo-context-triage.md`
- Modify: `docs/generated/action-catalog.json`
- Modify: `docs/generated/action-catalog.md`
- Modify: `docs/generated/api-routes.json`
- Modify: `docs/generated/api-routes.md`
- Modify: `docs/generated/cli-help.md`
- Modify: `docs/generated/mcp-help.json`
- Modify: `docs/generated/mcp-help.md`
- Modify: `docs/generated/openapi.json`
- Modify: `docs/generated/service-catalog.json`
- Modify: `docs/generated/service-catalog.md`

- [ ] **Step 1: Add `tools` declarations to built-in snippets**

For each built-in snippet, add a `tools` inline array to frontmatter. Use live ids from `labby gateway list` and Code Mode search. Examples:

```yaml
tools: [labby::gateway, synapse2::scout]
```

Use an empty array only when the snippet truly does not call upstream MCP tools:

```yaml
tools: []
```

- [ ] **Step 2: Document the contract**

In `docs/snippets/README.md`, add this section:

```markdown
## Typed Tool Declarations

Snippets may declare the upstream MCP tools they expect to call:

```yaml
tools: [synapse2::scout, qdrant::query]
```

Tool ids use the same Code Mode format accepted by `callTool`: `<upstream>::<tool>`.
`labby snippets validate <name>` resolves each id against the live gateway catalog and reports:

- whether the tool exists
- whether an input schema is available
- whether an output schema is available
- whether the upstream marks the tool destructive

`labby snippets exec <name>` performs this dependency preflight before running JavaScript. The Code Mode broker still validates every actual `callTool` payload against the upstream tool input schema at call time.
```

- [ ] **Step 3: Regenerate docs**

Run:

```bash
target/debug/labby docs generate
target/debug/labby docs check
```

Expected: both commands exit 0.

- [ ] **Step 4: Commit**

```bash
git add docs/snippets docs/generated
git commit -m "docs(snippets): document typed tool declarations"
```

---

### Task 6: Final Verification

**Files:**
- Verify all modified Rust, docs, and UI files.

- [ ] **Step 1: Format Rust**

Run:

```bash
cargo fmt --all
```

Expected: exit 0.

- [ ] **Step 2: Run Rust snippets tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run full Rust library tests**

Run:

```bash
cargo test -p labby --lib --all-features
```

Expected: PASS with the existing ignored tests unchanged.

- [ ] **Step 4: Run targeted frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test \
  components/snippets/snippets-page-content.test.tsx \
  lib/api/snippets-client.test.ts \
  components/app-sidebar.test.tsx \
  lib/app-command-palette.test.ts
```

Expected: PASS.

- [ ] **Step 5: Run targeted frontend lint**

Run:

```bash
pnpm --dir apps/gateway-admin exec eslint \
  components/snippets/snippets-page-content.tsx \
  components/snippets/snippets-page-content.test.tsx \
  lib/types/snippets.ts \
  lib/api/snippets-client.ts
```

Expected: PASS.

- [ ] **Step 6: Run TypeScript and production build**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsc --noEmit
pnpm --dir apps/gateway-admin build
```

Expected: both commands exit 0 and the Next route output includes `/snippets`.

- [ ] **Step 7: Run CLI smoke tests**

Run:

```bash
target/debug/labby snippets validate homelab-readonly-pulse --json
target/debug/labby snippets test homelab-readonly-pulse --json
```

Expected: `validate` returns `"valid": true` and a `tools` report. `test` either passes or returns a structured upstream/service error that names the failing dependency.

- [ ] **Step 8: Final commit**

```bash
git status --short
git add crates/lab/src/dispatch/snippets.rs crates/lab/src/dispatch/snippets crates/lab/src/api/services/snippets.rs crates/lab/src/cli/snippets.rs apps/gateway-admin docs/snippets docs/generated
git commit -m "feat(snippets): use upstream schemas for snippet validation"
```

---

## Self-Review

**Spec coverage:** The plan covers typed upstream MCP schemas, snippet frontmatter declarations, validation before execution, incomplete user-specific snippet inputs staying separate from upstream tool schemas, MCP/API/CLI shared dispatch ownership, and UI visibility.

**Placeholder scan:** No task depends on an unspecified component. Every new type, field, command, and response shape is named.

**Type consistency:** Frontmatter uses `tools: [upstream::tool]`; Rust uses `SnippetToolRef { id }`; validation returns `SnippetToolSchemaReport`; frontend uses the same `SnippetToolSchemaReport` field under `SnippetValidation.tools`.
