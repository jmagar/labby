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
use rmcp::model::{LoggingLevel, ReadResourceResult, ResourceContents};
use rmcp::service::RequestContext;

use crate::config::UpstreamConfig;
use crate::dispatch::upstream::pool::{UpstreamPool, redact_resource_uri_for_logging};
use crate::mcp::logging::DispatchLogOutcome;
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
            resource_uri = redact_resource_uri_for_logging(uri),
            route = "gateway",
            "dispatch route selected"
        );
        let Some(pool) = self.current_upstream_pool().await else {
            let elapsed_ms = start.elapsed().as_millis();
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri = redact_resource_uri_for_logging(uri),
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

        let json = if uri == "lab://gateway/servers" {
            Some(pool.gateway_servers_doc().await)
        } else if let Some(name) = uri
            .strip_prefix("lab://gateway/")
            .and_then(|rest| rest.strip_suffix("/schema"))
            .filter(|name| !name.is_empty() && !name.contains('/'))
        {
            pool.gateway_server_schema(name).await
        } else {
            None
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
                            resource_uri = redact_resource_uri_for_logging(uri),
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
                    resource_uri = redact_resource_uri_for_logging(uri),
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
                    resource_uri = redact_resource_uri_for_logging(uri),
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
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            resource_uri = redact_resource_uri_for_logging(uri),
            route = "upstream",
            "dispatch route selected"
        );
        match pool.read_upstream_resource(uri).await {
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
                    resource_uri = redact_resource_uri_for_logging(uri),
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
                    resource_uri = redact_resource_uri_for_logging(uri),
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
                    resource_uri = redact_resource_uri_for_logging(uri),
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
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            resource_uri = redact_resource_uri_for_logging(uri),
            route = "upstream_ui",
            "dispatch route selected"
        );
        match pool.read_upstream_ui_resource(uri).await {
            Some(Ok(result)) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    resource_uri = redact_resource_uri_for_logging(uri),
                    elapsed_ms,
                    "ui resource proxy ok"
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
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    resource_uri = redact_resource_uri_for_logging(uri),
                    elapsed_ms,
                    kind = "internal_error",
                    error = %message,
                    "ui resource proxy failed"
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
                tracing::warn!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    resource_uri = redact_resource_uri_for_logging(uri),
                    elapsed_ms,
                    kind = "not_found",
                    "no upstream owns ui resource"
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
                    format!("unknown UI resource: {uri}"),
                    None,
                ))
            }
        }
    }

    /// Subject-scoped resource proxy branch. The caller passes the
    /// already-resolved `pool`, `config`, and `oauth_subject` and invokes
    /// this only when all guards matched.
    pub(crate) async fn read_subject_scoped_resource_impl(
        &self,
        pool: &Arc<UpstreamPool>,
        config: &UpstreamConfig,
        oauth_subject: &str,
        uri: &str,
        subject: &str,
        start: Instant,
        context: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            resource_uri = redact_resource_uri_for_logging(uri),
            upstream = %config.name,
            route = "subject_scoped",
            oauth_subject = %oauth_subject,
            "dispatch route selected"
        );
        match pool
            .subject_scoped_read_resource(config, oauth_subject, uri)
            .await
        {
            Ok(result) => {
                let elapsed_ms = start.elapsed().as_millis();
                tracing::info!(
                    surface = "mcp",
                    service = "labby",
                    action = "read_resource",
                    subject,
                    oauth_subject = %oauth_subject,
                    upstream = %config.name,
                    resource_uri = redact_resource_uri_for_logging(uri),
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
                    resource_uri = redact_resource_uri_for_logging(uri),
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
