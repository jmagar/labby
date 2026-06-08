//! `list_tools` handler body + gateway meta-tool input-schema construction.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.4`) as an inherent
//! `impl LabMcpServer` method. The `ServerHandler` trait impl in
//! `server.rs` keeps a one-line delegator.
//!
//! `CODE_EXECUTE_DESCRIPTION` has exactly one definition (in `server.rs`,
//! `pub(crate)`); this module imports it. No behavior change — relocation
//! only.

use std::sync::Arc;
use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{ListToolsResult, Meta, PaginatedRequestParams, Tool};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::mcp::call_tool_codemode::CODE_EXECUTE_DESCRIPTION;
use crate::mcp::catalog::{CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME};
use crate::mcp::completion::action_schema;
use crate::mcp::context::{auth_context_from_extensions, oauth_upstream_subject_for_request};
use crate::mcp::handlers_resources::code_mode_app_resource_uri_for_tool;
use crate::mcp::logging::DispatchLogOutcome;
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    pub(crate) async fn list_tools_impl(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            "dispatch start"
        );
        let schema = Arc::new(action_schema());
        let mut tools = Vec::new();
        let mut builtin_tool_count = 0usize;
        let mut upstream_tool_count = 0usize;
        let mut subject_scoped_tool_count = 0usize;
        let mut gateway_tool_count = 0usize;
        let mut suppressed_builtin_tool_count = 0usize;
        let visibility = self.code_mode_visibility().await;
        let manager_code_mode_enabled = visibility.exposes_synthetic_tools();
        let process_code_mode_enabled = crate::config::process_code_mode_enabled();
        let hide_raw_tools = visibility.hides_raw_tools();
        let visibility_mode = visibility.mode_label();
        for svc in self.registry.services() {
            if self.service_visible_on_mcp(svc.name).await {
                if hide_raw_tools {
                    suppressed_builtin_tool_count += 1;
                } else {
                    tools.push(Tool::new(svc.name, svc.description, Arc::clone(&schema)));
                    builtin_tool_count += 1;
                }
            }
        }
        if visibility.exposes_synthetic_tools() {
            // ── Gateway meta-tools: search (Boa JS against upstream catalog) +
            // execute (subprocess sandbox). Both take `{ code: string }`.
            // See mcp/CLAUDE.md for the exception rationale and
            // dispatch/gateway/dispatch.rs guard.
            let search_schema = match serde_json::json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript async arrow function to search the upstream MCP tool catalog. \
                            The sandbox injects `const tools = [...]` where each entry has id, upstream, \
                            name, description, schema, output_schema, signature, and dts. Return JSON-serializable results. \
                            Examples: \
                            `async () => tools.filter(t => /container.*log/i.test(t.description)).map(t => ({id:t.id, signature:t.signature, dts:t.dts}))`; \
                            `async () => tools.find(t => t.id === \"github::search_issues\")`; \
                            `async () => tools.filter(t => t.upstream === \"github\").slice(0, 20)`."
                    }
                },
                "required": ["code"]
            }) {
                Value::Object(map) => Arc::new(map),
                _ => unreachable!("search schema must be an object"),
            };
            let trace_output_schema = code_mode_trace_output_schema();
            tools.push(Tool::new(
                CODE_MODE_SEARCH_TOOL_NAME,
                "Filter the upstream MCP tool catalog with JavaScript. Write an async arrow function \
                that filters `const tools = [...]` (each entry: id, upstream, name, description, schema, output_schema, signature, dts) \
                and returns what you need. Use before execute() to discover the right tool id.",
                search_schema,
            ).with_raw_output_schema(Arc::clone(&trace_output_schema))
                .with_meta(code_mode_tool_meta(CODE_MODE_SEARCH_TOOL_NAME)));
            gateway_tool_count += 1;
            let execute_schema = match serde_json::json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "JavaScript async arrow function to execute. Use await callTool(id, params) with JSON-serializable params."
                    },
                    "upstreams": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional upstream allowlist for this execution."
                    },
                    "tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tool allowlist for this execution. Accepts raw tool names or <name>::<tool> ids."
                    }
                },
                "required": ["code"]
            }) {
                Value::Object(map) => Arc::new(map),
                _ => unreachable!("execute schema must be an object"),
            };
            debug_assert!(CODE_EXECUTE_DESCRIPTION.len() < 8192);
            tracing::info!(
                surface = "mcp",
                service = "execute",
                action = "tool.describe",
                description_bytes = CODE_EXECUTE_DESCRIPTION.len(),
                "registered Code Mode execute description"
            );
            tools.push(
                Tool::new(
                    TOOL_EXECUTE_TOOL_NAME,
                    CODE_EXECUTE_DESCRIPTION,
                    execute_schema,
                )
                .with_raw_output_schema(Arc::clone(&trace_output_schema))
                .with_meta(code_mode_tool_meta(TOOL_EXECUTE_TOOL_NAME)),
            );
            gateway_tool_count += 1;
        }

        // Merge upstream tools (healthy only, filtered for collisions with built-in services).
        if !hide_raw_tools && let Some(pool) = self.current_upstream_pool().await {
            let mut builtin_names = Vec::new();
            for service in self.registry.services() {
                if self.service_visible_on_mcp(service.name).await {
                    builtin_names.push(service.name);
                }
            }
            let upstream_tools = pool.healthy_tools().await;
            for ut in upstream_tools {
                let tool_name = ut.tool.name.as_ref();
                if builtin_names.contains(&tool_name) {
                    tracing::debug!(
                        surface = "mcp",
                        service = "labby",
                        action = "tool.register",
                        tool = tool_name,
                        "skipping upstream tool that collides with built-in service"
                    );
                    continue;
                }
                tools.push(ut.tool);
                upstream_tool_count += 1;
            }
            let auth = auth_context_from_extensions(&context.extensions);
            if let Some(oauth_subject) =
                oauth_upstream_subject_for_request(auth, self.request_subject(&context))
            {
                for (_upstream_name, upstream_tools) in pool
                    .subject_scoped_tools(
                        &self.oauth_upstream_configs().await,
                        oauth_subject.as_ref(),
                    )
                    .await
                {
                    for ut in upstream_tools {
                        let tool_name = ut.name.as_ref();
                        if builtin_names.contains(&tool_name)
                            || tools.iter().any(|tool| tool.name.as_ref() == tool_name)
                        {
                            continue;
                        }
                        tools.push(ut);
                        subject_scoped_tool_count += 1;
                    }
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "list_tools",
            subject,
            elapsed_ms,
            builtin_tool_count,
            gateway_tool_count,
            upstream_tool_count,
            subject_scoped_tool_count,
            suppressed_builtin_tool_count,
            manager_code_mode_enabled,
            process_code_mode_enabled,
            hide_raw_tools,
            visibility_mode,
            total_tool_count = tools.len(),
            "tool list ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "list_tools",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(ListToolsResult::with_all_items(tools))
    }
}

fn code_mode_tool_meta(tool_name: &str) -> Meta {
    let resource_uri = code_mode_app_resource_uri_for_tool(tool_name)
        .expect("Code Mode tools must have an associated UI resource");
    let mut meta = serde_json::Map::new();
    meta.insert(
        "ui".to_string(),
        serde_json::json!({
            "resourceUri": resource_uri,
        }),
    );
    Meta(meta)
}

fn code_mode_trace_output_schema() -> Arc<serde_json::Map<String, Value>> {
    match serde_json::json!({
        "type": "object",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "code_mode_search_trace" },
                    "query_kind": { "type": "string" },
                    "elapsed_ms": { "type": "integer", "minimum": 0 },
                    "match_count": { "type": "integer", "minimum": 0 },
                    "displayed_count": { "type": "integer", "minimum": 0 },
                    "truncated": { "type": "boolean" },
                    "matches": { "type": "array", "items": { "type": "object" } },
                    "result_shape": { "type": "object" }
                },
                "required": ["kind", "query_kind", "elapsed_ms", "match_count", "displayed_count", "truncated", "matches", "result_shape"],
                "additionalProperties": true
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "code_mode_execute_trace" },
                    "call_count": { "type": "integer", "minimum": 0 },
                    "calls": { "type": "array", "items": { "type": "object" } },
                    "result_shape": { "type": "object" },
                    "logs_count": { "type": "integer", "minimum": 0 }
                },
                "required": ["kind", "call_count", "calls", "result_shape", "logs_count"],
                "additionalProperties": true
            }
        ]
    }) {
        Value::Object(map) => Arc::new(map),
        _ => unreachable!("trace output schema must be an object"),
    }
}

#[cfg(test)]
mod tests;
