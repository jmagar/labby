//! A single long-lived Code Mode runner process and its parent-side I/O.
//!
//! A `PooledRunner` owns one `labby internal code-mode-runner` subprocess that
//! stays alive across executions. The expensive `fork()` + process startup is
//! paid once at spawn; each execution builds a FRESH `javy::Runtime` inside the
//! process (runner-side contract), so no JS state leaks between callers.
//!
//! Security invariants preserved at spawn (set once, persist for the process):
//! - `env_clear()` — the child inherits no `LAB_*`/ambient env.
//! - `process_group(0)` (Unix) / Job Object (Windows) — `killpg`/job close
//!   reaps grandchildren on shutdown/eviction/drop.
//! - `kill_on_drop(true)` — dropping the handle kills the process.
//!
//! Per-execution invariants (heap/timeout/stack, fresh jail) are enforced
//! runner-side per `Start`.

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader as TokioBufReader;
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Mutex, Notify};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::dispatch::error::ToolError;

/// Per-line safety cap mirrored from the original driver: 64 MiB heap + framing
/// headroom. A longer line is a protocol violation.
///
/// Note this is a per-runner transient ceiling: the parent may buffer up to this
/// much for a single oversized stdout line, multiplied by the number of live
/// runners (`LAB_CODE_MODE_POOL_SIZE` + `LAB_CODE_MODE_POOL_MAX_OVERFLOW`, ~24 at
/// defaults). It is a hard bound that errors rather than growing unbounded, not a
/// steady-state allocation; raising the pool/overflow knobs raises this worst
/// case proportionally.
pub(in crate::dispatch::gateway::code_mode) const MAX_LINE_BYTES: usize =
    64 * 1024 * 1024 + 4 * 1024;

/// Stdout line stream type for a pooled runner.
pub(in crate::dispatch::gateway::code_mode) type RunnerLines =
    FramedRead<tokio::process::ChildStdout, LinesCodec>;

/// Shared, continuously-drained stderr buffer for one runner.
///
/// The runner redirects `console.*` to stderr; a background task drains it to
/// EOF so a >64 KiB burst can never block the child on a full pipe. Per-execution
/// log capture slices `[start_index..]` of this buffer.
#[derive(Clone)]
pub(in crate::dispatch::gateway::code_mode) struct StderrBuffer {
    lines: Arc<Mutex<Vec<String>>>,
    /// Signalled on every push so a waiter can poll for post-`Done` flush.
    notify: Arc<Notify>,
}

impl StderrBuffer {
    fn new() -> Self {
        Self {
            lines: Arc::new(Mutex::new(Vec::new())),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Current line count — the start index for the next execution's capture.
    pub(in crate::dispatch::gateway::code_mode) async fn mark(&self) -> usize {
        self.lines.lock().await.len()
    }

    /// Return lines appended since `start_index`, then release all retained
    /// stderr lines for this runner. A runner executes one request at a time, so
    /// completed executions do not need historical stderr retained after the
    /// response has been materialized.
    pub(in crate::dispatch::gateway::code_mode) async fn take_since_and_clear(
        &self,
        start_index: usize,
    ) -> Vec<String> {
        let mut guard = self.lines.lock().await;
        let captured = guard
            .get(start_index..)
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        guard.clear();
        captured
    }

    /// Release retained stderr without returning it, used when the runner
    /// reports a reusable per-execution error.
    pub(in crate::dispatch::gateway::code_mode) async fn clear(&self) {
        self.lines.lock().await.clear();
    }

    /// Wait (bounded) for the stderr drain to flush lines emitted before `Done`.
    ///
    /// `Done` arrives on stdout once the JS settles; the child has already
    /// *written* its console output to the stderr pipe by then, but the parent's
    /// async drain may not have read it yet. Poll for the buffer to stop growing,
    /// bounded by a short deadline — logs are best-effort, never a correctness
    /// boundary.
    pub(in crate::dispatch::gateway::code_mode) async fn flush_settle(&self) {
        const SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_millis(50);
        let deadline = tokio::time::Instant::now() + SETTLE_BUDGET;
        let mut last_len = self.lines.lock().await.len();
        loop {
            let notified = self.notify.notified();
            match tokio::time::timeout_at(deadline, notified).await {
                Ok(()) => {
                    let len = self.lines.lock().await.len();
                    if len == last_len {
                        // Spurious wake without growth; stop polling.
                        break;
                    }
                    last_len = len;
                }
                Err(_) => break, // settle budget elapsed
            }
        }
    }
}

/// A single long-lived runner process plus its parent-side I/O channels.
pub(in crate::dispatch::gateway::code_mode) struct PooledRunner {
    pub(in crate::dispatch::gateway::code_mode) child: tokio::process::Child,
    pub(in crate::dispatch::gateway::code_mode) child_pid: Option<u32>,
    pub(in crate::dispatch::gateway::code_mode) stdin: ChildStdin,
    pub(in crate::dispatch::gateway::code_mode) lines: RunnerLines,
    pub(in crate::dispatch::gateway::code_mode) stderr: StderrBuffer,
    /// Number of executions this runner has served (for recycle-after-K).
    pub(in crate::dispatch::gateway::code_mode) executions: u64,
    /// Windows Job Object guard; reaps the descendant tree when dropped. On Unix
    /// the process-group + `killpg` covers the same role.
    #[cfg(windows)]
    _job_guard: Option<crate::dispatch::upstream::process_guard::JobObjectGuard>,
    /// Background stderr drain task; aborted on drop.
    drain_task: tokio::task::JoinHandle<()>,
    /// The runner's spawn cwd. Held for the runner's whole life so the
    /// per-execution jail subdirs the runner creates always have a stable base;
    /// its `Drop` removes the tree when the runner handle is dropped.
    _temp_dir: tempfile::TempDir,
}

impl PooledRunner {
    /// Spawn a fresh long-lived runner process. The `current_exe` is resolved by
    /// the caller and passed in so spawn failures surface as a clean error.
    pub(in crate::dispatch::gateway::code_mode) fn spawn(
        exe: &std::path::Path,
    ) -> Result<Self, ToolError> {
        // Each runner gets its own isolated cwd. It is a long-lived TempDir; the
        // runner creates a fresh per-execution subdir under it on every `Start`.
        let temp_dir = tempfile::TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create Code Mode sandbox directory: {err}"),
        })?;

        let mut cmd = Command::new(exe);
        cmd.args(["internal", "code-mode-runner"])
            .current_dir(temp_dir.path())
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to spawn Code Mode runner: {err}"),
        })?;
        let child_pid = child.id();

        #[cfg(windows)]
        let job_guard =
            child_pid.map(crate::dispatch::upstream::process_guard::JobObjectGuard::arm);

        let stdin = child.stdin.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdin was not available".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdout was not available".to_string(),
        })?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stderr was not available".to_string(),
        })?;

        let stderr = StderrBuffer::new();
        let drain_task = spawn_stderr_drain(stderr_pipe, stderr.clone());

        let lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_LINE_BYTES));

        Ok(Self {
            child,
            child_pid,
            stdin,
            lines,
            stderr,
            executions: 0,
            #[cfg(windows)]
            _job_guard: job_guard,
            drain_task,
            _temp_dir: temp_dir,
        })
    }

    /// Test-only: spawn a long-lived stand-in process that parks reading stdin
    /// (like a parked runner) without speaking the protocol. Used to unit-test
    /// the pool's lease / free-list / recycle / eviction bookkeeping and PID
    /// reuse without needing the real labby binary (`current_exe()` in a lib
    /// unit test is the test harness, not the runner).
    ///
    /// The stand-in program must exist on the test host AND resolve under the
    /// `env_clear()` in `spawn_stub_command` (no inherited `PATH`). On Unix `cat`
    /// satisfies both; on Windows `cat`/`sleep` don't exist, so we use System32
    /// built-ins, which `CreateProcess` finds via its default search order even
    /// with an empty environment. `findstr` reads stdin and parks until EOF,
    /// mirroring `cat`.
    #[cfg(test)]
    pub(in crate::dispatch::gateway::code_mode) fn spawn_stub() -> Result<Self, ToolError> {
        #[cfg(not(windows))]
        {
            Self::spawn_stub_command("cat", &[])
        }
        #[cfg(windows)]
        {
            // `findstr ^` matches every line and reads stdin until EOF, so it
            // parks on an open-but-idle stdin pipe just like `cat`.
            Self::spawn_stub_command(r"C:\Windows\System32\findstr.exe", &["^"])
        }
    }

    /// Test-only: a stub that consumes nothing on stdout and stays alive for a
    /// long time, modelling a runner that never replies. Used to exercise the
    /// parent-side wall-clock timeout path in `drive_runner`.
    #[cfg(test)]
    pub(in crate::dispatch::gateway::code_mode) fn spawn_stub_silent() -> Result<Self, ToolError> {
        // The program ignores stdin and emits nothing on stdout, so the drive
        // loop's `lines.next()` pends until the wall-clock deadline fires.
        #[cfg(not(windows))]
        {
            Self::spawn_stub_command("sleep", &["3600"])
        }
        #[cfg(windows)]
        {
            // `timeout` refuses redirected stdin, and PowerShell startup can be
            // noisy/host-dependent on self-hosted CI. `cmd /C ping ... >NUL`
            // is quiet, long-lived, and uses absolute System32 paths because
            // this stub runs under `env_clear()`.
            Self::spawn_stub_command(
                r"C:\Windows\System32\cmd.exe",
                &[
                    "/D",
                    "/Q",
                    "/C",
                    r"C:\Windows\System32\ping.exe -n 3600 127.0.0.1 >NUL",
                ],
            )
        }
    }

    #[cfg(test)]
    fn spawn_stub_command(program: &str, args: &[&str]) -> Result<Self, ToolError> {
        let temp_dir = tempfile::TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create stub sandbox directory: {err}"),
        })?;
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.current_dir(temp_dir.path())
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to spawn stub runner: {err}"),
        })?;
        let child_pid = child.id();
        #[cfg(windows)]
        let job_guard =
            child_pid.map(crate::dispatch::upstream::process_guard::JobObjectGuard::arm);
        let stdin = child.stdin.take().expect("stub stdin");
        let stdout = child.stdout.take().expect("stub stdout");
        let stderr_pipe = child.stderr.take().expect("stub stderr");
        let stderr = StderrBuffer::new();
        let drain_task = spawn_stderr_drain(stderr_pipe, stderr.clone());
        let lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_LINE_BYTES));
        Ok(Self {
            child,
            child_pid,
            stdin,
            lines,
            stderr,
            executions: 0,
            #[cfg(windows)]
            _job_guard: job_guard,
            drain_task,
            _temp_dir: temp_dir,
        })
    }
}

impl Drop for PooledRunner {
    fn drop(&mut self) {
        // Stop draining stderr.
        self.drain_task.abort();
        // Reap the process group (Unix) so grandchildren are not orphaned. The
        // child itself is killed by `kill_on_drop(true)`; on Windows the Job
        // Object guard's Drop terminates the descendant tree.
        #[cfg(unix)]
        if let Some(pid) = self.child_pid {
            use nix::sys::signal::Signal;
            use nix::unistd::Pid;
            let _ = nix::sys::signal::killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
    }
}

/// Background task: drain a runner's stderr pipe to EOF, appending lines to the
/// shared buffer (with the same hard caps the original single-run drain used).
fn spawn_stderr_drain(
    stderr: tokio::process::ChildStderr,
    buffer: StderrBuffer,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Caps mirror the original per-run drain so a single runaway execution
        // cannot grow the buffer without bound before the per-execution log caps
        // are applied downstream.
        const CAP_ENTRIES: usize = 100_000;
        const CAP_BYTES: usize = 8 * 1024 * 1024;
        let mut lines = TokioBufReader::new(stderr).lines();
        let mut total_bytes = 0usize;
        let mut capped = false;
        while let Ok(Some(line)) = lines.next_line().await {
            if capped {
                continue;
            }
            total_bytes += line.len() + 1;
            {
                let mut buf = buffer.lines.lock().await;
                if buf.len() >= CAP_ENTRIES || total_bytes > CAP_BYTES {
                    capped = true;
                } else {
                    buf.push(line);
                }
            }
            buffer.notify.notify_waiters();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::StderrBuffer;

    #[tokio::test]
    async fn stderr_buffer_take_since_and_clear_releases_retained_lines() {
        let buffer = StderrBuffer::new();
        buffer.lines.lock().await.push("before".to_string());
        let mark = buffer.mark().await;
        {
            let mut lines = buffer.lines.lock().await;
            lines.push("during-one".to_string());
            lines.push("during-two".to_string());
        }

        let captured = buffer.take_since_and_clear(mark).await;

        assert_eq!(captured, ["during-one", "during-two"]);
        assert!(buffer.lines.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stderr_buffer_clear_discards_retained_lines() {
        let buffer = StderrBuffer::new();
        buffer.lines.lock().await.push("discard me".to_string());

        buffer.clear().await;

        assert!(buffer.lines.lock().await.is_empty());
    }
}
