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
//! Non-Unix builds: this module is empty (no process-group concept on
//! Windows in the same shape).

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
            let _ = child.kill();
            let _ = child.wait();
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
                        let _ = child.kill();
                        let _ = child.wait();
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
        let _ = child.kill();
        let _ = child.wait();
    }
}
