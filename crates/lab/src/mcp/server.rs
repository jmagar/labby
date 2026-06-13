//! `LabMcpServer` — the MCP `ServerHandler` implementation.
//!
//! Extracted from `cli/serve.rs` so that both the stdio and HTTP transports
//! can share the same handler logic.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use axum::http;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, CompleteRequestParams, CompleteResult,
    ExtensionCapabilities, GetPromptRequestParams, GetPromptResult, ListPromptsResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ServerCapabilities, ServerInfo, SetLevelRequestParams,
};
use rmcp::service::{NotificationContext, Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tokio::sync::RwLock;

use crate::config::NodeRole;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::mcp::completion::{complete_prompt_arg, completion_info};
use crate::mcp::context::subject_from_extensions;
use crate::mcp::logging::{DispatchLogOutcome, logging_level_rank};
use crate::mcp::route_scope::McpRouteScope;
use crate::registry::ToolRegistry;

/// MCP server handler — one tool per registered service.
pub struct LabMcpServer {
    pub registry: Arc<ToolRegistry>,
    /// Shared gateway manager used to resolve the current live upstream pool.
    pub gateway_manager: Option<Arc<GatewayManager>>,
    /// Resolved role for the current device.
    pub node_role: Option<NodeRole>,
    /// Connected peers for list-changed notifications.
    pub peers: Arc<RwLock<Vec<Peer<RoleServer>>>>,
    /// Negotiated RMCP logging threshold for this server/session.
    pub logging_level: Arc<AtomicU8>,
    /// Visibility and dispatch constraints for this MCP route/session.
    pub(crate) route_scope: McpRouteScope,
    #[cfg(test)]
    pub(crate) code_mode_widget_callbacks_enabled_for_test: bool,
}

pub fn verify_upstream_subject_resolution_support() -> anyhow::Result<()> {
    let (parts, _) = http::Request::new(()).into_parts();
    let auth = crate::api::oauth::AuthContext {
        sub: "startup-self-test".to_string(),
        actor_key: None,
        scopes: Vec::new(),
        issuer: "https://lab.example.com".to_string(),
        via_session: false,
        csrf_token: None,
        email: None,
    };

    let mut extensions = rmcp::model::Extensions::new();
    let mut parts = parts;
    parts.extensions.insert(auth);
    extensions.insert(parts);

    if subject_from_extensions(&extensions) == Some("startup-self-test") {
        return Ok(());
    }

    anyhow::bail!(
        "rmcp subject extraction self-test failed: RequestContext.extensions did not yield \
         http::request::Parts/AuthContext. The current runtime expects rmcp 1.4 request \
         extension propagation (Plan A). Wire the tokio::task_local fallback (Plan B) or pin \
         a compatible rmcp version before starting."
    );
}

/// Advertise the MCP Apps UI extension (`io.modelcontextprotocol/ui`, SEP-1724)
/// so hosts like Claude.ai know to render the Code Mode inspector widgets served
/// at `ui://lab/code-mode/{search,execute,history}`. The `mimeTypes` value mirrors
/// the MIME the widget resources are published with (`text/html;profile=mcp-app`).
fn mcp_apps_ui_extension() -> ExtensionCapabilities {
    let mut extensions = ExtensionCapabilities::new();
    let mut ui_ext = serde_json::Map::new();
    ui_ext.insert(
        "mimeTypes".to_string(),
        serde_json::json!([crate::mcp::handlers_resources::CODE_MODE_APP_MIME]),
    );
    extensions.insert("io.modelcontextprotocol/ui".to_string(), ui_ext);
    extensions
}

impl ServerHandler for LabMcpServer {
    fn get_info(&self) -> ServerInfo {
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "server.info",
            subsystem = "mcp_server",
            phase = "server.info",
            builtin_service_count = self.registry.services().len(),
            gateway_manager_configured = self.gateway_manager.is_some(),
            node_role = ?self.node_role,
            "advertising MCP server capabilities"
        );
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_list_changed()
                .enable_prompts()
                .enable_prompts_list_changed()
                .enable_logging()
                .enable_completions()
                .enable_extensions_with(mcp_apps_ui_extension())
                .build(),
        )
    }

    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.logging_level
            .store(logging_level_rank(request.level), Ordering::Release);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "logging.setLevel",
            level = ?request.level,
            "rmcp logging level updated"
        );
        Ok(())
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let mut peers = self.peers.write().await;
        peers.push(context.peer);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.connect",
            subsystem = "mcp_server",
            phase = "session.initialized",
            peer_count = peers.len(),
            "mcp session connected"
        );
    }

    async fn complete(
        &self,
        request: CompleteRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, ErrorData> {
        let start = Instant::now();
        let subject = self.request_subject_log_tag(&context);
        let reference_type = request.r#ref.reference_type();
        let prompt = request.r#ref.as_prompt_name().map(str::to_string);
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            "dispatch start"
        );

        let completion = match prompt.as_deref() {
            Some(prompt_name) => complete_prompt_arg(
                &self.registry,
                prompt_name,
                &request.argument.name,
                &request.argument.value,
            ),
            None => completion_info(Vec::new()),
        };

        let elapsed_ms = start.elapsed().as_millis();
        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "completion.complete",
            subject,
            reference_type,
            prompt = prompt.as_deref().unwrap_or(""),
            argument = %request.argument.name,
            result_count = completion.values.len(),
            elapsed_ms,
            "completion ok"
        );
        self.emit_dispatch_notification(
            &context,
            "lab",
            "completion.complete",
            elapsed_ms,
            DispatchLogOutcome::Success,
        )
        .await;

        Ok(CompleteResult::new(completion))
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.list_prompts_impl(request, context).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        self.get_prompt_impl(request, context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.list_resources_impl(request, context).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.read_resource_impl(request, context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.list_tools_impl(request, context).await
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.call_tool_impl(request, context).await
    }
}

use crate::mcp::catalog::CatalogSnapshot;

impl LabMcpServer {
    pub(crate) async fn notify_catalog_changes(
        &self,
        before: &CatalogSnapshot,
        after: &CatalogSnapshot,
    ) {
        if before == after {
            return;
        }

        let peers = self.peers.read().await.clone();
        let peer_count = peers.len();
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "catalog.notify",
            subsystem = "mcp_server",
            phase = "catalog.notify",
            peer_count,
            tools_changed = before.tools != after.tools,
            resources_changed = before.resources != after.resources,
            prompts_changed = before.prompts != after.prompts,
            "notifying MCP peers about catalog change"
        );
        let mut alive = Vec::with_capacity(peers.len());
        for (peer_index, peer) in peers.into_iter().enumerate() {
            let mut ok = true;
            if before.tools != after.tools {
                if peer.notify_tool_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "tools",
                        "failed to notify peer about tool catalog change; pruning stale session"
                    );
                    ok = false;
                }
            }
            if ok && before.resources != after.resources {
                if peer.notify_resource_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "resources",
                        "failed to notify peer about resource catalog change; pruning stale session"
                    );
                    ok = false;
                }
            }
            if ok && before.prompts != after.prompts {
                if peer.notify_prompt_list_changed().await.is_err() {
                    tracing::warn!(
                        surface = "mcp",
                        service = "peers",
                        action = "peer.disconnect",
                        peer_index,
                        phase = "prompts",
                        "failed to notify peer about prompt catalog change; pruning stale session"
                    );
                    ok = false;
                }
            }
            if ok {
                alive.push(peer);
            }
        }
        let mut guard = self.peers.write().await;
        let added_since_snapshot = if guard.len() > peer_count {
            guard.split_off(peer_count)
        } else {
            Vec::new()
        };
        let alive_count = alive.len();
        *guard = alive;
        guard.extend(added_since_snapshot);
        let pruned = peer_count.saturating_sub(alive_count);
        tracing::info!(
            surface = "mcp",
            service = "peers",
            action = "peer.gc",
            pruned_count = pruned,
            active_count = guard.len(),
            "MCP peer catalog-change notification complete"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{LabMcpServer, logging_level_rank, verify_upstream_subject_resolution_support};
    use crate::registry::ToolRegistry;
    use rmcp::ServerHandler;

    #[test]
    fn server_capabilities_advertise_list_changed_support() {
        let server = LabMcpServer {
            registry: std::sync::Arc::new(ToolRegistry::new()),
            gateway_manager: None,
            node_role: None,
            peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
            logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
                logging_level_rank(rmcp::model::LoggingLevel::Info),
            )),
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            code_mode_widget_callbacks_enabled_for_test: false,
        };

        let info = server.get_info();
        assert_eq!(
            info.capabilities.tools.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.resources.and_then(|c| c.list_changed),
            Some(true)
        );
        assert_eq!(
            info.capabilities.prompts.and_then(|c| c.list_changed),
            Some(true)
        );
        assert!(
            info.capabilities.logging.is_some(),
            "RMCP logging capability must be advertised"
        );
        assert!(
            info.capabilities.completions.is_some(),
            "RMCP completion capability must be advertised"
        );

        // MCP Apps UI extension (SEP-1724) must be advertised so hosts render
        // the Code Mode inspector widgets.
        let extensions = info
            .capabilities
            .extensions
            .expect("MCP Apps UI extension capability must be advertised");
        let ui_ext = extensions
            .get("io.modelcontextprotocol/ui")
            .expect("io.modelcontextprotocol/ui extension must be present");
        assert_eq!(
            ui_ext.get("mimeTypes"),
            Some(&serde_json::json!(["text/html;profile=mcp-app"])),
            "UI extension must advertise the mcp-app widget MIME type"
        );
    }

    #[test]
    fn upstream_subject_resolution_self_test_passes_for_plan_a() {
        verify_upstream_subject_resolution_support().expect("self-test");
    }
}
