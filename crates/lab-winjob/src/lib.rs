//! Windows Job Object helpers for process-tree reaping.
//!
//! This crate is the **sanctioned unsafe boundary** for `lab`'s Windows Job
//! Object FFI, mirroring how the Unix path routes its unsafe through the
//! external `nix` crate. The workspace sets `unsafe_code = "forbid"` (which a
//! `#[allow]` cannot escape), so `lab` and `lab-apis` stay unsafe-free. The raw
//! `windows-sys` calls are encapsulated here behind a **safe** public API:
//! callers never write `unsafe`.
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
//!
//! ## Handle representation
//!
//! The job handle is stored and passed as `isize` (the raw `HANDLE` value), not
//! `HANDLE` itself. In `windows-sys 0.59` `HANDLE` is `*mut c_void`
//! (`!Send + !Sync`), which would poison the `Send`/`Sync` bounds of any struct
//! that stores it (and break `lab`'s axum router). `isize` is
//! `Copy + Send + Sync`; we cast back to `HANDLE` only at the `CloseHandle`
//! boundary inside [`close_job`].
//!
//! On non-Windows targets this crate compiles to an empty library (`lab` only
//! depends on it under `cfg(windows)`).

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            QueryInformationJobObject, SetInformationJobObject,
        },
        Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE},
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
/// `!Send + !Sync`. Storing it raw in a struct would poison that struct's
/// `Send`/`Sync` bounds. `isize` is `Copy + Send + Sync`, so the stored/passed
/// value crosses thread boundaries cleanly; we cast back to `HANDLE` only at
/// the `CloseHandle` boundary.
///
/// This is a **safe** function: the `unsafe` FFI is fully encapsulated here, so
/// callers (in `lab`, which forbids unsafe) never write `unsafe`.
#[cfg(windows)]
#[must_use]
pub fn create_job_for_pid(pid: u32) -> isize {
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
            target: "lab_winjob",
            pid,
            "OpenProcess failed — job object not created; falling back to per-PID kill"
        );
        return 0;
    }

    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        tracing::warn!(
            target: "lab_winjob",
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
            u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(u32::MAX),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        tracing::warn!(
            target: "lab_winjob",
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
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(u32::MAX),
            )
        };
        if set_ok == 0 {
            tracing::warn!(
                target: "lab_winjob",
                pid,
                "SetInformationJobObject failed — KILL_ON_JOB_CLOSE not set"
            );
        }
    }

    let assigned = unsafe { AssignProcessToJobObject(job, proc_handle) };
    unsafe { CloseHandle(proc_handle) };
    if assigned == 0 {
        tracing::warn!(
            target: "lab_winjob",
            pid,
            "AssignProcessToJobObject failed — job created but child not assigned; closing job"
        );
        unsafe { CloseHandle(job) };
        return 0;
    }

    tracing::debug!(
        target: "lab_winjob",
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
/// Takes the handle as `isize` (the `Send`/`Sync`-safe representation stored by
/// the caller); `0` is the "no job" sentinel and is a no-op. The value is cast
/// back to `HANDLE` only here, immediately before `CloseHandle`.
///
/// Logs warnings on failure but never panics — safe to call from `Drop`.
///
/// This is a **safe** function: the `unsafe` FFI is fully encapsulated here.
#[cfg(windows)]
pub fn close_job(job: isize, pid: u32) {
    if job == 0 {
        return;
    }
    let job = job as HANDLE;
    let ok = unsafe { CloseHandle(job) };
    if ok == 0 {
        tracing::warn!(
            target: "lab_winjob",
            pid,
            "CloseHandle(job) failed — descendant processes may have orphaned"
        );
    } else {
        tracing::debug!(
            target: "lab_winjob",
            pid,
            "job object handle closed — OS reaping descendant tree"
        );
    }
}

/// Return `true` if `pid` refers to a process that is still running.
///
/// Used by the Job Object reaping integration test (which lives in `lab`, where
/// unsafe is forbidden) to assert that a grandchild was terminated. The unsafe
/// FFI is encapsulated here so the test stays unsafe-free.
///
/// Returns `false` if the process has exited, was never present, or cannot be
/// opened (e.g. insufficient permission) — for the test's purposes "not
/// observably alive" is the signal it needs.
#[cfg(windows)]
#[must_use]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WAIT_OBJECT_0, WaitForSingleObject,
    };

    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return false; // Process already gone or no permission.
    }
    // Zero timeout: an already-exited process signals immediately (WAIT_OBJECT_0).
    let result = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    result != WAIT_OBJECT_0
}

/// Find the PID of the first child process whose parent is `parent_pid`.
///
/// Walks the system process snapshot via the ToolHelp API. Used by the Job
/// Object reaping integration test to locate the grandchild (`ping.exe` under
/// `cmd`). Returns `None` if the snapshot cannot be taken or no child is found.
///
/// The unsafe FFI is encapsulated here so the test (in `lab`) stays unsafe-free.
#[cfg(windows)]
#[must_use]
pub fn find_first_child_pid(parent_pid: u32) -> Option<u32> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    let Ok(size) = u32::try_from(size_of::<PROCESSENTRY32W>()) else {
        unsafe { CloseHandle(snap) };
        return None;
    };
    entry.dwSize = size;

    let mut found: Option<u32> = None;
    let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
    while more {
        if entry.th32ParentProcessID == parent_pid {
            found = Some(entry.th32ProcessID);
            break;
        }
        more = unsafe { Process32NextW(snap, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snap) };
    found
}
