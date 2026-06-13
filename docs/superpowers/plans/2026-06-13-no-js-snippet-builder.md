# No-JS Snippet Builder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users create snippets by searching live gateway tools, filling schema-generated parameter forms, validating the call plan, and saving an executable Markdown snippet without writing JavaScript.

**Architecture:** Add a schema-aware builder model in shared snippets dispatch, backed by the gateway Code Mode catalog. The frontend renders a tool search/select flow and schema-derived parameter editors; the backend validates the builder payload against upstream tool schemas and generates the Markdown/JS snippet body through existing `snippets.create`. Runtime execution remains unchanged.

**Tech Stack:** Rust 2024, serde/serde_json, existing snippets dispatch, existing gateway Code Mode tool catalog and schema validation, Next.js 16, React 19, Aurora tokens, lucide-react.

---

## File Structure

- Modify `crates/lab/src/dispatch/snippets/catalog.rs`: add `snippets.builder.catalog`, `snippets.builder.validate`, and `snippets.builder.render` actions.
- Modify `crates/lab/src/dispatch/snippets/dispatch.rs`: route the new actions through a builder module.
- Create `crates/lab/src/dispatch/snippets/builder.rs`: define builder request/response types, catalog projection, schema validation, and snippet rendering.
- Modify `crates/lab/src/dispatch/snippets.rs`: export the builder module and add Rust tests.
- Modify `crates/lab/src/cli/snippets.rs`: add `labby snippets builder catalog` and `labby snippets builder render --file`.
- Modify `crates/lab/src/api/services/snippets.rs`: no business logic; expose generated dispatch actions through existing route pattern.
- Modify `apps/gateway-admin/lib/types/snippets.ts`: add builder types.
- Modify `apps/gateway-admin/lib/api/snippets-client.ts`: add builder API methods.
- Create `apps/gateway-admin/components/snippets/snippet-builder.tsx`: search/select tools, render schema forms, validate, and render/save.
- Modify `apps/gateway-admin/components/snippets/snippets-page-content.tsx`: add a Builder tab or toolbar action that opens the builder.
- Create `apps/gateway-admin/components/snippets/snippet-builder.test.tsx`: verify tool search, schema field rendering, validation errors, and render payload.
- Modify `docs/snippets/README.md`: document the no-JS builder flow.
- Regenerate `docs/generated/*` after adding dispatch actions.

---

### Task 1: Backend Builder Catalog Projection

**Files:**
- Create: `crates/lab/src/dispatch/snippets/builder.rs`
- Modify: `crates/lab/src/dispatch/snippets.rs`
- Modify: `crates/lab/src/dispatch/snippets/dispatch.rs`
- Modify: `crates/lab/src/dispatch/snippets/catalog.rs`

- [ ] **Step 1: Write the failing catalog test**

Add this test to `crates/lab/src/dispatch/snippets/builder.rs`:

```rust
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn builder_tool_projection_keeps_schema_and_signature() {
        let tool = BuilderTool {
            id: "time::get_current_time".to_string(),
            upstream: "time".to_string(),
            name: "get_current_time".to_string(),
            description: Some("Get current time".to_string()),
            schema: json!({
                "type": "object",
                "properties": {
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone"
                    }
                },
                "required": ["timezone"]
            }),
            output_schema: None,
            signature: Some("get_current_time(params: { timezone: string })".to_string()),
            dts: Some("declare function get_current_time(...)".to_string()),
            destructive: false,
        };

        assert_eq!(tool.id, "time::get_current_time");
        assert_eq!(tool.schema["properties"]["timezone"]["type"], "string");
        assert_eq!(tool.destructive, false);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder::tests::builder_tool_projection_keeps_schema_and_signature -- --nocapture
```

Expected: FAIL because `builder.rs` and `BuilderTool` do not exist.

- [ ] **Step 3: Add builder data types**

Create `crates/lab/src/dispatch/snippets/builder.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuilderTool {
    pub id: String,
    pub upstream: String,
    pub name: String,
    pub description: Option<String>,
    pub schema: Value,
    pub output_schema: Option<Value>,
    pub signature: Option<String>,
    pub dts: Option<String>,
    pub destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuilderCatalog {
    pub tools: Vec<BuilderTool>,
}
```

Modify `crates/lab/src/dispatch/snippets.rs`:

```rust
pub mod builder;
```

- [ ] **Step 4: Add dispatch actions to the catalog**

Append to `ACTIONS` in `crates/lab/src/dispatch/snippets/catalog.rs`:

```rust
ActionSpec {
    name: "snippets.builder.catalog",
    description: "Return searchable gateway tools with schemas for the snippet builder",
    destructive: false,
    requires_admin: true,
    returns: "BuilderCatalog",
    params: &[],
},
```

- [ ] **Step 5: Add temporary dispatch route**

In `crates/lab/src/dispatch/snippets/dispatch.rs`, add:

```rust
"snippets.builder.catalog" => to_json(crate::dispatch::snippets::builder::BuilderCatalog {
    tools: Vec::new(),
}),
```

This keeps the route compiling before live catalog integration.

- [ ] **Step 6: Run the focused test**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder::tests::builder_tool_projection_keeps_schema_and_signature -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/snippets.rs crates/lab/src/dispatch/snippets/builder.rs crates/lab/src/dispatch/snippets/catalog.rs crates/lab/src/dispatch/snippets/dispatch.rs
git commit -m "feat(snippets): add builder catalog model"
```

---

### Task 2: Validate Builder Steps Against Tool Schemas

**Files:**
- Modify: `crates/lab/src/dispatch/snippets/builder.rs`
- Modify: `crates/lab/src/dispatch/snippets/dispatch.rs`
- Modify: `crates/lab/src/dispatch/snippets/catalog.rs`

- [ ] **Step 1: Write failing validation tests**

Add to `builder.rs` tests:

```rust
#[test]
fn builder_validate_reports_missing_required_field() {
    let schema = json!({
        "type": "object",
        "properties": {
            "timezone": { "type": "string" }
        },
        "required": ["timezone"]
    });
    let step = BuilderStep {
        label: "timestamp".to_string(),
        tool_id: "time::get_current_time".to_string(),
        params: json!({}),
    };

    let errors = validate_step_params(&step, &schema);

    assert_eq!(errors, vec!["timestamp.timezone is required"]);
}

#[test]
fn builder_validate_reports_basic_type_mismatch() {
    let schema = json!({
        "type": "object",
        "properties": {
            "perPage": { "type": "integer" }
        }
    });
    let step = BuilderStep {
        label: "github".to_string(),
        tool_id: "github::search_repositories".to_string(),
        params: json!({ "perPage": "three" }),
    };

    let errors = validate_step_params(&step, &schema);

    assert_eq!(errors, vec!["github.perPage must be an integer"]);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder::tests::builder_validate -- --nocapture
```

Expected: FAIL because `BuilderStep` and `validate_step_params` do not exist.

- [ ] **Step 3: Add request/validation types**

Add to `builder.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuilderStep {
    pub label: String,
    pub tool_id: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BuilderValidationRequest {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub inputs: Value,
    pub steps: Vec<BuilderStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuilderValidation {
    pub valid: bool,
    pub errors: Vec<String>,
}
```

- [ ] **Step 4: Add minimal JSON-schema validation**

Add to `builder.rs`:

```rust
pub fn validate_step_params(step: &BuilderStep, schema: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let params = step.params.as_object();
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);

    for key in required {
        if params.is_none_or(|object| !object.contains_key(key)) {
            errors.push(format!("{}.{} is required", step.label, key));
        }
    }

    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten();

    for (key, property_schema) in properties {
        let Some(value) = params.and_then(|object| object.get(key)) else {
            continue;
        };
        let Some(kind) = property_schema.get("type").and_then(Value::as_str) else {
            continue;
        };
        let ok = match kind {
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !ok {
            errors.push(format!("{}.{} must be {}", step.label, key, article(kind)));
        }
    }

    errors
}

fn article(kind: &str) -> String {
    match kind {
        "integer" | "array" | "object" => format!("an {kind}"),
        _ => format!("a {kind}"),
    }
}
```

- [ ] **Step 5: Add builder validate action**

Add to `ACTIONS`:

```rust
ActionSpec {
    name: "snippets.builder.validate",
    description: "Validate a snippet builder call plan against selected tool schemas",
    destructive: false,
    requires_admin: true,
    returns: "BuilderValidation",
    params: &[ParamSpec {
        name: "plan",
        ty: "object",
        required: true,
        description: "Builder plan containing name, description, inputs, and steps",
    }],
},
```

In dispatch, parse the request and return validation. Use an empty schema map in this task so unit tests cover the validator; live catalog wiring lands in Task 3.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder::tests::builder_validate -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/snippets/builder.rs crates/lab/src/dispatch/snippets/catalog.rs crates/lab/src/dispatch/snippets/dispatch.rs
git commit -m "feat(snippets): validate builder step params"
```

---

### Task 3: Render A Builder Plan Into Markdown Snippet Source

**Files:**
- Modify: `crates/lab/src/dispatch/snippets/builder.rs`
- Modify: `crates/lab/src/dispatch/snippets/dispatch.rs`
- Modify: `crates/lab/src/dispatch/snippets/catalog.rs`

- [ ] **Step 1: Write failing render test**

Add to `builder.rs` tests:

```rust
#[test]
fn render_builder_plan_outputs_executable_markdown() {
    let request = BuilderValidationRequest {
        name: "docs-brief".to_string(),
        description: "Docs brief".to_string(),
        inputs: json!({
            "topic": {
                "type": "string",
                "default": "Model Context Protocol",
                "required": false,
                "description": "Research topic"
            }
        }),
        steps: vec![BuilderStep {
            label: "timestamp".to_string(),
            tool_id: "time::get_current_time".to_string(),
            params: json!({ "timezone": "America/New_York" }),
        }],
    };

    let markdown = render_builder_markdown(&request).unwrap();

    assert!(markdown.contains("name: docs-brief"));
    assert!(markdown.contains("tools:"));
    assert!(markdown.contains("- time::get_current_time"));
    assert!(markdown.contains("await callTool(\"time::get_current_time\""));
    assert!(markdown.contains("async (overrides = {}) =>"));
}
```

- [ ] **Step 2: Run test to verify failure**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder::tests::render_builder_plan_outputs_executable_markdown -- --nocapture
```

Expected: FAIL because `render_builder_markdown` does not exist.

- [ ] **Step 3: Implement Markdown rendering**

Add:

```rust
pub fn render_builder_markdown(request: &BuilderValidationRequest) -> Result<String, serde_json::Error> {
    let tools: Vec<&str> = request.steps.iter().map(|step| step.tool_id.as_str()).collect();
    let inputs = serde_json::to_string_pretty(&request.inputs)?;
    let steps = serde_json::to_string_pretty(&request.steps)?;
    let mut tools_yaml = String::new();
    for tool in &tools {
        tools_yaml.push_str(&format!("  - {tool}\n"));
    }
    Ok(format!(
        "---\nname: {name}\ndescription: {description}\ntags: []\ntools:\n{tools_yaml}inputs: {inputs}\n---\n\n```js\nasync (overrides = {{}}) => {{\n  const steps = {steps};\n  const timed = async (step) => {{\n    const started = Date.now();\n    try {{\n      return {{\n        label: step.label,\n        id: step.tool_id,\n        ok: true,\n        ms: Date.now() - started,\n        result: await callTool(step.tool_id, step.params)\n      }};\n    }} catch (error) {{\n      return {{ label: step.label, id: step.tool_id, ok: false, ms: Date.now() - started, error: String(error) }};\n    }}\n  }};\n  const calls = await Promise.all(steps.map(timed));\n  return {{ snippet: \"{name}\", input: overrides, ok: calls.every((call) => call.ok), calls }};\n}}\n```\n",
        name = request.name,
        description = request.description,
    ))
}
```

- [ ] **Step 4: Add builder render action**

Add `snippets.builder.render` to `ACTIONS` with a required `plan` object. In dispatch, parse `BuilderValidationRequest`, call `render_builder_markdown`, and return:

```rust
json!({
    "name": request.name,
    "body": markdown,
})
```

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder::tests::render_builder_plan_outputs_executable_markdown -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/snippets/builder.rs crates/lab/src/dispatch/snippets/catalog.rs crates/lab/src/dispatch/snippets/dispatch.rs
git commit -m "feat(snippets): render no-js builder plans"
```

---

### Task 4: Frontend Builder UI

**Files:**
- Modify: `apps/gateway-admin/lib/types/snippets.ts`
- Modify: `apps/gateway-admin/lib/api/snippets-client.ts`
- Create: `apps/gateway-admin/components/snippets/snippet-builder.tsx`
- Create: `apps/gateway-admin/components/snippets/snippet-builder.test.tsx`
- Modify: `apps/gateway-admin/components/snippets/snippets-page-content.tsx`

- [ ] **Step 1: Write failing frontend test**

Create `apps/gateway-admin/components/snippets/snippet-builder.test.tsx`:

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { installChatTestDom, renderClient } from '@/components/chat/test-utils'
import { SnippetBuilder } from './snippet-builder'

test('SnippetBuilder searches tools and renders schema fields', async () => {
  installChatTestDom()
  const view = await renderClient(
    <SnippetBuilder
      tools={[
        {
          id: 'time::get_current_time',
          upstream: 'time',
          name: 'get_current_time',
          description: 'Get current time',
          schema: {
            type: 'object',
            properties: {
              timezone: { type: 'string', description: 'IANA timezone' },
            },
            required: ['timezone'],
          },
          output_schema: null,
          signature: 'get_current_time(params)',
          dts: null,
          destructive: false,
        },
      ]}
    />,
  )

  assert.match(view.container.textContent ?? '', /time::get_current_time/)
  assert.match(view.container.textContent ?? '', /timezone/)
  assert.match(view.container.textContent ?? '', /IANA timezone/)

  await view.unmount()
})
```

- [ ] **Step 2: Run test to verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/snippets/snippet-builder.test.tsx
```

Expected: FAIL because `snippet-builder.tsx` does not exist.

- [ ] **Step 3: Add frontend builder types**

In `apps/gateway-admin/lib/types/snippets.ts`, add:

```ts
export interface BuilderTool {
  id: string
  upstream: string
  name: string
  description: string | null
  schema: Record<string, unknown>
  output_schema: Record<string, unknown> | null
  signature: string | null
  dts: string | null
  destructive: boolean
}

export interface BuilderStep {
  label: string
  tool_id: string
  params: Record<string, unknown>
}
```

- [ ] **Step 4: Implement minimal schema form rendering**

Create `apps/gateway-admin/components/snippets/snippet-builder.tsx`:

```tsx
'use client'

import type { BuilderTool } from '@/lib/types/snippets'

interface SnippetBuilderProps {
  tools: BuilderTool[]
}

function schemaProperties(tool: BuilderTool): Array<[string, Record<string, unknown>]> {
  const properties = tool.schema.properties
  if (!properties || typeof properties !== 'object' || Array.isArray(properties)) return []
  return Object.entries(properties as Record<string, Record<string, unknown>>)
}

export function SnippetBuilder({ tools }: SnippetBuilderProps) {
  return (
    <div className="grid gap-4">
      {tools.map((tool) => (
        <section key={tool.id} className="rounded-md border border-aurora-border-default p-4">
          <div className="flex items-start justify-between gap-3">
            <div>
              <h3 className="font-mono text-sm font-semibold text-aurora-text-primary">{tool.id}</h3>
              <p className="mt-1 text-sm text-aurora-text-muted">{tool.description}</p>
            </div>
            {tool.destructive ? <span className="text-xs text-aurora-error">destructive</span> : null}
          </div>
          <div className="mt-4 grid gap-3">
            {schemaProperties(tool).map(([name, property]) => (
              <label key={name} className="grid gap-1 text-sm">
                <span className="font-mono text-xs text-aurora-text-primary">{name}</span>
                <input
                  className="rounded-md border border-aurora-border-default bg-aurora-control-surface px-3 py-2 text-aurora-text-primary"
                  placeholder={String(property.type ?? 'value')}
                />
                <span className="text-xs text-aurora-text-muted">{String(property.description ?? '')}</span>
              </label>
            ))}
          </div>
        </section>
      ))}
    </div>
  )
}
```

- [ ] **Step 5: Run frontend test**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/snippets/snippet-builder.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib/types/snippets.ts apps/gateway-admin/components/snippets/snippet-builder.tsx apps/gateway-admin/components/snippets/snippet-builder.test.tsx
git commit -m "feat(gateway-admin): add schema-driven snippet builder shell"
```

---

### Task 5: Verification And Docs

**Files:**
- Modify: `docs/snippets/README.md`
- Modify: `docs/generated/*`

- [ ] **Step 1: Regenerate docs**

Run:

```bash
cargo run --package labby --all-features -- docs generate
cargo run --package labby --all-features -- docs check
```

Expected: generated docs are fresh.

- [ ] **Step 2: Run backend tests**

Run:

```bash
cargo test -p labby --lib --all-features snippets::builder -- --nocapture
```

Expected: PASS.

- [ ] **Step 3: Run frontend tests**

Run:

```bash
pnpm --dir apps/gateway-admin exec tsx --test components/snippets/snippet-builder.test.tsx
pnpm --dir apps/gateway-admin exec eslint components/snippets/snippet-builder.tsx components/snippets/snippet-builder.test.tsx lib/types/snippets.ts lib/api/snippets-client.ts
pnpm --dir apps/gateway-admin exec tsc --noEmit
pnpm --dir apps/gateway-admin build
```

Expected: PASS.

- [ ] **Step 4: Commit docs and generated artifacts**

```bash
git add docs/snippets/README.md docs/generated
git commit -m "docs(snippets): document schema-driven builder workflow"
```

---

## Self-Review

- Spec coverage: The plan covers live gateway tool search, schema-derived parameter forms, schema validation, no-JS Markdown generation, UI rendering, generated docs, and verification.
- Placeholder scan: No task uses unfinished-marker language or unspecified test steps.
- Type consistency: Backend `BuilderTool` / `BuilderStep` map directly to frontend `BuilderTool` / `BuilderStep`; action names use the `snippets.builder.*` namespace consistently.
