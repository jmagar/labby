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

#[cfg(feature = "gateway")]
use std::time::SystemTime;
use std::{future::Future, pin::Pin, time::Instant};

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use serde_json::Value;

use crate::dispatch::error::ToolError;
#[cfg(feature = "gateway")]
use crate::dispatch::gateway::manager::CallbackToolLookup;
#[cfg(feature = "gateway")]
use crate::dispatch::upstream::types::UpstreamTool;
#[cfg(feature = "gateway")]
use crate::mcp::bound_access::{ProjectExecutionBinding, project_execution_binding};
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_upstream::PreResolvedUpstreamTool;
use crate::mcp::catalog::SERVER_LOGS_TOOL_NAME;
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, CODE_MODE_UI_TOOL_NAME,
    GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME, SETTINGS_TOOL_NAME,
};
#[cfg(feature = "gateway")]
use crate::mcp::catalog_coalesce::schedule_catalog_notification;
#[cfg(feature = "gateway")]
use crate::mcp::catalog_notifications::CatalogNotificationChanges;
use crate::mcp::context::{
    auth_context_from_extensions, propagated_caller_auth, resolve_caller_authorization,
    tool_execute_builtin_action_allowed, tool_execute_scope_allowed,
};
use crate::mcp::envelope::{build_error, build_error_extra};
use crate::mcp::error::DispatchError;
#[cfg(feature = "gateway")]
use crate::mcp::handlers_resources::admin_app_resources_visible;
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel, spawn_dispatch_notification};
#[cfg(feature = "gateway")]
use crate::mcp::permanent_tools::PermanentToolId;
use crate::mcp::result_format::{
    error_result_from_envelope, estimate_tokens_args, format_dispatch_result, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;

#[cfg(feature = "skills")]
pub(super) struct SkillLibraryCallbackBoundary {
    pub(super) identity: labby_auth::VerifiedIdentity,
    pub(super) scopes: Vec<String>,
    pub(super) product_credential_bound: bool,
}

#[cfg(feature = "skills")]
impl std::fmt::Debug for SkillLibraryCallbackBoundary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SkillLibraryCallbackBoundary")
            .field("authenticator", &self.identity.authenticator())
            .field("scope_count", &self.scopes.len())
            .field("product_bound", &self.product_credential_bound)
            .finish()
    }
}

/// Project only host-established authentication facts into the Skill Library
/// app callback. Browser bridge metadata and cookies are deliberately outside
/// this boundary: an MCP App iframe can request an action, but it cannot mint
/// identity, scopes, or callback provenance.
#[cfg(feature = "skills")]
pub(super) fn skill_library_callback_boundary(
    parts: &axum::http::request::Parts,
) -> Result<SkillLibraryCallbackBoundary, ToolError> {
    use axum::http::header::COOKIE;

    let auth = parts
        .extensions
        .get::<labby_auth::auth_context::AuthContext>()
        .ok_or_else(|| ToolError::Forbidden {
            message: "Skill Library requires host-established authentication".to_owned(),
            required_scopes: Vec::new(),
        })?;
    if auth.via_session || parts.headers.contains_key(COOKIE) {
        return Err(ToolError::Forbidden {
            message: "Skill Library app callbacks require bearer authentication".to_owned(),
            required_scopes: vec![
                "lab:read".to_owned(),
                "lab".to_owned(),
                "lab:admin".to_owned(),
            ],
        });
    }
    let identity = parts
        .extensions
        .get::<labby_auth::VerifiedIdentity>()
        .cloned()
        .ok_or_else(|| ToolError::Forbidden {
            message: "Skill Library requires a verified host identity".to_owned(),
            required_scopes: Vec::new(),
        })?;
    let product_credential_bound =
        if identity.authenticator() == labby_auth::Authenticator::ProductCredential {
            let source = parts
                .extensions
                .get::<labby_primitives::product_credential::ProductCredentialGrant>();
            let bound = parts
                .extensions
                .get::<labby_primitives::product_credential::BoundAccessGrant>();
            match source.zip(bound) {
                Some((source, bound))
                    if crate::dispatch::skill_library::auth::product_grants_are_route_bound(
                        source, bound,
                    ) =>
                {
                    true
                }
                _ => {
                    return Err(ToolError::Forbidden {
                        message: "Skill Library product credential binding is invalid".to_owned(),
                        required_scopes: Vec::new(),
                    });
                }
            }
        } else {
            false
        };
    Ok(SkillLibraryCallbackBoundary {
        identity,
        scopes: auth.scopes.clone(),
        product_credential_bound,
    })
}

#[cfg(feature = "skills")]
pub(super) fn skill_library_callback_correlation(
    value: Option<&str>,
) -> Result<crate::dispatch::skill_library::audit::SkillLibraryCorrelationId, ToolError> {
    static REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let value = value.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "mcp-skill-library-{}",
            REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    });
    crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(value).map_err(|()| {
        ToolError::InvalidParam {
            message: "invalid request correlation".to_owned(),
            param: "x-request-id".to_owned(),
        }
    })
}

#[cfg(feature = "skills")]
fn skill_library_safe_callback_correlation(context: &RequestContext<RoleServer>) -> String {
    static REJECTIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let supplied = context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.headers.get("x-request-id"))
        .and_then(|value| value.to_str().ok());
    if let Some(value) = supplied
        && skill_library_callback_correlation(Some(value)).is_ok()
    {
        return value.to_owned();
    }
    format!(
        "mcp-skill-library-rejection-{}",
        REJECTIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[cfg(feature = "gateway")]
enum WidgetCallbackGate {
    Allowed {
        resolved: Box<PreResolvedUpstreamTool>,
        /// True when the callback target is a tool that Code Mode keeps hidden
        /// from `list_tools` (an MCP App sibling, or any exposed tool surfaced
        /// only through the legacy `LABBY_CODE_MODE_WIDGET_CALLBACKS` bypass).
        /// Calling such a hidden tool via the bypass requires the `lab`/
        /// `lab:admin` scope check below. It is `false` only for `DirectMcpApp`
        /// candidates, which are already advertised in `list_tools`.
        requires_scope_check: bool,
    },
    Destructive {
        resolved: Box<PreResolvedUpstreamTool>,
    },
    Ambiguous {
        valid: Vec<String>,
    },
}

fn route_scope_denied_result(service: &str, action: &str, message: String) -> CallToolResult {
    let envelope = build_error(service, action, "route_scope_denied", &message);
    error_result_from_envelope(envelope)
}

#[cfg(feature = "gateway")]
fn retain_route_visible_gateway_status_rows(
    value: &mut Value,
    route_scope: &crate::mcp::route_scope::McpRouteScope,
) {
    let Value::Array(rows) = value else {
        return;
    };
    rows.retain(|row| {
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        let name = row.get("name").and_then(Value::as_str).unwrap_or_default();
        match row.get("source").and_then(Value::as_str) {
            Some("custom_gateway") => route_scope.allows_upstream(id),
            Some("in_process") => {
                route_scope.allows_service(id) || route_scope.allows_service(name)
            }
            _ => false,
        }
    });
}

/// Attach the authenticated MCP subject to gateway mutations without replacing caller values.
#[cfg(feature = "gateway")]
fn inject_gateway_origin_param(action: &str, params: Value, subject: Option<&str>) -> Value {
    if !crate::dispatch::gateway::shared::action_accepts_runtime_owner(action) {
        return params;
    }
    let raw = subject
        .map(|value| format!("mcp:{value}"))
        .unwrap_or_else(|| "mcp:anonymous".to_string());
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    object.insert(
        "owner".to_string(),
        serde_json::json!({
            "surface": "mcp",
            "subject": subject,
            "raw": raw,
        }),
    );
    object.insert("origin".to_string(), Value::String(raw));
    Value::Object(object)
}

#[cfg(all(test, feature = "gateway"))]
mod gateway_origin_tests {
    use serde_json::json;

    use super::inject_gateway_origin_param;

    #[test]
    fn strict_read_only_gateway_actions_do_not_receive_runtime_owner_params() {
        let params = json!({"upstream": "fixture"});
        assert_eq!(
            inject_gateway_origin_param("gateway.skills.list", params.clone(), Some("alice")),
            params
        );
    }

    #[test]
    fn gateway_mutations_keep_mcp_runtime_owner_provenance() {
        let enriched =
            inject_gateway_origin_param("gateway.add", json!({"spec": {}}), Some("alice"));
        assert_eq!(enriched["owner"]["surface"], "mcp");
        assert_eq!(enriched["owner"]["subject"], "alice");
        assert_eq!(enriched["origin"], "mcp:alice");
    }
}

impl LabMcpServer {
    /// Whether this request may receive the HTTP-only Skill Library management
    /// projection. This is an advertisement/admission gate, not authorization:
    /// every action still resolves canonical Access policy in shared dispatch.
    #[cfg(feature = "skills")]
    pub(crate) fn skill_library_http_management_visible(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> bool {
        if self.transport_label != "http" {
            return false;
        }
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            return false;
        };
        parts
            .headers
            .get("x-labby-project-id")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.trim().is_empty())
            && skill_library_callback_boundary(parts).is_ok()
    }

    #[cfg(feature = "skills")]
    async fn skill_library_http_action_allowed(
        &self,
        context: &RequestContext<RoleServer>,
        action: &str,
    ) -> bool {
        self.skill_library_http_management_visible(context)
            && crate::dispatch::artifacts::ACTIONS
                .iter()
                .any(|spec| spec.name == action)
            && self.action_allowed_on_mcp("artifacts", action).await
    }

    async fn mcp_action_policy_denial(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
        action: &str,
        registered: bool,
    ) -> Option<CallToolResponse> {
        #[cfg(not(feature = "skills"))]
        let _ = context;

        let action_allowed = if service == "artifacts" && action.starts_with("artifacts.") {
            #[cfg(feature = "skills")]
            {
                self.skill_library_http_action_allowed(context, action)
                    .await
            }
            #[cfg(not(feature = "skills"))]
            {
                false
            }
        } else {
            self.action_allowed_on_mcp(service, action).await
        };
        if !registered || action_allowed {
            return None;
        }

        let mut extra = serde_json::Map::new();
        if let Some(valid) = self.allowed_mcp_actions(service).await {
            extra.insert(
                "valid".to_string(),
                serde_json::to_value(valid).unwrap_or(Value::Array(Vec::new())),
            );
        }
        #[cfg(feature = "skills")]
        if service == "artifacts" && action.starts_with("artifacts.") {
            extra.insert(
                "correlation_id".to_owned(),
                Value::String(skill_library_safe_callback_correlation(context)),
            );
        }
        let envelope = build_error_extra(
            service,
            action,
            "unknown_action",
            &format!("action `{action}` is not exposed for service `{service}`"),
            &Value::Object(extra),
        );
        Some(error_result_from_envelope(envelope).into())
    }

    #[cfg(feature = "gateway")]
    /// Record one structured failure event for a handled Add Server callback.
    async fn log_add_server_failure(
        &self,
        context: &RequestContext<RoleServer>,
        action: &str,
        kind: &'static str,
        message: &str,
        elapsed_ms: u128,
    ) {
        let subject = self.request_subject_log_tag(context);
        if kind == "internal_error" {
            tracing::error!(
                surface = "mcp",
                service = ADD_SERVER_TOOL_NAME,
                action,
                subject,
                elapsed_ms,
                kind,
                error = %message,
                "Add Server dispatch error"
            );
        } else {
            tracing::warn!(
                surface = "mcp",
                service = ADD_SERVER_TOOL_NAME,
                action,
                subject,
                elapsed_ms,
                kind,
                error = %message,
                "Add Server dispatch error"
            );
        }
        self.emit_dispatch_notification(
            context,
            ADD_SERVER_TOOL_NAME,
            action,
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: if kind == "internal_error" {
                    LoggingLevel::Error
                } else {
                    LoggingLevel::Warning
                },
                kind,
            },
        )
        .await;
    }

    pub(crate) fn log_route_scope_denial(
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

        let actor_key = crate::mcp::context::actor_key_from_extensions(&context.extensions)
            .map(ToOwned::to_owned);
        spawn_dispatch_notification(
            context.peer.clone(),
            actor_key,
            service.to_string(),
            action.to_string(),
            elapsed_ms,
            DispatchLogOutcome::Failure {
                level: LoggingLevel::Warning,
                kind: "route_scope_denied",
            },
        );
    }

    pub(crate) fn boxed_call_tool_response_impl<'a>(
        &'a self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResponse, ErrorData>> + Send + 'a>> {
        Box::pin(self.call_tool_response_impl_inner(request, context))
    }

    #[cfg(feature = "gateway")]
    fn direct_tool_route_scope_denial(
        &self,
        context: &RequestContext<RoleServer>,
        service: &str,
    ) -> Option<CallToolResponse> {
        let is_pre_gate_tool = matches!(
            self.registry.permanent_tools().resolve(service),
            Some(PermanentToolId::CodeMode | PermanentToolId::CodeModeRead)
        ) || service == CODE_MODE_UI_TOOL_NAME
            || service == MCP_APP_TOOL_NAME;
        if self.route_scope.exposes_tools() || is_pre_gate_tool {
            return None;
        }

        const MESSAGE: &str = "MCP Tools are disabled by this loadout; use Code Mode if it is exposed, or ask the operator to enable Tools for this loadout";
        self.log_route_scope_denial(context, service, "call_tool", MESSAGE, 0);
        Some(route_scope_denied_result(service, "call_tool", MESSAGE.to_string()).into())
    }

    #[cfg(test)]
    pub(crate) async fn call_tool_response_impl(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.boxed_call_tool_response_impl(request, context).await
    }

    #[cfg(feature = "gateway")]
    async fn call_project_regular_tool_terminal(
        &self,
        request: CallToolRequestParams,
        context: &RequestContext<RoleServer>,
        binding: Option<(
            &crate::mcp::bound_access::TransportBoundAccessContext,
            &labby_auth::VerifiedIdentity,
        )>,
    ) -> Result<CallToolResponse, ErrorData> {
        let start = Instant::now();
        let _in_flight = crate::mcp::catalog_churn::InFlightToolCall::enter();
        let service = request.name.to_string();
        let subject = self.request_subject_log_tag(context);
        let actor_key = self.request_actor_key(context);
        let param_key_count = request.arguments.as_ref().map_or(0, serde_json::Map::len);
        tracing::info!(
            surface = "mcp",
            service,
            action = "call_tool",
            subject,
            actor_key,
            tool = %service,
            param_key_count,
            route = "project_exact_complete",
            "dispatch start"
        );
        let access_context_unavailable = binding.is_none();
        let result = match (binding, self.gateway_manager.as_deref()) {
            (Some((transport, identity)), Some(manager)) => {
                crate::mcp::tool_execution::execute_transport_bound_project_complete_tool(
                    self.access_runtime.as_ref(),
                    manager,
                    transport,
                    identity,
                    request,
                )
                .await
            }
            _ => Err(crate::mcp::tool_execution::ToolExecutionResolutionError::Unavailable),
        };
        let elapsed_ms = start.elapsed().as_millis();
        match result {
            Ok(result) => {
                self.emit_dispatch_notification(
                    context,
                    &service,
                    "call_tool",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(CallToolResponse::Complete(result))
            }
            Err(error) => {
                use crate::mcp::tool_execution::ToolExecutionResolutionError as E;
                let (kind, message, extra, level) = match error {
                    E::Unavailable => (
                        "not_found",
                        "Tool is unavailable.",
                        None,
                        LoggingLevel::Warning,
                    ),
                    E::QueueUnavailable => (
                        "service_unavailable",
                        "Tool execution is temporarily unavailable.",
                        None,
                        LoggingLevel::Error,
                    ),
                    E::Mcp { kind, code } => (
                        kind,
                        "Upstream tool rejected the call.",
                        Some(serde_json::json!({ "upstream_code": code })),
                        LoggingLevel::Warning,
                    ),
                    E::Transport => (
                        "network_error",
                        "Upstream tool transport failed.",
                        None,
                        LoggingLevel::Error,
                    ),
                    E::Protocol
                    | E::InputRequiredRoundsExceeded
                    | E::UnsupportedTerminalResponse => (
                        "upstream_error",
                        "Upstream tool returned an unsupported response.",
                        None,
                        LoggingLevel::Error,
                    ),
                    E::Timeout => (
                        "timeout",
                        "Upstream tool call timed out.",
                        None,
                        LoggingLevel::Error,
                    ),
                    E::Cancelled => (
                        "cancelled",
                        "Tool execution was cancelled.",
                        None,
                        LoggingLevel::Warning,
                    ),
                    E::Other => (
                        "internal_error",
                        "Tool execution failed.",
                        None,
                        LoggingLevel::Error,
                    ),
                    E::TooLarge => (
                        "response_too_large",
                        "Upstream tool response was too large.",
                        None,
                        LoggingLevel::Error,
                    ),
                };
                self.emit_dispatch_notification(
                    context,
                    &service,
                    "call_tool",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level,
                        kind: if access_context_unavailable {
                            "access_context_unavailable"
                        } else {
                            kind
                        },
                    },
                )
                .await;
                let envelope = extra.as_ref().map_or_else(
                    || build_error(&service, "call_tool", kind, message),
                    |extra| build_error_extra(&service, "call_tool", kind, message, extra),
                );
                Ok(CallToolResponse::Complete(error_result_from_envelope(
                    envelope,
                )))
            }
        }
    }

    async fn call_tool_response_impl_inner(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        #[cfg(feature = "gateway")]
        match project_execution_binding(&context.extensions, SystemTime::now()) {
            ProjectExecutionBinding::Legacy => {}
            ProjectExecutionBinding::Unavailable => {
                return self
                    .call_project_regular_tool_terminal(request, &context, None)
                    .await;
            }
            // Project discovery proves only AssetDiscover. Every Bound call enters the exact
            // AssetUse seam; owned built-ins and synthetics fail closed there until they have
            // their own exact execution authorization.
            ProjectExecutionBinding::Bound {
                transport,
                identity,
            } => {
                #[cfg(feature = "skills")]
                if is_project_artifact_management_call(&request) {
                    if let Some(response) =
                        Box::pin(self.destructive_confirmation_response(&request, &context)).await
                    {
                        return Ok(response);
                    }
                    return Box::pin(self.call_tool_response_dispatch_impl(request, context, true))
                        .await;
                }
                return self
                    .call_project_regular_tool_terminal(
                        request,
                        &context,
                        Some((transport, identity)),
                    )
                    .await;
            }
        }
        let project_owned = false;
        if let Some(response) =
            Box::pin(self.destructive_confirmation_response(&request, &context)).await
        {
            return Ok(response);
        }
        #[cfg(feature = "gateway")]
        if let Some(response) = self.direct_tool_route_scope_denial(&context, request.name.as_ref())
        {
            return Ok(response);
        }
        Box::pin(self.call_tool_response_dispatch_impl(request, context, project_owned)).await
    }

    async fn call_tool_response_dispatch_impl(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
        project_owned: bool,
    ) -> Result<CallToolResponse, ErrorData> {
        let start = Instant::now();
        // Marks the caller's turn as open for the whole dispatch, including
        // every early return below. A catalog notification emitted while this
        // is held invalidates a binding the caller is actively using, so the
        // fanout reports it as `during_tool_call` — the signal that separates
        // harmless catalog movement from the flapping clients actually feel.
        let _in_flight = crate::mcp::catalog_churn::InFlightToolCall::enter();
        let service = request.name.as_ref().to_string();
        // This request remains live until the upstream tail. Keep its large
        // serde value off this already broad dispatch future's stack frame.
        let upstream_request = Box::new(request.clone());
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
            // ── Always-available MCP App control tool. It is root-gateway scoped so a
            // protected subset cannot mutate gateway-global UI visibility.
            if service == MCP_APP_TOOL_NAME {
                if !self.route_scope.is_root() {
                    let elapsed_ms = start.elapsed().as_millis();
                    self.log_route_scope_denial(
                        &context,
                        &service,
                        "call_tool",
                        "MCP App management is only available on the root gateway route",
                        elapsed_ms,
                    );
                    return Ok(route_scope_denied_result(
                        &service,
                        "call_tool",
                        "MCP App management is only available on the root gateway route"
                            .to_string(),
                    )
                    .into());
                }

                let auth = auth_context_from_extensions(&context.extensions);
                let synthetic_action = match args.get("action") {
                    None => "status",
                    Some(Value::String(value)) if value.is_empty() => "status",
                    Some(Value::String(value)) => value.as_str(),
                    Some(_) => {
                        let envelope = build_error_extra(
                            &service,
                            "call_tool",
                            "invalid_param",
                            "MCP App action must be a string",
                            &serde_json::json!({ "param": "action", "expected": "string" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                };
                let params_object = match args.get("params") {
                    None | Some(Value::Null) => None,
                    Some(Value::Object(value)) => Some(value),
                    Some(_) => {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "invalid_param",
                            "MCP App params must be an object",
                            &serde_json::json!({ "param": "params", "expected": "object" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                };
                if !tool_execute_scope_allowed(auth) {
                    let envelope = build_error_extra(
                        &service,
                        synthetic_action,
                        "forbidden",
                        "mcp_app requires one of scopes: lab, lab:admin",
                        &serde_json::json!({ "required_scopes": ["lab", "lab:admin"] }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }

                if let (Some(top), Some(nested)) = (
                    args.get("target"),
                    params_object.and_then(|params| params.get("target")),
                ) {
                    let Some(top) = top.as_str() else {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "invalid_param",
                            "MCP App target must be a string",
                            &serde_json::json!({ "param": "target", "expected": "string" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    };
                    let Some(nested) = nested.as_str() else {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "invalid_param",
                            "MCP App target must be a string",
                            &serde_json::json!({ "param": "params.target", "expected": "string" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    };
                    if top != nested {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "invalid_param",
                            "conflicting MCP App targets were provided",
                            &serde_json::json!({ "param": "target" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                }

                let target = if let Some(value) = args.get("target") {
                    let Some(target) = value.as_str() else {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "invalid_param",
                            "MCP App target must be a string",
                            &serde_json::json!({ "param": "target", "expected": "string" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    };
                    target
                } else if let Some(value) = params_object.and_then(|params| params.get("target")) {
                    let Some(target) = value.as_str() else {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "invalid_param",
                            "MCP App target must be a string",
                            &serde_json::json!({ "param": "params.target", "expected": "string" }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    };
                    target
                } else {
                    "codemode"
                };
                if !matches!(
                    target,
                    "manager"
                        | "codemode"
                        | "gateway_status"
                        | "server_logs"
                        | "add_server"
                        | "settings"
                        | "all"
                ) {
                    let envelope = build_error_extra(
                        &service,
                        synthetic_action,
                        "invalid_param",
                        &format!("unsupported MCP App target `{target}`"),
                        &serde_json::json!({
                            "valid": ["manager", "codemode", "gateway_status", "server_logs", "add_server", "settings", "all"]
                        }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }

                let desired = match synthetic_action {
                    "status" => None,
                    "enable" => Some(true),
                    "disable" => Some(false),
                    _ => {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "unknown_action",
                            &format!("unknown MCP App action `{synthetic_action}`"),
                            &serde_json::json!({ "valid": ["status", "enable", "disable"] }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                };

                if desired.is_some() && !admin_app_resources_visible(auth) {
                    let envelope = build_error_extra(
                        &service,
                        synthetic_action,
                        "forbidden",
                        "changing MCP App state requires lab:admin scope",
                        &serde_json::json!({ "required_scopes": ["lab:admin"] }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }

                let previous_config = match self.gateway_manager.as_ref() {
                    Some(manager) => Some(Box::new(manager.current_config().await)),
                    None => None,
                };
                let previous_code_mode = previous_config.as_ref().map_or_else(
                    || self.code_mode_app_state.is_enabled(),
                    |cfg| cfg.code_mode.mcp_ui_enabled,
                );
                let previous_apps = previous_config.as_ref().map_or_else(
                    labby_runtime::gateway_config::McpAppsConfig::default,
                    |cfg| cfg.mcp_apps,
                );

                let current_config = if let Some(desired) = desired {
                    if let Some(manager) = self.gateway_manager.as_ref() {
                        match manager
                            .set_mcp_app_visibility(
                                target,
                                desired,
                                Some(labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP),
                            )
                            .await
                        {
                            Ok(current) => Some(Box::new(current)),
                            Err(error) => {
                                let envelope =
                                    tool_error_envelope(&service, synthetic_action, &error);
                                return Ok(error_result_from_envelope(envelope).into());
                            }
                        }
                    } else if target == "codemode" {
                        self.code_mode_app_state.set_enabled(desired);
                        if previous_code_mode != desired {
                            schedule_catalog_notification(
                                &self.peers,
                                CatalogNotificationChanges::new(true, true, false),
                                labby_runtime::catalog_notify::SOURCE_MCP_CALL_MCP_APP,
                            );
                        }
                        None
                    } else {
                        let envelope = build_error_extra(
                            &service,
                            synthetic_action,
                            "gateway_unavailable",
                            "non-Code-Mode MCP App visibility requires a live gateway manager",
                            &serde_json::json!({ "target": target }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                } else {
                    previous_config.clone()
                };

                let enabled_code_mode = current_config.as_ref().map_or_else(
                    || self.code_mode_app_state.is_enabled(),
                    |cfg| cfg.code_mode.mcp_ui_enabled,
                );
                let enabled_apps = current_config
                    .as_ref()
                    .map_or(previous_apps, |cfg| cfg.mcp_apps);
                let enabled = match target {
                    "manager" => enabled_apps.manager,
                    "codemode" => enabled_code_mode,
                    "gateway_status" => enabled_apps.gateway_status,
                    "server_logs" => enabled_apps.server_logs,
                    "add_server" => enabled_apps.add_server,
                    "settings" => enabled_apps.settings,
                    "all" => {
                        enabled_apps.manager
                            && enabled_code_mode
                            && enabled_apps.gateway_status
                            && enabled_apps.server_logs
                            && enabled_apps.add_server
                            && enabled_apps.settings
                    }
                    _ => unreachable!("target validated above"),
                };
                let changed = desired.is_some()
                    && match target {
                        "manager" => previous_apps.manager != enabled_apps.manager,
                        "codemode" => previous_code_mode != enabled_code_mode,
                        "gateway_status" => {
                            previous_apps.gateway_status != enabled_apps.gateway_status
                        }
                        "server_logs" => previous_apps.server_logs != enabled_apps.server_logs,
                        "add_server" => previous_apps.add_server != enabled_apps.add_server,
                        "settings" => previous_apps.settings != enabled_apps.settings,
                        "all" => {
                            previous_code_mode != enabled_code_mode || previous_apps != enabled_apps
                        }
                        _ => false,
                    };
                let notification_scheduled = changed;
                tracing::info!(
                    surface = "mcp",
                    service = MCP_APP_TOOL_NAME,
                    action = synthetic_action,
                    subject = self.request_subject_log_tag(&context),
                    target,
                    enabled,
                    changed,
                    notification_scheduled,
                    elapsed_ms = start.elapsed().as_millis(),
                    "Labby MCP App state evaluated"
                );
                let payload = serde_json::json!({
                    "kind": "mcp_app_control",
                    "target": target,
                    "enabled": enabled,
                    "changed": changed,
                    "scope": "gateway",
                    "manager_tool": MCP_APP_TOOL_NAME,
                    "text_tool": CODE_MODE_TOOL_NAME,
                    "ui_tool": CODE_MODE_UI_TOOL_NAME,
                    "apps": {
                        "manager": {
                            "enabled": enabled_apps.manager,
                            "tool": MCP_APP_TOOL_NAME,
                        },
                        "codemode": {
                            "enabled": enabled_code_mode,
                            "tool": CODE_MODE_UI_TOOL_NAME,
                            "text_tool": CODE_MODE_TOOL_NAME,
                        },
                        "gateway_status": {
                            "enabled": enabled_apps.gateway_status,
                            "tool": GATEWAY_STATUS_TOOL_NAME,
                        },
                        "server_logs": {
                            "enabled": enabled_apps.server_logs,
                            "tool": SERVER_LOGS_TOOL_NAME,
                        },
                        "add_server": {
                            "enabled": enabled_apps.add_server,
                            "tool": ADD_SERVER_TOOL_NAME,
                        },
                        "settings": {
                            "enabled": enabled_apps.settings,
                            "tool": SETTINGS_TOOL_NAME,
                        },
                    },
                    "notification_scheduled": notification_scheduled,
                });
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(payload.to_string())]);
                result.structured_content = Some(payload);
                return Ok(result.into());
            }

            // ── Gateway Code Mode execution. Both public names share one backend;
            // only `codemode_ui` is advertised with MCP App metadata. The
            // text-only name resolves through the permanent tool registry so its
            // identity survives upstream churn.
            if matches!(
                self.registry.permanent_tools().resolve(&service),
                Some(PermanentToolId::CodeMode | PermanentToolId::CodeModeRead)
            ) || service == CODE_MODE_UI_TOOL_NAME
            {
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
                    )
                    .into());
                }
                if service == CODE_MODE_UI_TOOL_NAME && !self.code_mode_app_enabled_on_mcp().await {
                    let envelope = build_error_extra(
                        &service,
                        "call_tool",
                        "app_disabled",
                        "the Code Mode MCP App is disabled; use codemode for text-only execution or mcp_app to re-enable it",
                        &serde_json::json!({
                            "text_tool": CODE_MODE_TOOL_NAME,
                            "read_tool": CODE_MODE_READ_TOOL_NAME,
                            "control_tool": MCP_APP_TOOL_NAME,
                        }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
                }
                return self
                    .call_tool_codemode_impl(&service, &args, &context)
                    .await
                    .map(Into::into);
            }

            let handles_add_server = service == ADD_SERVER_TOOL_NAME
                && admin_app_resources_visible(auth_context_from_extensions(&context.extensions))
                && self.add_server_app_available_on_mcp().await;
            if handles_add_server {
                let synthetic_action = if action.is_empty() {
                    "open"
                } else {
                    action.as_str()
                };
                let auth = auth_context_from_extensions(&context.extensions);
                let result = match synthetic_action {
                    "open" => Ok(serde_json::json!({
                        "kind": "add_server",
                        "status": "ready",
                    })),
                    "test" | "create" => {
                        let Some(manager) = &self.gateway_manager else {
                            let message = "gateway manager not wired";
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "internal_error",
                                message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope =
                                build_error(&service, synthetic_action, "internal_error", message);
                            return Ok(error_result_from_envelope(envelope).into());
                        };
                        let gateway_action = if synthetic_action == "test" {
                            "gateway.test"
                        } else {
                            "gateway.add"
                        };
                        if !self.action_allowed_on_mcp("gateway", gateway_action).await {
                            let message = format!(
                                "action `{gateway_action}` is not exposed for service `gateway`"
                            );
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "unknown_action",
                                &message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope = build_error_extra(
                                &service,
                                synthetic_action,
                                "unknown_action",
                                &message,
                                &serde_json::json!({
                                    "canonical_action": gateway_action,
                                    "valid": self.allowed_mcp_actions("gateway").await,
                                }),
                            );
                            return Ok(error_result_from_envelope(envelope).into());
                        }
                        let gateway_entry = self
                            .registry
                            .services()
                            .iter()
                            .find(|entry| entry.name == "gateway");
                        let Some(gateway_entry) = gateway_entry else {
                            let message = "gateway registry entry not wired";
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "internal_error",
                                message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope =
                                build_error(&service, synthetic_action, "internal_error", message);
                            return Ok(error_result_from_envelope(envelope).into());
                        };
                        if !tool_execute_builtin_action_allowed(
                            gateway_entry,
                            gateway_action,
                            &resolve_caller_authorization(
                                auth,
                                self.absent_auth_trust(),
                                propagated_caller_auth(request.meta.as_ref()),
                            ),
                        ) {
                            let message =
                                format!("action `{gateway_action}` requires `lab:admin` scope");
                            self.log_add_server_failure(
                                &context,
                                synthetic_action,
                                "forbidden",
                                &message,
                                start.elapsed().as_millis(),
                            )
                            .await;
                            let envelope = build_error_extra(
                                &service,
                                synthetic_action,
                                "forbidden",
                                &message,
                                &serde_json::json!({ "required_scopes": ["lab:admin"] }),
                            );
                            return Ok(error_result_from_envelope(envelope).into());
                        }
                        let params = inject_gateway_origin_param(
                            gateway_action,
                            params,
                            self.request_subject(&context),
                        );
                        let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                            route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                            oauth_subject: crate::mcp::context::oauth_upstream_subject_for_request(
                                auth_context_from_extensions(&context.extensions),
                                self.request_subject(&context),
                            )
                            .map(|subject| subject.into_owned()),
                        };
                        Box::pin(crate::dispatch::gateway::dispatch_with_manager_scoped(
                            manager,
                            gateway_action,
                            params,
                            enrichment_scope,
                        ))
                        .await
                    }
                    _ => Err(ToolError::UnknownAction {
                        message: format!("unknown Add Server action `{synthetic_action}`"),
                        valid: vec!["open".to_string(), "test".to_string(), "create".to_string()],
                        hint: None,
                    }),
                };
                let result =
                    result.map_err(|error| anyhow::Error::from(DispatchError::from(error)));
                let elapsed_ms = start.elapsed().as_millis();
                let input_tokens = estimate_tokens_args(&args);
                let (result, outcome) = format_dispatch_result(
                    result,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    &self.request_subject_log_tag(&context),
                    self.request_actor_key(&context),
                    input_tokens,
                );
                self.emit_dispatch_notification(
                    &context,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    outcome,
                )
                .await;
                return Ok(result.into());
            }

            let handles_gateway_status = service == GATEWAY_STATUS_TOOL_NAME
                && admin_app_resources_visible(auth_context_from_extensions(&context.extensions))
                && self.gateway_status_app_available_on_mcp().await;
            if handles_gateway_status {
                let synthetic_action = if action.is_empty() {
                    "open"
                } else {
                    action.as_str()
                };
                let result = match synthetic_action {
                    "open" | "refresh" => {
                        let manager = self
                            .gateway_manager
                            .as_ref()
                            .expect("availability requires a gateway manager");
                        let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                            route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                            oauth_subject: crate::mcp::context::oauth_upstream_subject_for_request(
                                auth_context_from_extensions(&context.extensions),
                                self.request_subject(&context),
                            )
                            .map(|subject| subject.into_owned()),
                        };
                        if synthetic_action == "refresh" {
                            drop(
                                manager
                                    .refresh_gateway_status_catalog(&enrichment_scope, None)
                                    .await,
                            );
                        }
                        Box::pin(crate::dispatch::gateway::dispatch_with_manager_scoped(
                            manager,
                            "gateway.list",
                            serde_json::json!({}),
                            enrichment_scope,
                        ))
                        .await
                        .map(|mut value| {
                            retain_route_visible_gateway_status_rows(&mut value, &self.route_scope);
                            value
                        })
                    }
                    _ => Err(ToolError::UnknownAction {
                        message: format!("unknown Gateway Status action `{synthetic_action}`"),
                        valid: vec!["open".to_string(), "refresh".to_string()],
                        hint: None,
                    }),
                };
                let result =
                    result.map_err(|error| anyhow::Error::from(DispatchError::from(error)));
                let elapsed_ms = start.elapsed().as_millis();
                let input_tokens = estimate_tokens_args(&args);
                let (result, outcome) = format_dispatch_result(
                    result,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    &self.request_subject_log_tag(&context),
                    self.request_actor_key(&context),
                    input_tokens,
                );
                self.emit_dispatch_notification(
                    &context,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    outcome,
                )
                .await;
                return Ok(result.into());
            }

            let handles_settings = service == SETTINGS_TOOL_NAME
                && self.mcp_apps_config().await.settings
                && admin_app_resources_visible(auth_context_from_extensions(&context.extensions))
                && self.route_scope.allows_service("setup")
                && self.service_visible_on_mcp("setup").await;
            if handles_settings {
                let synthetic_action = if action.is_empty() {
                    "open"
                } else {
                    action.as_str()
                };
                let setup_action = match synthetic_action {
                    "open" | "schema" => Some("settings.schema"),
                    "state" => Some("settings.state"),
                    "config.update" => Some("settings.config.update"),
                    "env.update" => Some("settings.env.update"),
                    _ => None,
                };
                let result = if let Some(setup_action) = setup_action {
                    Box::pin(crate::dispatch::setup::dispatch(setup_action, params)).await
                } else {
                    Err(ToolError::UnknownAction {
                        message: format!("unknown Settings action `{synthetic_action}`"),
                        valid: vec!["open", "schema", "state", "config.update", "env.update"]
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                        hint: None,
                    })
                };
                let result =
                    result.map_err(|error| anyhow::Error::from(DispatchError::from(error)));
                let elapsed_ms = start.elapsed().as_millis();
                let input_tokens = estimate_tokens_args(&args);
                let (result, outcome) = format_dispatch_result(
                    result,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    &self.request_subject_log_tag(&context),
                    self.request_actor_key(&context),
                    input_tokens,
                );
                self.emit_dispatch_notification(
                    &context,
                    &service,
                    synthetic_action,
                    elapsed_ms,
                    outcome,
                )
                .await;
                return Ok(result.into());
            }
        }

        if svc.is_some() && !self.route_scope.allows_service(&service) {
            let elapsed_ms = start.elapsed().as_millis();
            let message = format!("service `{service}` is not exposed on this MCP route");
            self.log_route_scope_denial(&context, &service, &action, &message, elapsed_ms);
            return Ok(route_scope_denied_result(&service, &action, message).into());
        }

        if svc.is_some() && !self.service_visible_on_mcp(&service).await {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` is not enabled on the mcp surface"),
            );
            return Ok(error_result_from_envelope(envelope).into());
        }

        if service == "skills" && svc.is_none() {
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                "service `skills` is not enabled on the mcp surface",
            );
            return Ok(error_result_from_envelope(envelope).into());
        }

        if let Some(response) =
            Box::pin(self.mcp_action_policy_denial(&context, &service, &action, svc.is_some()))
                .await
        {
            return Ok(response);
        }

        // Upstream widget-callback resolution is a gateway-only concern (it
        // proxies to upstream MCP tools). Without the gateway feature there are
        // no upstream tools, so this resolution and the upstream tail below are
        // both compiled out.
        #[cfg(feature = "gateway")]
        let mut resolved_upstream_tool = None;
        #[cfg(feature = "gateway")]
        if self.code_mode_visibility().await.hides_raw_tools() && service != SERVER_LOGS_TOOL_NAME {
            let widget_callback = if svc.is_none() {
                match self.resolve_widget_callback_gate(&service, &context).await {
                    Ok(gate) => gate,
                    Err(err) => {
                        let envelope = tool_error_envelope(&service, "call_tool", &err);
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                }
            } else {
                None
            };
            match widget_callback {
                Some(WidgetCallbackGate::Destructive { resolved }) => {
                    if !tool_execute_scope_allowed(auth_context_from_extensions(
                        &context.extensions,
                    )) {
                        let envelope = build_error_extra(
                            &service,
                            &action,
                            "forbidden",
                            "destructive MCP App callbacks require one of scopes: lab, lab:admin",
                            &serde_json::json!({
                                "required_scopes": ["lab", "lab:admin"],
                            }),
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                    resolved_upstream_tool = Some(*resolved);
                }
                Some(WidgetCallbackGate::Ambiguous { valid }) => {
                    let envelope = build_error_extra(
                        &service,
                        &action,
                        "ambiguous_tool",
                        &format!("tool `{service}` matched multiple MCP App sibling tools"),
                        &serde_json::json!({ "valid": valid }),
                    );
                    return Ok(error_result_from_envelope(envelope).into());
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
                        return Ok(error_result_from_envelope(envelope).into());
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
                    return Ok(error_result_from_envelope(envelope).into());
                }
            }
        }

        if matches!(service.as_str(), "skills" | "artifacts")
            && !matches!(action.as_str(), "help" | "schema")
            && !resolve_caller_authorization(
                auth_context_from_extensions(&context.extensions),
                self.absent_auth_trust(),
                propagated_caller_auth(request.meta.as_ref()),
            )
            .can_read()
        {
            let envelope = build_error_extra(
                &service,
                &action,
                "forbidden",
                "skills require one of scopes: lab:read, lab, lab:admin",
                &serde_json::json!({
                    "required_scopes": ["lab:read", "lab", "lab:admin"]
                }),
            );
            return Ok(error_result_from_envelope(envelope).into());
        }

        if let Some(entry) = svc
            && !tool_execute_builtin_action_allowed(
                entry,
                &action,
                &resolve_caller_authorization(
                    auth_context_from_extensions(&context.extensions),
                    self.absent_auth_trust(),
                    propagated_caller_auth(request.meta.as_ref()),
                ),
            )
        {
            // This return precedes the `dispatch start` log, so without this
            // the denial produces NO server-side telemetry at all. `transport`
            // is the field that makes it diagnosable: the in-process peer
            // transport denies every `requires_admin` builtin by design, and
            // "why is gateway.add forbidden under codemode" is otherwise
            // unanswerable from the logs.
            tracing::warn!(
                surface = "mcp",
                service = %service,
                action = %action,
                subject = self.request_subject_log_tag(&context),
                transport = self.transport_label,
                elapsed_ms = start.elapsed().as_millis(),
                kind = "forbidden",
                "builtin action denied by admin gate"
            );
            let envelope = build_error_extra(
                &service,
                &action,
                "forbidden",
                &format!("action `{action}` for service `{service}` requires `lab:admin` scope"),
                &serde_json::json!({ "required_scopes": ["lab:admin"] }),
            );
            return Ok(error_result_from_envelope(envelope).into());
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
                return self
                    .call_snippets_promote_impl(
                        &action, params, &args, start, &subject, actor_key, &context,
                    )
                    .await
                    .map(Into::into);
            }
            let result = if self.registry.dispatch_capability(&service)
                == Some(crate::registry::DispatchCapability::CallerBound)
                && !matches!(action.as_str(), "help" | "schema")
            {
                self.dispatch_caller_bound_service(
                    &service,
                    &action,
                    params,
                    &context,
                    request.meta.as_ref(),
                )
                .await
            } else if service == "artifacts" {
                #[cfg(feature = "skills")]
                {
                    self.dispatch_artifact_tool_boxed(
                        &context,
                        request.meta.as_ref(),
                        &action,
                        params,
                    )
                    .await
                }
                #[cfg(not(feature = "skills"))]
                {
                    (entry.dispatch)(action.clone(), params).await
                }
            } else if service == "gateway" {
                #[cfg(feature = "gateway")]
                {
                    let Some(manager) = &self.gateway_manager else {
                        let envelope = build_error(
                            &service,
                            &action,
                            "internal_error",
                            "gateway manager not wired",
                        );
                        return Ok(error_result_from_envelope(envelope).into());
                    };
                    let params = inject_gateway_origin_param(
                        &action,
                        params,
                        self.request_subject(&context),
                    );
                    let enrichment_scope = crate::dispatch::gateway::GatewayEnrichmentScope {
                        route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
                        oauth_subject: crate::mcp::context::oauth_upstream_subject_for_request(
                            auth_context_from_extensions(&context.extensions),
                            self.request_subject(&context),
                        )
                        .map(|subject| subject.into_owned()),
                    };
                    Box::pin(crate::dispatch::gateway::dispatch_with_manager_scoped(
                        manager,
                        &action,
                        params,
                        enrichment_scope,
                    ))
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
            return Ok(result.into());
        }

        // Fall through to upstream proxy dispatch (raw + subject-scoped +
        // no-dispatcher-wired fallback). The helper returns unconditionally.
        // The upstream proxy only exists with the gateway feature; without it an
        // unresolved service name is simply not found.
        #[cfg(feature = "gateway")]
        {
            if project_owned {
                let elapsed_ms = start.elapsed().as_millis();
                let envelope =
                    build_error(&service, "call_tool", "not_found", "Tool is unavailable.");
                self.emit_dispatch_notification(
                    &context,
                    &service,
                    "call_tool",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "not_found",
                    },
                )
                .await;
                return Ok(error_result_from_envelope(envelope).into());
            }
            self.boxed_call_tool_upstream_impl(
                &service,
                &action,
                *upstream_request,
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
            let _ = (upstream_request, actor_key, start);
            let envelope = build_error(
                &service,
                &action,
                "not_found",
                &format!("service `{service}` not found"),
            );
            Ok(error_result_from_envelope(envelope).into())
        }
    }

    async fn destructive_confirmation_response(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> Option<CallToolResponse> {
        #[cfg(feature = "gateway")]
        {
            let auth = auth_context_from_extensions(&context.extensions);
            if !tool_execute_scope_allowed(auth)
                && self.code_mode_visibility().await.hides_raw_tools()
                && request.name.as_ref() != SERVER_LOGS_TOOL_NAME
                && matches!(
                    self.resolve_widget_callback_gate(request.name.as_ref(), context)
                        .await,
                    Ok(Some(WidgetCallbackGate::Destructive { .. }))
                )
            {
                // Authorization precedes elicitation: a read-only caller must not
                // be prompted to confirm a destructive app operation they cannot
                // execute. The normal call path will return `forbidden`.
                return None;
            }
        }

        if !self.tool_request_is_destructive(request, context).await {
            return None;
        }

        let service = request.name.as_ref();
        let action = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("call_tool");
        let auth = auth_context_from_extensions(&context.extensions);
        let catalog = self
            .registry
            .services()
            .iter()
            .flat_map(|entry| {
                entry.actions.iter().map(move |candidate| {
                    format!(
                        "{}:{}:{}:{}",
                        entry.name, candidate.name, candidate.requires_admin, candidate.destructive
                    )
                })
            })
            .collect::<Vec<_>>();
        let material = serde_json::json!({
            "service": service, "action": action, "arguments": request.arguments,
            "issuer": auth.map(|value| value.issuer.as_str()),
            "subject": auth.map(|value| value.sub.as_str()),
            "actor": auth.and_then(|value| value.actor_key.as_deref()),
            "scopes": auth.map(|value| value.scopes.as_slice()),
            "transport": self.transport_label,
            // Streamable HTTP constructs a fresh LabMcpServer per POST, so its
            // relay id is request-scoped rather than session-scoped. Binding it
            // would reject the protocol-mandated input_required retry.
            "mcp_session": (self.transport_label != "http").then_some(self.relay_session_id),
            "route": format!("{:?}", self.route_scope), "catalog": catalog,
        });
        let binding = labby_runtime::artifacts::canonical_json::digest(&material)
            .unwrap_or_else(|_| "invalid-confirmation-binding".to_string());
        let owner_material = serde_json::json!({
            "issuer": auth.map(|value| value.issuer.as_str()),
            "subject": auth.map(|value| value.sub.as_str()),
            "actor": auth.and_then(|value| value.actor_key.as_deref()),
            "transport": self.transport_label,
            "mcp_session": (self.transport_label != "http").then_some(self.relay_session_id),
            "route": format!("{:?}", self.route_scope),
        });
        let owner = labby_runtime::artifacts::canonical_json::digest(&owner_material)
            .unwrap_or_else(|_| "invalid-confirmation-owner".to_string());
        match crate::mcp::elicitation::destructive_confirmation(
            request, service, action, &binding, &owner,
        ) {
            crate::mcp::elicitation::DestructiveConfirmation::Proceed => None,
            crate::mcp::elicitation::DestructiveConfirmation::InputRequired(result) => {
                Some(CallToolResponse::InputRequired(result))
            }
            crate::mcp::elicitation::DestructiveConfirmation::Refused => {
                let envelope = build_error(
                    service,
                    action,
                    "confirmation_required",
                    &format!("action `{action}` is destructive — confirm to proceed"),
                );
                Some(error_result_from_envelope(envelope).into())
            }
        }
    }

    /// Complete-only test/internal adapter. Protocol callers use
    /// [`Self::call_tool_response_impl`] so MRTR and task result variants are
    /// preserved on the wire.
    #[cfg(test)]
    pub(crate) async fn call_tool_impl(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match self.call_tool_response_impl(request, context).await? {
            CallToolResponse::Complete(result) => Ok(result),
            CallToolResponse::InputRequired(_) => Err(ErrorData::internal_error(
                "complete-only call adapter received input_required",
                None,
            )),
            CallToolResponse::Task(_) => Err(ErrorData::internal_error(
                "complete-only call adapter received task result",
                None,
            )),
            _ => Err(ErrorData::internal_error(
                "complete-only call adapter received unknown result type",
                None,
            )),
        }
    }
}

#[cfg(not(feature = "gateway"))]
impl LabMcpServer {
    /// Resolve whether a built-in tool call needs RC-native MRTR elicitation.
    ///
    /// Gateway-only synthetic and upstream tools are unavailable in this
    /// feature slice, so the registry is the complete classification source.
    pub(crate) async fn tool_request_is_destructive(
        &self,
        request: &CallToolRequestParams,
        _context: &RequestContext<RoleServer>,
    ) -> bool {
        let service = request.name.as_ref();
        let action = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("");

        self.registry
            .services()
            .iter()
            .find(|entry| entry.name == service)
            .is_some_and(|entry| {
                entry
                    .actions
                    .iter()
                    .any(|candidate| candidate.name == action && candidate.destructive)
            })
    }
}

#[cfg(feature = "gateway")]
impl LabMcpServer {
    /// Resolve whether a tool call needs RC-native MRTR elicitation.
    ///
    /// This is deliberately classification-only. The protocol handler returns
    /// `input_required`; the normal dispatcher never starts an in-flight
    /// server-to-client elicitation RPC.
    pub(crate) async fn tool_request_is_destructive(
        &self,
        request: &CallToolRequestParams,
        context: &RequestContext<RoleServer>,
    ) -> bool {
        let service = request.name.as_ref();
        let action = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("action"))
            .and_then(Value::as_str)
            .unwrap_or("");

        if let Some(entry) = self
            .registry
            .services()
            .iter()
            .find(|entry| entry.name == service)
        {
            return entry
                .actions
                .iter()
                .any(|candidate| candidate.name == action && candidate.destructive);
        }

        #[cfg(feature = "gateway")]
        {
            if service == ADD_SERVER_TOOL_NAME {
                let gateway_action = match action {
                    "test" => Some("gateway.test"),
                    "create" => Some("gateway.add"),
                    _ => None,
                };
                return gateway_action.is_some_and(|gateway_action| {
                    self.registry
                        .services()
                        .iter()
                        .find(|entry| entry.name == "gateway")
                        .is_some_and(|entry| {
                            entry.actions.iter().any(|candidate| {
                                candidate.name == gateway_action && candidate.destructive
                            })
                        })
                });
            }

            if service == SETTINGS_TOOL_NAME {
                let setup_action = match action {
                    "config.update" => Some("settings.config.update"),
                    "env.update" => Some("settings.env.update"),
                    _ => None,
                };
                return setup_action.is_some_and(|setup_action| {
                    self.registry
                        .services()
                        .iter()
                        .find(|entry| entry.name == "setup")
                        .is_some_and(|entry| {
                            entry.actions.iter().any(|candidate| {
                                candidate.name == setup_action && candidate.destructive
                            })
                        })
                });
            }

            if self.code_mode_visibility().await.hides_raw_tools()
                && service != SERVER_LOGS_TOOL_NAME
            {
                return matches!(
                    self.resolve_widget_callback_gate(service, context).await,
                    Ok(Some(WidgetCallbackGate::Destructive { .. }))
                );
            }

            let Some(manager) = &self.gateway_manager else {
                return false;
            };
            let owner = self.request_runtime_owner(context);
            let oauth_subject = crate::mcp::context::oauth_upstream_subject_for_request(
                auth_context_from_extensions(&context.extensions),
                self.request_subject(context),
            );
            return manager
                .resolve_raw_upstream_tool_scoped(
                    service,
                    self.route_scope.allowed_upstreams(),
                    Some(&owner),
                    oauth_subject.as_deref(),
                )
                .await
                .is_ok_and(|(_, tool)| tool.destructive);
        }

        #[cfg(not(feature = "gateway"))]
        false
    }

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

#[cfg(feature = "skills")]
fn is_project_artifact_management_call(request: &CallToolRequestParams) -> bool {
    request.name.as_ref() == "artifacts"
        && request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("action"))
            .and_then(Value::as_str)
            .is_some_and(|action| {
                crate::dispatch::skill_library::catalog::LOCAL_ACTIONS
                    .iter()
                    .any(|candidate| candidate.name == action)
            })
}

#[cfg(all(test, feature = "skills"))]
mod project_artifact_routing_tests {
    use rmcp::model::CallToolRequestParams;
    use serde_json::json;

    use super::is_project_artifact_management_call;

    fn request(name: &str, action: &str) -> CallToolRequestParams {
        CallToolRequestParams::new(name.to_owned()).with_arguments(
            json!({"action": action, "params": {}})
                .as_object()
                .expect("fixture arguments")
                .clone(),
        )
    }

    #[test]
    fn project_artifact_library_calls_use_the_access_authorized_builtin_path() {
        assert!(is_project_artifact_management_call(&request(
            "artifacts",
            "artifacts.import",
        )));
        assert!(!is_project_artifact_management_call(&request(
            "artifacts",
            "artifacts.search_remote",
        )));
        assert!(!is_project_artifact_management_call(&request(
            "gateway",
            "artifacts.import",
        )));
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
    let (upstream_name, tool) = candidates.into_iter().next().expect("checked len");
    let resolved: Box<PreResolvedUpstreamTool> = PreResolvedUpstreamTool {
        upstream_name,
        tool,
        route,
    }
    .into();
    if resolved.tool.destructive {
        return Some(WidgetCallbackGate::Destructive { resolved });
    }

    Some(WidgetCallbackGate::Allowed {
        resolved,
        requires_scope_check,
    })
}

#[cfg(all(test, feature = "skills"))]
#[path = "call_tool/skill_library_callback_tests.rs"]
mod skill_library_callback_tests;
