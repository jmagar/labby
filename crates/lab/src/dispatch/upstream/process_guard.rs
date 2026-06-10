//! RAII guard that SIGTERM+SIGKILLs a process group on Drop unless disarmed.
//!
//! Used in `connect_stdio_upstream` to ensure that if the connect future is
//! dropped between `spawn()` and the successful construction of
//! `UpstreamConnection` (discovery timeout, `list_tools` error,
//! `buffer_unordered` cancellation, etc.), the child's process group —
//! created via `process_wrap::ProcessGroup::leader()` — is reaped.
//! Without the guard, only the lead PID dies; grandchildren (`npx` → node,
//! `sh -c` → python) orphan and accumulate. See `process_spawn_culprit`
//! memory and bead `lab-4z8sx.1`.
//!
//! On the happy path the guard is `.disarm()`'d and the pgid is transferred
//! to `UpstreamConnection`, whose own `Drop` impl takes over the role.
//!
//! On Windows, `JobObjectGuard` plays the same role using a Windows Job Object
//! handle instead of a process group id. Closing the handle causes the OS to
//! terminate all processes assigned to the job (including grandchildren) when
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is set. The guard is armed immediately
//! after spawn, disarmed on successful `UpstreamConnection` construction (the
//! raw handle is stored in `UpstreamRuntimeMetadata.job_handle`), and the
//! stored handle is closed in `UpstreamConnection::Drop` / `shutdown()`.

/// RAII guard wrapping a process group id. On `Drop` the guard sends
/// `SIGTERM` then `SIGKILL` to the group — synchronously, with no wait
/// between the signals (Drop is not allowed to block). The graceful
/// `SIGTERM` → 150 ms wait → conditional `SIGKILL` sequence belongs in the
/// async `UpstreamConnection::shutdown()` path.
///
/// `.disarm()` consumes the guard and returns the pgid for transfer to
/// `UpstreamConnection::runtime.pgid`.
#[cfg(unix)]
pub struct ProcessGroupGuard {
    pgid: Option<u32>,
}

#[cfg(unix)]
impl ProcessGroupGuard {
    /// Arm the guard with a pgid. With `process_wrap::ProcessGroup::leader()`
    /// the child is its own group leader, so the spawned PID *is* the pgid.
    #[must_use]
    pub fn arm(pgid: u32) -> Self {
        Self { pgid: Some(pgid) }
    }

    /// Disarm the guard, returning the pgid. After disarm `Drop` does nothing
    /// and ownership of the pgid passes to the caller — typically
    /// `UpstreamConnection::runtime.pgid`.
    pub fn disarm(mut self) -> Option<u32> {
        self.pgid.take()
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        let Some(pgid) = self.pgid.take() else {
            return;
        };
        // Sync syscalls (nix::sys::signal::killpg). Safe in Drop —
        // no allocation, no async, no panicking.
        if let Err(error) = crate::process::unix::terminate_process_group_sigterm(pgid) {
            tracing::warn!(
                target: "upstream.process_guard",
                pgid,
                ?error,
                "process group SIGTERM failed (still attempting SIGKILL)"
            );
        }
        if let Err(error) = crate::process::unix::terminate_process_group_sigkill(pgid) {
            tracing::warn!(
                target: "upstream.process_guard",
                pgid,
                ?error,
                "process group SIGKILL failed (pgid may still be alive)"
            );
        } else {
            tracing::debug!(
                target: "upstream.process_guard",
                pgid,
                "process group reaped on guard drop"
            );
        }
    }
}

/// Windows Job Object RAII guard.
///
/// Arms immediately after `spawn()` by calling [`JobObjectGuard::arm`] with
/// the child PID. On drop, the job handle is closed; because
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` was set, the OS terminates every
/// process in the job (direct child + all descendants).
///
/// On the happy path, `.disarm()` returns the handle (as `isize`) for storage
/// in `UpstreamRuntimeMetadata.job_handle`. `UpstreamConnection::Drop` and
/// `shutdown()` close that handle, mirroring the Unix `killpg` path.
///
/// A failed `create_job_for_pid` (Win32 API error) returns `0`; the guard still
/// arms with that sentinel value and `Drop` / `disarm` treat it as a no-op, so
/// the connect path is never fatal due to a job-creation failure.
///
/// The handle is held as `isize` rather than `HANDLE` because `windows-sys
/// 0.59`'s `HANDLE` (`*mut c_void`) is `!Send + !Sync`; `isize` keeps the guard
/// (and any struct that stores its disarmed value) thread-safe with no unsafe
/// trait impls. The cast back to `HANDLE` happens only inside `close_job`.
#[cfg(windows)]
pub struct JobObjectGuard {
    /// Raw job handle value as `isize`. `0` means creation failed; treated as no-op.
    job: isize,
    /// PID of the process assigned to the job; used only for log messages.
    pid: u32,
}

#[cfg(windows)]
impl JobObjectGuard {
    /// Arm the guard: create a Job Object, assign `pid` to it, and set
    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
    #[must_use]
    pub fn arm(pid: u32) -> Self {
        // SAFETY: create_job_for_pid only calls Win32 APIs with well-formed
        // arguments. Any failure is logged and returns the `0` sentinel.
        let job = unsafe { crate::process::windows::create_job_for_pid(pid) };
        Self { job, pid }
    }

    /// Disarm the guard, returning the job handle as `isize`.
    ///
    /// After disarm the guard's `Drop` does nothing; the caller takes ownership
    /// of the handle and is responsible for closing it (typically via
    /// `UpstreamRuntimeMetadata.job_handle`).
    pub fn disarm(mut self) -> isize {
        let handle = self.job;
        // Zero out so Drop no-ops.
        self.job = 0;
        handle
    }
}

#[cfg(windows)]
impl Drop for JobObjectGuard {
    fn drop(&mut self) {
        // SAFETY: close_job guards against the `0` sentinel and only calls
        // CloseHandle on a handle value we created.
        unsafe { crate::process::windows::close_job(self.job, self.pid) };
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Command as StdCommand, Stdio};
    use std::time::Duration;

    /// Helper: spawn a long-lived child in its own session/process group via
    /// `setsid -w sleep 30`. Returns `None` if `setsid` is unavailable
    /// (minimal container images) — tests that depend on it bail out.
    fn spawn_setsid_sleep() -> Option<std::process::Child> {
        StdCommand::new("setsid")
            .args(["-w", "sleep", "30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }

    /// Spawn a real long-lived child in its own process group; drop an armed
    /// guard; assert the child exits (becomes reapable) within 1 s.
    ///
    /// We poll `try_wait()` rather than `pid_is_alive(pid)` because a process
    /// killed-but-not-reaped is a zombie, and `kill -0` against a zombie
    /// returns success on Linux — so a strict pid_is_alive check is racy.
    #[test]
    fn drop_kills_unarmed_process_group() {
        let Some(mut child) = spawn_setsid_sleep() else {
            return;
        };
        let pid = child.id();
        // Verify the child landed in its own process group with pgid==pid.
        // If not, setsid didn't behave as expected on this platform — skip.
        let observed_pgid = crate::process::unix::process_group_id(pid);
        if observed_pgid != Some(pid) {
            eprintln!("process_guard test skip: pid {pid} pgid {observed_pgid:?} (expected {pid})");
            drop(child.kill());
            drop(child.wait());
            return;
        }
        let guard = ProcessGroupGuard::arm(pid);
        drop(guard);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => return, // reaped — pass
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let still_alive = crate::process::unix::pid_is_alive(pid);
                        drop(child.kill());
                        drop(child.wait());
                        panic!(
                            "guard drop did not exit pgid {pid} within 2s (pid_is_alive={still_alive})"
                        );
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => panic!("try_wait failed: {e}"),
            }
        }
    }

    /// Disarm the guard, drop, then verify the child is still alive.
    /// Caller is responsible for cleanup.
    #[test]
    fn disarm_prevents_kill() {
        let Some(mut child) = spawn_setsid_sleep() else {
            return;
        };
        let pid = child.id();
        let guard = ProcessGroupGuard::arm(pid);
        let disarmed = guard.disarm();
        assert_eq!(disarmed, Some(pid), "disarm returns the armed pgid");

        // The child should NOT have exited yet — give the scheduler 50 ms
        // and confirm try_wait returns None.
        std::thread::sleep(Duration::from_millis(50));
        match child.try_wait() {
            Ok(None) => (), // still running, expected
            Ok(Some(s)) => panic!("disarmed guard let child exit: {s:?}"),
            Err(e) => panic!("try_wait failed: {e}"),
        }

        // Cleanup.
        let _ = crate::process::unix::terminate_process_group_sigkill(pid);
        drop(child.kill());
        drop(child.wait());
    }
}
