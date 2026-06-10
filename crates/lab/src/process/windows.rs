//! Windows Job Object helpers for process-tree reaping.
//!
//! On Windows there is no concept of a process group. A Job Object is the
//! nearest OS equivalent: the kernel associates a child process (and, when
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is set, its entire descendant tree)
//! with the job. Closing the last handle to the job with that flag set causes
//! the OS to terminate every process in the job — including grandchildren such
//! as `cmd → node → python` that would otherwise orphan when only the direct
//! child is killed.
//!
//! ## Design choice: raw `windows-sys` + `AssignProcessToJobObject`
//!
//! We use `windows-sys` plus `AssignProcessToJobObject` after spawn (mirroring
//! how the Unix path uses raw `pgid` + `killpg`) rather than `process_wrap`'s
//! `JobObject` wrapper. Reason: `process_wrap`'s `JobObject` integrates with
//! its own `TokioChildProcess` drop semantics and sets `CREATE_SUSPENDED` so
//! the child does not race ahead before the job assignment. Once `rmcp`'s
//! `TokioChildProcess::builder(...).spawn()` takes ownership of the spawned
//! child process, the `process_wrap` drop-based cleanup no longer fires — that
//! is exactly why the Unix path uses a separate raw-`pgid` `ProcessGroupGuard`.
//! The Windows guard mirrors that shape: we obtain the child's HANDLE via
//! `OpenProcess`, assign it to a fresh job, set `KILL_ON_JOB_CLOSE`, then own
//! just the job handle in the guard. Drop closes the handle and the OS reaps
//! the whole tree.

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject,
        },
        Threading::OpenProcess,
        Threading::PROCESS_SET_QUOTA,
        Threading::PROCESS_TERMINATE,
    },
};

/// Create a new Windows Job Object, assign the given PID to it, and set
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so every descendant is terminated
/// when the last handle to the job is closed.
///
/// Returns the raw job `HANDLE`, or `INVALID_HANDLE_VALUE` on any Win32
/// failure (logged as a warning but never fatal — the caller falls back to
/// per-PID kill).
///
/// # Safety
///
/// This function calls Win32 APIs and is therefore unsafe.
#[cfg(windows)]
pub unsafe fn create_job_for_pid(pid: u32) -> HANDLE {
    // Open the child process with the rights needed to assign it to a job and
    // to terminate it.
    let proc_handle = unsafe {
        OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE,
            0, // bInheritHandle = FALSE
            pid,
        )
    };
    if proc_handle == 0 || proc_handle == INVALID_HANDLE_VALUE {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "OpenProcess failed — job object not created; falling back to per-PID kill"
        );
        return INVALID_HANDLE_VALUE;
    }

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job == 0 || job == INVALID_HANDLE_VALUE {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "CreateJobObjectW failed — falling back to per-PID kill"
        );
        unsafe { CloseHandle(proc_handle) };
        return INVALID_HANDLE_VALUE;
    }

    // Read current extended limit info, flip on KILL_ON_JOB_CLOSE, write back.
    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of_mut!(info).cast(),
            u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .unwrap_or(u32::MAX),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "QueryInformationJobObject failed — killing job without KILL_ON_JOB_CLOSE"
        );
    } else {
        info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set_ok = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(u32::MAX),
            )
        };
        if set_ok == 0 {
            tracing::warn!(
                target: "upstream.process_guard.windows",
                pid,
                "SetInformationJobObject failed — KILL_ON_JOB_CLOSE not set"
            );
        }
    }

    let assigned = unsafe { AssignProcessToJobObject(job, proc_handle) };
    unsafe { CloseHandle(proc_handle) };
    if assigned == 0 {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "AssignProcessToJobObject failed — job created but child not assigned; closing job"
        );
        unsafe { CloseHandle(job) };
        return INVALID_HANDLE_VALUE;
    }

    tracing::debug!(
        target: "upstream.process_guard.windows",
        pid,
        "process assigned to job object with KILL_ON_JOB_CLOSE"
    );
    job
}

/// Close the job object handle, causing the OS to terminate all processes in
/// the job (including grandchildren) if `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
/// was set.
///
/// Logs warnings on failure but never panics — safe to call from `Drop`.
///
/// # Safety
///
/// `job` must be a valid Job Object `HANDLE` obtained from [`create_job_for_pid`].
#[cfg(windows)]
pub unsafe fn close_job(job: HANDLE, pid: u32) {
    if job == INVALID_HANDLE_VALUE || job == 0 {
        return;
    }
    let ok = unsafe { CloseHandle(job) };
    if ok == 0 {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "CloseHandle(job) failed — descendant processes may have orphaned"
        );
    } else {
        tracing::debug!(
            target: "upstream.process_guard.windows",
            pid,
            "job object handle closed — OS reaping descendant tree"
        );
    }
}
