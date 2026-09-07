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
//! Health-accounting contract (bead `lab-ak0mh`): **the pool owns upstream
//! health accounting for every call that reaches an upstream.** Both pooled
//! paths (`call_tool_once_classified` /
//! `subject_scoped_call_tool_once_classified` via `timed_capability_call`) and the relay path (`call_tool_relayed`,
//! including `acquire_or_connect_relay` connect failures) record circuit-breaker
//! success/failure themselves — success for completed results AND for valid
//! JSON-RPC/MCP application errors (`CapabilityCallError::Mcp`, which prove the
//! connection is alive), failure for transport-class errors. This file must not
//! record on those outcomes: doing so double-counted transport failures
//! (halving the effective `CIRCUIT_BREAKER_THRESHOLD`) and flapped healthy
//! upstreams toward `Unhealthy` on caller mistakes. The ONE proxy-level record
//! left is the pooled `None` (not-connected) arm, because `acquire_peer` only
//! logs and records nothing.
//!
//! Other side effects: `notify_catalog_changes` ×3 (raw arms only),
//! `emit_dispatch_notification` at the resolution-fail, three raw arms, two
//! subject-scoped arms, and the fallback.
//!
//! `normalize_upstream_result` lives in `upstream.rs`.

use std::{future::Future, pin::Pin, sync::Arc, time::Instant};

use labby_gateway::upstream::pool::{CapabilityCallError, TaskRouteAuthorization, UpstreamPool};
use labby_gateway::upstream::tool_error::{mcp_error_data_kind, safety_hints_from_annotations};
use labby_runtime::agent_error::sanitize_error_text;
use labby_runtime::catalog_notify::SOURCE_MCP_CALL_UPSTREAM;
use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{CallToolRequestParams, CallToolResponse, ClientCapabilities, RequestId};
use rmcp::service::{Peer, RequestContext};
use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::mcp::context::{
    auth_context_from_extensions, oauth_upstream_subject_for_request, redacted_oauth_subject_label,
};
use crate::mcp::envelope::build_error_extra;
use crate::mcp::error::canonical_kind;
use crate::mcp::handlers_tools::strip_resource_backed_ui_meta;
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::result_format::{
    error_result_from_envelope, estimate_tokens, estimate_tokens_args, format_dispatch_result,
    tool_error_envelope,
};
use crate::mcp::server::{LabMcpServer, LabRequestCancellation};
use crate::mcp::upstream::{normalize_upstream_result, qualified_upstream_tool};

use crate::config::UpstreamConfig;
use crate::dispatch::upstream::types::UpstreamTool;

#[derive(Debug, Clone)]
pub(crate) struct PreResolvedUpstreamTool {
    pub(crate) upstream_name: String,
    pub(crate) tool: UpstreamTool,
    pub(crate) route: &'static str,
}

fn prepare_upstream_tool_request(
    mut request: CallToolRequestParams,
    upstream_tool_name: &str,
) -> CallToolRequestParams {
    request.name = upstream_tool_name.to_string().into();
    request
}

fn relay_capabilities_for_request(request: &CallToolRequestParams) -> Option<ClientCapabilities> {
    crate::mcp::context::forwardable_client_capabilities(request.meta.as_ref())
}

fn relay_cancellation_token(context: &RequestContext<RoleServer>) -> CancellationToken {
    context
        .extensions
        .get::<LabRequestCancellation>()
        .map(LabRequestCancellation::token)
        .unwrap_or_else(|| context.ct.clone())
}

/// Bytes of the raw transport error inspected for classification. Auth
/// signals appear at the front of real transport errors; lowercasing an
/// unbounded upstream-controlled string would be wasted allocation.
const MAX_CLASSIFY_BYTES: usize = 2048;

/// True when `text` contains `401` as a standalone token (not embedded in a
/// longer number or identifier such as `1401` or `x4012`). Covers the
/// "http 401" / "status 401" / "HTTP/1.1 401" shapes without matching byte
/// counts like "read 1401 bytes".
fn contains_standalone_401(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(position) = text[search_from..].find("401") {
        let start = search_from + position;
        let end = start + 3;
        let boundary_before = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let boundary_after = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if boundary_before && boundary_after {
            return true;
        }
        search_from = start + 1;
    }
    false
}

/// Model-facing classification of a raw upstream transport failure on the
/// live MCP call path.
///
/// `oauth_needs_reauth` is a DELIBERATE refinement of the breaker-side
/// `auth_failed`: it carries reauthorization recovery guidance
/// (`gateway.oauth.start`) instead of a generic auth failure. The breaker /
/// operator-logging vocabulary lives in
/// `labby_gateway::upstream::pool::helpers::classify_upstream_error` — keep
/// the two classifiers' auth heuristics aligned when either changes. The
/// relationship is documented in `docs/dev/ERRORS.md`.
fn upstream_failure_kind(error: &str) -> &'static str {
    // Lowercase only a bounded prefix — `error` is upstream-controlled.
    let mut end = error.len().min(MAX_CLASSIFY_BYTES);
    while end > 0 && !error.is_char_boundary(end) {
        end -= 1;
    }
    let error = error[..end].to_ascii_lowercase();
    if contains_standalone_401(&error)
        || [
            "unauthorized",
            "invalid_grant",
            "invalid_token",
            "token expired",
            "oauth error",
            "oauth authorization",
            "oauth token",
            "requires oauth",
            "reauth",
        ]
        .iter()
        .any(|needle| error.contains(needle))
    {
        "oauth_needs_reauth"
    } else {
        "upstream_error"
    }
}

fn upstream_transport_error_envelope(
    service: &str,
    action: &str,
    upstream_name: &str,
    raw_error: &str,
) -> (serde_json::Value, &'static str) {
    let kind = upstream_failure_kind(raw_error);
    let tool = qualified_upstream_tool(upstream_name, service);
    let cause = sanitize_error_text(raw_error, 4096);
    let message = if kind == "oauth_needs_reauth" {
        format!(
            "Tool `{tool}` did not return a completed MCP result because upstream `{upstream_name}` requires authorization. Do not retry this tool call unchanged. Call the `gateway` tool with action `gateway.oauth.start` and parameter `upstream = {upstream_name}`, complete authorization, then verify whether the original operation took effect before retrying."
        )
    } else {
        format!(
            "Tool `{tool}` did not return a completed MCP result because the transport to upstream `{upstream_name}` failed. Retry after the upstream reconnects, but first verify whether the previous call may have committed partial effects."
        )
    };
    let envelope = build_error_extra(
        service,
        action,
        kind,
        &message,
        &serde_json::json!({
            "tool": tool,
            "upstream": upstream_name,
            "cause": cause,
        }),
    );
    (envelope, kind)
}

/// A failed upstream tool call, keeping the failure class when the pooled
/// path produced one.
///
/// Both pooled and relay paths preserve [`CapabilityCallError`] so structured
/// MCP kinds and retry guidance survive the gateway boundary.
///
/// The classified error is boxed to keep the `Result` error arm small
/// (`clippy::result_large_err` — `CapabilityCallError::Mcp` carries a full
/// `ErrorData`).
enum UpstreamCallFailure {
    Classified(Box<CapabilityCallError>),
}

struct AbortTaskOnDrop(tokio::task::AbortHandle);

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn call_tool_relayed_on_fresh_stack(
    pool: Arc<UpstreamPool>,
    config: UpstreamConfig,
    oauth_subject: Option<String>,
    params: CallToolRequestParams,
    downstream: Peer<RoleServer>,
    downstream_request_id: RequestId,
    downstream_cancel: CancellationToken,
    relay_session_id: u64,
    capabilities: ClientCapabilities,
    caller_subject: Option<String>,
    task_authorization: TaskRouteAuthorization,
) -> Option<Result<CallToolResponse, CapabilityCallError>> {
    let current_span = tracing::Span::current();
    let task = tokio::spawn(
        async move {
            pool.call_tool_relayed(
                &config,
                oauth_subject.as_deref(),
                params,
                downstream,
                downstream_request_id,
                downstream_cancel,
                relay_session_id,
                capabilities,
                caller_subject.as_deref(),
                task_authorization,
            )
            .await
        }
        .instrument(current_span),
    );
    let _abort_on_drop = AbortTaskOnDrop(task.abort_handle());
    match task.await {
        Ok(result) => result,
        Err(error) => Some(Err(CapabilityCallError::Other {
            message: format!("relayed upstream task failed: {error}"),
        })),
    }
}

impl UpstreamCallFailure {
    fn classified(error: CapabilityCallError) -> Self {
        Self::Classified(Box::new(error))
    }
}

fn upstream_call_failure_envelope(
    service: &str,
    action: &str,
    upstream_name: &str,
    failure: &UpstreamCallFailure,
) -> (serde_json::Value, &'static str) {
    match failure {
        UpstreamCallFailure::Classified(error) => {
            upstream_classified_error_envelope(service, action, upstream_name, error)
        }
    }
}

/// Envelope for a typed [`CapabilityCallError`] from the pooled call path.
///
/// `Mcp` is an application-level rejection carried over a healthy connection:
/// it surfaces with the shared `ErrorData`-derived stable kind
/// (`mcp_error_data_kind`, same vocabulary as Code Mode) instead of the
/// generic `upstream_error`. Timeout/queue/cap/cancel classes surface their
/// own stable kinds (`docs/dev/ERRORS.md`). Transport-shaped classes keep the
/// string classifier so the `oauth_needs_reauth` refinement (and its
/// `gateway.oauth.start` recovery guidance) is preserved.
fn upstream_classified_error_envelope(
    service: &str,
    action: &str,
    upstream_name: &str,
    error: &CapabilityCallError,
) -> (serde_json::Value, &'static str) {
    let tool = qualified_upstream_tool(upstream_name, service);
    let (kind, message, cause) = match error {
        CapabilityCallError::Mcp { data, .. } => {
            let kind = mcp_error_data_kind(data);
            let mut cause = sanitize_error_text(&data.message, 4096);
            if kind == "upstream_error" {
                // Keep the numeric JSON-RPC code visible when the class
                // collapses to the generic kind, mirroring Code Mode.
                cause = format!("{cause} (JSON-RPC code {})", data.code.0);
            }
            let message = format!(
                "Tool `{tool}` was rejected by upstream `{upstream_name}` with an MCP `{kind}` error. The upstream connection is healthy — use the cause to decide whether to revise the request before retrying."
            );
            (kind, message, cause)
        }
        CapabilityCallError::Timeout { message } => (
            "timeout",
            format!(
                "Tool `{tool}` did not return a completed MCP result because the call to upstream `{upstream_name}` timed out. Verify whether the previous call may have committed partial effects before retrying."
            ),
            sanitize_error_text(message, 4096),
        ),
        CapabilityCallError::QueueSaturated { message } => (
            "queue_saturated",
            format!(
                "Tool `{tool}` was not sent because the gateway's concurrency queue for upstream `{upstream_name}` is saturated. The upstream was not called; retry later with the same arguments."
            ),
            sanitize_error_text(message, 4096),
        ),
        CapabilityCallError::ResponseTooLarge { message } => (
            "response_too_large",
            format!(
                "Tool `{tool}` completed on upstream `{upstream_name}` but the response exceeded the gateway's response-size cap and was not forwarded. Reduce the requested output size before retrying."
            ),
            sanitize_error_text(message, 4096),
        ),
        CapabilityCallError::Cancelled { message } => (
            "cancelled",
            format!(
                "Tool `{tool}` on upstream `{upstream_name}` was cancelled before completing. Do not retry automatically; verify whether partial effects occurred."
            ),
            sanitize_error_text(message, 4096),
        ),
        CapabilityCallError::InputRequiredRoundsExceeded { message } => (
            "confirmation_required",
            format!(
                "Tool `{tool}` on upstream `{upstream_name}` kept requiring interactive input past the round cap. Complete the required confirmation before retrying."
            ),
            sanitize_error_text(message, 4096),
        ),
        // Transport/protocol/unknown failures: preserve the historical string
        // classification (including the oauth_needs_reauth refinement).
        CapabilityCallError::Transport { message }
        | CapabilityCallError::Protocol { message }
        | CapabilityCallError::Other { message } => {
            return upstream_transport_error_envelope(service, action, upstream_name, message);
        }
    };
    let envelope = build_error_extra(
        service,
        action,
        kind,
        &message,
        &serde_json::json!({
            "tool": tool,
            "upstream": upstream_name,
            "cause": cause,
        }),
    );
    (envelope, kind)
}

impl LabMcpServer {
    /// Construct the large upstream-proxy future in this small synchronous
    /// frame so the already-large top-level tool dispatcher stores only a
    /// boxed pointer while awaiting it.
    pub(crate) fn boxed_call_tool_upstream_impl<'a>(
        &'a self,
        service: &'a str,
        action: &'a str,
        upstream_request: CallToolRequestParams,
        resolved_upstream_tool: Option<PreResolvedUpstreamTool>,
        start: Instant,
        subject: &'a str,
        actor_key: Option<&'a str>,
        context: &'a RequestContext<RoleServer>,
    ) -> Pin<Box<dyn Future<Output = Result<CallToolResponse, ErrorData>> + Send + 'a>> {
        Box::pin(self.call_tool_upstream_impl(
            service,
            action,
            upstream_request,
            resolved_upstream_tool,
            start,
            subject,
            actor_key,
            context,
        ))
    }

    /// Upstream-proxy tail. Reached by fall-through from `call_tool_impl`
    /// when `svc.is_none()`. Owns raw + subject-scoped proxy branches and
    /// the no-dispatcher-wired fallback; returns unconditionally.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn call_tool_upstream_impl(
        &self,
        service: &str,
        action: &str,
        upstream_request: CallToolRequestParams,
        resolved_upstream_tool: Option<PreResolvedUpstreamTool>,
        start: Instant,
        subject: &str,
        actor_key: Option<&str>,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !self.route_scope.exposes_tools() {
            tracing::warn!(
                surface = "mcp",
                service,
                action = "call_tool",
                route_scope = %self.route_scope.label(),
                kind = "loadout_capability_disabled",
                "direct upstream tool call denied by loadout"
            );
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                "direct upstream MCP Tools are disabled by this loadout; use Code Mode if the loadout exposes it, or ask the operator to enable Tools for this loadout".to_string(),
                None,
            ));
        }
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
            .map(|resolved| resolved.upstream_name.clone());
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
            Some(Ok((resolved.upstream_name, resolved.tool, resolved.route)))
        } else if let Some(manager) = &self.gateway_manager {
            Some(
                manager
                    .resolve_raw_upstream_tool_scoped(
                        service,
                        self.route_scope.allowed_upstreams(),
                        Some(&raw_runtime_owner),
                        raw_oauth_subject.as_deref(),
                    )
                    .await
                    .map(|(upstream_name, tool)| (upstream_name, tool, "upstream")),
            )
        } else {
            None
        };
        let pre_resolved_safety = raw_resolved
            .as_ref()
            .and_then(|resolved| resolved.as_ref().ok())
            .map(|(_, tool, _)| safety_hints_from_annotations(tool.tool.annotations.as_ref()))
            .unwrap_or_default();
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
            return Ok(error_result_from_envelope(envelope).into());
        }
        if let Some(pool) = self.current_upstream_pool().await
            && let Some(Ok((upstream_name, resolved_tool, route))) = raw_resolved
            && pre_resolved_oauth_config.is_none()
        {
            let safety = safety_hints_from_annotations(resolved_tool.tool.annotations.as_ref());
            let before = self.snapshot_tool_catalog_for_request(context).await;
            tracing::info!(
                surface = "mcp",
                service,
                action = upstream_action,
                tool = %service,
                upstream = %upstream_name,
                route,
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

            let upstream_params = prepare_upstream_tool_request(upstream_request.clone(), service);

            // The 2026-07-28 protocol declares client capabilities per request.
            // Relay only when this request carries capabilities that the normal
            // unit-handler connection cannot represent, and snapshot the exact
            // capability set for the dedicated upstream connection.
            let relay_capabilities = relay_capabilities_for_request(&upstream_params);
            let relay_config = if relay_capabilities.is_some() {
                match &self.gateway_manager {
                    Some(manager) => manager
                        .upstream_config(&upstream_name)
                        .await
                        .filter(crate::mcp::context::upstream_uses_capability_relay),
                    None => None,
                }
            } else {
                None
            };
            // Both call paths own ALL circuit-breaker recording for this call:
            // `call_tool_relayed`/`acquire_or_connect_relay` for the relay path
            // (success, call failure, AND connect failure), and
            // `call_tool_once_classified` (via `timed_capability_call`) for the
            // pooled path. The pooled path does NOT record a connect failure
            // itself (`acquire_peer` only logs), so the `None` arm below records
            // it — but only for the pooled path, else a relayed connect failure
            // would be counted twice.
            let used_relay = relay_config.is_some();
            let call_outcome: Option<Result<CallToolResponse, UpstreamCallFailure>> = match (
                relay_config,
                relay_capabilities,
            ) {
                (Some(config), Some(capabilities)) => {
                    tracing::debug!(
                    surface = "mcp",
                    service,
                    action = upstream_action,
                    tool = %service,
                    upstream = %upstream_name,
                    route = "relayed",
                    "proxying to upstream over relayed dedicated connection"
                    );
                    let cancellation = relay_cancellation_token(context);
                    let cancellation_token = cancellation.clone();
                    let relay_timeout = pool.relay_timeout();
                    let call = call_tool_relayed_on_fresh_stack(
                        Arc::clone(&pool),
                        config,
                        None,
                        upstream_params,
                        context.peer.clone(),
                        context.id.clone(),
                        cancellation,
                        self.relay_session_id,
                        capabilities,
                        self.request_subject(context).map(str::to_owned),
                        self.route_scope.task_authorization(),
                    );
                    tokio::pin!(call);
                    tokio::select! {
                        // At the deadline the timeout classification wins over
                        // the cancellation result produced while the relay is
                        // tearing down. An explicit cancellation that arrives
                        // before the deadline still completes `call` first and
                        // retains its `cancelled` classification.
                        biased;
                        () = tokio::time::sleep(relay_timeout) => {
                            cancellation_token.cancel();
                            drop(tokio::time::timeout(
                                std::time::Duration::from_secs(2),
                                &mut call,
                            ).await);
                            Some(Err(UpstreamCallFailure::classified(
                                CapabilityCallError::Timeout {
                                    message: format!("upstream `{upstream_name}` relay request timed out"),
                                },
                            )))
                        }
                        result = &mut call => result.map(|result| result.map_err(UpstreamCallFailure::classified)),
                    }
                }
                // The pooled path carries the same cancellation token as the
                // relayed one above. Without it a downstream client that
                // disconnects (or whose request the HTTP transport times
                // out) leaves this call running to completion unheard, and
                // the upstream is never told to stop — so a client that
                // retries can execute a side-effecting tool twice.
                _ => pool
                    .call_tool_once_classified(
                        &upstream_name,
                        upstream_params,
                        Some(&relay_cancellation_token(context)),
                    )
                    .await
                    .map(|result| result.map_err(UpstreamCallFailure::classified)),
            };

            match call_outcome {
                Some(Ok(result)) => {
                    // The pool already recorded circuit-breaker success for
                    // every completed outcome on both call paths — no proxy-
                    // level `record_success` here.
                    let CallToolResponse::Complete(result) = result else {
                        let elapsed_ms = start.elapsed().as_millis();
                        tracing::info!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            subject,
                            tool = %service,
                            upstream = %upstream_name,
                            elapsed_ms,
                            result_type = "incomplete",
                            "upstream proxy returned non-complete result"
                        );
                        return Ok(result);
                    };
                    let elapsed_ms = start.elapsed().as_millis();
                    let (mut result, kind) = normalize_upstream_result(
                        service,
                        upstream_action,
                        &upstream_name,
                        result,
                        &safety,
                    );
                    if !self.route_scope.exposes_resources() {
                        strip_resource_backed_ui_meta(&mut result.meta);
                    }
                    // A completed `isError: true` result is a tool-execution
                    // failure for the model, never a health failure.
                    let outcome = if kind == "ok" {
                        DispatchLogOutcome::Success
                    } else {
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Warning,
                            kind,
                        }
                    };
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
                    self.emit_dispatch_notification(
                        context,
                        service,
                        upstream_action,
                        elapsed_ms,
                        outcome,
                    )
                    .await;
                    let after = self.snapshot_tool_catalog_for_request(context).await;
                    self.notify_catalog_changes(
                        after.changes_since(&before),
                        SOURCE_MCP_CALL_UPSTREAM,
                    )
                    .await;
                    return Ok(result.into());
                }
                Some(Err(failure)) => {
                    // No proxy-level `record_failure` here: the pool owns
                    // health accounting for both call paths. For a
                    // `CapabilityCallError::Mcp` it recorded SUCCESS (a valid
                    // JSON-RPC error proves the connection is alive — a caller
                    // mistake must not flap a healthy upstream), and for
                    // transport-class failures it already recorded the breaker
                    // failure — recording again would double-count and halve
                    // the effective `CIRCUIT_BREAKER_THRESHOLD`.
                    let after = self.snapshot_tool_catalog_for_request(context).await;
                    self.notify_catalog_changes(
                        after.changes_since(&before),
                        SOURCE_MCP_CALL_UPSTREAM,
                    )
                    .await;
                    let elapsed_ms = start.elapsed().as_millis();
                    let (envelope, kind) = upstream_call_failure_envelope(
                        service,
                        upstream_action,
                        &upstream_name,
                        &failure,
                    );
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
                        error_kind = "upstream_call_failed",
                        "upstream proxy failed"
                    );
                    self.emit_dispatch_notification(
                        context,
                        service,
                        upstream_action,
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind,
                        },
                    )
                    .await;
                    return Ok(error_result_from_envelope(envelope).into());
                }
                None => {
                    // Connection is gone — record failure so the circuit
                    // breaker can eventually exclude this upstream. Skip when the
                    // relay path was used: `acquire_or_connect_relay` already
                    // recorded the connect failure, so recording again here would
                    // double-count it (the pooled `call_tool` path does not record
                    // connect failures itself, so it still needs this).
                    if !used_relay {
                        pool.record_failure(
                            &upstream_name,
                            format!("upstream `{upstream_name}` is not connected"),
                        )
                        .await;
                    }
                    let after = self.snapshot_tool_catalog_for_request(context).await;
                    self.notify_catalog_changes(
                        after.changes_since(&before),
                        SOURCE_MCP_CALL_UPSTREAM,
                    )
                    .await;
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
                    let (envelope, kind) = upstream_transport_error_envelope(
                        service,
                        upstream_action,
                        &upstream_name,
                        "upstream is not connected",
                    );
                    self.emit_dispatch_notification(
                        context,
                        service,
                        upstream_action,
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Error,
                            kind,
                        },
                    )
                    .await;
                    return Ok(error_result_from_envelope(envelope).into());
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
            let mut safety = pre_resolved_safety.clone();
            if owner.is_none() {
                for (upstream_name, tools) in pool
                    .subject_scoped_tools(&route_scoped_oauth_configs, oauth_subject.as_ref())
                    .await
                {
                    if let Some(tool) = tools.iter().find(|tool| tool.name.as_ref() == service) {
                        safety = safety_hints_from_annotations(tool.annotations.as_ref());
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
                    oauth_subject = redacted_oauth_subject_label(),
                    "dispatch route selected"
                );
                let input_tokens = upstream_request
                    .arguments
                    .as_ref()
                    .map_or(0, estimate_tokens_args);
                let upstream_params =
                    prepare_upstream_tool_request(upstream_request.clone(), service);
                // Relay path: for OAuth/subject-scoped upstreams, route
                // over a dedicated relay-handled connection so the upstream's
                // MRTR input requirements are preserved for the downstream
                // agent. The relay connect forwards `oauth_subject` so the
                // dedicated connection authenticates as this caller.
                let relay_capabilities = relay_capabilities_for_request(&upstream_params);
                // Health accounting is owned by the callee on both paths
                // (`call_tool_relayed` / `timed_capability_call`); this branch
                // records nothing, as before.
                let call_result: Result<CallToolResponse, UpstreamCallFailure> =
                    if let Some(capabilities) = relay_capabilities {
                        tracing::debug!(
                            surface = "mcp",
                            service,
                            action = upstream_action,
                            tool = %service,
                            upstream = %upstream_name,
                            route = "subject_scoped_relayed",
                            "proxying to upstream over relayed dedicated connection"
                        );
                        match call_tool_relayed_on_fresh_stack(
                            Arc::clone(&pool),
                            config,
                            Some(oauth_subject.to_string()),
                            upstream_params,
                            context.peer.clone(),
                            context.id.clone(),
                            relay_cancellation_token(context),
                            self.relay_session_id,
                            capabilities,
                            self.request_subject(context).map(str::to_owned),
                            self.route_scope.task_authorization(),
                        )
                        .await
                        {
                            Some(result) => result.map_err(UpstreamCallFailure::classified),
                            None => Err(UpstreamCallFailure::classified(
                                CapabilityCallError::Transport {
                                    message: format!(
                                        "relayed upstream `{upstream_name}` connect failed"
                                    ),
                                },
                            )),
                        }
                    } else {
                        pool.subject_scoped_call_tool_once_classified(
                            &config,
                            oauth_subject.as_ref(),
                            upstream_params,
                            Some(&relay_cancellation_token(context)),
                        )
                        .await
                        .map_err(UpstreamCallFailure::classified)
                    };
                match call_result {
                    Ok(result) => {
                        let CallToolResponse::Complete(result) = result else {
                            return Ok(result);
                        };
                        let elapsed_ms = start.elapsed().as_millis();
                        let (mut result, kind) = normalize_upstream_result(
                            service,
                            upstream_action,
                            &upstream_name,
                            result,
                            &safety,
                        );
                        if !self.route_scope.exposes_resources() {
                            strip_resource_backed_ui_meta(&mut result.meta);
                        }
                        let output_tokens = serde_json::to_string(&result)
                            .map(|output| estimate_tokens(&output))
                            .unwrap_or(0);
                        let outcome = if kind != "ok" {
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
                                oauth_subject = redacted_oauth_subject_label(),
                                elapsed_ms,
                                input_tokens,
                                output_tokens,
                                kind,
                                "upstream dispatch error"
                            );
                            // A completed `isError: true` result is a tool-
                            // execution failure for the model, not an
                            // infrastructure error.
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Warning,
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
                                oauth_subject = redacted_oauth_subject_label(),
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
                        return Ok(result.into());
                    }
                    Err(failure) => {
                        let elapsed_ms = start.elapsed().as_millis();
                        let (envelope, kind) = upstream_call_failure_envelope(
                            service,
                            upstream_action,
                            &upstream_name,
                            &failure,
                        );
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
                            kind,
                            error_kind = "upstream_call_failed",
                            "upstream dispatch error"
                        );
                        self.emit_dispatch_notification(
                            context,
                            service,
                            upstream_action,
                            elapsed_ms,
                            DispatchLogOutcome::Failure {
                                level: LoggingLevel::Error,
                                kind,
                            },
                        )
                        .await;
                        return Ok(error_result_from_envelope(envelope).into());
                    }
                }
            }
        }

        // Neither built-in nor upstream.
        let elapsed_ms = start.elapsed().as_millis();
        let err = anyhow::anyhow!("service `{service}` has no dispatcher wired");
        let input_tokens = upstream_request
            .arguments
            .as_ref()
            .map_or(0, estimate_tokens_args);
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
        Ok(result.into())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use std::collections::BTreeMap;

    use rmcp::model::{
        CallToolRequestParams, ClientCapabilities, ElicitationCapability,
        FormElicitationCapability, Implementation, ProtocolVersion, RequestMetaObject,
    };
    use serde_json::json;

    use super::{
        CapabilityCallError, prepare_upstream_tool_request, redacted_oauth_subject_label,
        relay_capabilities_for_request, upstream_classified_error_envelope, upstream_failure_kind,
        upstream_transport_error_envelope,
    };

    fn interactive_request() -> CallToolRequestParams {
        let capabilities = ClientCapabilities::builder()
            .enable_elicitation_with(
                ElicitationCapability::new().with_form(FormElicitationCapability::new()),
            )
            .build();
        let mut meta = RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("downstream-agent", "1.0.0"),
            capabilities,
        );
        meta.set_traceparent("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01");
        let mut request = CallToolRequestParams::new("gateway/echo").with_arguments(
            serde_json::Map::from_iter([("value".to_string(), json!("hello"))]),
        );
        request.meta = Some(meta);
        request.input_responses = Some(BTreeMap::from([(
            "confirmation".to_string(),
            json!({"action": "accept", "content": {"confirm": true}}),
        )]));
        request.request_state = Some("opaque-upstream-state".to_string());
        request
    }

    #[test]
    fn upstream_request_preserves_mrtr_and_extension_metadata() {
        let request = interactive_request();

        let forwarded = prepare_upstream_tool_request(request.clone(), "echo");

        assert_eq!(forwarded.name.as_ref(), "echo");
        assert_eq!(forwarded.arguments, request.arguments);
        assert_eq!(forwarded.input_responses, request.input_responses);
        assert_eq!(forwarded.request_state, request.request_state);
        assert_eq!(
            forwarded
                .meta
                .as_ref()
                .and_then(|meta| meta.get_traceparent()),
            Some("00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01")
        );
    }

    #[test]
    fn relay_capabilities_come_from_the_current_request() {
        let request = interactive_request();
        let capabilities = relay_capabilities_for_request(&request)
            .expect("current request advertises elicitation");
        assert!(capabilities.elicitation.is_some());

        let no_capabilities = CallToolRequestParams::new("echo");
        assert_eq!(
            relay_capabilities_for_request(&no_capabilities),
            Some(ClientCapabilities::default())
        );
    }

    #[test]
    fn oauth_transport_failure_is_course_correcting_and_redacted() {
        let raw = "401 unauthorized for user@example.com with sk-abcdefghijklmnopqrstuvwxyz123456";
        let (envelope, kind) =
            upstream_transport_error_envelope("create_issue", "call_tool", "github", raw);

        assert_eq!(kind, "oauth_needs_reauth");
        let error = &envelope["error"];
        assert_eq!(error["origin"], "policy");
        assert_eq!(error["recovery"]["action"], "reauthenticate");
        assert_eq!(error["tool"], "github::create_issue");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("gateway.oauth.start")
        );
        assert!(
            !envelope
                .to_string()
                .contains("sk-abcdefghijklmnopqrstuvwxyz")
        );
    }

    #[test]
    fn generic_transport_failure_stays_upstream_error() {
        assert_eq!(
            upstream_failure_kind("connection reset by peer"),
            "upstream_error"
        );
    }

    #[test]
    fn embedded_401_digits_do_not_classify_as_oauth() {
        assert_eq!(upstream_failure_kind("read 1401 bytes"), "upstream_error");
        assert_eq!(
            upstream_failure_kind("request id 84012 failed"),
            "upstream_error"
        );
    }

    #[test]
    fn standalone_401_tokens_classify_as_oauth() {
        assert_eq!(upstream_failure_kind("http 401"), "oauth_needs_reauth");
        assert_eq!(upstream_failure_kind("status 401"), "oauth_needs_reauth");
        assert_eq!(
            upstream_failure_kind("HTTP/1.1 401 Unauthorized"),
            "oauth_needs_reauth"
        );
        assert_eq!(
            upstream_failure_kind("error 401: nope"),
            "oauth_needs_reauth"
        );
    }

    #[test]
    fn classification_only_reads_a_bounded_prefix() {
        // An auth marker buried megabytes deep is not worth a full-string
        // lowercase; the bounded classifier treats it as a generic failure.
        let mut error = "x".repeat(1024 * 1024);
        error.push_str(" 401 unauthorized");
        assert_eq!(upstream_failure_kind(&error), "upstream_error");
    }

    #[test]
    fn oauth_subject_log_label_is_redacted() {
        let raw_subject = "user@example.com";

        let label = redacted_oauth_subject_label();

        assert_ne!(label, raw_subject);
        assert!(!label.contains("user@example.com"));
    }

    // ── Classified envelope mapping ──────────────────────────────────────────

    #[test]
    fn classified_mcp_error_surfaces_structured_kind_not_upstream_error() {
        let error = CapabilityCallError::Mcp {
            data: rmcp::model::ErrorData::invalid_params(
                "unknown field `since`, expected project, tool, limit",
                None,
            ),
            message: "upstream call failed: unknown field `since`".to_string(),
        };

        let (envelope, kind) =
            upstream_classified_error_envelope("project_context", "call_tool", "cortex", &error);

        assert_eq!(kind, "invalid_param");
        let error = &envelope["error"];
        assert_eq!(error["kind"], "invalid_param");
        assert_eq!(error["tool"], "cortex::project_context");
        assert_eq!(error["upstream"], "cortex");
        assert!(
            error["cause"]
                .as_str()
                .expect("cause preserved")
                .contains("unknown field `since`")
        );
    }

    #[test]
    fn classified_timeout_maps_to_timeout_kind() {
        let error = CapabilityCallError::Timeout {
            message: "upstream call timed out after 30000ms".to_string(),
        };

        let (envelope, kind) =
            upstream_classified_error_envelope("slow_tool", "call_tool", "slowstream", &error);

        assert_eq!(kind, "timeout");
        assert_eq!(envelope["error"]["kind"], "timeout");
        assert!(
            envelope["error"]["cause"]
                .as_str()
                .expect("cause preserved")
                .contains("timed out")
        );
    }

    #[test]
    fn classified_transport_error_keeps_oauth_refinement() {
        let error = CapabilityCallError::Transport {
            message: "http 401 unauthorized".to_string(),
        };

        let (_envelope, kind) =
            upstream_classified_error_envelope("create_issue", "call_tool", "github", &error);

        assert_eq!(kind, "oauth_needs_reauth");
    }

    // ── Proxy-path health accounting (bead lab-ak0mh) ────────────────────────
    //
    // These drive the REAL `call_tool_upstream_impl` proxy tail over an
    // in-process upstream wired through the pool's registration seam, then
    // read the pool's circuit-breaker state back out.

    mod proxy_health {
        use std::pin::Pin;
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use futures::future::BoxFuture;
        use rmcp::model::{
            CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, NumberOrString,
            ServerCapabilities, ServerInfo, Tool,
        };
        use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};
        use serde_json::Value;

        use crate::dispatch::upstream::pool::{
            InProcessConnector, InProcessRegistration, UpstreamConnection, UpstreamPool,
        };
        use crate::dispatch::upstream::types::{
            UpstreamHealth, UpstreamRuntimeMetadata, UpstreamTool,
        };
        use crate::mcp::call_tool_upstream::PreResolvedUpstreamTool;
        use crate::mcp::server::LabMcpServer;

        const UPSTREAM_NAME: &str = "probe-upstream";
        const TOOL_NAME: &str = "probe_tool";

        /// Upstream whose tool call is rejected with a valid JSON-RPC error —
        /// a caller mistake carried over a healthy connection.
        struct McpRejectingServer;
        impl ServerHandler for McpRejectingServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Err(ErrorData::invalid_params(
                    "unknown field `since`, expected project, tool, limit",
                    None,
                ))
            }
        }

        /// Upstream whose tool call never completes — a genuine transport-class
        /// failure (timeout) that the pool records against the breaker.
        struct StallingServer;
        impl ServerHandler for StallingServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                tokio::time::sleep(Duration::from_mins(10)).await;
                Ok(CallToolResult::success(vec![]).into())
            }
        }

        async fn in_process_connection<S>(server: S) -> UpstreamConnection
        where
            S: ServerHandler + Send + 'static,
        {
            let (server_transport, client_transport) = tokio::io::duplex(4 * 1024 * 1024);
            let server_task = tokio::spawn(async move {
                let running = server
                    .serve(server_transport)
                    .await
                    .expect("in-process upstream server starts");
                running.waiting().await.ok();
            });
            let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
                .serve(client_transport)
                .await
                .expect("in-process upstream client starts");
            let peer = client_service.peer().clone();
            UpstreamConnection::new(
                client_service,
                Some(server_task),
                peer,
                UpstreamRuntimeMetadata::default(),
            )
        }

        fn noop_dispatch(
            _action: String,
            _params: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>>
        {
            Box::pin(async { Ok(Value::Null) })
        }

        /// A one-service registry that only exists to drive the pool's
        /// in-process registration seam (the sole public way to seed catalog
        /// entry + live connection together from this crate).
        fn seam_registry() -> crate::registry::ToolRegistry {
            static ACTIONS: &[labby_primitives::action::ActionSpec] =
                &[labby_primitives::action::ActionSpec {
                    name: "probe.call",
                    description: "Probe tool",
                    destructive: false,
                    requires_admin: false,
                    params: &[],
                    returns: "object",
                }];
            let mut registry = crate::registry::ToolRegistry::new();
            registry.register(crate::registry::RegisteredService {
                name: "probe_seed",
                description: "Registration seam seed",
                category: "test",
                kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
                status: "available",
                actions: ACTIONS,
                dispatch: noop_dispatch,
            });
            registry
        }

        async fn pool_with_upstream(
            connection: UpstreamConnection,
            request_timeout: Option<Duration>,
        ) -> Arc<UpstreamPool> {
            let slot = Arc::new(std::sync::Mutex::new(Some(connection)));
            let connector: InProcessConnector = Arc::new(move |_service| {
                let slot = Arc::clone(&slot);
                let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                    Box::pin(async move {
                        let connection = slot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .take();
                        let entry_name: Arc<str> = Arc::from(UPSTREAM_NAME);
                        Ok(InProcessRegistration {
                            connection,
                            tools: vec![Tool::new(
                                TOOL_NAME,
                                "Probe tool",
                                Arc::new(serde_json::Map::new()),
                            )],
                            entry_name,
                            upstream_name: UPSTREAM_NAME.to_string(),
                        })
                    });
                future
            });
            let mut pool = UpstreamPool::new().with_in_process_connector(connector);
            if let Some(timeout) = request_timeout {
                pool = pool.with_request_timeout(timeout);
            }
            let pool = Arc::new(pool);
            pool.register_in_process_service_peers(&seam_registry())
                .await;
            pool
        }

        /// A `LabMcpServer` whose gateway manager holds `pool` and an EMPTY
        /// upstream config — so the raw proxy branch resolves via the
        /// pre-resolved tool and takes the pooled (non-relay) call path.
        async fn proxy_test_server(pool: Arc<UpstreamPool>) -> LabMcpServer {
            let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
            runtime.swap(Some(pool)).await;
            let manager = Arc::new(
                crate::dispatch::gateway::config_store::test_gateway_manager(
                    std::path::PathBuf::from("config.toml"),
                    runtime,
                ),
            );
            manager
                .seed_config_unchecked_for_tests(
                    crate::config::LabConfig::default().to_gateway_config(),
                )
                .await;
            LabMcpServer {
                registry: Arc::new(crate::registry::ToolRegistry::new()),
                access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
                file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
                gateway_manager: Some(manager),
                peers: Default::default(),
                code_mode_app_state: Default::default(),
                last_listed_tool_contract: Default::default(),
                route_runtime: Default::default(),
                client_registry: Default::default(),
                transport_label: "test",
                logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
                    crate::mcp::logging::logging_level_rank(
                        crate::mcp::logging::LoggingLevel::Emergency,
                    ),
                )),
                route_scope: crate::mcp::route_scope::McpRouteScope::Root,
                relay_session_id: 0,
                code_mode_widget_callbacks_enabled_for_test: false,
            }
        }

        /// Drive the real upstream-proxy tail and return the completed error
        /// envelope (`structured_content`).
        async fn call_through_proxy(server: LabMcpServer) -> Value {
            let resolved = PreResolvedUpstreamTool {
                upstream_name: UPSTREAM_NAME.to_string(),
                tool: UpstreamTool {
                    tool: Tool::new(TOOL_NAME, "Probe tool", Arc::new(serde_json::Map::new())),
                    input_schema: None,
                    output_schema: None,
                    upstream_name: Arc::from(UPSTREAM_NAME),
                    destructive: false,
                },
                route: "upstream",
            };
            let (transport, _client_half) = tokio::io::duplex(4 * 1024 * 1024);
            let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
                server, transport, None,
            );
            let context = rmcp::service::RequestContext::new(
                NumberOrString::String(Arc::from("proxy-health-test")),
                running.peer().clone(),
            );
            let response = running
                .service()
                .call_tool_upstream_impl(
                    TOOL_NAME,
                    "call_tool",
                    CallToolRequestParams::new(TOOL_NAME),
                    Some(resolved),
                    Instant::now(),
                    "test-subject",
                    None,
                    &context,
                )
                .await
                .expect("proxy tail returns a result envelope");
            let CallToolResponse::Complete(result) = response else {
                panic!("expected a completed result from the proxy tail");
            };
            assert_eq!(result.is_error, Some(true));
            result
                .structured_content
                .expect("error envelope in structured_content")
        }

        /// (a) A valid MCP application error through the proxy must leave the
        /// upstream routable with no last-error — the pool recorded SUCCESS
        /// and the proxy must not record a failure on top (no false
        /// disconnect). The envelope carries the structured `invalid_param`
        /// kind instead of a generic `upstream_error`.
        #[tokio::test]
        async fn proxy_mcp_app_error_leaves_upstream_health_untouched() {
            let pool =
                pool_with_upstream(in_process_connection(McpRejectingServer).await, None).await;
            let server = proxy_test_server(Arc::clone(&pool)).await;

            let envelope = call_through_proxy(server).await;

            assert_eq!(envelope["error"]["kind"], "invalid_param");
            assert!(
                envelope["error"]["cause"]
                    .as_str()
                    .expect("cause preserved")
                    .contains("unknown field `since`")
            );
            assert_eq!(
                pool.upstream_tool_last_error(UPSTREAM_NAME).await,
                None,
                "proxy must not set upstream_tool_last_error for a valid MCP error"
            );
            assert!(
                matches!(
                    pool.upstream_tool_health(UPSTREAM_NAME).await,
                    Some(UpstreamHealth::Healthy)
                ),
                "valid MCP error response must leave the upstream Healthy"
            );
        }

        /// (b) A genuine transport failure (timeout) is recorded EXACTLY once
        /// — by the pool. Before the fix the proxy recorded a second failure
        /// on top (`consecutive_failures: 2`), halving the effective
        /// `CIRCUIT_BREAKER_THRESHOLD`.
        #[tokio::test]
        async fn proxy_transport_timeout_records_failure_exactly_once() {
            let pool = pool_with_upstream(
                in_process_connection(StallingServer).await,
                Some(Duration::from_millis(100)),
            )
            .await;
            let server = proxy_test_server(Arc::clone(&pool)).await;

            let envelope = call_through_proxy(server).await;

            assert_eq!(envelope["error"]["kind"], "timeout");
            assert!(
                matches!(
                    pool.upstream_tool_health(UPSTREAM_NAME).await,
                    Some(UpstreamHealth::Unhealthy {
                        consecutive_failures: 1
                    })
                ),
                "one transport failure must be recorded exactly once, got {:?}",
                pool.upstream_tool_health(UPSTREAM_NAME).await
            );
            assert!(
                pool.upstream_tool_last_error(UPSTREAM_NAME)
                    .await
                    .expect("pool records the timeout as last error")
                    .contains("timed out")
            );
        }
    }
}
