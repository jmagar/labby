//! `read_resource` proxy branch bodies: gateway-synthetic, upstream,
//! and subject-scoped resource proxying.
//!
//! Extracted from `server.rs` (bead `lab-kvji.24.1.3`) as inherent
//! `impl LabMcpServer` methods. `read_resource_impl` in
//! `handlers_resources.rs` keeps the prefix-dispatch skeleton and the
//! local `lab://catalog` / `lab://<svc>/actions` branch; these helpers
//! own each proxy branch.
//!
//! Seam discipline (Revision 2 finding #4): the resolved `pool`,
//! `oauth_subject`, and `config` are threaded in from the caller's
//! guards so the three-branch ordering and per-branch side effects
//! (structured logging + `pool.read_upstream_resource` ordering — no
//! circuit-breaker `record_*`) are byte-identical to the original.

use std::sync::Arc;
use std::time::Instant;

use rmcp::ErrorData;
use rmcp::RoleServer;
use rmcp::model::{
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceContents,
    ResultType,
};
use rmcp::service::RequestContext;

use crate::config::UpstreamConfig;
use crate::dispatch::upstream::pool::{UpstreamPool, redact_resource_uri_for_logging};
use crate::mcp::context::{
    auth_context_from_extensions, forwardable_client_capabilities,
    oauth_upstream_subject_for_request, redacted_oauth_subject_label,
};
use crate::mcp::logging::{DispatchLogOutcome, LoggingLevel};
use crate::mcp::resource_errors::render as resource_render_error;
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    /// Gateway-synthetic resource branch (`lab://gateway/...`). Returns
    /// unconditionally; the caller invokes this only when the URI prefix
    /// matches.
    pub(crate) async fn read_gateway_resource_impl(
        &self,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject,
            resource_uri = redact_resource_uri_for_logging(&uri),
            route = "gateway",
            "dispatch route selected"
        );
        let Some(manager) = &self.gateway_manager else {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri = redact_resource_uri_for_logging(&uri),
                route = "gateway",
                elapsed_ms,
                kind = "unavailable",
                "upstream pool not configured"
            );
            self.emit_dispatch_notification(
                context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "unavailable",
                },
            )
            .await;
            return Err(ErrorData::resource_not_found(
                "upstream pool not configured".to_string(),
                None,
            ));
        };

        let auth = auth_context_from_extensions(&context.extensions);
        let scope = crate::dispatch::gateway::GatewayEnrichmentScope {
            route_visible_upstreams: self.route_scope.allowed_upstreams().cloned(),
            oauth_subject: self
                .route_oauth_subject(oauth_upstream_subject_for_request(
                    auth,
                    self.request_subject(context),
                ))
                .map(std::borrow::Cow::into_owned),
        };
        let json = if uri == "lab://gateway/servers" {
            manager.gateway_servers_doc_scoped(&scope).await.map(Some)
        } else if let Some(name) = uri
            .strip_prefix("lab://gateway/")
            .and_then(|rest| rest.strip_suffix("/schema"))
            .filter(|name| !name.is_empty() && !name.contains('/'))
        {
            manager
                .gateway_server_schema_scoped(name, &scope)
                .await
                .map(Some)
        } else {
            Ok(None)
        };
        let json = match json {
            Ok(json) => json,
            Err(error) => {
                let elapsed_ms = start.elapsed().as_millis();
                let error_kind = match &error {
                    labby_runtime::error::ToolError::Sdk { sdk_kind, .. } => sdk_kind.as_str(),
                    _ => "internal_error",
                };
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    route = "gateway",
                    elapsed_ms,
                    kind = error_kind,
                    error = %error,
                    "synthetic gateway resource discovery failed"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "upstream_error",
                    },
                )
                .await;
                return Err(resource_render_error(uri, error.to_string()));
            }
        };

        let elapsed_ms = start.elapsed().as_millis();
        match json {
            Some(value) => {
                let text = match serde_json::to_string_pretty(&value) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(
                            surface = "mcp",
                            service = "labby",
                            action = "read_resource",
                            resource_uri = redact_resource_uri_for_logging(&uri),
                            error = %e,
                            "failed to serialize synthetic gateway resource"
                        );
                        return Err(ErrorData::internal_error(
                            format!("failed to serialize resource: {e}"),
                            None,
                        ));
                    }
                };
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    route = "gateway",
                    elapsed_ms,
                    "synthetic resource ok"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, uri.to_string())
                        .with_mime_type("application/json"),
                ]))
            }
            None => {
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    route = "gateway",
                    elapsed_ms,
                    kind = "not_found",
                    "synthetic resource not found"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "not_found",
                    },
                )
                .await;
                Err(ErrorData::resource_not_found(
                    format!("unknown resource: {uri}"),
                    None,
                ))
            }
        }
    }

    /// Upstream resource proxy branch (`lab://upstream/...`). The caller
    /// passes the already-resolved `pool` and invokes this only when the
    /// pool is present and the URI prefix matches.
    pub(crate) async fn read_upstream_resource_impl(
        &self,
        pool: &Arc<UpstreamPool>,
        request: ReadResourceRequestParams,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            resource_uri = redact_resource_uri_for_logging(&uri),
            route = "upstream",
            "dispatch route selected"
        );
        let upstream_name = uri
            .strip_prefix("lab://upstream/")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("unknown")
            .to_string();
        let relay_capabilities = forwardable_client_capabilities(request.meta.as_ref());
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
        let result = match (relay_config, relay_capabilities) {
            (Some(config), Some(capabilities)) => {
                pool.read_resource_relayed(
                    &config,
                    None,
                    request,
                    context.peer.clone(),
                    context.id.clone(),
                    context.ct.clone(),
                    self.relay_session_id,
                    capabilities,
                )
                .await
            }
            _ => pool
                .read_upstream_resource_request_allowed(
                    request,
                    self.route_scope.allowed_upstreams(),
                )
                .await
                .map(|outcome| outcome.map(Into::into)),
        };
        match result {
            Some(Ok(result)) => {
                let elapsed_ms = start.elapsed().as_millis();
                let upstream = uri
                    .strip_prefix("lab://upstream/")
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or("unknown");
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    upstream,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    "resource proxy ok"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(result)
            }
            Some(Err(message)) => {
                let elapsed_ms = start.elapsed().as_millis();
                let upstream = uri
                    .strip_prefix("lab://upstream/")
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or("unknown");
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    upstream,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    kind = "internal_error",
                    error = %message,
                    "resource proxy failed"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Error,
                        kind: "internal_error",
                    },
                )
                .await;
                Err(ErrorData::internal_error(message, None))
            }
            None => {
                let elapsed_ms = start.elapsed().as_millis();
                let upstream = uri
                    .strip_prefix("lab://upstream/")
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or("unknown");
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    upstream,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    kind = "not_found",
                    "upstream not connected for resource"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "not_found",
                    },
                )
                .await;
                Err(ErrorData::resource_not_found(
                    format!("unknown resource: {uri}"),
                    None,
                ))
            }
        }
    }

    /// Upstream MCP Apps (mcp-ui) widget resource branch (`ui://<upstream>/…`).
    ///
    /// These are native `ui://` resources owned by an upstream peer (referenced
    /// by a tool result's `_meta.ui.resourceUri`). The caller invokes this only
    /// for non-local `ui://` URIs — `ui://lab/code-mode/*` stays on the local
    /// Code Mode app handler. Reverse-lookup + forwarding lives in the pool;
    /// this method stays envelope-only.
    pub(crate) async fn read_upstream_ui_resource_impl(
        &self,
        pool: &Arc<UpstreamPool>,
        request: ReadResourceRequestParams,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let auth = auth_context_from_extensions(&context.extensions);
        let oauth_subject = self.route_oauth_subject(oauth_upstream_subject_for_request(
            auth,
            self.request_subject(context),
        ));
        if let Some(oauth_subject) = oauth_subject.as_ref() {
            let configs = self.route_scoped_oauth_upstream_configs().await;
            match pool
                .cached_subject_scoped_ui_resource_owner(
                    &configs,
                    oauth_subject.as_ref(),
                    &uri,
                    self.route_scope.allowed_upstreams(),
                )
                .await
            {
                Ok(Some(config)) => {
                    tracing::info!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        resource_uri = redact_resource_uri_for_logging(&uri),
                        upstream = %config.name,
                        route = "subject_scoped_ui",
                        oauth_subject = redacted_oauth_subject_label(),
                        "dispatch route selected"
                    );
                    return self
                        .read_subject_scoped_resource_impl(
                            pool,
                            &config,
                            oauth_subject.as_ref(),
                            request,
                            subject,
                            start,
                            context,
                        )
                        .await;
                }
                Err(message) => {
                    let elapsed_ms = start.elapsed().as_millis();
                    tracing::warn!(
                        surface = "mcp",
                        service = "labby",
                        action = "read_resource",
                        resource_uri = redact_resource_uri_for_logging(&uri),
                        elapsed_ms,
                        kind = "subject_scoped_ui_unavailable",
                        error = %message,
                        "OAuth UI resource cannot be routed safely"
                    );
                    self.emit_dispatch_notification(
                        context,
                        "lab",
                        "read_resource",
                        elapsed_ms,
                        DispatchLogOutcome::Failure {
                            level: LoggingLevel::Warning,
                            kind: "subject_scoped_ui_unavailable",
                        },
                    )
                    .await;
                    return Err(ErrorData::resource_not_found(
                        format!("unavailable UI resource: {uri}"),
                        None,
                    ));
                }
                Ok(None) => {}
            }
        }

        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            resource_uri = redact_resource_uri_for_logging(&uri),
            route = "upstream_ui",
            "dispatch route selected"
        );
        let result = pool
            .read_upstream_ui_resource_allowed(&uri, self.route_scope.allowed_upstreams())
            .await;
        let elapsed_ms = start.elapsed().as_millis();
        let (outcome, response) = match result {
            Some(Ok(mut result)) => {
                // Older upstream revisions legitimately omit the SEP-2322
                // discriminator. Labby negotiated the current revision with
                // its downstream peer, so its response must restore the
                // required complete marker at this protocol boundary.
                result.result_type.get_or_insert(ResultType::COMPLETE);
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    "ui resource proxy ok"
                );
                (DispatchLogOutcome::Success, Ok(result.into()))
            }
            Some(Err(message)) => {
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    kind = "internal_error",
                    error = %message,
                    "ui resource proxy failed"
                );
                (
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Error,
                        kind: "internal_error",
                    },
                    Err(ErrorData::internal_error(message, None)),
                )
            }
            None => {
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    kind = "not_found",
                    "no upstream owns ui resource"
                );
                (
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "not_found",
                    },
                    Err(ErrorData::resource_not_found(
                        format!("unknown UI resource: {uri}"),
                        None,
                    )),
                )
            }
        };
        self.emit_dispatch_notification(context, "lab", "read_resource", elapsed_ms, outcome)
            .await;
        response
    }

    /// Subject-scoped resource proxy branch. The caller passes the
    /// already-resolved `pool`, `config`, and `oauth_subject` and invokes
    /// this only when all guards matched.
    pub(crate) async fn read_subject_scoped_resource_impl(
        &self,
        pool: &Arc<UpstreamPool>,
        config: &UpstreamConfig,
        oauth_subject: &str,
        request: ReadResourceRequestParams,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            resource_uri = redact_resource_uri_for_logging(&uri),
            upstream = %config.name,
            route = "subject_scoped",
            oauth_subject = redacted_oauth_subject_label(),
            "dispatch route selected"
        );
        let relay_capabilities = forwardable_client_capabilities(request.meta.as_ref());
        let upstream_outcome = if let Some(capabilities) = relay_capabilities {
            pool.read_resource_relayed(
                config,
                Some(oauth_subject),
                request,
                context.peer.clone(),
                context.id.clone(),
                context.ct.clone(),
                self.relay_session_id,
                capabilities,
            )
            .await
            .unwrap_or_else(|| Err(format!("relayed upstream `{}` connect failed", config.name)))
        } else {
            pool.subject_scoped_read_resource_request(config, oauth_subject, request)
                .await
                .map(Into::into)
        };
        match upstream_outcome {
            Ok(result) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    oauth_subject = redacted_oauth_subject_label(),
                    upstream = %config.name,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    "subject-scoped resource proxy ok"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Success,
                )
                .await;
                Ok(result)
            }
            Err(message) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    upstream = %config.name,
                    resource_uri = redact_resource_uri_for_logging(&uri),
                    elapsed_ms,
                    kind = "upstream_error",
                    error = %message,
                    "subject-scoped resource proxy failed"
                );
                self.emit_dispatch_notification(
                    context,
                    "lab",
                    "read_resource",
                    elapsed_ms,
                    DispatchLogOutcome::Failure {
                        level: LoggingLevel::Warning,
                        kind: "upstream_error",
                    },
                )
                .await;
                Err(ErrorData::invalid_params(message, None))
            }
        }
    }
}
