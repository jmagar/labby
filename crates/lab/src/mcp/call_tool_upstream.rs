//! Upstream-proxy tail of `call_tool`: raw upstream proxy + subject-scoped
//! upstream proxy + the no-dispatcher-wired fallback.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.5`) as a single
//! inherent `impl LabMcpServer` method. It owns the ENTIRE fall-through
//! tail and returns unconditionally — the raw and subject-scoped branches
//! are conditional `if let` blocks that fall through when unmatched, so
//! the fallback must stay inside this method (do not signal "didn't match"
//! via `Option`).
//!
//! Seam contract (Revision 2 finding #1): side effects preserved
//! byte-identically — `record_failure` ×3, `record_success` ×1,
//! `notify_catalog_changes` ×3 (raw arms only), `emit_dispatch_notification`
//! at the resolution-fail, three raw arms, two subject-scoped arms, and the
//! fallback. The subject-scoped branch intentionally has NO `record_*`.
//!
//! `normalize_upstream_result` lives in `upstream.rs` (Revision 2 / #2).
//! No behavior change.

use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolRequestParams, CallToolResult, Content, JsonObject, LoggingLevel};
use rmcp::service::RequestContext;

use crate::mcp::context::{auth_context_from_extensions, oauth_upstream_subject_for_request};
use crate::mcp::envelope::build_error;
use crate::mcp::error::canonical_kind;
use crate::mcp::logging::DispatchLogOutcome;
use crate::mcp::result_format::{
    estimate_tokens, estimate_tokens_args, format_dispatch_result, tool_error_envelope,
};
use crate::mcp::server::LabMcpServer;
use crate::mcp::upstream::normalize_upstream_result;

use crate::config::UpstreamConfig;
use crate::dispatch::upstream::types::UpstreamTool;

impl LabMcpServer {
    /// Upstream-proxy tail. Reached by fall-through from `call_tool_impl`
    /// when `svc.is_none()`. Owns raw + subject-scoped proxy branches and
    /// the no-dispatcher-wired fallback; returns unconditionally.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn call_tool_upstream_impl(
        &self,
        service: &str,
        action: &str,
        raw_arguments: Option<JsonObject>,
        resolved_upstream_tool: Option<(String, UpstreamTool)>,
        start: Instant,
        subject: &str,
        actor_key: Option<&str>,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        // Upstream tools don't use lab's action/params wrapper — they receive
        // raw arguments. Use "call_tool" as the action label for logging/envelopes.
        let upstream_action = "call_tool";
        let upstream_capability = "tools";
        let upstream_operation = "tool.call";
        let raw_runtime_owner = self.request_runtime_owner(context);
        let raw_oauth_subject = oauth_upstream_subject_for_request(
            auth_context_from_extensions(&context.extensions),
            self.request_subject(context),
        );
        let pre_resolved_upstream = resolved_upstream_tool
            .as_ref()
            .map(|(upstream_name, _)| upstream_name.clone());
        let route_scoped_oauth_configs = self.route_scoped_oauth_upstream_configs().await;
        let pre_resolved_oauth_config: Option<UpstreamConfig> = raw_oauth_subject
            .as_ref()
            .and(pre_resolved_upstream.as_ref())
            .and_then(|upstream_name| {
                route_scoped_oauth_configs
                    .iter()
                    .find(|config| config.name == *upstream_name && config.oauth.is_some())
                    .cloned()
            });
        let raw_resolved = if let Some(resolved) = resolved_upstream_tool {
            Some(Ok(resolved))
        } else if let Some(manager) = &self.gateway_manager {
            Some(
                manager
                    .resolve_raw_upstream_tool_scoped(
                        service,
                        self.route_scope.allowed_upstreams(),
                        Some(&raw_runtime_owner),
                        raw_oauth_subject.as_deref(),
                    )
                    .await,
            )
        } else {
            None
        };
        if let Some(Err(err)) = &raw_resolved
            && !matches!(err.kind(), "unknown_tool" | "not_found")
        {
            let elapsed_ms = start.elapsed().as_millis();
            let kind = canonical_kind(err.kind());
            tracing::warn!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                elapsed_ms,
                kind,
                error = %err,
                "upstream proxy resolution failed"
            );
            let envelope = tool_error_envelope(service, upstream_action, err);
            self.emit_dispatch_notification(
                context,
                service,
                upstream_action,
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind,
                },
            )
            .await;
            return Ok(CallToolResult::error(vec![Content::text(
                envelope.to_string(),
            )]));
        }
        if let Some(pool) = self.current_upstream_pool().await
            && let Some(Ok((upstream_name, _tool))) = raw_resolved
            && pre_resolved_oauth_config.is_none()
        {
            let before = self.snapshot_catalog().await;
            tracing::info!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                upstream = %upstream_name,
                route = "upstream",
                "dispatch route selected"
            );
            tracing::debug!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                upstream = %upstream_name,
                capability = upstream_capability,
                operation = upstream_operation,
                subject_scoped = false,
                "proxying to upstream"
            );

            let mut upstream_params = CallToolRequestParams::new(service.to_string());
            upstream_params.arguments = raw_arguments;

            match pool.call_tool(&upstream_name, upstream_params).await {
                Some(Ok(result)) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    let (result, kind, counts_as_failure) =
                        normalize_upstream_result(service, upstream_action, result);
                    let outcome = if counts_as_failure || kind != "ok" {
                        DispatchLogOutcome::Failure {
                            level: if counts_as_failure {
                                LoggingLevel::Error
                            } else {
                                LoggingLevel::Warning
                            },
                            kind,
                        }
                    } else {
                        DispatchLogOutcome::Success
                    };
                    if counts_as_failure {
                        pool.record_failure(
                            &upstream_name,
                            format!("upstream `{upstream_name}` returned `{kind}`"),
                        )
                        .await;
                        tracing::warn!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            tool = %service,
                            upstream = %upstream_name,
                            capability = upstream_capability,
                            operation = upstream_operation,
                            subject_scoped = false,
                            elapsed_ms,
                            kind,
                            "upstream proxy failed"
                        );
                    } else {
                        pool.record_success(&upstream_name).await;
                        tracing::info!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            subject,
                            tool = %service,
                            upstream = %upstream_name,
                            capability = upstream_capability,
                            operation = upstream_operation,
                            subject_scoped = false,
                            elapsed_ms,
                            "upstream proxy ok"
                        );
                    }
                    self.emit_dispatch_notification(
                        context,
                        service,
                        upstream_action,
                        elapsed_ms,
                        outcome,
                    )
                    .await;
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    return Ok(result);
                }
                Some(Err(e)) => {
                    pool.record_failure(&upstream_name, e.clone()).await;
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service,
                        action = upstream_action,
                        tool = %service,
                        upstream = %upstream_name,
                        capability = upstream_capability,
                        operation = upstream_operation,
                        subject_scoped = false,
                        elapsed_ms,
                        kind = "upstream_error",
                        error = %e,
                        "upstream proxy failed"
                    );
                    let envelope = build_error(
                        service,
                        upstream_action,
                        "upstream_error",
                        &format!("upstream `{upstream_name}` call failed: {e}"),
                    );
                    self.emit_dispatch_notification(
                        context,
                        service,
                        upstream_action,
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind: "upstream_error",
                        },
                    )
                    .await;
                    return Ok(CallToolResult::error(vec![Content::text(
                        envelope.to_string(),
                    )]));
                }
                None => {
                    // Connection is gone — record failure so the circuit
                    // breaker can eventually exclude this upstream.
                    pool.record_failure(
                        &upstream_name,
                        format!("upstream `{upstream_name}` is not connected"),
                    )
                    .await;
                    let after = self.snapshot_catalog().await;
                    self.notify_catalog_changes(&before, &after).await;
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service,
                        action = upstream_action,
                        tool = %service,
                        upstream = %upstream_name,
                        capability = upstream_capability,
                        operation = upstream_operation,
                        subject_scoped = false,
                        elapsed_ms,
                        kind = "upstream_error",
                        error = "upstream disconnected",
                        "upstream not connected"
                    );
                    let envelope = build_error(
                        service,
                        upstream_action,
                        "upstream_error",
                        &format!("upstream `{upstream_name}` is not connected"),
                    );
                    self.emit_dispatch_notification(
                        context,
                        service,
                        upstream_action,
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind: "upstream_error",
                        },
                    )
                    .await;
                    return Ok(CallToolResult::error(vec![Content::text(
                        envelope.to_string(),
                    )]));
                }
            }
        }

        let auth = auth_context_from_extensions(&context.extensions);
        if let Some(oauth_subject) =
            oauth_upstream_subject_for_request(auth, self.request_subject(context))
            && let Some(pool) = self.current_upstream_pool().await
        {
            let mut owner = pre_resolved_oauth_config
                .as_ref()
                .map(|config| config.name.clone());
            if owner.is_none() {
                for (upstream_name, tools) in pool
                    .subject_scoped_tools(&route_scoped_oauth_configs, oauth_subject.as_ref())
                    .await
                {
                    if tools.iter().any(|tool| tool.name.as_ref() == service) {
                        owner = Some(upstream_name);
                        break;
                    }
                }
            }

            if let Some(upstream_name) = owner
                && let Some(config) = pre_resolved_oauth_config.or_else(|| {
                    route_scoped_oauth_configs
                        .iter()
                        .find(|config| config.name == upstream_name)
                        .cloned()
                })
            {
                tracing::info!(
                    surface = "mcp",
                    service,
                    action = upstream_action,
                    tool = %service,
                    upstream = %upstream_name,
                    route = "subject_scoped",
                    oauth_subject = %oauth_subject,
                    "dispatch route selected"
                );
                let input_tokens = raw_arguments.as_ref().map_or(0, estimate_tokens_args);
                let mut upstream_params = CallToolRequestParams::new(service.to_string());
                upstream_params.arguments = raw_arguments;
                match pool
                    .subject_scoped_call_tool(&config, oauth_subject.as_ref(), upstream_params)
                    .await
                {
                    Ok(result) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        let (result, kind, counts_as_failure) =
                            normalize_upstream_result(service, upstream_action, result);
                        let output_tokens = serde_json::to_string(&result)
                            .map(|output| estimate_tokens(&output))
                            .unwrap_or(0);
                        let outcome = if counts_as_failure || kind != "ok" {
                            tracing::warn!(
                                surface = "mcp",
                                service,
                                action = upstream_action,
                                tool = %service,
                                upstream = %upstream_name,
                                capability = upstream_capability,
                                operation = upstream_operation,
                                subject_scoped = true,
                                subject,
                                actor_key,
                                actor_label = subject,
                                agent_kind = "agent",
                                oauth_subject = %oauth_subject,
                                elapsed_ms,
                                input_tokens,
                                output_tokens,
                                kind,
                                "upstream dispatch error"
                            );
                            DispatchLogOutcome::Failure {
                                level: if counts_as_failure {
                                    LoggingLevel::Error
                                } else {
                                    LoggingLevel::Warning
                                },
                                kind,
                            }
                        } else {
                            tracing::info!(
                                surface = "mcp",
                                service,
                                action = upstream_action,
                                tool = %service,
                                upstream = %upstream_name,
                                capability = upstream_capability,
                                operation = upstream_operation,
                                subject_scoped = true,
                                subject,
                                actor_key,
                                actor_label = subject,
                                agent_kind = "agent",
                                oauth_subject = %oauth_subject,
                                elapsed_ms,
                                input_tokens,
                                output_tokens,
                                "upstream dispatch ok"
                            );
                            DispatchLogOutcome::Success
                        };
                        self.emit_dispatch_notification(
                            context,
                            service,
                            upstream_action,
                            elapsed_ms,
                            outcome,
                        )
                        .await;
                        return Ok(result);
                    }
                    Err(e) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        tracing::warn!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            tool = %service,
                            upstream = %upstream_name,
                            capability = upstream_capability,
                            operation = upstream_operation,
                            subject_scoped = true,
                            subject,
                            actor_key,
                            actor_label = subject,
                            agent_kind = "agent",
                            elapsed_ms,
                            input_tokens,
                            output_tokens = 0,
                            kind = "upstream_error",
                            error = %e,
                            "upstream dispatch error"
                        );
                        let envelope = build_error(
                            service,
                            upstream_action,
                            "upstream_error",
                            &format!("upstream `{upstream_name}` call failed: {e}"),
                        );
                        self.emit_dispatch_notification(
                            context,
                            service,
                            upstream_action,
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Error,
                                kind: "upstream_error",
                            },
                        )
                        .await;
                        return Ok(CallToolResult::error(vec![Content::text(
                            envelope.to_string(),
                        )]));
                    }
                }
            }
        }

        // Neither built-in nor upstream.
        let elapsed_ms = start.elapsed().as_millis();
        let err = anyhow::anyhow!("service `{service}` has no dispatcher wired");
        let input_tokens = raw_arguments.as_ref().map_or(0, estimate_tokens_args);
        let (result, outcome) = format_dispatch_result(
            Err(err),
            service,
            action,
            elapsed_ms,
            subject,
            actor_key,
            input_tokens,
        );
        self.emit_dispatch_notification(context, service, action, elapsed_ms, outcome)
            .await;
        Ok(result)
    }
}
