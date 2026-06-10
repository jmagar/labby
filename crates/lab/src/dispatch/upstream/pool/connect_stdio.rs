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

    // SECURITY (S1): never inherit labby's full environment — it holds
    // LAB_OAUTH_ENCRYPTION_KEY and every upstream credential. Start from a
    // scrubbed allowlist of runtime essentials (so npx/uvx/docker/etc. can still
    // find binaries, caches, and TLS roots), then layer the upstream's declared
    // env (and bearer token, below) on top.
    const STDIO_ENV_ALLOWLIST: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "TERM",
        "TZ",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "LC_CTYPE",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "WINDIR",
        "PATHEXT",
        "COMSPEC",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "PROGRAMFILES",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
    ];

    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.env_clear();
    for key in STDIO_ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.envs(config.env.iter());

    // Set bearer token env var on the child if configured
    if let Some(ref env_name) = config.bearer_token_env
        && let Some(token) = configured_bearer_token(env_name)
    {
        cmd.env(env_name, &token);
    }

    // A stdio MCP server logs to stderr (stdout is the JSON-RPC channel), so the
    // child's stderr is the ONLY place its server-side diagnostics go. Capture
    // it by default and forward into the gateway log; opt out per `LAB_GW_UPSTREAM_STDERR`.
    let capture_stderr = upstream_stderr_capture_enabled();
    let stderr_cfg = || {
        if capture_stderr {
            Stdio::piped()
        } else {
            Stdio::null()
        }
    };

    #[cfg(unix)]
    let (process, child_stderr) = {
        let mut wrapped = CommandWrap::from(cmd);
        wrapped.wrap(ProcessGroup::leader());
        TokioChildProcess::builder(wrapped)
            .stderr(stderr_cfg())
            .spawn()?
    };
    #[cfg(not(unix))]
    let (process, child_stderr) = TokioChildProcess::builder(cmd)
        .stderr(stderr_cfg())
        .spawn()?;

    // INVARIANT: a piped child stderr MUST be drained continuously. A chatty
    // upstream (e.g. axon at INFO) fills the ~64 KB pipe buffer and then blocks
    // on its next stderr write, hanging the upstream. The drain task reads to
    // EOF so failures are recoverable from the gateway log instead of lost.
    forward_upstream_stderr(child_stderr, config.name.clone());

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

/// Whether to capture spawned-upstream stderr and forward it into the gateway
/// log. Default: enabled. Set `LAB_GW_UPSTREAM_STDERR` to `null`/`off`/`0` to
/// discard it (the pre-capture behavior) for an extremely chatty upstream.
fn upstream_stderr_capture_enabled() -> bool {
    match std::env::var("LAB_GW_UPSTREAM_STDERR") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "null" | "off" | "0" | "none" | "discard" | "false"
        ),
        Err(_) => true,
    }
}

/// Drain a piped child stderr to EOF, forwarding each non-empty line into the
/// gateway log under the `labby::upstream_stderr` target (silence just this
/// stream with `LAB_LOG=labby::upstream_stderr=warn`). Draining is mandatory:
/// an unread pipe buffer fills and blocks the child's next stderr write.
fn forward_upstream_stderr(stderr: Option<tokio::process::ChildStderr>, upstream: String) {
    let Some(stderr) = stderr else { return };
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    tracing::info!(
                        target: "labby::upstream_stderr",
                        surface = "dispatch",
                        service = "upstream.pool",
                        upstream = %upstream,
                        stream = "stderr",
                        "{line}",
                    );
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::debug!(
                        target: "labby::upstream_stderr",
                        upstream = %upstream,
                        error = %error,
                        "upstream stderr drain ended on read error",
                    );
                    break;
                }
            }
        }
    });
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
