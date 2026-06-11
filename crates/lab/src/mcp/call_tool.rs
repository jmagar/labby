//! `call_tool` dispatch entry: arg parse + service lookup, the gateway
//! meta-tool routing (search/execute), the post-meta-tool gates
//! (visibility / action-allowed / code_mode-hidden / admin-scope /
//! destructive elicitation), the builtin dispatch branch, and the
//! fall-through to the upstream proxy tail.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as an inherent
//! `impl LabMcpServer` method. The `ServerHandler` trait impl in
//! `server.rs` keeps a one-line delegator.
//!
//! Preserves the exact early-return ordering (search → execute →
//! visibility → action → code_mode-hidden → admin-scope → elicitation
//! → builtin → upstream tail). The codemode and upstream branches live in
//! `call_tool_codemode.rs` / `call_tool_upstream.rs`. No behavior change.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, LoggingLevel, LoggingMessageNotificationParam,
};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::mcp::catalog::{CODE_MODE_SEARCH_TOOL_NAME, TOOL_EXECUTE_TOOL_NAME};
use crate::mcp::context::{auth_context_from_extensions, tool_execute_builtin_action_allowed};
use crate::mcp::elicitation::{ElicitResult, elicit_confirm};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::error::DispatchError;
use crate::mcp::result_format::{estimate_tokens_args, format_dispatch_result};
use crate::mcp::server::LabMcpServer;

fn route_scope_denied_result(service: &str, action: &str, message: String) -> CallToolResult {
    let envelope = build_error(service, action, "route_scope_denied", &message);
    CallToolResult::error(vec![Content::text(envelope.to_string())])
}

fn inject_gateway_origin_param(params: Value, subject: Option<&str>) -> Value {
    let raw = subject
        .map(|value| format!("mcp:{value}"))
        .unwrap_or_else(|| "mcp:anonymous".to_string());
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    object.entry("owner".to_string()).or_insert_with(|| {
        serde_json::json!({
            "surface": "mcp",
            "subject": subject,
            "raw": raw,
        })
    });
    object
        .entry("origin".to_string())
        .or_insert_with(|| Value::String(raw));
    Value::Object(object)
}

impl LabMcpServer {
    fn log_route_scope_denial(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
        action: &str,
        message: &str,
        elapsed_ms: u128,
    ) {
        let subject = self.request_subject_log_tag(context);
        tracing::warn!(
            surface = "mcp",
            service,
            action,
            subject,
            route_scope = %self.route_scope.label(),
            elapsed_ms,
            kind = "route_scope_denied",
            error = %message,
            "MCP call denied by protected route scope"
        );
        if !self.should_emit_logging_notification(LoggingLevel::Warning) {
            return;
        }

        let peer = context.peer.clone();
        let actor_key = crate::mcp::context::actor_key_from_extensions(&context.extensions)
            .map(ToOwned::to_owned);
        let service = service.to_string();
        let action = action.to_string();
        tokio::spawn(async move {
            let mut payload = serde_json::json!({
                "surface": "mcp",
                "service": service,
                "action": action,
                "elapsed_ms": elapsed_ms,
                "kind": "route_scope_denied",
            });
            if let Some(actor_key) = actor_key {
                payload["actor_key"] = serde_json::json!(actor_key);
            }
            if let Err(error) = peer
                .notify_logging_message(
                    LoggingMessageNotificationParam::new(LoggingLevel::Warning, payload)
                        .with_logger("lab.mcp.dispatch"),
                )
                .await
            {
                tracing::debug!(
                    surface = "mcp",
                    service = %service,
                    action = %action,
                    level = ?LoggingLevel::Warning,
                    error = %error,
                    "failed to send rmcp logging notification"
                );
            }
        });
    }

    pub(crate) async fn call_tool_impl(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let start = Instant::now();
        let service = request.name.as_ref().to_string();
        let raw_arguments = request.arguments.clone();
        let args = request.arguments.unwrap_or_default();
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let params = args.get("params").cloned().unwrap_or(Value::Null);
        let instance = params
            .get("instance")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let param_key_count = params.as_object().map_or(0, serde_json::Map::len);

        let svc = self.registry.services().iter().find(|s| s.name == service);

        // ── Gateway `search` tool: run caller's JS over the upstream catalog ──
        if service == CODE_MODE_SEARCH_TOOL_NAME {
            if !self.route_scope.exposes_code_mode() {
                let elapsed_ms = start.elapsed().as_millis();
                self.log_route_scope_denial(
                    &context,
                    &service,
                    "call_tool",
                    "Code Mode is not exposed on this MCP route",
                    elapsed_ms,
                );
                return Ok(route_scope_denied_result(
                    &service,
                    "call_tool",
                    "Code Mode is not exposed on this MCP route".to_string(),
                ));
            }
            return self.call_code_mode_impl(&service, &args, &context).await;
        }

        // ── Gateway `execute` tool: run caller's JS in the subprocess sandbox ─
        if service == TOOL_EXECUTE_TOOL_NAME {
            if !self.route_scope.exposes_code_mode() {
                let elapsed_ms = start.elapsed().as_millis();
                self.log_route_scope_denial(
                    &context,
                    &service,
                    "call_tool",
                    "Code Mode is not exposed on this MCP route",
                    elapsed_ms,
                );
                return Ok(route_scope_denied_result(
                    &service,
                    "call_tool",
                    "Code Mode is not exposed on this MCP route".to_string(),
                ));
            }
            return self.call_tool_execute_impl(&service, &args, &context).await;
        }

        if svc.is_some() && !self.route_scope.allows_service(&service) {
            let elapsed_ms = start.elapsed().as_millis();
            let message = format!("service `{service}` is not exposed on this MCP route");
            self.log_route_scope_denial(&context, &service, &action, &message, elapsed_ms);
            return Ok(route_scope_denied_result(&service, &action, message));
        }

        if svc.is_some() && !self.service_visible_on_mcp(&service).await {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` is not enabled on the mcp surface"),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }

        if svc.is_some() && !self.action_allowed_on_mcp(&service, &action).await {
            let mut extra = serde_json::Map::new();
            if let Some(valid) = self.allowed_mcp_actions(&service).await {
                extra.insert(
                    "valid".to_string(),
                    serde_json::to_value(valid).unwrap_or(Value::Array(Vec::new())),
                );
            }
            let envelope = build_error_extra(
                &service,
                &action,
                "unknown_action",
                &format!("action `{action}` is not exposed for service `{service}`"),
                &Value::Object(extra),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }

        if self.code_mode_visibility().await.hides_raw_tools() {
            // MCP App tools stay visible in `list_tools` even while Code Mode
            // hides ordinary raw tools, so their host callbacks must be allowed
            // through the same raw proxy path. Operators can still opt into the
            // broader legacy callback bypass for non-UI tools with
            // `LAB_CODE_MODE_WIDGET_CALLBACKS=1`.
            let widget_tool = if svc.is_none()
                && let Some(pool) = self.current_upstream_pool().await
            {
                pool.find_tool(&service).await.filter(|(_, tool)| {
                    crate::dispatch::upstream::pool::tool_has_mcp_app_ui_resource(tool)
                        || crate::config::code_mode_widget_callbacks_enabled()
                })
            } else {
                None
            };
            match widget_tool {
                // Destructive upstream effects are NOT reachable over the
                // callback bypass: it has no confirmation channel, so unlike the
                // `execute` path (which gates destructive upstream calls behind
                // `confirm: true`) it could otherwise run a destructive tool
                // unconfirmed. The sanctioned path for those is `execute`.
                Some((_upstream, tool)) if tool.destructive => {
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
                Some(_) => {
                    tracing::info!(
                        surface = "mcp",
                        service = %service,
                        action = %action,
                        route = "upstream_widget_callback",
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

        if let Some(entry) = svc
            && !tool_execute_builtin_action_allowed(
                entry,
                &action,
                auth_context_from_extensions(&context.extensions),
            )
        {
            let envelope = build_error_extra(
                &service,
                &action,
                "forbidden",
                &format!("action `{action}` for service `{service}` requires `lab:admin` scope"),
                &serde_json::json!({ "required_scopes": ["lab:admin"] }),
            );
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }

        // Elicitation gate: if the action is destructive and the client supports
        // elicitation, ask for confirmation before dispatching.
        if let Some(entry) = svc {
            let is_destructive = entry
                .actions
                .iter()
                .any(|a| a.name == action && a.destructive);
            if is_destructive {
                match elicit_confirm(&context, &service, &action).await {
                    ElicitResult::Confirmed => {}
                    ElicitResult::Declined | ElicitResult::Cancelled => {
                        let envelope = build_error(
                            &service,
                            &action,
                            "confirmation_required",
                            &format!("action `{action}` is destructive — confirm to proceed"),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                    ElicitResult::NotSupported => {
                        // Client does not support elicitation — allow params["confirm"] == true
                        // as a machine-to-machine bypass (mirrors HTTP's handle_action()).
                        if params.get("confirm").and_then(Value::as_bool) != Some(true) {
                            let envelope = build_error(
                                &service,
                                &action,
                                "confirmation_required",
                                &format!(
                                    "action `{action}` is destructive — pass \
                                     {{\"confirm\":true}} in params or use a client \
                                     that supports MCP elicitation"
                                ),
                            );
                            return Ok(CallToolResult::error(vec![Content::text(
                                envelope.to_string(),
                            )]));
                        }
                    }
                    ElicitResult::Failed => {
                        let envelope = build_error(
                            &service,
                            &action,
                            "confirmation_required",
                            &format!(
                                "action `{action}` is destructive — confirmation failed, retry with a client that supports MCP elicitation"
                            ),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                }
            }
        }

        let subject = self.request_subject_log_tag(&context);
        let actor_key = self.request_actor_key(&context);
        let dispatch_action = if svc.is_some() {
            action.as_str()
        } else {
            "call_tool"
        };
        tracing::info!(
            surface = "mcp",
            service,
            action = dispatch_action,
            subject,
            actor_key,
            tool = %service,
            instance = instance.as_deref(),
            param_key_count,
            "dispatch start"
        );

        // Try built-in dispatch first.
        if let Some(entry) = svc {
            tracing::info!(
                surface = "mcp",
                service,
                action = action.as_str(),
                tool = %service,
                route = "builtin",
                "dispatch route selected"
            );
            let params = if service == "gateway" {
                inject_gateway_origin_param(params, self.request_subject(&context))
            } else {
                params
            };
            let result = (entry.dispatch)(action.clone(), params)
                .await
                .map_err(|te| anyhow::Error::from(DispatchError::from(te)));
            let elapsed_ms = start.elapsed().as_millis();
            let input_tokens = estimate_tokens_args(&args);
            let (result, outcome) = format_dispatch_result(
                result,
                &service,
                &action,
                elapsed_ms,
                &subject,
                actor_key,
                input_tokens,
            );
            self.emit_dispatch_notification(&context, &service, &action, elapsed_ms, outcome)
                .await;
            return Ok(result);
        }

        // Fall through to upstream proxy dispatch (raw + subject-scoped +
        // no-dispatcher-wired fallback). The helper returns unconditionally.
        self.call_tool_upstream_impl(
            &service,
            &action,
            raw_arguments,
            start,
            &subject,
            actor_key,
            &context,
        )
        .await
    }
}
