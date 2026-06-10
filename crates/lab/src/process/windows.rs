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
//!
//! ## Spawn → assign race (accepted)
//!
//! Because we call `AssignProcessToJobObject` after the child has already been
//! spawned (not `CREATE_SUSPENDED` + resume), there is a window in which the
//! child can itself spawn grandchildren *before* the assignment completes. Any
//! grandchild born in that window will NOT be in the job and therefore will not
//! be reaped by `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
//!
//! This race is accepted for the following reasons:
//! - The window is extremely short (nanoseconds between `spawn()` returning and
//!   `OpenProcess` + `AssignProcessToJobObject` completing).
//! - Typical upstream MCP servers (`npx`, `uvx`, shell wrappers) do not spawn
//!   grandchildren synchronously in their first nanoseconds of execution.
//! - Using `CREATE_SUSPENDED` requires re-implementing the spawn path around
//!   `CreateProcess` directly, which is complex and would duplicate logic
//!   already in Tokio's process spawner.
//! - The Unix path has an analogous window between `spawn()` and the child's
//!   call to `setsid()`/`setpgid()` in `process_wrap::ProcessGroup::leader()`.
//!
//! If a future use-case requires a truly race-free assignment, the correct
//! approach is to use `JobObject` from `process-wrap` (which passes
//! `CREATE_SUSPENDED` to `CreateProcess`) with a custom Tokio child-process
//! wrapper that resumes the process after the job is assigned.

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
/// Returns the job handle as an `isize` (the raw `HANDLE` value), or `0` on
/// any Win32 failure (logged as a warning but never fatal — the caller falls
/// back to per-PID kill).
///
/// The handle is returned as `isize` rather than `HANDLE` (`*mut c_void`)
/// deliberately: in `windows-sys 0.59` `HANDLE` is a raw pointer, which is
/// `!Send + !Sync`. Storing it raw in `UpstreamRuntimeMetadata` would poison
/// `AppState`'s `Send`/`Sync` bounds and break the axum router. `isize` is
/// `Copy + Send + Sync`, so the stored/passed value crosses thread boundaries
/// cleanly; we cast back to `HANDLE` only at the `CloseHandle` boundary.
///
/// # Safety
///
/// This function calls Win32 APIs and is therefore unsafe.
#[cfg(windows)]
pub unsafe fn create_job_for_pid(pid: u32) -> isize {
    // Open the child process with the rights needed to assign it to a job and
    // to terminate it.
    let proc_handle = unsafe {
        OpenProcess(
            PROCESS_SET_QUOTA | PROCESS_TERMINATE,
            0, // bInheritHandle = FALSE
            pid,
        )
    };
    if proc_handle.is_null() || proc_handle == INVALID_HANDLE_VALUE {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "OpenProcess failed — job object not created; falling back to per-PID kill"
        );
        return 0;
    }

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        tracing::warn!(
            target: "upstream.process_guard.windows",
            pid,
            "CreateJobObjectW failed — falling back to per-PID kill"
        );
        unsafe { CloseHandle(proc_handle) };
        return 0;
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
        return 0;
    }

    tracing::debug!(
        target: "upstream.process_guard.windows",
        pid,
        "process assigned to job object with KILL_ON_JOB_CLOSE"
    );
    // Return the raw HANDLE value as isize — Send/Sync-safe to store and pass.
    job as isize
}

/// Close the job object handle, causing the OS to terminate all processes in
/// the job (including grandchildren) if `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
/// was set.
///
/// Takes the handle as `isize` (the `Send`/`Sync`-safe representation stored in
/// `UpstreamRuntimeMetadata.job_handle`); `0` is the "no job" sentinel and is a
/// no-op. The value is cast back to `HANDLE` only here, immediately before
/// `CloseHandle`.
///
/// Logs warnings on failure but never panics — safe to call from `Drop`.
///
/// # Safety
///
/// `job` must be a value obtained from [`create_job_for_pid`] (either a valid
/// job handle as `isize`, or `0`).
#[cfg(windows)]
pub unsafe fn close_job(job: isize, pid: u32) {
    if job == 0 {
        return;
    }
    let job = job as HANDLE;
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
