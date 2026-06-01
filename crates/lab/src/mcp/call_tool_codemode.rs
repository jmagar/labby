//! Code Mode gateway meta-tool branches of `call_tool`: `search` (Boa JS
//! over the upstream catalog) and `execute` (subprocess sandbox).
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as inherent
//! `impl LabMcpServer` helpers. Each helper is reached only after the
//! service-name match in `call_tool_impl` and self-`return`s its result.
//! Owns the single definitions of `CODE_EXECUTE_DESCRIPTION` and
//! `CODE_MODE_MAX_CODE_BYTES`, plus `string_array_arg`.
//!
//! These branches log via `tracing` directly (not
//! `emit_dispatch_notification`) and the `execute` branch fires
//! `notify_catalog_changes` around the broker call — preserved
//! byte-identically. No behavior change.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolResult, Content, JsonObject};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::dispatch::error::ToolError as DispatchToolError;
use crate::dispatch::gateway::code_mode::{
    CodeModeBroker, CodeModeCaller, CodeModeCapabilityFilter,
};
use crate::mcp::context::{
    auth_context_from_extensions, tool_execute_scope_allowed, tool_search_scope_allowed,
};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::result_format::{
    estimate_tokens, estimate_tokens_args, hash_arguments, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

pub(crate) const CODE_MODE_MAX_CODE_BYTES: usize = 20_000;
/// Tool description for the `execute` MCP tool (Code Mode sandbox).
///
/// This description is what the model receives. Keep it under 8192 bytes.
pub(crate) const CODE_EXECUTE_DESCRIPTION: &str = "\
Execute a JavaScript async arrow function in the Code Mode sandbox. Pass `code` as \
`async () => { ... }` — the sandbox awaits its return value (same shape as search). \
Discover tool ids and TypeScript signatures with `search` FIRST — search entries \
include `schema`, `output_schema`, `signature`, and `dts`. Every upstream MCP tool is \
then callable two ways: `callTool(id, params)`, or the auto-generated \
`codemode.<upstream>.<tool>(params)` helper (a thin wrapper over the same callTool, \
named from the live catalog — handy once `search` has told you the id).

```ts
// code is an async arrow function; whatever it returns becomes `result`.
async () => {
  const issues = await callTool('upstream::github::search_issues', { q: 'bug' });
  return issues.items.length;
}
```

`Promise.all([...])` dispatches `callTool` requests in parallel — batch independent \
reads instead of awaiting serially.

```ts
// codemode.<upstream>.<tool>() helpers are auto-generated from the live catalog and
// match the signatures returned by search.dts. callTool is the direct form and the
// escape hatch for dynamic ids.
declare function callTool<T = unknown>(
  id: `upstream::${string}::${string}`,
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

Scope: `lab:read` — catalog read only. `lab` / `lab:admin` — callTool execution.

Results are capped to the configured envelope budget (default 24 KB / 6000 tokens). \
Oversized results are replaced with a truncation marker containing `truncated`, \
`original_size`, `original_tokens`, `preview`, and `next_action`. Reduce data inside \
the sandbox before returning — that is the point of Code Mode.

Budget:
- Time: a 30s wall-clock timeout bounds the whole run (the meaningful limit). \
There is no small per-run call cap — fan out freely with `Promise.all`.
- Fuel: default 50M fuel supports heavy fan-out plus moderate result processing; \
base overhead ~100K, ~2K per callTool boundary.
- `code_mode_fuel_exhausted` or `timeout`: split the work across calls or reduce \
local processing.

Lab actions (`lab::*` tool IDs) are not available in Code Mode. For Lab built-in \
actions use the `execute` tool in Tool Search mode.";

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

impl LabMcpServer {
    /// `search` gateway meta-tool branch. Self-returns.
    pub(crate) async fn call_tool_search_impl(
        &self,
        service: &str,
        args: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let input_tokens = estimate_tokens_args(args);
        let subject = self.request_subject_log_tag(context);
        let auth = auth_context_from_extensions(&context.extensions);
        if !tool_search_scope_allowed(auth) {
            tracing::warn!(
                surface = "mcp",
                service = %service,
                action = "call_tool",
                subject,
                elapsed_ms = started.elapsed().as_millis(),
                input_tokens,
                kind = "forbidden",
                "gateway code search denied by scope"
            );
            let env = build_error_extra(
                service,
                "call_tool",
                "forbidden",
                "code_search requires one of scopes: lab:read, lab, lab:admin",
                &serde_json::json!({ "required_scopes": ["lab:read", "lab", "lab:admin"] }),
            );
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        let code = args
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let code_hash = hash_arguments(&Value::String(code.clone()));
        let Some(manager) = &self.gateway_manager else {
            let envelope = build_error(
                service,
                "call_tool",
                "unknown_tool",
                "code search is not enabled",
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        };
        tracing::info!(
            surface = "mcp",
            service = "code_search",
            action = "call_tool",
            subject,
            code_hash = %code_hash,
            code_len = code.len(),
            input_tokens,
            "gateway code search start"
        );
        let broker = CodeModeBroker::new(&self.registry, Some(manager));
        let caller = auth.map_or(CodeModeCaller::TrustedLocal, |auth| {
            CodeModeCaller::Scoped {
                scopes: auth.scopes.clone(),
                sub: self.request_subject(context).map(ToOwned::to_owned),
            }
        });
        match broker
            .search(&code, caller, self.code_mode_surface(false))
            .await
        {
            Ok(response) => {
                let output =
                    serde_json::to_string(&response).unwrap_or_else(|_| "null".to_string());
                let output_tokens = estimate_tokens(&output);
                tracing::info!(
                    surface = "mcp",
                    service = "code_search",
                    action = "call_tool",
                    subject,
                    code_hash = %code_hash,
                    code_len = code.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    input_tokens,
                    output_tokens,
                    "gateway code search ok"
                );
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(err) => {
                tracing::warn!(
                    surface = "mcp",
                    service = "code_search",
                    action = "call_tool",
                    subject,
                    code_hash = %code_hash,
                    code_len = code.len(),
                    elapsed_ms = started.elapsed().as_millis(),
                    input_tokens,
                    kind = err.kind(),
                    error = %err,
                    "gateway code search failed"
                );
                let env = tool_error_envelope(service, "call_tool", &err);
                Ok(CallToolResult::error(vec![Content::text(env.to_string())]))
            }
        }
    }

    /// `execute` gateway meta-tool branch. Self-returns.
    pub(crate) async fn call_tool_execute_impl(
        &self,
        service: &str,
        args: &JsonObject,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = Instant::now();
        let input_tokens = estimate_tokens_args(args);
        let subject = self.request_subject_log_tag(context);
        let auth = auth_context_from_extensions(&context.extensions);
        if !tool_execute_scope_allowed(auth) {
            tracing::warn!(
                surface = "mcp",
                service = %service,
                action = "call_tool",
                subject,
                elapsed_ms = started.elapsed().as_millis(),
                input_tokens,
                kind = "forbidden",
                "gateway code execute denied by scope"
            );
            let env = build_error_extra(
                service,
                "call_tool",
                "forbidden",
                "code_execute requires one of scopes: lab, lab:admin",
                &serde_json::json!({ "required_scopes": ["lab", "lab:admin"] }),
            );
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        let Some(manager) = &self.gateway_manager else {
            let envelope = build_error(
                service,
                "call_tool",
                "unknown_tool",
                "code execute is not enabled",
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        };
        let config = manager.code_mode_config().await;
        let code = args.get("code").and_then(Value::as_str).unwrap_or_default();
        if code.trim().is_empty() {
            let env = build_error_extra(
                service,
                "call_tool",
                "invalid_param",
                "code must not be empty",
                &serde_json::json!({ "param": "code" }),
            );
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        if code.len() > CODE_MODE_MAX_CODE_BYTES {
            let env = build_error_extra(
                service,
                "call_tool",
                "invalid_param",
                "code exceeds max length 20000 bytes",
                &serde_json::json!({ "param": "code" }),
            );
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
        let requested_max_tool_calls = args
            .get("max_tool_calls")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(config.max_tool_calls)
            .max(1)
            .min(config.max_tool_calls.max(1));
        let allow_destructive_actions = args.get("confirm").and_then(Value::as_bool) == Some(true);
        let capability_filter = match (
            string_array_arg(args, "upstreams"),
            string_array_arg(args, "tools"),
        ) {
            (Ok(upstreams), Ok(tools)) => CodeModeCapabilityFilter::new(upstreams, tools),
            (Err(err), _) | (_, Err(err)) => {
                let env = tool_error_envelope(service, "call_tool", &err);
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
        };
        let code_hash = hash_arguments(&Value::String(code.to_string()));
        tracing::info!(
            surface = "mcp",
            service = "code_execute",
            action = "call_tool",
            subject,
            code_hash = %code_hash,
            max_tool_calls = requested_max_tool_calls,
            input_tokens,
            "gateway code execute start"
        );
        let broker = CodeModeBroker::new(&self.registry, Some(manager));
        let caller = auth.map_or(CodeModeCaller::TrustedLocal, |auth| {
            CodeModeCaller::Scoped {
                scopes: auth.scopes.clone(),
                sub: self.request_subject(context).map(ToOwned::to_owned),
            }
        });
        let before = self.snapshot_catalog().await;
        let response = match broker
            .execute(
                code,
                requested_max_tool_calls,
                caller,
                self.code_mode_surface(allow_destructive_actions),
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
                let env = tool_error_envelope(service, "call_tool", &err);
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            }
        };
        let output = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
        let output_tokens = estimate_tokens(&output);
        tracing::info!(
            surface = "mcp",
            service = "code_execute",
            action = "call_tool",
            subject,
            code_hash = %code_hash,
            call_count = response.calls.len(),
            elapsed_ms = started.elapsed().as_millis(),
            input_tokens,
            output_tokens,
            "gateway code execute ok"
        );
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }
}

#[cfg(test)]
mod tests;
