#![allow(
    clippy::bool_assert_comparison,
    clippy::err_expect,
    clippy::field_reassign_with_default,
    clippy::float_cmp,
    clippy::len_zero,
    clippy::manual_string_new,
    clippy::needless_raw_string_hashes,
    clippy::panic,
    clippy::single_char_pattern,
    clippy::unnested_or_patterns
)]
#![cfg(all(windows, feature = "gateway"))]
//! Windows Job Object process-tree reaping integration test.
//!
//! This test is `#[ignore]` because it requires a real Windows host to
//! exercise the Win32 Job Object API. It is intended to be run on the
//! `windows-lab` self-hosted CI runner (the "Test (windows self-hosted)"
//! job in `.github/workflows/ci.yml`), which compiles and runs
//! the full test suite on a genuine Windows environment.
//!
//! On Linux/macOS the test cannot run the Windows-only code, so the entire
//! module is `#[cfg(windows)]`-gated. The `#[ignore]` attribute additionally
//! prevents accidental execution via `cargo nextest run` without the
//! `--include-ignored` flag.
//!
//! All Win32 FFI used here is routed through the SAFE API exposed by the
//! `labby-winjob` crate and through the production
//! `labby_gateway::upstream::process_guard::JobObjectGuard`. `lab` (and
//! therefore this test target) forbids `unsafe`, so the test contains zero
//! `unsafe` blocks — the unsafe lives inside `labby-winjob`, the sanctioned FFI
//! boundary.
//!
//! ## What is verified
//!
//! A two-level child tree is spawned:
//!
//! ```text
//! labby test process
//!   └─ cmd /c  (direct child)
//!        └─ ping -n 60 127.0.0.1  (grandchild — long-lived "sleep" equivalent)
//! ```
//!
//! The direct child is assigned to a Job Object before it starts the long-lived
//! grandchild. The grandchild PID is then captured before the Job Object handle
//! is closed. After closing the handle, the test polls process liveness to
//! confirm the grandchild has been terminated by the OS (not just the direct
//! child).
//!
//! This mirrors the production scenario where an upstream MCP server like
//! `npx → node` or `cmd → python` would orphan without Job Object reaping.

#[cfg(windows)]
mod windows_job_reaping {
    use labby_gateway::upstream::process_guard::JobObjectGuard;
    use labby_winjob::ProcessLiveness;
    use std::process::Child;
    use std::time::Duration;

    /// Spawn a `cmd /c ...` process that delays briefly before starting a
    /// long-lived `ping` grandchild.
    ///
    /// The delay gives the test time to assign the direct child to the Job
    /// Object before the long-lived grandchild is born. That matches production
    /// `JobObjectGuard::arm` usage, where the guard is armed immediately after
    /// spawn. Assigning a process to a job after it has already spawned a child
    /// does not retroactively capture that existing child.
    ///
    /// Returns the direct child.
    fn spawn_delayed_two_level_tree() -> std::io::Result<Child> {
        use std::process::{Command, Stdio};

        Command::new("cmd")
            .args([
                "/c",
                "ping -n 2 127.0.0.1 > nul & ping -n 60 127.0.0.1 > nul",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }

    fn wait_for_child_pid(parent_pid: u32, timeout: Duration) -> Option<u32> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(pid) = labby_winjob::find_first_child_pid(parent_pid) {
                return Some(pid);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn assert_alive(pid: u32, label: &str) {
        match labby_winjob::pid_liveness(pid) {
            Ok(ProcessLiveness::Alive) => {}
            Ok(state) => panic!("{label} pid {pid} should be alive, got {state:?}"),
            Err(error) => panic!("could not query {label} pid {pid}: {error}"),
        }
    }

    fn assert_not_alive(pid: u32, label: &str) {
        match labby_winjob::pid_liveness(pid) {
            Ok(ProcessLiveness::Exited | ProcessLiveness::NotFound) => {}
            Ok(ProcessLiveness::Alive) => panic!("{label} pid {pid} should be dead"),
            Err(error) => panic!("could not query {label} pid {pid}: {error}"),
        }
    }

    fn wait_for_child_to_stop(pid: u32, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match labby_winjob::pid_liveness(pid) {
                Ok(ProcessLiveness::Alive) => {}
                Ok(ProcessLiveness::Exited | ProcessLiveness::NotFound) => return true,
                Err(error) => panic!("could not query child pid {pid}: {error}"),
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_for_long_lived_child_pid(parent_pid: u32, timeout: Duration) -> Option<u32> {
        let first_child_pid = wait_for_child_pid(parent_pid, timeout)?;

        // The first child should be the short delay ping. If it is still alive
        // after the transition window, treat it as the long-lived child instead
        // of relying on a fixed sleep.
        if !wait_for_child_to_stop(first_child_pid, Duration::from_secs(3)) {
            return Some(first_child_pid);
        }

        wait_for_child_pid(parent_pid, timeout)
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
        let mut direct_child =
            spawn_delayed_two_level_tree().expect("spawn delayed two-level Windows process tree");

        let parent_pid = direct_child.id();

        // Arm the same production guard used immediately after spawning stdio
        // upstream processes, before the long-lived grandchild has started.
        // `disarm` mirrors the successful connect path, where ownership moves
        // into `UpstreamConnection::runtime.job_handle`.
        let job: isize = JobObjectGuard::arm(parent_pid).disarm();
        assert!(
            job != 0,
            "JobObjectGuard::arm should succeed; got sentinel handle {job}"
        );

        let Some(grandchild_pid) =
            wait_for_long_lived_child_pid(parent_pid, Duration::from_secs(5))
        else {
            let _kill = direct_child.kill();
            let _wait = direct_child.wait();
            panic!("direct child pid {parent_pid} did not spawn a long-lived grandchild");
        };

        // Confirm grandchild is alive before we close the job.
        assert_alive(grandchild_pid, "grandchild");

        // Close the job handle → OS terminates the whole tree.
        labby_winjob::close_job(job, parent_pid);

        // Give the OS a moment to reap the tree.
        std::thread::sleep(Duration::from_millis(500));

        // Poll grandchild: it should be gone.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match labby_winjob::pid_liveness(grandchild_pid) {
                Ok(ProcessLiveness::Exited | ProcessLiveness::NotFound) => {
                    break; // Pass — grandchild was reaped.
                }
                Ok(ProcessLiveness::Alive) => {}
                Err(error) => panic!("could not query grandchild pid {grandchild_pid}: {error}"),
            }
            if std::time::Instant::now() >= deadline {
                // Force-clean the tree so subsequent test runs start fresh.
                let _kill = direct_child.kill();
                let _wait = direct_child.wait();
                panic!(
                    "grandchild pid {grandchild_pid} still alive 5 s after job-object close; \
                     Windows Job Object reaping did not work"
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        // Also ensure the direct child is gone (belt-and-suspenders).
        assert_not_alive(parent_pid, "direct child");
        // Reap to avoid zombie handle.
        let _wait = direct_child.wait();
    }
}
