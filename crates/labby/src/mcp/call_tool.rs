//! `call_tool` dispatch entry: arg parse + service lookup, the gateway
//! meta-tool routing, the post-meta-tool gates
//! (visibility / action-allowed / code_mode-hidden / admin-scope /
//! destructive elicitation), the builtin dispatch branch, and the
//! fall-through to the upstream proxy tail.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as an inherent
//! `impl LabMcpServer` method. The `ServerHandler` trait impl in
//! `server.rs` keeps a one-line delegator.
//!
//! Preserves the exact early-return ordering (codemode → visibility → action →
//! code_mode-hidden → admin-scope → elicitation → builtin → upstream tail). The
//! codemode and upstream branches live in
//! `call_tool_codemode.rs` / `call_tool_upstream.rs`. No behavior change.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, LoggingLevel, LoggingMessageNotificationParam,
};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::dispatch::error::ToolError;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::CallbackToolLookup;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::types::UpstreamTool;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_upstream::PreResolvedUpstreamTool;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::CODE_MODE_TOOL_NAME;
use crate::mcp::context::{
    auth_context_from_extensions, tool_execute_builtin_action_allowed, tool_execute_scope_allowed,
};
use crate::mcp::elicitation::{ElicitResult, elicit_confirm};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::error::DispatchError;
use crate::mcp::result_format::{
    estimate_tokens_args, format_dispatch_result, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "gateway")]
enum WidgetCallbackGate {
    Allowed {
        resolved: Box<PreResolvedUpstreamTool>,
        /// True when the callback target is a tool that Code Mode keeps hidden
        /// from `list_tools` (an MCP App sibling, or any exposed tool surfaced
        /// only through the legacy `LAB_CODE_MODE_WIDGET_CALLBACKS` bypass).
        /// Calling such a hidden tool via the bypass requires the `lab`/
        /// `lab:admin` scope check below. It is `false` only for `DirectMcpApp`
        /// candidates, which are already advertised in `list_tools`.
        requires_scope_check: bool,
    },
    Ambiguous {
        valid: Vec<String>,
    },
    Destructive,
}

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

        #[cfg(feature = "gateway")]
        {
            // ── Gateway `codemode` tool: run caller's JS in the subprocess sandbox.
            if service == CODE_MODE_TOOL_NAME {
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
                return self
                    .call_tool_codemode_impl(&service, &args, &context)
                    .await;
            }
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

        // Upstream widget-callback resolution is a gateway-only concern (it
        // proxies to upstream MCP tools). Without the gateway feature there are
        // no upstream tools, so this resolution and the upstream tail below are
        // both compiled out.
        #[cfg(feature = "gateway")]
        let mut resolved_upstream_tool = None;
        #[cfg(feature = "gateway")]
        if self.code_mode_visibility().await.hides_raw_tools() {
            let widget_callback = if svc.is_none() {
                match self.resolve_widget_callback_gate(&service, &context).await {
                    Ok(gate) => gate,
                    Err(err) => {
                        let envelope = tool_error_envelope(&service, "call_tool", &err);
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                }
            } else {
                None
            };
            match widget_callback {
                Some(WidgetCallbackGate::Destructive) => {
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
                Some(WidgetCallbackGate::Ambiguous { valid }) => {
                    let envelope = build_error_extra(
                        &service,
                        &action,
                        "ambiguous_tool",
                        &format!("tool `{service}` matched multiple MCP App sibling tools"),
                        &serde_json::json!({ "valid": valid }),
                    );
                    return Ok(CallToolResult::error(vec![Content::text(
                        envelope.to_string(),
                    )]));
                }
                Some(WidgetCallbackGate::Allowed {
                    resolved,
                    requires_scope_check,
                }) => {
                    if requires_scope_check
                        && !tool_execute_scope_allowed(auth_context_from_extensions(
                            &context.extensions,
                        ))
                    {
                        let envelope = build_error_extra(
                            &service,
                            &action,
                            "forbidden",
                            "hidden-tool widget callbacks require one of scopes: lab, lab:admin",
                            &serde_json::json!({
                                "required_scopes": ["lab", "lab:admin"],
                            }),
                        );
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                    tracing::info!(
                        surface = "mcp",
                        service = %service,
                        action = %action,
                        upstream = %resolved.upstream_name,
                        route = resolved.route,
                        "code_mode raw-tool gate bypassed for MCP App widget callback"
                    );
                    resolved_upstream_tool = Some(*resolved);
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
            #[cfg(feature = "gateway")]
            if service == "snippets" && action == "snippets.promote" {
                let Some(manager) = &self.gateway_manager else {
                    let envelope = build_error(
                        &service,
                        &action,
                        "internal_error",
                        "gateway manager not wired",
                    );
                    return Ok(CallToolResult::error(vec![Content::text(
                        envelope.to_string(),
                    )]));
                };
                let auth = auth_context_from_extensions(&context.extensions);
                let capability_filter_fingerprint = self
                    .route_scope
                    .allowed_upstreams()
                    .map(|allowed| {
                        crate::dispatch::gateway::code_mode::ToolScope::scoped_namespaces(
                            allowed.iter().cloned().collect(),
                            Vec::new(),
                        )
                        .fingerprint()
                    })
                    .unwrap_or_else(|| {
                        crate::dispatch::gateway::code_mode::ToolScope::default().fingerprint()
                    });
                let promotion_context =
                    crate::dispatch::snippets::dispatch::SnippetPromotionContext {
                        actor_key: actor_key.map(ToOwned::to_owned),
                        is_admin: auth.is_none_or(|auth| {
                            auth.scopes.iter().any(|scope| scope == "lab:admin")
                        }),
                        route_scope: self.route_scope.label(),
                        capability_filter_fingerprint,
                    };
                let result =
                    crate::dispatch::snippets::dispatch::dispatch_with_manager_and_context(
                        manager,
                        &action,
                        params,
                        Some(promotion_context),
                    )
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
            let result = if service == "gateway" {
                #[cfg(feature = "gateway")]
                {
                    let Some(manager) = &self.gateway_manager else {
                        let envelope = build_error(
                            &service,
                            &action,
                            "internal_error",
                            "gateway manager not wired",
                        );
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    };
                    let params =
                        inject_gateway_origin_param(params, self.request_subject(&context));
                    let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                        route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                    };
                    crate::dispatch::gateway::dispatch_with_manager_scoped(
                        manager,
                        &action,
                        params,
                        enrichment_scope,
                    )
                    .await
                }
                #[cfg(not(feature = "gateway"))]
                {
                    (entry.dispatch)(action.clone(), params).await
                }
            } else {
                (entry.dispatch)(action.clone(), params).await
            };
            let result = result.map_err(|te| anyhow::Error::from(DispatchError::from(te)));
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
        // The upstream proxy only exists with the gateway feature; without it an
        // unresolved service name is simply not found.
        #[cfg(feature = "gateway")]
        {
            self.call_tool_upstream_impl(
                &service,
                &action,
                raw_arguments,
                resolved_upstream_tool,
                start,
                &subject,
                actor_key,
                &context,
            )
            .await
        }
        #[cfg(not(feature = "gateway"))]
        {
            let _ = (raw_arguments, actor_key, start);
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` not found"),
            );
            Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]))
        }
    }
}

#[cfg(feature = "gateway")]
impl LabMcpServer {
    async fn resolve_widget_callback_gate(
        &self,
        service: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<WidgetCallbackGate>, ToolError> {
        let Some(manager) = &self.gateway_manager else {
            return Ok(None);
        };
        let owner = self.request_runtime_owner(context);
        let oauth_subject = crate::mcp::context::oauth_upstream_subject_for_request(
            auth_context_from_extensions(&context.extensions),
            self.request_subject(context),
        );
        let allowed = self.route_scope.allowed_upstreams();

        if self.code_mode_widget_callbacks_enabled() {
            let candidates = manager
                .resolve_widget_callback_tool_candidates_scoped(
                    service,
                    allowed,
                    Some(&owner),
                    oauth_subject.as_deref(),
                    CallbackToolLookup::LegacyAnyExposed,
                )
                .await?;
            // Legacy mode surfaces ANY exposed non-destructive upstream tool,
            // including ones with no MCP App UI resource that are therefore NOT
            // advertised in `list_tools`. Calling such a hidden tool through the
            // bypass must require the `lab`/`lab:admin` scope check, so this path
            // sets `requires_scope_check = true` (matching the sibling path),
            // rather than the `false` that is only correct for advertised
            // `DirectMcpApp` candidates.
            return Ok(classify_widget_callback_candidates(
                "upstream_widget_callback_legacy",
                true,
                candidates,
            ));
        }

        let direct_candidates = manager
            .resolve_widget_callback_tool_candidates_scoped(
                service,
                allowed,
                Some(&owner),
                oauth_subject.as_deref(),
                CallbackToolLookup::DirectMcpApp,
            )
            .await?;
        if !direct_candidates.is_empty() {
            return Ok(classify_widget_callback_candidates(
                "upstream_widget_callback",
                false,
                direct_candidates,
            ));
        }

        let sibling_candidates = manager
            .resolve_widget_callback_tool_candidates_scoped(
                service,
                allowed,
                Some(&owner),
                oauth_subject.as_deref(),
                CallbackToolLookup::SiblingOfMcpApp,
            )
            .await?;
        Ok(classify_widget_callback_candidates(
            "upstream_widget_sibling_callback",
            true,
            sibling_candidates,
        ))
    }

    fn code_mode_widget_callbacks_enabled(&self) -> bool {
        #[cfg(test)]
        if self.code_mode_widget_callbacks_enabled_for_test {
            return true;
        }

        crate::config::code_mode_widget_callbacks_enabled()
    }
}

#[cfg(feature = "gateway")]
fn classify_widget_callback_candidates(
    route: &'static str,
    requires_scope_check: bool,
    candidates: Vec<(String, UpstreamTool)>,
) -> Option<WidgetCallbackGate> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() > 1 {
        let valid = candidates
            .iter()
            .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
            .collect();
        return Some(WidgetCallbackGate::Ambiguous { valid });
    }
    if candidates
        .iter()
        .any(|(_, candidate)| candidate.destructive)
    {
        return Some(WidgetCallbackGate::Destructive);
    }

    let (upstream_name, tool) = candidates.into_iter().next().expect("checked len");
    Some(WidgetCallbackGate::Allowed {
        resolved: PreResolvedUpstreamTool {
            upstream_name,
            tool,
            route,
        }
        .into(),
        requires_scope_check,
    })
}
