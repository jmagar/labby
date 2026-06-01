//! Connection establishment for stdio (child-process) and in-process upstreams.
//!
//! `connect_stdio_upstream` spawns a child process and arms the process-group
//! guard; `connect_in_process_service_peer` wires a `LabMcpServer` over an
//! in-memory duplex transport. Both are `pub(super)` so the pool module and the
//! sibling `connect` module can call them.

use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use rmcp::model::LoggingLevel;
use rmcp::{RoleClient, ServiceExt};
use tokio::sync::RwLock;

use crate::config::UpstreamConfig;
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};

use super::super::auth::configured_bearer_token;
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::UpstreamConnection;
use super::connect::runtime_origin_label;
use super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;

/// Connect to a stdio upstream MCP server (child process).
pub(super) async fn connect_stdio_upstream(
    command: &str,
    args: &[String],
    config: &UpstreamConfig,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    #[cfg(unix)]
    use process_wrap::tokio::{CommandWrap, ProcessGroup};
    use rmcp::transport::child_process::TokioChildProcess;
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.envs(config.env.iter());

    // Set bearer token env var on the child if configured
    if let Some(ref env_name) = config.bearer_token_env
        && let Some(token) = configured_bearer_token(env_name)
    {
        cmd.env(env_name, &token);
    }

    #[cfg(unix)]
    let (process, _stderr) = {
        let mut wrapped = CommandWrap::from(cmd);
        wrapped.wrap(ProcessGroup::leader());
        TokioChildProcess::builder(wrapped)
            .stderr(Stdio::null())
            .spawn()?
    };
    #[cfg(not(unix))]
    let (process, _stderr) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::null())
        .spawn()?;

    let pid = process.id();
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "stdio",
        action = "upstream.connect.start", command = %command, pid = ?pid,
        "upstream connect start",
    );

    // INVARIANT: arm the process-group guard immediately after spawn. If any
    // subsequent `?` propagates (serve fails, list_all_tools fails, the outer
    // future is dropped on timeout), `Drop` on this guard SIGTERM+SIGKILLs
    // the process group via `killpg`, reaping grandchildren (npx → node,
    // sh -c → python) that rmcp's per-PID TokioChildProcess Drop would
    // otherwise miss. With `ProcessGroup::leader()` the child is its own
    // group leader, so pgid == pid.
    #[cfg(unix)]
    let pg_guard = pid.map(super::super::process_guard::ProcessGroupGuard::arm);

    let service: rmcp::service::RunningService<RoleClient, ()> = ().serve(process).await?;
    let peer = service.peer().clone();

    // Discover tools
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "stdio",
        action = "upstream.connect.finish", pid = ?pid, tool_count = tools.len(),
        "upstream connect finish",
    );

    // INVARIANT: disarm the guard right before successful construction. The
    // pgid is transferred to UpstreamConnection.runtime.pgid; its own Drop
    // now owns cleanup. `shutdown()` will zero runtime.pgid before any
    // `.await` so its Drop no-ops on the graceful path.
    #[cfg(unix)]
    let pgid_for_runtime =
        pg_guard.and_then(super::super::process_guard::ProcessGroupGuard::disarm);
    #[cfg(not(unix))]
    let pgid_for_runtime: Option<u32> = pid;

    let conn = UpstreamConnection {
        _client_service: service,
        _server_task: None,
        peer,
        runtime: UpstreamRuntimeMetadata {
            pid,
            pgid: pgid_for_runtime,
            started_at: Some(std::time::SystemTime::now()),
            origin: runtime_origin_label(runtime_origin, runtime_owner),
            owner: runtime_owner.cloned(),
        },
    };

    Ok((conn, tools))
}

pub(super) async fn connect_in_process_service_peer(
    service: &RegisteredService,
) -> anyhow::Result<(UpstreamConnection, Vec<rmcp::model::Tool>)> {
    tracing::info!(
        service = service.name,
        phase = "in_process.connect.start",
        "connecting in-process peer"
    );
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let mut registry = ToolRegistry::new();
    registry.register(service.clone());
    let server = LabMcpServer {
        registry: Arc::new(registry),
        gateway_manager: None,
        node_role: None,
        peers: Arc::new(RwLock::new(Vec::new())),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(LoggingLevel::Emergency))),
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
                    tracing::warn!(service = service_name, phase = "in_process.server.waiting.error", error = %error, "in-process server exited with error");
                }
            }
            Err(error) => {
                tracing::warn!(service = service_name, phase = "in_process.server.serve.error", error = %error, "failed to start in-process server");
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
        process_tool_search_enabled = crate::config::process_tool_search_enabled(),
        "requesting in-process tool list"
    );
    let tools = peer.list_all_tools().await?;
    tracing::info!(
        service = service.name,
        phase = "in_process.list_tools.finish",
        tool_count = tools.len(),
        process_tool_search_enabled = crate::config::process_tool_search_enabled(),
        "in-process tool list received"
    );

    Ok((
        UpstreamConnection {
            _client_service: client_service,
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
        },
        tools,
    ))
}
