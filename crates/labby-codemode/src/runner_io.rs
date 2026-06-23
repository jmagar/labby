//! Parent-side helpers driving the Code Mode runner subprocess: stdin writes
//! and termination.

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};

use crate::error::ToolError;

use super::protocol::CodeModeRunnerInput;

pub(crate) async fn write_runner_input(
    stdin: &mut ChildStdin,
    input: &CodeModeRunnerInput,
) -> Result<(), ToolError> {
    let mut line = serde_json::to_vec(input).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to encode Code Mode runner input: {err}"),
    })?;
    line.push(b'\n');
    stdin.write_all(&line).await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to write Code Mode runner input: {err}"),
    })?;
    stdin.flush().await.map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to flush Code Mode runner input: {err}"),
    })
}

pub(crate) async fn terminate_code_mode_runner(child: &mut Child, _pid: Option<u32>) {
    // On Unix, kill the entire process group (pgid == pid because we spawned
    // with process_group(0)) so that grandchildren are not re-parented to
    // PID 1 and left running after the runner exits.
    #[cfg(unix)]
    {
        if let Some(raw_pid) = _pid {
            use nix::sys::signal::Signal;
            use nix::unistd::Pid;
            let _ = nix::sys::signal::killpg(Pid::from_raw(raw_pid as i32), Signal::SIGKILL);
        }
    }
    // On Windows, the `PooledRunner._job_guard` (a `JobObjectGuard` armed at
    // spawn in `pool/runner_handle.rs`) owns a Job Object with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Dropping that guard when the runner
    // handle drops (on eviction, including after a timeout) lets the OS terminate
    // the whole descendant tree. This kill() call is therefore a
    // belt-and-suspenders direct kill of the immediate child process.
    drop(child.kill().await);
    drop(child.wait().await);
}
