//! `impl CodeModeHost for GatewayManager`: the gateway's binding of the
//! extracted Code Mode kernel to its upstream MCP proxy pool.
//!
//! This is where gateway/upstream vocabulary is legitimately reintroduced: the
//! crate's neutral `CodeModeHost` methods are implemented in terms of the live
//! `UpstreamPool`, `UpstreamTool`, `UpstreamRuntimeOwner`, OAuth subjects, and
//! the snippet store. The crate never sees any of it.

use labby_codemode::snippet::store::{
    builtin_snippet_dir, code_for_snippet, merge_snippet_input, resolve_snippet,
};
use labby_codemode::{
    CodeModeCaller, CodeModeConfig, CodeModeHost, CodeModeSurface, ResolvedSnippet, RunnerPool,
    ToolCallOutcome, ToolScope, ToolsRender, UiLink, destructive_permitted,
};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Map, Value};

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::manager::GatewayManager;
use crate::upstream::types::UpstreamRuntimeOwner;
use labby_runtime::error::ToolError;
use labby_runtime::lab_home;

use super::search;
use super::validate_code_mode_params_against_schema;

impl CodeModeHost for GatewayManager {
    async fn list_tools(
        &self,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        scope: &ToolScope,
        include_snippets: bool,
        use_cache: bool,
    ) -> Result<ToolsRender, ToolError> {
        // MCP `codemode` execution must not spend the caller's wall-clock budget
        // cold-connecting every upstream just to render helper metadata; trivial
        // code that never calls a tool should reach the runner immediately.
        // Tool execution remains live because `call_tool` resolves the requested
        // upstream at the actual call boundary.
        let allow_cold_connect = surface == CodeModeSurface::Cli && caller.can_execute();
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);
        let allowed = scope.allowed_namespaces();
        search::build_tools_render(
            self,
            allow_cold_connect,
            &owner,
            oauth_subject,
            allowed,
            include_snippets,
            use_cache,
        )
        .await
    }

    async fn call_tool(
        &self,
        id: &str,
        params: Value,
        caller: &CodeModeCaller,
        surface: CodeModeSurface,
        _scope: &ToolScope,
    ) -> Result<ToolCallOutcome, ToolError> {
        let (upstream, tool) =
            labby_codemode::split_namespaced_id(id).ok_or_else(|| ToolError::Sdk {
                sdk_kind: "invalid_code_mode_id".to_string(),
                message: format!("Code Mode ids must use <namespace>::<tool>: `{id}`"),
            })?;
        let owner = runtime_owner(caller, surface);
        let oauth_subject = oauth_subject(caller);

        let upstream_tool = self
            .resolve_code_mode_upstream_tool(upstream, tool, Some(&owner), oauth_subject)
            .await?;

        // Host-side scope check: read-only callers cannot execute a destructive
        // upstream tool.
        if upstream_tool.destructive && !destructive_permitted(surface, caller) {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                upstream = upstream,
                tool = tool,
                kind = "forbidden",
                "blocked destructive Code Mode tool call for non-execute caller"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!(
                    "Tool `{upstream}::{tool}` requires Code Mode execute permission."
                ),
            });
        }
        validate_code_mode_params_against_schema(&params, upstream_tool.input_schema.as_ref())?;

        let Some(pool) = self.current_pool().await else {
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
                let ui = extract_ui_link(&result);
                if let Some(ui) = ui.as_ref() {
                    let resource_uri = ui_resource_uri(&ui.ui_meta).unwrap_or("<unknown>");
                    tracing::info!(
                        surface = "dispatch",
                        service = "code_mode",
                        action = "mcp_app.capture",
                        upstream,
                        tool,
                        resource_uri,
                        "captured upstream MCP App widget link"
                    );
                }
                Ok(ToolCallOutcome {
                    value: unwrap_code_mode_upstream_result(result),
                    ui,
                })
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

    async fn resolve_snippet(
        &self,
        name: &str,
        input: Value,
    ) -> Result<ResolvedSnippet, ToolError> {
        let lab_home = lab_home();
        let builtin_dir = builtin_snippet_dir();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let resolved = resolve_snippet(&lab_home, &builtin_dir, &name)?;
            let input = merge_snippet_input(&resolved, input)?;
            let code = code_for_snippet(&resolved)?;
            Ok::<_, ToolError>(ResolvedSnippet {
                name: resolved.name,
                code,
                input,
            })
        })
        .await
        .map_err(|err| ToolError::internal_message(format!("snippet resolve task failed: {err}")))?
    }

    async fn config(&self) -> CodeModeConfig {
        self.code_mode_config().await
    }

    fn runner_pool(&self) -> &RunnerPool {
        self.code_mode_runner_pool()
    }
}

/// Map a Code Mode caller + surface onto an `UpstreamRuntimeOwner`. Lifted out
/// of the (now neutral) `CodeModeCaller` so the kernel carries no gateway type.
fn runtime_owner(caller: &CodeModeCaller, surface: CodeModeSurface) -> UpstreamRuntimeOwner {
    let surface = surface.tag();
    let subject = caller.subject().map(ToOwned::to_owned);
    let raw = subject
        .as_ref()
        .map(|subject| format!("{surface}:{subject}"))
        .unwrap_or_else(|| format!("{surface}:trusted-local"));
    UpstreamRuntimeOwner {
        surface: surface.to_string(),
        subject,
        request_id: None,
        session_id: None,
        client_name: None,
        raw: Some(raw),
    }
}

/// The upstream OAuth subject for a Code Mode caller.
///
/// Admin/operator callers share the single gateway-owned upstream credential
/// (`SHARED_GATEWAY_OAUTH_SUBJECT`); non-admin callers keep their own `sub` so a
/// personal upstream grant is used; a `sub`-less caller falls back to the shared
/// subject. Mirrors `oauth_upstream_subject_for_request`.
fn oauth_subject(caller: &CodeModeCaller) -> Option<&str> {
    if caller.is_admin() {
        return Some(SHARED_GATEWAY_OAUTH_SUBJECT);
    }
    Some(caller.subject().unwrap_or(SHARED_GATEWAY_OAUTH_SUBJECT))
}

fn extract_ui_link(result: &CallToolResult) -> Option<UiLink> {
    let meta = result.meta.as_ref()?;
    let ui = meta.get("ui")?;
    ui.get("resourceUri")?.as_str()?;
    Some(UiLink {
        ui_meta: ui.clone(),
    })
}

fn ui_resource_uri(ui_meta: &Value) -> Option<&str> {
    ui_meta.get("resourceUri").and_then(Value::as_str)
}

/// Unwrap an upstream `CallToolResult` into the value Code Mode returns.
fn unwrap_code_mode_upstream_result(result: CallToolResult) -> Value {
    if let Some(value) = result.structured_content {
        return value;
    }
    let all_text = !result.content.is_empty()
        && result
            .content
            .iter()
            .all(|content| content.as_text().is_some());
    if all_text {
        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|content| content.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        return serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text));
    }
    if result.content.is_empty() {
        Value::Null
    } else {
        serde_json::json!(result)
    }
}

fn code_mode_canonical_error_kind(s: &str) -> &'static str {
    match s {
        "unknown_action" => "unknown_action",
        "unknown_subaction" => "unknown_subaction",
        "missing_param" => "missing_param",
        "invalid_param" => "invalid_param",
        "unknown_instance" => "unknown_instance",
        "confirmation_required" => "confirmation_required",
        "conflict" => "conflict",
        "auth_failed" => "auth_failed",
        "not_found" => "not_found",
        "rate_limited" => "rate_limited",
        "validation_failed" => "validation_failed",
        "network_error" => "network_error",
        "server_error" => "server_error",
        "decode_error" => "decode_error",
        "internal_error" => "internal_error",
        "upstream_error" => "upstream_error",
        "code_mode_timeout" => "code_mode_timeout",
        // `code_mode_fuel_exhausted` is intentionally NOT mapped: it is reserved
        // for the dead Wasmtime/fuel path and is never emitted on the live
        // Javy/QuickJS path. Normalize to `internal_error` rather than pass
        // through a reserved/dead kind. See docs/dev/ERRORS.md.
        _ => "internal_error",
    }
}

/// Classify an upstream error payload into `(kind, message, counts_as_failure)`.
fn code_mode_upstream_error_info(text: Option<&str>) -> (&'static str, String, bool) {
    let Some(text) = text else {
        return (
            "upstream_error",
            "upstream returned a non-text error payload".to_string(),
            true,
        );
    };
    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        return ("upstream_error", text.to_string(), true);
    };
    let error_obj = parsed
        .get("error")
        .and_then(Value::as_object)
        .or_else(|| parsed.as_object());
    let Some(error_obj) = error_obj else {
        return ("upstream_error", text.to_string(), true);
    };
    let kind = error_obj
        .get("kind")
        .and_then(Value::as_str)
        .map(code_mode_canonical_error_kind)
        .unwrap_or("upstream_error");
    let message = error_obj
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(text)
        .to_string();
    let counts_as_failure = matches!(
        kind,
        "upstream_error" | "network_error" | "server_error" | "decode_error" | "internal_error"
    );
    (kind, message, counts_as_failure)
}
