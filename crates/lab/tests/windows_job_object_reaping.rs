//! Windows Job Object process-tree reaping integration test.
//!
//! This test is `#[ignore]` because it requires a real Windows host to
//! exercise the Win32 Job Object API. It is intended to be run on the
//! `windows-lab` self-hosted CI runner (the "Test (windows self-hosted)"
//! job in `.github/workflows/test-windows.yml`), which compiles and runs
//! the full test suite on a genuine Windows environment.
//!
//! On Linux/macOS the test cannot compile the `#[cfg(windows)]` code, so
//! the entire module is cfg-gated. The `#[ignore]` attribute additionally
//! prevents accidental execution via `cargo nextest run` without the
//! `--include-ignored` flag.
//!
//! ## What is verified
//!
//! A two-level child tree is spawned:
//!
//! ```text
//! labby test process
//!   └─ cmd /c  (direct child)
//!        └─ ping -n 30 127.0.0.1  (grandchild — long-lived "sleep" equivalent)
//! ```
//!
//! The grandchild PID is captured before the Job Object handle is closed.
//! After closing the handle, the test polls `OpenProcess` to confirm the
//! grandchild has been terminated by the OS (not just the direct child).
//!
//! This mirrors the production scenario where an upstream MCP server like
//! `npx → node` or `cmd → python` would orphan without Job Object reaping.

#[cfg(windows)]
mod windows_job_reaping {
    use std::time::Duration;

    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WAIT_OBJECT_0,
            WaitForSingleObject,
        },
    };

    /// Returns `true` if the given PID is still running (process handle is
    /// valid and `WaitForSingleObject` with zero timeout does NOT signal).
    unsafe fn pid_is_alive(pid: u32) -> bool {
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
        // A zero timeout: if the process has already exited the handle is
        // signalled immediately (WAIT_OBJECT_0).
        let result = unsafe { WaitForSingleObject(handle, 0) };
        unsafe { CloseHandle(handle) };
        result != WAIT_OBJECT_0
    }

    /// Spawn `cmd /c "start /B ping -n 60 127.0.0.1"` as a grandchild tree.
    ///
    /// We use `ping -n 60 127.0.0.1` (60 one-second pings) as a portable
    /// Windows "sleep 60" substitute that does not require any extra tools.
    /// `cmd /c` is the direct child; `ping` is the grandchild.
    ///
    /// Returns `(direct_child_process::Child, grandchild_pid)`.
    fn spawn_two_level_tree() -> Option<(std::process::Child, u32)> {
        use std::process::{Command, Stdio};

        // Spawn the direct child (cmd).
        let child = Command::new("cmd")
            .args(["/c", "ping -n 60 127.0.0.1 > nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let parent_pid = child.id();

        // Give cmd time to spawn ping.
        std::thread::sleep(Duration::from_millis(800));

        // Walk the process list to find the `ping.exe` child of `parent_pid`.
        // We use the Windows `CreateToolhelp32Snapshot` API for this.
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };

        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).ok()?;

        let mut grandchild_pid: Option<u32> = None;
        let mut more = unsafe { Process32FirstW(snap, &mut entry) } != 0;
        while more {
            if entry.th32ParentProcessID == parent_pid {
                // Any child of cmd is our grandchild candidate (ping.exe).
                grandchild_pid = Some(entry.th32ProcessID);
                break;
            }
            more = unsafe { Process32NextW(snap, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snap) };

        let grandchild_pid = grandchild_pid?;
        Some((child, grandchild_pid))
    }

    /// Verify that closing a Job Object handle with `KILL_ON_JOB_CLOSE` terminates
    /// the entire process tree (direct child + grandchild), not just the direct child.
    ///
    /// This test is `#[ignore]` — run it explicitly on the `windows-lab` CI runner:
    ///
    /// ```sh
    /// cargo nextest run --test windows_job_object_reaping --include-ignored
    /// ```
    #[test]
    #[ignore = "requires real Windows host; run on windows-lab CI runner"]
    fn job_object_kills_grandchild_on_close() {
        let Some((mut direct_child, grandchild_pid)) = spawn_two_level_tree() else {
            eprintln!("SKIP: could not spawn two-level process tree (ping not available?)");
            return;
        };

        let parent_pid = direct_child.id();

        // Confirm grandchild is alive before we do anything.
        assert!(
            unsafe { pid_is_alive(grandchild_pid) },
            "grandchild (pid {grandchild_pid}) should be alive before job-object close"
        );

        // Create a Job Object for the direct child (same API as production code).
        // `create_job_for_pid` returns the handle as `isize`; `0` is the failure
        // sentinel.
        let job: isize = unsafe { labby::process::windows::create_job_for_pid(parent_pid) };
        assert!(
            job != 0,
            "create_job_for_pid should succeed; got sentinel handle {job}"
        );

        // Close the job handle → OS terminates the whole tree.
        unsafe { labby::process::windows::close_job(job, parent_pid) };

        // Give the OS a moment to reap the tree.
        std::thread::sleep(Duration::from_millis(500));

        // Poll grandchild: it should be gone.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if !unsafe { pid_is_alive(grandchild_pid) } {
                break; // Pass — grandchild was reaped.
            }
            if std::time::Instant::now() >= deadline {
                // Force-clean the tree so subsequent test runs start fresh.
                let _ = direct_child.kill();
                let _ = direct_child.wait();
                panic!(
                    "grandchild pid {grandchild_pid} still alive 5 s after job-object close; \
                     Windows Job Object reaping did not work"
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // Also ensure the direct child is gone (belt-and-suspenders).
        assert!(
            !unsafe { pid_is_alive(parent_pid) },
            "direct child (pid {parent_pid}) should also be dead after job-object close"
        );
        // Reap to avoid zombie handle.
        let _ = direct_child.wait();
    }
}
