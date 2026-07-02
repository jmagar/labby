//! `CodeModeBroker::execute` and the host-brokered tool-call path.

use std::time::Duration;

use serde_json::Value;

use crate::error::ToolError;
use crate::host::CodeModeHost;
use labby_runtime::{CodeModeConfig, CodeModeResultShapePolicy};

use super::CodeModeBroker;
use super::normalize_user_code;
use super::shape::shape_final_result;
use super::truncate::{response_within_budget, truncate_execution_response};
use super::types::{
    CodeModeCaller, CodeModeCatalogKind, CodeModeDiscoveryEntry, CodeModeExecutionError,
    CodeModeExecutionOutcome, CodeModeExecutionResponse, CodeModeSurface, CodeModeToolId,
    CodeModeToolRef, ToolDescriptor, ToolScope,
};

/// Compatibility key a Code Mode snippet can return
/// (`return { __ui: <result> }`) to unwrap the final result payload while using
/// the last-wins captured mcp-ui widget link.
const UI_OPT_IN_KEY: &str = "__ui";

/// Reserved namespace for host-internal pseudo-tool calls that are NOT real
/// Code Mode tool calls — they never reach `host.call_tool`, never consume
/// the per-run call budget, and never appear in `response.calls`. The
/// sandbox's generated JS calls these via the ordinary `callTool(id, params)`
/// primitive so no new sandbox protocol surface is needed; `call_tool_id`
/// intercepts ids in this namespace before the normal scope check.
const LAB_INTERNAL_NAMESPACE: &str = "__lab_internal";

/// Maximum accepted semantic query size in bytes for the reserved
/// `__lab_internal::semantic_rank` call. Oversized queries are truncated on a
/// char boundary — never errored — before reaching `host.semantic_rank`, so a
/// hostile sandbox cannot ship arbitrarily large payloads to the embedding
/// service. Mirrors the adjacent `limit.clamp(1, 50)` clamp-don't-reject
/// pattern (FAIL-OPEN invariant).
const MAX_SEMANTIC_QUERY_BYTES: usize = 8 * 1024;

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    pub async fn execute(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        Ok(self
            .execute_with_raw_response(code, caller, surface, config, scope)
            .await?
            .display_response)
    }

    pub async fn execute_with_raw_response(
        &self,
        code: &str,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        config: CodeModeConfig,
        scope: ToolScope,
    ) -> Result<CodeModeExecutionOutcome, CodeModeExecutionError> {
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
        let raw_response = response.clone();
        let shaped = shape_final_result(
            response.result.take(),
            config.result_shape_policy,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        let result_shape_changed = shaped.metadata.changed;
        let result_shape_truncated = shaped.metadata.truncated;
        let result_shape_original_size_bytes = shaped.metadata.original_size_bytes;
        let result_shape_shaped_size_bytes = shaped.metadata.shaped_size_bytes;
        response.result = shaped.result;
        if config.result_shape_policy != CodeModeResultShapePolicy::Off {
            response.result_shaping = Some(shaped.metadata);
        }
        let shaped_result = response.result.clone();
        let was_truncated = !response_within_budget(
            &response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        let mut response = truncate_execution_response(
            response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        );
        if response.result != shaped_result {
            response.result_shaping = None;
        }
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
            result_shape_policy = ?config.result_shape_policy,
            result_shape_changed,
            result_shape_truncated,
            result_shape_original_size_bytes,
            result_shape_shaped_size_bytes,
            logs_count = response.logs.len(),
            truncated = was_truncated,
            "code execution complete"
        );
        Ok(CodeModeExecutionOutcome {
            raw_response,
            display_response: response,
        })
    }

    async fn build_code_mode_proxy(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<String, ToolError> {
        let Some(host) = self.host else {
            return Ok(if local_providers_allowed(caller, scope) {
                super::preamble::generate_local_provider_js()
            } else {
                String::new()
            });
        };
        let (include_snippets, use_cache) = discovery_render_params(caller, surface, scope);
        let render = host
            .list_tools(caller, surface, scope, include_snippets, use_cache)
            .await?;
        let catalog = render
            .entries
            .into_iter()
            .filter(|entry| discovery_entry_visible(entry, scope))
            .collect::<Vec<_>>();
        let mut namespaces: Vec<String> = catalog
            .iter()
            .filter(|entry| entry.kind == CodeModeCatalogKind::Tool)
            .map(|entry| entry.namespace.clone())
            .collect();
        namespaces.sort();
        namespaces.dedup();

        let discovery_entries = catalog
            .iter()
            .map(CodeModeDiscoveryEntry::from_catalog)
            .collect::<Vec<_>>();
        let code_mode_config = host.config().await;
        let blend_weight = code_mode_config.semantic_search.blend_weight;
        let discovery_js = super::preamble::generate_discovery_js(&discovery_entries, blend_weight)
            .map_err(|message| ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message,
            })?;
        let tool_entries = catalog
            .iter()
            .filter(|entry| entry.kind == CodeModeCatalogKind::Tool)
            .collect::<Vec<_>>();
        let namespace_js =
            super::preamble::generate_js_proxy_from_catalog(&tool_entries, &namespaces).map_err(
                |message| ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message,
                },
            )?;
        let local_provider_js = if local_providers_allowed(caller, scope) {
            super::preamble::generate_local_provider_js()
        } else {
            String::new()
        };
        Ok(format!(
            "{local_provider_js}\n{discovery_js}\n{namespace_js}"
        ))
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
                if namespace == LAB_INTERNAL_NAMESPACE {
                    return self
                        .dispatch_internal_call(&tool, params, &caller, surface, scope)
                        .await;
                }
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

    /// Dispatch a reserved `__lab_internal::*` pseudo-tool call. These never
    /// reach `host.call_tool` and are never subject to `scope.allows()` —
    /// see the `LAB_INTERNAL_NAMESPACE` doc comment for why that's safe.
    async fn dispatch_internal_call(
        &self,
        tool: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        let Some(host) = self.host else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "no tool source configured".to_string(),
            });
        };
        match tool {
            "semantic_rank" => {
                let query = clamp_semantic_query(
                    params
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                let limit = params
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n.clamp(1, 50) as usize)
                    .unwrap_or(50);
                // Fail-open at this layer too: even though the trait contract
                // says implementations return `Ok(Vec::new())` on degraded
                // paths (never `Err`), an accidental `Err` from a host bug
                // still must not break `codemode.search()` — degrade to an
                // empty ranked list, identical to the "no semantic signal"
                // case.
                let ranked = host
                    .semantic_rank(query, limit, caller, surface, scope)
                    .await
                    .unwrap_or_default();
                let ranked_json: Vec<Value> = ranked
                    .into_iter()
                    .map(|(id, score)| serde_json::json!({ "id": id, "score": score }))
                    .collect();
                Ok(serde_json::json!({ "ranked": ranked_json }))
            }
            _ => Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown internal tool `{LAB_INTERNAL_NAMESPACE}::{tool}`"),
            }),
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

pub(crate) fn local_providers_allowed(caller: &CodeModeCaller, scope: &ToolScope) -> bool {
    caller.is_admin() && !scope.is_scoped()
}

/// Truncate a semantic query to at most [`MAX_SEMANTIC_QUERY_BYTES`], cutting
/// on a char boundary so the result stays valid UTF-8. Never errors — an
/// oversized query degrades to its prefix, mirroring the fail-open posture of
/// the rest of the semantic path.
fn clamp_semantic_query(mut query: String) -> String {
    if query.len() > MAX_SEMANTIC_QUERY_BYTES {
        // `str::floor_char_boundary` is nightly-only; walk back from the cap
        // (at most 3 steps — UTF-8 sequences are ≤ 4 bytes) to the nearest
        // boundary. `is_char_boundary(0)` is always true, so this terminates.
        let mut boundary = MAX_SEMANTIC_QUERY_BYTES;
        while !query.is_char_boundary(boundary) {
            boundary -= 1;
        }
        query.truncate(boundary);
    }
    query
}

/// The `(include_snippets, use_cache)` discovery-render parameters for a Code
/// Mode execution's `caller`/`surface`/`scope`.
///
/// This is the single source of truth for the formulas `build_code_mode_proxy`
/// uses when rendering the sandbox's own discovery catalog. Hosts that
/// recompute the same render out-of-band (e.g. a gateway's `semantic_rank`
/// recomputing the scope-filtered entry set) MUST call this instead of
/// restating the formulas — the semantic-scope security invariant rests on
/// the two sites never diverging.
///
/// - snippets are included only for snippet-capable callers on unscoped runs;
/// - the host's render cache is only safe for CLI executions with no explicit
///   namespace scope (everything else builds live).
pub fn discovery_render_params(
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    scope: &ToolScope,
) -> (bool, bool) {
    let include_snippets = caller.can_use_snippets() && !scope.is_scoped();
    let use_cache = surface == CodeModeSurface::Cli && scope.allowed_namespaces().is_none();
    (include_snippets, use_cache)
}

/// Whether a rendered catalog entry is visible to the sandbox's discovery
/// catalog under `scope`: snippets are always visible, tools must pass
/// `scope.allows`.
///
/// Single source of truth for the post-render entry filter shared by
/// `build_code_mode_proxy` and any host recomputing the same scope-filtered
/// entry set (e.g. a gateway's `semantic_rank`) — see
/// [`discovery_render_params`] for why divergence here is a security bug,
/// not a style issue.
pub fn discovery_entry_visible(entry: &ToolDescriptor, scope: &ToolScope) -> bool {
    entry.kind == CodeModeCatalogKind::Snippet || scope.allows(&entry.namespace, &entry.name)
}

fn ui_resource_uri(ui_meta: &Value) -> Option<&str> {
    ui_meta.get("resourceUri").and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::NoopHost;
    use crate::types::{CodeModeCallerCapabilities, UiLink};
    use serde_json::json;

    fn response_with_result(result: Value) -> CodeModeExecutionResponse {
        CodeModeExecutionResponse {
            execution_id: None,
            result: Some(result),
            result_shaping: None,
            ui: None,
            calls: Vec::new(),
            logs: Vec::new(),
            artifacts: Vec::new(),
        }
    }

    #[tokio::test]
    async fn call_tool_id_routes_lab_internal_namespace_before_scope_check() {
        // A ToolScope that allows nothing should still let `__lab_internal::*`
        // through, because it's intercepted before the scope.allows() check.
        let host = NoopHost::default();
        let broker = CodeModeBroker::new(Some(&host));
        let empty_scope = ToolScope::scoped_namespaces(vec![], vec![]);
        let result = broker
            .call_tool_id(
                "__lab_internal::semantic_rank",
                json!({ "query": "test", "limit": 5 }),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &empty_scope,
            )
            .await;
        // NoopHost's semantic_rank always returns Ok(vec![]), so this must
        // succeed with an empty ranked list, not a `forbidden`/`unknown_tool`
        // scope error.
        let value = result.expect("internal dispatch must bypass scope.allows()");
        assert_eq!(value, json!({ "ranked": [] }));
    }

    #[tokio::test]
    async fn call_tool_id_rejects_unknown_internal_tool() {
        let host = NoopHost::default();
        let broker = CodeModeBroker::new(Some(&host));
        let scope = ToolScope::default();
        let result = broker
            .call_tool_id(
                "__lab_internal::not_a_real_internal_tool",
                json!({}),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &scope,
            )
            .await;
        assert!(result.is_err());
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

    #[test]
    fn clamp_semantic_query_leaves_small_queries_untouched() {
        assert_eq!(clamp_semantic_query("hello".to_string()), "hello");
        let exactly_max = "a".repeat(MAX_SEMANTIC_QUERY_BYTES);
        assert_eq!(clamp_semantic_query(exactly_max.clone()), exactly_max);
    }

    #[test]
    fn clamp_semantic_query_truncates_oversized_ascii_without_error() {
        let oversized = "a".repeat(MAX_SEMANTIC_QUERY_BYTES + 1000);
        let clamped = clamp_semantic_query(oversized);
        assert_eq!(clamped.len(), MAX_SEMANTIC_QUERY_BYTES);
    }

    #[test]
    fn clamp_semantic_query_truncates_on_char_boundary() {
        // 4-byte scorpions straddling the cap: the clamp must land on a char
        // boundary (valid UTF-8), never split a code point.
        let oversized = "\u{1F982}".repeat(MAX_SEMANTIC_QUERY_BYTES / 4 + 10);
        let clamped = clamp_semantic_query(oversized);
        assert!(clamped.len() <= MAX_SEMANTIC_QUERY_BYTES);
        assert_eq!(
            clamped.len(),
            MAX_SEMANTIC_QUERY_BYTES - MAX_SEMANTIC_QUERY_BYTES % 4
        );
        assert!(clamped.chars().all(|c| c == '\u{1F982}'));
    }

    #[tokio::test]
    async fn dispatch_internal_call_truncates_oversized_query_instead_of_erroring() {
        let host = NoopHost::default();
        let broker = CodeModeBroker::new(Some(&host));
        let oversized = "q".repeat(MAX_SEMANTIC_QUERY_BYTES * 4);
        let result = broker
            .call_tool_id(
                "__lab_internal::semantic_rank",
                json!({ "query": oversized, "limit": 5 }),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &ToolScope::default(),
            )
            .await;
        let value = result.expect("oversized query must be truncated, not errored");
        assert_eq!(value, json!({ "ranked": [] }));
    }

    #[test]
    fn local_providers_require_unscoped_admin_scope() {
        assert!(local_providers_allowed(
            &CodeModeCaller::TrustedLocal,
            &ToolScope::default()
        ));
        assert!(local_providers_allowed(
            &CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_execute: true,
                    can_use_snippets: true,
                    is_admin: true,
                },
                sub: Some("admin".to_string()),
            },
            &ToolScope::default()
        ));
        assert!(!local_providers_allowed(
            &CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_execute: true,
                    can_use_snippets: false,
                    is_admin: false,
                },
                sub: Some("user".to_string()),
            },
            &ToolScope::default()
        ));
        assert!(!local_providers_allowed(
            &CodeModeCaller::TrustedLocal,
            &ToolScope::scoped_namespaces(vec!["github".to_string()], vec![])
        ));
        assert!(!local_providers_allowed(
            &CodeModeCaller::TrustedLocal,
            &ToolScope::new(vec![], vec!["github::list_pull_requests".to_string()])
        ));
    }
}
