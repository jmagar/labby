//! Parent-side helpers driving the Code Mode runner subprocess: stdin writes,
//! termination, and upstream error classification.

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};

use crate::dispatch::error::ToolError;

use super::protocol::CodeModeRunnerInput;

pub(in crate::dispatch::gateway::code_mode) async fn write_runner_input(
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

pub(in crate::dispatch::gateway::code_mode) async fn terminate_code_mode_runner(
    child: &mut Child,
    _pid: Option<u32>,
) {
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
    // On Windows, the `_runner_job_guard` in `run_in_runner_with_config` owns
    // a Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. Dropping that guard
    // (which happens on every return path, including timeout) lets the OS
    // terminate the whole descendant tree. This kill() call is therefore a
    // belt-and-suspenders direct kill of the immediate child process.
    drop(child.kill().await);
    drop(child.wait().await);
}

fn code_mode_canonical_error_kind(s: &str) -> &'static str {
    match s {
        "unknown_action" => "unknown_action",
        "unknown_subaction" => "unknown_subaction",
        "missing_param" => "missing_param",
        "invalid_param" => "invalid_param",
        "unknown_instance" => "unknown_instance",
        "confirmation_required" => "confirmation_required",
        "conflict" => "conflict",
        "auth_failed" => "auth_failed",
        "not_found" => "not_found",
        "rate_limited" => "rate_limited",
        "validation_failed" => "validation_failed",
        "network_error" => "network_error",
        "server_error" => "server_error",
        "decode_error" => "decode_error",
        "internal_error" => "internal_error",
        "upstream_error" => "upstream_error",
        "code_mode_timeout" => "code_mode_timeout",
        "code_mode_fuel_exhausted" => "code_mode_fuel_exhausted",
        _ => "internal_error",
    }
}

pub(in crate::dispatch::gateway::code_mode) fn code_mode_upstream_error_info(
    text: Option<&str>,
) -> (&'static str, String, bool) {
    let Some(text) = text else {
        return (
            "upstream_error",
            "upstream returned a non-text error payload".to_string(),
            true,
        );
    };

    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        return ("upstream_error", text.to_string(), true);
    };

    let error_obj = parsed
        .get("error")
        .and_then(Value::as_object)
        .or_else(|| parsed.as_object());
    let Some(error_obj) = error_obj else {
        return ("upstream_error", text.to_string(), true);
    };

    let kind = error_obj
        .get("kind")
        .and_then(Value::as_str)
        .map(code_mode_canonical_error_kind)
        .unwrap_or("upstream_error");
    let message = error_obj
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(text)
        .to_string();
    let counts_as_failure = matches!(
        kind,
        "upstream_error" | "network_error" | "server_error" | "decode_error" | "internal_error"
    );

    (kind, message, counts_as_failure)
}
