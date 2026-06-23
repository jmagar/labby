//! Windows Job Object guard for the runner subprocess.
//!
//! Plays the same descendant-reaping role as the Unix process-group +
//! `killpg`: closing the job handle terminates every process assigned to the
//! job (including grandchildren) when `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is
//! set. The raw FFI is encapsulated in the sanctioned `lab-winjob` crate so
//! this crate stays unsafe-free under `unsafe_code = "forbid"`.

#![cfg(windows)]

/// RAII guard wrapping a Windows Job Object handle for a spawned runner PID.
pub(crate) struct JobObjectGuard {
    job: isize,
    pid: u32,
}

impl JobObjectGuard {
    /// Create a job for `pid` and assign the process to it. On any Win32
    /// failure `lab_winjob::create_job_for_pid` returns the `0` sentinel and the
    /// guard becomes a no-op (the `kill_on_drop` fallback still kills the lead
    /// PID).
    #[must_use]
    pub(crate) fn arm(pid: u32) -> Self {
        let job = lab_winjob::create_job_for_pid(pid);
        Self { job, pid }
    }
}

impl Drop for JobObjectGuard {
    fn drop(&mut self) {
        // `lab_winjob::close_job` is a SAFE wrapper that guards against the `0`
        // sentinel; the `CloseHandle` FFI is encapsulated in `lab-winjob`.
        lab_winjob::close_job(self.job, self.pid);
    }
}
