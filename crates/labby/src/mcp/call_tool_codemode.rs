//! Code Mode gateway tool branch of `call_tool`.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as inherent
//! `impl LabMcpServer` helpers. Each helper is reached only after the
//! service-name match in `call_tool_impl` and self-`return`s its result.
//! Owns the single definition of the Code Mode tool description renderer, plus
//! `string_array_arg`.
//!
//! This branch logs via `tracing` directly (not `emit_dispatch_notification`)
//! and fires `notify_catalog_changes` around the broker call.

use std::collections::BTreeSet;
use std::time::Instant;

use labby_codemode::CodeModeExecutedCall;
use labby_codemode::{MAX_SOURCE_BYTES, SERVICE as CODE_MODE_SERVICE};
use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolResult, Content, JsonObject, Meta};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::dispatch::error::ToolError as DispatchToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeCallerCapabilities, CodeModeExecutionSource,
    CodeModeHistoryEntry, CodeModeHistoryKind, ToolScope, code_mode_execute_trace,
};
use crate::mcp::context::{auth_context_from_extensions, tool_execute_scope_allowed};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::result_format::{
    estimate_tokens, estimate_tokens_args, hash_arguments, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

/// Static body for the primary `codemode` MCP tool description.
///
/// The final model-visible description is rendered with the current upstream
/// namespace snapshot by [`code_mode_description`]. Keep the rendered result
/// under 8192 bytes.
pub(crate) const CODE_MODE_DESCRIPTION_BODY: &str = "\
Execute JavaScript in a sandbox with access to the Labby gateway catalog.

## Workflow

1. Discover: `const hits = await codemode.search({ query: \"short intent phrase\", limit: 5 });`
2. Inspect: `const docs = await codemode.describe(hits.results[0].path);`
3. Call: `await codemode.<upstream>.<tool>(params)` or `await callTool(\"upstream::tool\", params);`

Never guess helper or method names. If you have not already confirmed the exact \
tool, run `codemode.search(...)` first. `codemode.search` returns compact \
signatures; `codemode.describe(\"upstream.tool\")` returns focused TypeScript \
declarations and call details.

Pass `code` as `async () => { ... }` — the sandbox awaits its return value. \
Whatever it returns becomes `result`.

```ts
async () => {
  const hits = await codemode.search({ query: 'github issues', limit: 1 });
  const docs = await codemode.describe(hits.results[0].path);
  const issues = await codemode.github.search_issues({ q: 'bug' });
  return { tool: docs.path, count: issues.items.length };
}
```

Available globals: `codemode`, `callTool`, and `writeArtifact`. There is no \
`require`, `process`, `fs`, `fetch`, Node.js, Deno, or Bun API. All external I/O \
goes through gateway tools.

Optional top-level inputs to this MCP tool:
- `upstreams`: restrict this run to specific upstream namespaces.
- `tools`: restrict this run to specific tools; accepts raw tool names or \
`upstream::tool` ids.

Every upstream MCP tool is callable two ways: `callTool(id, params)`, or the \
auto-generated `codemode.<upstream>.<tool>(params)` helper (a thin wrapper over \
the same callTool, named from the live catalog). Snippets are discoverable \
through `codemode.search` and `codemode.describe`; run them with \
`codemode.run(\"<snippet>\", input)`.

`Promise.all([...])` dispatches `callTool` requests in parallel — batch independent \
reads instead of awaiting serially.

```ts
// codemode.<upstream>.<tool>() helpers are auto-generated from the live catalog.
// Use codemode.search() / codemode.describe() for compact docs, and callTool for
// dynamic ids.
declare function callTool<T = unknown>(
  id: `${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

Successful return: the upstream tool's structuredContent if present, else the parsed \
text of the first content[0] block. Never the raw MCP envelope.

Error handling:
```ts
// To recover: const env: CodeModeError = JSON.parse(String(e.message));
// Retry-safe:    rate_limited (honor retry_after_ms), timeout, network_error
// Fix-and-retry: missing_param, invalid_param, validation_failed, confirmation_required
// Terminal:      unknown_tool, unknown_action, auth_failed, server_error, internal_error
```
A failed callTool rejects only its own promise — the run continues, so catch it and \
proceed. For catch-and-continue fan-out, prefer `Promise.allSettled` so every call \
settles before you return.

Scope: `codemode` requires `lab` or `lab:admin`.

Results are capped to the configured envelope budget (default 24 KB / 6000 tokens). \
Oversized results are replaced with a truncation marker containing `truncated`, \
`original_size`, `original_tokens`, `preview`, and `next_action`. Reduce data inside \
the sandbox before returning — that is the point of Code Mode.

Budget:
- Time: a 30 s wall-clock timeout bounds the whole run. Split work across \
calls or reduce local computation if the `timeout` kind is returned.
- Tool calls: default 512 `callTool` calls per run, configurable by the host up \
to 2048. Extra tool calls reject with `call_budget_exceeded`.
- Memory: 64 MiB heap limit enforced by the QuickJS runtime. Reduce the data \
processed inside the sandbox if the runner exits with `server_error`.
- Stack: QuickJS enforces a native stack depth limit; avoid deep recursion.
- The only recoverable budget kind is `timeout` — retry with a smaller payload \
or split into multiple `codemode` calls.

Lab actions (`lab::*` tool IDs) are not available in Code Mode. For Lab built-in \
actions, use the native Lab service tools instead of Code Mode.";

pub(crate) const CODE_MODE_DESCRIPTION_MAX_BYTES: usize = 8192;

fn code_mode_call_metrics_json(calls: &[CodeModeExecutedCall]) -> String {
    let calls = calls
        .iter()
        .map(|call| {
            let (namespace, tool) = call.id.split_once("::").unwrap_or(("", call.id.as_str()));
            serde_json::json!({
                "id": call.id,
                "namespace": namespace,
                "tool": tool,
                "ok": call.ok,
                "elapsed_ms": call.elapsed_ms,
                "error_kind": call.error_kind,
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&calls).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeModeUpstreamDescription {
    pub(crate) name: String,
    pub(crate) hint: Option<String>,
}

fn push_description_line(out: &mut String, line: &str) -> bool {
    if out.len() + line.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES {
        out.push_str(line);
        true
    } else {
        false
    }
}

fn append_truncation_marker(out: &mut String, omitted_count: usize) {
    let marker = format!("- {omitted_count} more omitted; use codemode.search\n");
    while out.len() + marker.len() > CODE_MODE_DESCRIPTION_MAX_BYTES {
        if out.pop().is_none() {
            break;
        }
    }
    out.push_str(&marker);
}

#[must_use]
pub(crate) fn code_mode_description(upstreams: &[CodeModeUpstreamDescription]) -> String {
    let mut out = format!("{CODE_MODE_DESCRIPTION_BODY}\n\n## Available upstream namespaces\n\n");
    if upstreams.is_empty() {
        push_description_line(&mut out, "- none currently configured\n");
        return out.trim_end().to_string();
    }
    for (idx, upstream) in upstreams.iter().enumerate() {
        let line = match upstream
            .hint
            .as_deref()
            .and_then(labby_runtime::gateway_config::normalize_code_mode_hint)
        {
            Some(hint) => format!("- `{}` -- {}\n", upstream.name, hint),
            None => format!("- `{}`\n", upstream.name),
        };
        if !push_description_line(&mut out, &line) {
            append_truncation_marker(&mut out, upstreams.len() - idx);
            tracing::warn!(
                omitted_count = upstreams.len() - idx,
                max_bytes = CODE_MODE_DESCRIPTION_MAX_BYTES,
                "code mode upstream namespace description truncated"
            );
            break;
        }
    }
    out.trim_end().to_string()
}

pub(crate) fn string_array_arg(
    args: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<String>, DispatchToolError> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| DispatchToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("`{key}` must be an array of strings when provided"),
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| DispatchToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: format!("`{key}` entries must be strings"),
                })
        })
        .collect()
}

pub(crate) fn code_arg(args: &JsonObject) -> Result<&str, DispatchToolError> {
    let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
    if code.trim().is_empty() {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: "code must not be empty".to_string(),
        });
    }
    if code.len() > MAX_SOURCE_BYTES {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message: format!("code exceeds max length {MAX_SOURCE_BYTES} bytes"),
        });
    }
    Ok(code)
}

fn route_scoped_capability_filter(
    args: &JsonObject,
    route_allowed: Option<&BTreeSet<String>>,
) -> Result<ToolScope, DispatchToolError> {
    let requested_upstreams = string_array_arg(args, "upstreams")?;
    if let Some(allowed) = route_allowed
        && requested_upstreams
            .iter()
            .any(|name| !allowed.contains(name))
    {
        return Err(DispatchToolError::Sdk {
            sdk_kind: "route_scope_denied".to_string(),
            message: "Code Mode requested an upstream outside this protected route scope"
                .to_string(),
        });
    }

    let tools = string_array_arg(args, "tools")?;
    let Some(allowed) = route_allowed else {
        return Ok(ToolScope::new(requested_upstreams, tools));
    };
    let filter = if requested_upstreams.is_empty() {
        ToolScope::scoped_namespaces(allowed.iter().cloned().collect(), tools)
    } else {
        ToolScope::scoped_namespaces(requested_upstreams, tools)
    };
    Ok(filter)
}

impl LabMcpServer {
    /// `codemode` gateway tool branch. Self-returns.
    pub(crate) async fn call_tool_codemode_impl(
        &self,
        service: &str,
        args: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let input_tokens = estimate_tokens_args(args);
        let subject = self.request_subject_log_tag(context);
        let actor_key = self.request_actor_key(context);
        let auth = auth_context_from_extensions(&context.extensions);
        if !tool_execute_scope_allowed(auth) {
            let err = DispatchToolError::Forbidden {
                message: "codemode requires one of scopes: lab, lab:admin".to_string(),
                required_scopes: vec!["lab".to_string(), "lab:admin".to_string()],
            };
            tracing::warn!(
                surface = "mcp",
                service = %service,
                action = "call_tool",
                subject,
                actor_key,
                actor_label = subject,
                agent_kind = "agent",
                elapsed_ms = started.elapsed().as_millis(),
                input_tokens,
                kind = "forbidden",
                "gateway codemode denied by scope"
            );
            let env = tool_error_envelope(service, "call_tool", &err);
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        let Some(manager) = &self.gateway_manager else {
            let envelope = build_error(
                service,
                "call_tool",
                "unknown_tool",
                "codemode is not enabled",
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        };
        let config = manager.code_mode_config().await;
        let code = match code_arg(args) {
            Ok(code) => code,
            Err(err) => {
                let env = build_error_extra(
                    service,
                    "call_tool",
                    err.kind(),
                    &err.to_string(),
                    &serde_json::json!({ "param": "code" }),
                );
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
        };
        let capability_filter =
            match route_scoped_capability_filter(args, self.route_scope.allowed_upstreams()) {
                Ok(filter) => filter,
                Err(err) => {
                    let env = tool_error_envelope(service, "call_tool", &err);
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
            };
        let code_hash = hash_arguments(&Value::String(code.to_string()));
        let execution_id = ulid::Ulid::new().to_string();
        let capability_filter_fingerprint = capability_filter.fingerprint();
        tracing::info!(
            surface = "mcp",
            service = CODE_MODE_SERVICE,
            code_mode_tool = %service,
            action = "call_tool",
            subject,
            actor_key,
            actor_label = subject,
            agent_kind = "agent",
            code_hash = %code_hash,
            input_tokens,
            "gateway codemode start"
        );
        let broker = CodeModeBroker::new(Some(manager.as_ref()));
        let caller = auth.map_or(CodeModeCaller::TrustedLocal, |auth| {
            CodeModeCaller::Scoped {
                capabilities: code_mode_capabilities_for_scopes(&auth.scopes),
                sub: self.request_subject(context).map(ToOwned::to_owned),
            }
        });
        let before = self.snapshot_catalog().await;
        let mut response = match broker
            .execute(
                code,
                caller,
                self.code_mode_surface(),
                config,
                capability_filter,
            )
            .await
        {
            Ok(response) => {
                let after = self.snapshot_catalog().await;
                self.notify_catalog_changes(&before, &after).await;
                response
            }
            Err(err) => {
                let after = self.snapshot_catalog().await;
                self.notify_catalog_changes(&before, &after).await;
                let calls = err.calls().to_vec();
                let code_mode_calls = code_mode_call_metrics_json(&calls);
                let error_kind = err.kind().to_string();
                let elapsed_ms = started.elapsed().as_millis();
                tracing::warn!(
                    surface = "mcp",
                    service = CODE_MODE_SERVICE,
                    code_mode_tool = %service,
                    action = "call_tool",
                    subject,
                    actor_key,
                    actor_label = subject,
                    agent_kind = "agent",
                    code_hash = %code_hash,
                    call_count = calls.len(),
                    code_mode_calls = %code_mode_calls,
                    elapsed_ms,
                    input_tokens,
                    output_tokens = 0,
                    kind = error_kind.as_str(),
                    "gateway codemode failed"
                );
                let tool_error = err.into_tool_error();
                manager
                    .record_code_mode_history(CodeModeHistoryEntry {
                        execution_id: Some(execution_id.clone()),
                        seq: 0,
                        route_scope: self.route_scope.label(),
                        kind: CodeModeHistoryKind::Execute,
                        ok: false,
                        elapsed_ms,
                        input_tokens: Some(input_tokens),
                        output_tokens: Some(0),
                        error_kind: Some(error_kind),
                        calls,
                        match_count: None,
                    })
                    .await;
                let env = tool_error_envelope(service, "call_tool", &tool_error);
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
        };
        response.execution_id = Some(execution_id.clone());
        // Mirror the upstream's `_meta.ui` verbatim onto the codemode result so
        // the host renders the native mcp-ui widget (last-wins). The widget
        // itself is driven by the `ui://` resource read, not by inline content,
        // so the Code Mode trace content is left intact.
        let ui_meta = response.ui.as_ref().map(|ui| {
            let mut map = serde_json::Map::new();
            map.insert("ui".to_string(), ui.ui_meta.clone());
            Meta(map)
        });
        let mirrored_resource_uri = response.ui.as_ref().and_then(|ui| {
            ui.ui_meta
                .get("resourceUri")
                .and_then(|value| value.as_str())
        });
        if response.ui.is_some() {
            tracing::info!(
                surface = "mcp",
                service = CODE_MODE_SERVICE,
                code_mode_tool = %service,
                action = "mcp_app.mirror",
                subject,
                actor_key,
                actor_label = subject,
                agent_kind = "agent",
                resource_uri = mirrored_resource_uri.unwrap_or("<unknown>"),
                "mirroring upstream MCP App widget metadata onto codemode result"
            );
        }
        let output = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
        let output_tokens = estimate_tokens(&output);
        manager
            .record_code_mode_history(CodeModeHistoryEntry {
                execution_id: Some(execution_id.clone()),
                seq: 0,
                route_scope: self.route_scope.label(),
                kind: CodeModeHistoryKind::Execute,
                ok: true,
                elapsed_ms: started.elapsed().as_millis(),
                input_tokens: Some(input_tokens),
                output_tokens: Some(output_tokens),
                error_kind: None,
                calls: response.calls.clone(),
                match_count: None,
            })
            .await;
        let is_admin = auth.is_none_or(|auth| auth.scopes.iter().any(|scope| scope == "lab:admin"));
        if is_admin && code.len() <= MAX_SOURCE_BYTES {
            manager
                .record_code_mode_source(CodeModeExecutionSource {
                    execution_id: execution_id.clone(),
                    created_at_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as i64)
                        .unwrap_or_default(),
                    actor_key: actor_key.map(ToOwned::to_owned),
                    is_admin,
                    route_scope: self.route_scope.label(),
                    surface: self.code_mode_surface(),
                    capability_filter_fingerprint,
                    code: code.to_string(),
                })
                .await;
        }
        let mut structured = code_mode_execute_trace(&response);
        if let Some(object) = structured.as_object_mut() {
            object.insert(
                "execution_id".to_string(),
                Value::String(execution_id.clone()),
            );
            object.insert("input_tokens".to_string(), Value::from(input_tokens as u64));
            object.insert(
                "output_tokens".to_string(),
                Value::from(output_tokens as u64),
            );
        }
        let trace_result_type = structured
            .get("result_shape")
            .and_then(|shape| shape.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let trace_has_result = structured.get("result").is_some();
        let shape_truncated = response
            .result_shaping
            .as_ref()
            .map(|shape| shape.truncated)
            .unwrap_or(false);
        let legacy_truncated = response
            .result
            .as_ref()
            .and_then(|result| result.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let truncated = shape_truncated || legacy_truncated;
        let result_shape_policy = response
            .result_shaping
            .as_ref()
            .and_then(|shape| serde_json::to_value(shape.policy).ok())
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "legacy".to_string());
        tracing::info!(
            surface = "mcp",
            service = CODE_MODE_SERVICE,
            code_mode_tool = %service,
            action = "call_tool",
            subject,
            actor_key,
            actor_label = subject,
            agent_kind = "agent",
            code_hash = %code_hash,
            call_count = response.calls.len(),
            code_mode_calls = %code_mode_call_metrics_json(&response.calls),
            artifact_writes = response.artifacts.len(),
            truncated,
            result_shape_policy,
            elapsed_ms = started.elapsed().as_millis(),
            input_tokens,
            output_tokens,
            trace_has_result,
            trace_result_type,
            mirrored_ui_resource_uri = mirrored_resource_uri.unwrap_or("<none>"),
            "gateway codemode ok"
        );
        Ok(call_result_with_structured(output, structured, ui_meta))
    }
}

fn code_mode_capabilities_for_scopes(scopes: &[String]) -> CodeModeCallerCapabilities {
    let is_admin = scopes.iter().any(|scope| scope == "lab:admin");
    CodeModeCallerCapabilities {
        can_execute: scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin")),
        can_use_snippets: is_admin,
        is_admin,
    }
}

fn call_result_with_structured(
    text: String,
    structured: Value,
    ui_meta: Option<Meta>,
) -> CallToolResult {
    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(structured);
    result.meta = ui_meta;
    result
}

#[cfg(test)]
mod tests;
