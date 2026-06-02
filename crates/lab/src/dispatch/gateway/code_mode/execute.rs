//! `CodeModeBroker::execute` and the upstream tool-call path.

use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use serde_json::{Map, Value};

use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::upstream::types::UpstreamRuntimeOwner;

use super::CodeModeBroker;
use super::normalize_user_code;
use super::runner_io::code_mode_upstream_error_info;
use super::schema::{unwrap_code_mode_upstream_result, validate_code_mode_params_against_schema};
use super::truncate::{response_within_budget, truncate_execution_response};
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeExecutionResponse, CodeModeSurface,
    CodeModeToolId, CodeModeToolRef, destructive_permitted,
};

impl CodeModeBroker<'_> {
    pub async fn execute(
        &self,
        code: &str,
        max_tool_calls: usize,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: crate::config::CodeModeConfig,
        capability_filter: CodeModeCapabilityFilter,
    ) -> Result<CodeModeExecutionResponse, ToolError> {
        // `execute` is exposed only when the gateway search/execute surface is
        // enabled (tool_search.enabled → RootSynthetic), and the MCP handler
        // gates on `exposes_synthetic_tools()` before reaching here. There is no
        // separate per-tool enable: when the surface is on, both `search` and
        // `execute` work (subject to scope), exactly like the Cloudflare blog.
        if !caller.can_execute() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "code_execute requires one of scopes: lab, lab:admin".to_string(),
            });
        }
        let started = std::time::Instant::now();
        let response = self
            .execute_sandboxed(
                code,
                max_tool_calls.max(1).min(config.max_tool_calls.max(1)),
                Duration::from_millis(config.timeout_ms.max(1)),
                caller,
                surface,
                config.max_log_entries,
                config.max_log_bytes,
                capability_filter,
            )
            .await?;
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
            action = "code_execute",
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
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<String, ToolError> {
        let Some(manager) = self.gateway_manager else {
            return Ok(String::new());
        };
        let allow_cold_connect = caller.can_execute();
        let owner = caller.runtime_owner(surface);
        let oauth_subject = caller.oauth_subject();
        let tools = manager
            .code_mode_catalog_tools(allow_cold_connect, Some(&owner), oauth_subject)
            .await
            .map_err(|err| ToolError::Sdk {
                sdk_kind: err.kind().to_string(),
                message: err.user_message().to_string(),
            })?;
        let tools = tools
            .into_iter()
            .filter(|tool| {
                capability_filter.allows(tool.upstream_name.as_ref(), tool.tool.name.as_ref())
            })
            .collect::<Vec<_>>();
        if tools.is_empty() {
            return Ok(String::new());
        }
        let mut upstreams: Vec<String> =
            tools.iter().map(|t| t.upstream_name.to_string()).collect();
        upstreams.sort();
        upstreams.dedup();
        super::preamble::generate_js_proxy(&tools, &upstreams).map_err(|message| ToolError::Sdk {
            sdk_kind: "invalid_param".to_string(),
            message,
        })
    }

    async fn execute_sandboxed(
        &self,
        code: &str,
        max_tool_calls: usize,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        capability_filter: CodeModeCapabilityFilter,
    ) -> Result<CodeModeExecutionResponse, ToolError> {
        // Cloudflare-parity: no typed TypeScript preamble is injected. The
        // sandbox exposes only `callTool(id, params)`; the agent uses tool ids
        // discovered via `search`. Normalize the user code and run it directly.
        let code_to_run = normalize_user_code(code);

        // Build the runtime `codemode.*` proxy from the live upstream catalog
        // (same source `search` uses). On any failure, fall back to an empty
        // proxy rather than aborting execute — `callTool` is always available as
        // the documented escape hatch, so the run can still proceed without the
        // typed namespace.
        // Bound proxy generation by the same wall-clock budget as the run so a
        // slow upstream catalog cannot blow past the configured timeout before
        // the runner even starts. On elapsed or failure, fall back to an empty
        // proxy and continue — `callTool` is always available as the escape hatch.
        let proxy = match tokio::time::timeout(
            timeout,
            self.build_code_mode_proxy(&caller, surface, &capability_filter),
        )
        .await
        {
            Ok(Ok(proxy)) => proxy,
            Ok(Err(err)) => {
                tracing::warn!(
                    kind = err.kind(),
                    "code_mode.proxy_generation_failed; continuing with callTool only"
                );
                String::new()
            }
            Err(_elapsed) => {
                tracing::warn!(
                    timeout_ms = timeout.as_millis(),
                    "code_mode.proxy_generation_timed_out; continuing with callTool only"
                );
                String::new()
            }
        };

        self.run_in_runner(
            code_to_run,
            proxy,
            max_tool_calls,
            timeout,
            caller,
            surface,
            max_log_entries,
            max_log_bytes,
            capability_filter,
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
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<Value, ToolError> {
        match tokio::time::timeout_at(
            deadline,
            self.call_tool_id(id, params, caller, surface, capability_filter),
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
        capability_filter: &CodeModeCapabilityFilter,
    ) -> Result<Value, ToolError> {
        let parsed = CodeModeToolId::parse(id)?;
        let Some(manager) = self.gateway_manager else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "no gateway manager configured".to_string(),
            });
        };
        match parsed.reference {
            CodeModeToolRef::UpstreamTool { upstream, tool } => {
                if !capability_filter.allows(&upstream, &tool) {
                    return Err(ToolError::Sdk {
                        sdk_kind: "unknown_tool".to_string(),
                        message: format!(
                            "upstream tool `{}` is outside this Code Mode execution capability set",
                            parsed.raw
                        ),
                    });
                }
                let owner = caller.runtime_owner(surface);
                let oauth_subject = caller.oauth_subject();
                self.call_upstream_tool(
                    manager,
                    &upstream,
                    &tool,
                    params,
                    &owner,
                    oauth_subject,
                    surface,
                    &caller,
                )
                .await
            }
        }
    }

    async fn call_upstream_tool(
        &self,
        manager: &GatewayManager,
        upstream: &str,
        tool: &str,
        params: Value,
        owner: &UpstreamRuntimeOwner,
        oauth_subject: Option<&str>,
        surface: CodeModeSurface,
        caller: &CodeModeCaller,
    ) -> Result<Value, ToolError> {
        let upstream_tool = manager
            .resolve_code_mode_upstream_tool(upstream, tool, Some(owner), oauth_subject)
            .await?;

        // Host-side destructive action gate: block tools with destructive=true
        // unless the action is permitted (see `destructive_permitted`).
        if upstream_tool.destructive && !destructive_permitted(surface, caller) {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "code_execute",
                upstream = upstream,
                tool = tool,
                kind = "confirmation_required",
                "blocked destructive Code Mode tool call; allow_destructive_actions is not set"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "confirmation_required".to_string(),
                message: format!(
                    "Tool `{upstream}::{tool}` has destructive=true. \
                     Set allow_destructive_actions=true in the Code Mode surface to proceed."
                ),
            });
        }
        validate_code_mode_params_against_schema(&params, upstream_tool.input_schema.as_ref())?;
        let Some(pool) = manager.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: "gateway upstream pool is unavailable".to_string(),
            });
        };
        let mut upstream_params = CallToolRequestParams::new(tool.to_string());
        upstream_params.arguments = Some(match params {
            Value::Object(map) => map,
            _ => Map::new(),
        });
        match pool.call_tool(upstream, upstream_params).await {
            Some(Ok(result)) => {
                if result.is_error == Some(true) {
                    let error_text = result
                        .content
                        .first()
                        .and_then(|content| content.as_text())
                        .map(|content| content.text.as_str());
                    let (kind, message, counts_as_failure) =
                        code_mode_upstream_error_info(error_text);
                    if counts_as_failure {
                        pool.record_failure(upstream, message.clone()).await;
                    } else {
                        pool.record_success(upstream).await;
                    }
                    return Err(ToolError::Sdk {
                        sdk_kind: kind.to_string(),
                        message,
                    });
                }
                pool.record_success(upstream).await;
                Ok(unwrap_code_mode_upstream_result(result))
            }
            Some(Err(err)) => {
                pool.record_failure(upstream, err.clone()).await;
                Err(ToolError::Sdk {
                    sdk_kind: "upstream_error".to_string(),
                    message: err,
                })
            }
            None => {
                pool.record_failure(upstream, format!("upstream `{upstream}` is not connected"))
                    .await;
                Err(ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("upstream tool `{upstream}::{tool}` was not found"),
                })
            }
        }
    }
}
