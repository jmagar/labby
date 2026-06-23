//! `CodeModeBroker::execute` and the host-brokered tool-call path.

use std::time::Duration;

use serde_json::Value;

use crate::error::ToolError;
use crate::host::CodeModeHost;
use lab_runtime::CodeModeConfig;

use super::CodeModeBroker;
use super::normalize_user_code;
use super::truncate::{response_within_budget, truncate_execution_response};
use super::types::{
    CodeModeCaller, CodeModeDiscoveryEntry, CodeModeExecutionError, CodeModeExecutionResponse,
    CodeModeSurface, CodeModeToolId, CodeModeToolRef, ToolScope,
};

/// Compatibility key a Code Mode snippet can return
/// (`return { __ui: <result> }`) to unwrap the final result payload while using
/// the last-wins captured mcp-ui widget link.
const UI_OPT_IN_KEY: &str = "__ui";

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    pub async fn execute(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // `codemode` is exposed only when the host's Code Mode surface is
        // enabled; the surface handler gates on that before reaching here.
        if !caller.can_execute() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "codemode requires one of scopes: lab, lab:admin".to_string(),
            }
            .into());
        }
        let started = std::time::Instant::now();
        let mut response = self
            .execute_sandboxed(
                code,
                Duration::from_millis(config.timeout_ms.max(1)),
                caller,
                surface,
                config.max_log_entries,
                config.max_log_bytes,
                config.trace_params,
                scope,
            )
            .await?;
        // Surface any last-wins captured mcp-ui widget link. `{ __ui: <result> }`
        // remains a compatibility form that also unwraps the inner payload.
        // Done before truncation so the (tiny) `ui` field is preserved while
        // `result` may be capped.
        self.apply_ui_opt_in(&mut response);
        let was_truncated = !response_within_budget(
            &response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        let response = truncate_execution_response(
            response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        tracing::info!(
            surface = "dispatch",
            service = "code_mode",
            action = "codemode",
            tool_calls = response.calls.len(),
            elapsed_ms = started.elapsed().as_millis(),
            result_bytes = response
                .result
                .as_ref()
                .map(|v| v.to_string().len())
                .unwrap_or(0),
            logs_count = response.logs.len(),
            truncated = was_truncated,
            "code execution complete"
        );
        Ok(response)
    }

    async fn build_code_mode_proxy(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<String, ToolError> {
        let Some(host) = self.host else {
            return Ok(String::new());
        };
        let include_snippets = caller.can_use_snippets() && !scope.is_scoped();
        // CLI with no explicit namespace scope can be served from the host's
        // render cache; everything else builds live.
        let use_cache = surface == CodeModeSurface::Cli && scope.allowed_namespaces().is_none();
        let render = host
            .list_tools(caller, surface, scope, include_snippets, use_cache)
            .await?;
        let catalog = render
            .entries
            .into_iter()
            .filter(|entry| {
                entry.kind == super::types::CodeModeCatalogKind::Snippet
                    || scope.allows(&entry.namespace, &entry.name)
            })
            .collect::<Vec<_>>();
        let mut namespaces: Vec<String> = catalog
            .iter()
            .filter(|entry| entry.kind == super::types::CodeModeCatalogKind::Tool)
            .map(|entry| entry.namespace.clone())
            .collect();
        namespaces.sort();
        namespaces.dedup();

        let discovery_entries = catalog
            .iter()
            .map(CodeModeDiscoveryEntry::from_catalog)
            .collect::<Vec<_>>();
        let discovery_js =
            super::preamble::generate_discovery_js(&discovery_entries).map_err(|message| {
                ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message,
                }
            })?;
        let tool_entries = catalog
            .iter()
            .filter(|entry| entry.kind == super::types::CodeModeCatalogKind::Tool)
            .collect::<Vec<_>>();
        let namespace_js =
            super::preamble::generate_js_proxy_from_catalog(&tool_entries, &namespaces).map_err(
                |message| ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message,
                },
            )?;
        Ok(format!("{discovery_js}\n{namespace_js}"))
    }

    async fn execute_sandboxed(
        &self,
        code: &str,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        trace_params: bool,
        scope: ToolScope,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        // Cloudflare-parity: no typed TypeScript preamble is injected. The
        // sandbox exposes only `callTool(id, params)`; the agent uses tool ids
        // discovered via `search`. Normalize the user code and run it directly.
        let code_to_run = normalize_user_code(code);

        // Build the runtime `codemode.*` proxy from the live catalog (same
        // source `search` uses) before starting the runner. Proxy failure is an
        // execution failure: otherwise `codemode.search`, `codemode.describe`,
        // and generated helpers silently disappear while raw `callTool` can
        // still make the run look successful.
        let deadline = tokio::time::Instant::now() + timeout;
        let proxy = match tokio::time::timeout_at(
            deadline,
            self.build_code_mode_proxy(&caller, surface, &scope),
        )
        .await
        {
            Ok(Ok(proxy)) => proxy,
            Ok(Err(err)) => {
                tracing::warn!(kind = err.kind(), "code_mode.proxy_generation_failed");
                return Err(err.into());
            }
            Err(_elapsed) => {
                tracing::warn!(
                    timeout_ms = timeout.as_millis(),
                    "code_mode.proxy_generation_timed_out"
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "timeout".to_string(),
                    message: "Code Mode proxy generation timed out".to_string(),
                }
                .into());
            }
        };
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or_default();
        if remaining.is_zero() {
            return Err(ToolError::Sdk {
                sdk_kind: "timeout".to_string(),
                message: "Code Mode execution timed out before sandbox start".to_string(),
            }
            .into());
        }

        self.run_in_runner(
            code_to_run,
            proxy,
            remaining,
            caller,
            surface,
            max_log_entries,
            max_log_bytes,
            trace_params,
            scope,
        )
        .await
    }

    pub(crate) async fn call_tool_id_before_deadline(
        &self,
        id: &str,
        params: Value,
        deadline: tokio::time::Instant,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        match tokio::time::timeout_at(
            deadline,
            self.call_tool_id(id, params, caller, surface, scope),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ToolError::Sdk {
                sdk_kind: "timeout".to_string(),
                message: "Code Mode execution timed out".to_string(),
            }),
        }
    }

    pub(crate) async fn call_tool_id(
        &self,
        id: &str,
        params: Value,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        let parsed = CodeModeToolId::parse(id)?;
        let Some(host) = self.host else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "no tool source configured".to_string(),
            });
        };
        match parsed.reference {
            CodeModeToolRef::Tool { namespace, tool } => {
                if !scope.allows(&namespace, &tool) {
                    return Err(ToolError::Sdk {
                        sdk_kind: "unknown_tool".to_string(),
                        message: format!(
                            "tool `{}` is outside this Code Mode execution capability set",
                            parsed.raw
                        ),
                    });
                }
                // The host applies destructive-tool policy when it resolves the
                // call (read-only callers cannot run a tool the host marks
                // destructive); it surfaces a `forbidden` error which passes
                // straight through.
                let outcome = host
                    .call_tool(&parsed.raw, params, &caller, surface, scope)
                    .await?;
                if let Some(ui) = outcome.ui {
                    if let Ok(mut sink) = self.ui_capture.lock() {
                        *sink = Some(ui);
                    } else {
                        tracing::warn!(
                            surface = "dispatch",
                            service = "code_mode",
                            action = "mcp_app.capture",
                            kind = "ui_capture_lock_poisoned",
                            "failed to store captured MCP App widget link"
                        );
                    }
                }
                Ok(outcome.value)
            }
        }
    }

    /// Apply a captured MCP App widget link to a finished response.
    ///
    /// When the user code's return value is an object with a `__ui` key, the
    /// inner value is unwrapped into `result` for compatibility with the older
    /// wrapper convention. Either way, if the run captured a widget-bearing
    /// result, attach the last-wins link to `ui`.
    fn apply_ui_opt_in(&self, response: &mut CodeModeExecutionResponse) {
        // Clone the inner value out (ending the borrow of `response.result`)
        // before reassigning. No `__ui` key → keep the result as-is.
        let inner = match response.result.as_ref() {
            Some(Value::Object(map)) => map.get(UI_OPT_IN_KEY).cloned(),
            _ => None,
        };
        let had_ui_opt_in = inner.is_some();
        if let Some(inner) = inner {
            response.result = Some(inner);
        }
        if let Ok(mut sink) = self.ui_capture.lock() {
            response.ui = sink.take();
            match response.ui.as_ref() {
                Some(ui) => tracing::info!(
                    surface = "dispatch",
                    service = "code_mode",
                    action = "mcp_app.opt_in",
                    resource_uri = ui_resource_uri(&ui.ui_meta).unwrap_or("<unknown>"),
                    "attached captured MCP App widget to execute response"
                ),
                None if had_ui_opt_in => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "mcp_app.opt_in",
                        kind = "ui_capture_missing",
                        "Code Mode returned __ui but no MCP App widget was captured"
                    );
                }
                None => {}
            }
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "mcp_app.opt_in",
                kind = "ui_capture_lock_poisoned",
                "Code Mode returned __ui but captured MCP App widget could not be read"
            );
        }
    }
}

fn ui_resource_uri(ui_meta: &Value) -> Option<&str> {
    ui_meta.get("resourceUri").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::NoopHost;
    use crate::types::UiLink;
    use serde_json::json;

    fn response_with_result(result: Value) -> CodeModeExecutionResponse {
        CodeModeExecutionResponse {
            execution_id: None,
            result: Some(result),
            ui: None,
            calls: Vec::new(),
            logs: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn apply_ui_opt_in_unwraps_and_attaches_captured_link() {
        let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
        *broker.ui_capture.lock().unwrap() = Some(UiLink {
            ui_meta: json!({ "resourceUri": "ui://axon/status-dashboard" }),
        });
        let mut response = response_with_result(json!({ "__ui": { "degraded": false } }));
        broker.apply_ui_opt_in(&mut response);
        // Inner payload is surfaced as `result`, wrapper removed.
        assert_eq!(response.result, Some(json!({ "degraded": false })));
        assert_eq!(
            response.ui.as_ref().expect("widget attached").ui_meta["resourceUri"],
            "ui://axon/status-dashboard"
        );
    }

    #[test]
    fn apply_ui_opt_in_without_optin_is_noop() {
        let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
        let mut response = response_with_result(json!({ "degraded": false }));
        broker.apply_ui_opt_in(&mut response);
        assert_eq!(response.result, Some(json!({ "degraded": false })));
        assert!(
            response.ui.is_none(),
            "no captured widget → no widget attached"
        );
    }

    #[test]
    fn apply_ui_opt_in_surfaces_direct_ui_tool_result() {
        let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
        *broker.ui_capture.lock().unwrap() = Some(UiLink {
            ui_meta: json!({ "resourceUri": "ui://ytdl-mcp/youtube-search.html" }),
        });

        let mut response = response_with_result(json!({
            "query": "phish",
            "limit": 1,
            "results": []
        }));

        broker.apply_ui_opt_in(&mut response);

        assert_eq!(
            response.ui.as_ref().expect("widget attached").ui_meta["resourceUri"],
            "ui://ytdl-mcp/youtube-search.html"
        );
    }
}
