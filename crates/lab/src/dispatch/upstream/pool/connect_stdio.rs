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
    // it by default and forward into the gateway log at the level resolved from
    // `LAB_GW_UPSTREAM_STDERR` (default DEBUG; `off` discards).
    let stderr_level = upstream_stderr_log_level();
    let stderr_cfg = || {
        if stderr_level.is_some() {
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
    let (process, child_stderr) = {
        TokioChildProcess::builder(cmd)
            .stderr(stderr_cfg())
            .spawn()?
    };

    // INVARIANT: a piped child stderr MUST be drained continuously. A chatty
    // upstream (e.g. axon at INFO) fills the ~64 KB pipe buffer and then blocks
    // on its next stderr write, hanging the upstream. The drain task reads to
    // EOF so failures are recoverable from the gateway log instead of lost.
    if let Some(level) = stderr_level {
        forward_upstream_stderr(child_stderr, config.name.clone(), level);
    }

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
    // reaping resource (pgid on Unix, job handle on Windows) is transferred
    // to UpstreamConnection.runtime; its own Drop now owns cleanup.
    // `shutdown()` clears the field before any `.await` so Drop no-ops on
    // the graceful path.
    #[cfg(unix)]
    let pgid_for_runtime =
        pg_guard.and_then(super::super::process_guard::ProcessGroupGuard::disarm);
    #[cfg(not(unix))]
    let pgid_for_runtime: Option<u32> = pid;

    #[cfg(windows)]
    let job_handle_for_runtime = job_guard
        .map(super::super::process_guard::JobObjectGuard::disarm)
        .unwrap_or(windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);

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

/// Resolve the log level for forwarded upstream stderr from
/// `LAB_GW_UPSTREAM_STDERR`.
///
/// - unset / `on` / `1` / `true` / unknown values → `Some(DEBUG)` (the
///   reviewed default: a chatty upstream must not pollute the INFO stream)
/// - `trace` / `debug` / `info` / `warn` → that level (e.g. `info` restores
///   the pre-hardening visibility for an upstream being actively debugged)
/// - `null` / `off` / `0` / `none` / `discard` / `false` → `None` — stderr is
///   discarded entirely (the pre-capture behavior)
fn upstream_stderr_log_level() -> Option<tracing::Level> {
    parse_stderr_level(std::env::var("LAB_GW_UPSTREAM_STDERR").ok().as_deref())
}

fn parse_stderr_level(raw: Option<&str>) -> Option<tracing::Level> {
    let Some(raw) = raw else {
        return Some(tracing::Level::DEBUG);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "null" | "off" | "0" | "none" | "discard" | "false" => None,
        "trace" => Some(tracing::Level::TRACE),
        "info" => Some(tracing::Level::INFO),
        "warn" | "warning" => Some(tracing::Level::WARN),
        // "debug", enable-flavored values, and anything unrecognized fall back
        // to the default level.
        _ => Some(tracing::Level::DEBUG),
    }
}

/// Truncate `line` to at most `max` bytes without splitting a UTF-8 codepoint.
///
/// A plain `&line[..max]` panics when byte `max` lands inside a multi-byte
/// character — and stderr content is upstream-controlled, so that slice was a
/// remotely triggerable panic in the drain task.
fn cap_line_bytes(line: &str, max: usize) -> &str {
    if line.len() <= max {
        return line;
    }
    let mut cut = max;
    while cut > 0 && !line.is_char_boundary(cut) {
        cut -= 1;
    }
    &line[..cut]
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
/// gateway log under the `labby::upstream_stderr` target at `level`
/// (default **DEBUG**; tune per `LAB_GW_UPSTREAM_STDERR`, or silence just this
/// stream with `LAB_LOG=labby::upstream_stderr=warn`).
///
/// Security / observability invariants (O-M2 / P-L5):
/// - Default level is DEBUG so a chatty upstream does not pollute the INFO
///   stream; the operator can raise it per `LAB_GW_UPSTREAM_STDERR=info` while
///   actively debugging an upstream.
/// - Each line is capped at `STDERR_LINE_MAX_BYTES` before logging.
/// - Lines are rate-limited to `STDERR_RATE_CAP_PER_SEC`; bursts above the cap
///   are dropped with a single warning rather than forwarded verbatim.
/// - Each line is run through `redact_stdio_value` so a third-party upstream
///   printing its own token cannot launder credentials into labby's log stream.
///
/// Draining is mandatory: an unread pipe buffer fills and blocks the child's
/// next stderr write, hanging the upstream.
fn forward_upstream_stderr(
    stderr: Option<tokio::process::ChildStderr>,
    upstream: String,
    level: tracing::Level,
) {
    let Some(stderr) = stderr else {
        // Capture was requested (level resolved) but the spawn returned no
        // stderr handle — diagnostics for this upstream are being lost.
        tracing::warn!(
            target: "labby::upstream_stderr",
            upstream = %upstream,
            "stderr capture enabled but child returned no stderr handle; upstream diagnostics will be lost"
        );
        return;
    };
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

                    // Truncate long lines (UTF-8-boundary-safe) before
                    // redaction/emission.
                    let capped = cap_line_bytes(&line, STDERR_LINE_MAX_BYTES);
                    let truncated = if capped.len() < line.len() {
                        format!("{capped}…[truncated]")
                    } else {
                        line.clone()
                    };

                    // Redact credential-shaped tokens before forwarding.
                    let redacted = redact_stdio_value(&truncated);

                    // `tracing` macros require a const level — dispatch on the
                    // configured level explicitly.
                    macro_rules! emit {
                        ($macro:ident) => {
                            tracing::$macro!(
                                target: "labby::upstream_stderr",
                                surface = "dispatch",
                                service = "upstream.pool",
                                upstream = %upstream,
                                stream = "stderr",
                                "{redacted}",
                            )
                        };
                    }
                    match level {
                        tracing::Level::TRACE => emit!(trace),
                        tracing::Level::INFO => emit!(info),
                        tracing::Level::WARN | tracing::Level::ERROR => emit!(warn),
                        _ => emit!(debug),
                    }
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

#[cfg(test)]
mod tests {
    use super::{cap_line_bytes, parse_stderr_level};

    #[test]
    fn stderr_level_unset_defaults_to_debug() {
        assert_eq!(parse_stderr_level(None), Some(tracing::Level::DEBUG));
    }

    #[test]
    fn stderr_level_named_levels_parse() {
        assert_eq!(
            parse_stderr_level(Some("trace")),
            Some(tracing::Level::TRACE)
        );
        assert_eq!(
            parse_stderr_level(Some("debug")),
            Some(tracing::Level::DEBUG)
        );
        assert_eq!(parse_stderr_level(Some("INFO")), Some(tracing::Level::INFO));
        assert_eq!(
            parse_stderr_level(Some(" warn ")),
            Some(tracing::Level::WARN)
        );
        assert_eq!(
            parse_stderr_level(Some("warning")),
            Some(tracing::Level::WARN)
        );
    }

    #[test]
    fn stderr_level_disable_values_discard() {
        for raw in ["null", "off", "0", "none", "discard", "FALSE"] {
            assert_eq!(parse_stderr_level(Some(raw)), None, "{raw}");
        }
    }

    #[test]
    fn stderr_level_enable_flavored_and_unknown_fall_back_to_debug() {
        for raw in ["", "on", "1", "true", "verbose", "garbage"] {
            assert_eq!(
                parse_stderr_level(Some(raw)),
                Some(tracing::Level::DEBUG),
                "{raw}"
            );
        }
    }

    #[test]
    fn cap_line_bytes_is_utf8_boundary_safe() {
        // 'é' is 2 bytes; a cut at byte 3 lands mid-codepoint and must back up.
        let line = "aéé";
        assert_eq!(cap_line_bytes(line, 3), "aé");
        // ASCII passes through untouched below the cap.
        assert_eq!(cap_line_bytes("abc", 8), "abc");
        // Exact-cap multi-byte input is not truncated.
        assert_eq!(cap_line_bytes("éé", 4), "éé");
        // A pathological all-multibyte line cut at byte 1 yields empty, not a panic.
        assert_eq!(cap_line_bytes("ééé", 1), "");
    }

    #[test]
    fn cap_line_bytes_never_panics_on_any_boundary() {
        let line = "x😀y漢字é";
        for max in 0..=line.len() + 2 {
            let capped = cap_line_bytes(line, max.min(line.len()));
            assert!(line.starts_with(capped));
        }
    }
}
