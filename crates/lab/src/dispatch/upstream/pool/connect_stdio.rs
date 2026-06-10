//! Connection establishment for stdio (child-process) upstreams.
//!
//! `connect_stdio_upstream` spawns a child process and arms the process-group
//! guard. In-process peer construction is owned by `crate::mcp::in_process_peer`
//! (the `InProcessConnector` IoC seam) — this module no longer imports from
//! `crate::mcp` (A-M6 fix).

use crate::config::UpstreamConfig;
use rmcp::{RoleClient, ServiceExt};

use super::super::auth::configured_bearer_token;
use super::super::types::{UpstreamRuntimeMetadata, UpstreamRuntimeOwner};
use super::UpstreamConnection;
use super::connect::runtime_origin_label;

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

/// Maximum number of bytes forwarded per line from a child's stderr.
///
/// Lines longer than this are truncated before emission so a chatty or
/// adversarial upstream cannot amplify log volume with a single huge write.
const STDERR_LINE_MAX_BYTES: usize = 1024;

/// Maximum number of lines forwarded per second from a single upstream's stderr.
///
/// Exceeding this cap drops lines and emits a single `WARN` instead, protecting
/// labby's log stream from a firehose upstream (O-M2 / P-L5).
const STDERR_RATE_CAP_PER_SEC: u32 = 50;

/// Drain a piped child stderr to EOF, forwarding each non-empty line into the
/// gateway log under the `labby::upstream_stderr` target at **DEBUG** level
/// (silence just this stream with `LAB_LOG=labby::upstream_stderr=warn`).
///
/// Security / observability invariants (O-M2 / P-L5):
/// - Lines are emitted at DEBUG so a chatty upstream does not pollute the INFO stream.
/// - Each line is capped at `STDERR_LINE_MAX_BYTES` before logging.
/// - Lines are rate-limited to `STDERR_RATE_CAP_PER_SEC`; bursts above the cap
///   are dropped with a single warning rather than forwarded verbatim.
/// - Each line is run through `redact_stdio_value` so a third-party upstream
///   printing its own token cannot launder credentials into labby's log stream.
///
/// Draining is mandatory: an unread pipe buffer fills and blocks the child's
/// next stderr write, hanging the upstream.
fn forward_upstream_stderr(stderr: Option<tokio::process::ChildStderr>, upstream: String) {
    let Some(stderr) = stderr else { return };
    tokio::spawn(async move {
        use crate::dispatch::redact::redact_stdio_value;
        use tokio::io::{AsyncBufReadExt, BufReader};

        let mut lines = BufReader::new(stderr).lines();
        // Rate-limiting state: count of lines emitted in the current second.
        let mut window_start = std::time::Instant::now();
        let mut lines_this_window: u32 = 0;
        let mut dropped_this_window: u32 = 0;

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }

                    // Rotate rate-limit window every second.
                    if window_start.elapsed().as_secs() >= 1 {
                        if dropped_this_window > 0 {
                            tracing::warn!(
                                target: "labby::upstream_stderr",
                                upstream = %upstream,
                                dropped = dropped_this_window,
                                "upstream stderr rate cap exceeded; lines dropped"
                            );
                        }
                        window_start = std::time::Instant::now();
                        lines_this_window = 0;
                        dropped_this_window = 0;
                    }

                    if lines_this_window >= STDERR_RATE_CAP_PER_SEC {
                        dropped_this_window += 1;
                        continue;
                    }
                    lines_this_window += 1;

                    // Truncate long lines before redaction/emission.
                    let truncated = if line.len() > STDERR_LINE_MAX_BYTES {
                        format!("{}…[truncated]", &line[..STDERR_LINE_MAX_BYTES])
                    } else {
                        line
                    };

                    // Redact credential-shaped tokens before forwarding.
                    let redacted = redact_stdio_value(&truncated);

                    tracing::debug!(
                        target: "labby::upstream_stderr",
                        surface = "dispatch",
                        service = "upstream.pool",
                        upstream = %upstream,
                        stream = "stderr",
                        "{redacted}",
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
