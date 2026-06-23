//! Connection establishment for stdio (child-process) upstreams.
//!
//! `connect_stdio_upstream` spawns a child process and arms the process-group
//! guard. In-process peer construction is owned by `crate::mcp::in_process_peer`
//! (the `InProcessConnector` IoC seam) — this module no longer imports from
//! `crate::mcp` (A-M6 fix).

use labby_runtime::gateway_config::UpstreamConfig;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

use super::super::auth::configured_bearer_token;
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::UpstreamConnection;
use super::connect::runtime_origin_label;
use super::stdio_stderr::{
    StdioConnectError, StdioDiagnostics, forward_upstream_stderr, upstream_stderr_log_level,
};

/// Connect to a stdio upstream MCP server (child process).
///
/// ## Security invariants
///
/// - **`env_clear` + allowlist (S1):** the child process is started with a
///   scrubbed environment (`cmd.env_clear()`). Only vars in `STDIO_ENV_ALLOWLIST`
///   (runtime essentials: PATH, HOME, TZ, SSL roots, …) are forwarded; the
///   upstream's declared `env` map and the optional bearer-token var are then
///   layered on top. `LAB_*` secrets and every other ambient labby env var are
///   excluded.
///
/// - **Spawn-guard allowlist (S6 — accepted residual):** `validate_stdio_command`
///   in `spawn_guard.rs` checks that the command basename is in
///   `ALLOWED_RUNTIME_HINTS`. The check is **basename-only** — a path like
///   `/tmp/x/node` passes because `Path::file_name()` extracts `node`. This is
///   an accepted residual: the trust boundary is admin-write access to the gateway
///   config file or authenticated `gateway.add` / `gateway.update` calls. The
///   allowlist is applied at config-write time, not here.
pub(super) async fn connect_stdio_upstream<H: ClientHandler + Clone>(
    command: &str,
    args: &[String],
    config: &UpstreamConfig,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    handler: H,
) -> anyhow::Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>)> {
    // Cross-process spawn lock: stdio servers launched via `npx -y`/`uvx` install
    // into a shared package cache on first cold spawn; two processes installing
    // the same package at once corrupt it. Hold an advisory file lock (keyed on
    // the command + args) for the whole connect — spawn, handshake, list_tools,
    // and a possible targeted cache repair/retry.
    let mut spawn_lock = super::spawn_lock::open(command, args);
    let _spawn_guard = super::spawn_lock::acquire(spawn_lock.as_mut()).await;

    match connect_stdio_upstream_once(
        command,
        args,
        config,
        runtime_origin,
        runtime_owner,
        handler.clone(),
    )
    .await
    {
        Ok(ok) => Ok(ok),
        Err(first_error) => {
            let diagnostics = first_error.diagnostics_with_error();
            let repair = super::cache_repair::maybe_repair(command, &diagnostics).await;
            match &repair {
                super::cache_repair::CacheRepairOutcome::Repaired { summary } => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        upstream = %config.name,
                        command = %command,
                        action = "upstream.cache_repair",
                        repair = %summary,
                        "stdio package-runner cache repaired after startup failure; retrying once"
                    );
                }
                super::cache_repair::CacheRepairOutcome::Failed { summary } => {
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        upstream = %config.name,
                        command = %command,
                        action = "upstream.cache_repair",
                        repair = %summary,
                        "stdio package-runner cache repair failed; returning original startup error"
                    );
                    return Err(first_error.into_anyhow());
                }
                _ => return Err(first_error.into_anyhow()),
            }

            match connect_stdio_upstream_once(
                command,
                args,
                config,
                runtime_origin,
                runtime_owner,
                handler,
            )
            .await
            {
                Ok(ok) => Ok(ok),
                Err(retry_error) => Err(anyhow::anyhow!(
                    "stdio upstream failed after package-runner cache repair retry: {}",
                    retry_error.diagnostics_with_error()
                )),
            }
        }
    }
}

async fn connect_stdio_upstream_once<H: ClientHandler>(
    command: &str,
    args: &[String],
    config: &UpstreamConfig,
    runtime_origin: Option<&str>,
    runtime_owner: Option<&UpstreamRuntimeOwner>,
    handler: H,
) -> Result<(UpstreamConnection<H>, Vec<rmcp::model::Tool>), StdioConnectError> {
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
    // it by default and forward into the gateway log at the level resolved from
    // `LAB_GW_UPSTREAM_STDERR` (default DEBUG; `off` discards).
    let stderr_level = upstream_stderr_log_level();
    let stderr_capture = StdioDiagnostics::default();
    let stderr_cfg = || Stdio::piped();

    #[cfg(unix)]
    let (process, child_stderr) = {
        let mut wrapped = CommandWrap::from(cmd);
        wrapped.wrap(ProcessGroup::leader());
        TokioChildProcess::builder(wrapped)
            .stderr(stderr_cfg())
            .spawn()
            .map_err(StdioConnectError::without_diagnostics)?
    };
    #[cfg(not(unix))]
    let (process, child_stderr) = {
        TokioChildProcess::builder(cmd)
            .stderr(stderr_cfg())
            .spawn()
            .map_err(StdioConnectError::without_diagnostics)?
    };

    // INVARIANT: a piped child stderr MUST be drained continuously. A chatty
    // upstream (e.g. axon at INFO) fills the ~64 KB pipe buffer and then blocks
    // on its next stderr write, hanging the upstream. The drain task reads to
    // EOF so failures are recoverable from the gateway log instead of lost.
    forward_upstream_stderr(
        child_stderr,
        config.name.clone(),
        stderr_level,
        stderr_capture.clone(),
    );

    let pid = process.id();
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "stdio",
        action = "upstream.connect.start", command = %command, pid = ?pid,
        "upstream connect start",
    );

    // INVARIANT: arm the process-tree guard immediately after spawn. If any
    // subsequent `?` propagates (serve fails, list_all_tools fails, the outer
    // future is dropped on timeout), `Drop` on this guard reaps grandchildren
    // (npx → node, sh -c → python) that rmcp's per-PID TokioChildProcess Drop
    // would otherwise miss.
    //
    // Unix: `ProcessGroup::leader()` made the child its own group leader
    //   (pgid == pid). The guard SIGTERMs+SIGKILLs the group via `killpg`.
    //
    // Windows: `JobObjectGuard::arm` creates a Job Object, assigns the child
    //   (and therefore all its future descendants) to it, and sets
    //   JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Closing the handle (on Drop or in
    //   shutdown) lets the OS terminate the whole tree.
    #[cfg(unix)]
    let pg_guard = pid.map(super::super::process_guard::ProcessGroupGuard::arm);
    #[cfg(windows)]
    let job_guard = pid.map(super::super::process_guard::JobObjectGuard::arm);

    let service: rmcp::service::RunningService<RoleClient, H> = match handler.serve(process).await {
        Ok(service) => service,
        Err(error) => return Err(StdioConnectError::with_diagnostics(error, &stderr_capture).await),
    };
    let peer = service.peer().clone();

    // Discover tools
    let tools = match peer.list_all_tools().await {
        Ok(tools) => tools,
        Err(error) => return Err(StdioConnectError::with_diagnostics(error, &stderr_capture).await),
    };
    tracing::info!(
        surface = "dispatch", service = "upstream.pool",
        upstream = %config.name, transport = "stdio",
        action = "upstream.connect.finish", pid = ?pid, tool_count = tools.len(),
        "upstream connect finish",
    );

    // INVARIANT: disarm the guard right before successful construction. The
    // reaping resource (pgid on Unix, job handle on Windows) is transferred
    // to UpstreamConnection.runtime; its own Drop now owns cleanup.
    // `shutdown()` clears the field before any `.await` so Drop no-ops on
    // the graceful path.
    #[cfg(unix)]
    let pgid_for_runtime =
        pg_guard.and_then(super::super::process_guard::ProcessGroupGuard::disarm);
    // On Windows pgid has no meaning — leave it None. The job_handle field
    // (set below) is the Windows-only reaping resource.
    #[cfg(windows)]
    let pgid_for_runtime: Option<u32> = None;
    // Non-Unix, non-Windows (hypothetical future target): no process-group
    // reaping mechanism; pgid stays None.
    #[cfg(all(not(unix), not(windows)))]
    let pgid_for_runtime: Option<u32> = None;

    // `disarm()` returns the job handle as `isize` (`0` == no job). When no
    // pid was available the guard is `None`, so default to the `0` sentinel.
    #[cfg(windows)]
    let job_handle_for_runtime: isize = job_guard
        .map(super::super::process_guard::JobObjectGuard::disarm)
        .unwrap_or(0);

    let conn = UpstreamConnection {
        _client_service: service,
        _server_task: None,
        peer,
        runtime: UpstreamRuntimeMetadata {
            pid,
            pgid: pgid_for_runtime,
            #[cfg(windows)]
            job_handle: job_handle_for_runtime,
            started_at: Some(std::time::SystemTime::now()),
            origin: runtime_origin_label(runtime_origin, runtime_owner),
            owner: runtime_owner.cloned(),
        },
    };

    Ok((conn, tools))
}
