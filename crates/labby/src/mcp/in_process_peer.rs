//! MCP-owned in-process peer construction for built-in Lab services.

use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use rmcp::model::LoggingLevel;
use rmcp::{RoleClient, ServiceExt};
use tokio::sync::RwLock;

use labby_gateway::registry::InProcessService;

use crate::dispatch::upstream::pool::{
    InProcessConnector, InProcessRegistration, UpstreamConnection, in_process_upstream_name,
};
use crate::dispatch::upstream::types::UpstreamRuntimeMetadata;
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};

const IN_PROCESS_PEER_BUFFER_BYTES: usize = 256 * 1024;

pub(crate) fn connector() -> InProcessConnector {
    Arc::new(|service: Box<dyn InProcessService>| {
        Box::pin(async move {
            // The gateway pool hands back the type-erased service it was given;
            // recover the concrete `RegisteredService` this crate registered.
            let service = service
                .as_any()
                .downcast::<RegisteredService>()
                .map_err(|_| {
                    anyhow::anyhow!(
                        "in-process connector received a non-RegisteredService peer descriptor"
                    )
                })?;
            connect_in_process_service_peer(*service).await
        })
    })
}

async fn connect_in_process_service_peer(
    service: RegisteredService,
) -> anyhow::Result<InProcessRegistration> {
    tracing::info!(
        service = service.name,
        phase = "in_process.connect.start",
        "connecting in-process peer"
    );
    let upstream_name = in_process_upstream_name(service.name);
    let entry_name: Arc<str> = Arc::from(upstream_name.as_str());
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let mut registry = ToolRegistry::new();
    registry.register(service.clone());
    let server = LabMcpServer {
        registry: Arc::new(registry),
        gateway_manager: None,
        node_role: None,
        peers: Arc::new(RwLock::new(Vec::new())),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Emergency))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: crate::mcp::server::next_relay_session_id(),
        #[cfg(test)]
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let service_name = service.name;
    let server_task = tokio::spawn(async move {
        tracing::info!(
            service = service_name,
            phase = "in_process.server.spawned",
            "starting in-process server task"
        );
        match server.serve(server_transport).await {
            Ok(running) => {
                tracing::info!(
                    service = service_name,
                    phase = "in_process.server.ready",
                    "in-process server transport ready"
                );
                if let Err(error) = running.waiting().await {
                    tracing::warn!(
                        service = service_name,
                        phase = "in_process.server.waiting.error",
                        error = %error,
                        "in-process server exited with error"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    service = service_name,
                    phase = "in_process.server.serve.error",
                    error = %error,
                    "failed to start in-process server"
                );
            }
        }
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> =
        ().serve(client_transport).await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.client.ready",
        "in-process client transport ready"
    );
    let peer = client_service.peer().clone();
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.start",
        process_code_mode_enabled = crate::config::process_code_mode_enabled(),
        "requesting in-process tool list"
    );
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.finish",
        tool_count = tools.len(),
        process_code_mode_enabled = crate::config::process_code_mode_enabled(),
        "in-process tool list received"
    );

    Ok(InProcessRegistration {
        connection: Some(UpstreamConnection {
            _client_service: client_service,
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        }),
        tools,
        entry_name,
        upstream_name,
    })
}
