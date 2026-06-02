//! `CodeModeBroker::run_in_runner`: spawn the runner subprocess and drive the
//! tool-call/log/completion protocol loop.

use std::process::Stdio;
use std::time::Duration;

use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader as TokioBufReader;
use tokio::process::Command;

use crate::dispatch::error::ToolError;

use super::CodeModeBroker;
use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};
use super::runner_io::{terminate_code_mode_runner, write_runner_input};
use super::truncate::apply_log_caps;
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeExecutedCall, CodeModeExecutionResponse,
    CodeModeSurface,
};

impl CodeModeBroker<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::dispatch::gateway::code_mode) async fn run_in_runner(
        &self,
        code_to_run: String,
        proxy: String,
        max_tool_calls: usize,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        capability_filter: CodeModeCapabilityFilter,
    ) -> Result<CodeModeExecutionResponse, ToolError> {
        let exe = std::env::current_exe().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to locate current executable for Code Mode runner: {err}"),
        })?;
        let temp_dir = TempDir::new().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to create Code Mode sandbox directory: {err}"),
        })?;
        let mut cmd = Command::new(exe);
        cmd.args(["internal", "code-mode-runner"])
            .current_dir(temp_dir.path())
            .env_clear()
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Make the child its own process group leader (pgid = pid) so that
        // killpg can reach grandchildren (e.g. any processes spawned by the
        // Boa/Javy runtime) and not just the immediate child.
        // process_group is Unix-only; on Windows we fall back to kill() on the
        // direct child only (handled in terminate_code_mode_runner).
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to spawn Code Mode runner: {err}"),
        })?;
        // Capture pid immediately after spawn (Unix only); it becomes None once
        // the child has been waited on, so we save it for killpg before any
        // await points.
        #[cfg(unix)]
        let child_pid = child.id();
        #[cfg(not(unix))]
        let child_pid = None::<u32>;

        let mut stdin = child.stdin.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdin was not available".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "Code Mode runner stdout was not available".to_string(),
        })?;

        // Drain stderr continuously in a background task to prevent pipe-buffer
        // deadlock when the runner emits more than ~64KB of console output. The
        // javy runner redirects console output to stderr, so this is where the
        // captured logs come from.
        let (stderr_lines, stderr_task) = {
            let stderr = child.stderr.take().ok_or_else(|| ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: "Code Mode runner stderr was not available".to_string(),
            })?;
            let stderr_buf = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
            let stderr_buf_clone = stderr_buf.clone();
            let task = tokio::spawn(async move {
                // Mirror the runner-side hard caps so the parent buffer can't
                // grow unbounded when the wasm feature swaps the runner backend.
                const CAP_ENTRIES: usize = 10_000;
                const CAP_BYTES: usize = 1024 * 1024;
                let mut lines = TokioBufReader::new(stderr).lines();
                let mut total_bytes = 0usize;
                let mut capped = false;
                // Keep draining the pipe to EOF even after the cap is reached:
                // stopping the read would let the child's stderr pipe fill and
                // block the child on write (hang on large log output). Once
                // capped, discard further lines instead of appending.
                while let Ok(Some(line)) = lines.next_line().await {
                    if capped {
                        continue;
                    }
                    total_bytes += line.len() + 1;
                    let mut buf = stderr_buf_clone.lock().await;
                    if buf.len() >= CAP_ENTRIES || total_bytes > CAP_BYTES {
                        capped = true;
                        continue;
                    }
                    buf.push(line);
                }
            });
            (stderr_buf, task)
        };

        write_runner_input(
            &mut stdin,
            &CodeModeRunnerInput::Start {
                code: code_to_run,
                proxy,
            },
        )
        .await?;

        let mut lines = TokioBufReader::new(stdout).lines();
        let mut calls = Vec::new();
        let mut pending_tool_calls = FuturesUnordered::new();
        let mut started_tool_calls = 0usize;
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            tokio::select! {
                line = tokio::time::timeout_at(deadline, lines.next_line()) => {
                    let line = match line {
                        Ok(line) => line,
                        Err(_) => {
                            terminate_code_mode_runner(&mut child, child_pid).await;
                            return Err(ToolError::Sdk {
                                sdk_kind: "timeout".to_string(),
                                message: "Code Mode execution timed out".to_string(),
                            });
                        }
                    };
                    let Some(line) = line.map_err(|err| ToolError::Sdk {
                        sdk_kind: "internal_error".to_string(),
                        message: format!("failed to read Code Mode runner output: {err}"),
                    })?
                    else {
                        let status = child.wait().await.map_err(|err| ToolError::Sdk {
                            sdk_kind: "internal_error".to_string(),
                            message: format!("failed to wait for Code Mode runner: {err}"),
                        })?;
                        return Err(ToolError::Sdk {
                            sdk_kind: "server_error".to_string(),
                            message: format!(
                                "Code Mode runner exited before completion with status {status}"
                            ),
                        });
                    };
                    match serde_json::from_str::<CodeModeRunnerOutput>(&line).map_err(|err| {
                        ToolError::Sdk {
                            sdk_kind: "internal_error".to_string(),
                            message: format!("Code Mode runner emitted invalid protocol JSON: {err}"),
                        }
                    })? {
                        CodeModeRunnerOutput::ToolCall { seq, id, params } => {
                            if started_tool_calls >= max_tool_calls {
                                terminate_code_mode_runner(&mut child, child_pid).await;
                                return Err(ToolError::Sdk {
                                    sdk_kind: "tool_call_limit_exceeded".to_string(),
                                    message: format!(
                                        "Code Mode execution exceeded max_tool_calls={max_tool_calls}"
                                    ),
                                });
                            }
                            started_tool_calls += 1;
                            let call_id = id.clone();
                            let caller = caller.clone();
                            let capability_filter = capability_filter.clone();
                            pending_tool_calls.push(
                                async move {
                                    let call_start = std::time::Instant::now();
                                    let result = self
                                        .call_tool_id_before_deadline(
                                            &id, params, deadline, caller, surface,
                                            &capability_filter,
                                        )
                                        .await;
                                    let elapsed_ms = call_start.elapsed().as_millis();
                                    (seq, call_id, result, elapsed_ms)
                                }
                                .boxed(),
                            );
                        }
                        CodeModeRunnerOutput::Done { result, logs } => {
                            if !pending_tool_calls.is_empty() {
                                terminate_code_mode_runner(&mut child, child_pid).await;
                                return Err(ToolError::Sdk {
                                    sdk_kind: "internal_error".to_string(),
                                    message: "Code Mode runner completed with pending tool calls".to_string(),
                                });
                            }
                            // Cloudflare parity: pure computation (filter, sort, reduce
                            // over already-known data) is a valid Code Mode use case.
                            // Do not require at least one callTool — let the user return
                            // a computed value from `result` without any tool calls.
                            let status = child.wait().await.map_err(|err| ToolError::Sdk {
                                sdk_kind: "internal_error".to_string(),
                                message: format!("failed to wait for Code Mode runner: {err}"),
                            })?;
                            if !status.success() {
                                return Err(ToolError::Sdk {
                                    sdk_kind: "server_error".to_string(),
                                    message: format!("Code Mode runner exited with status {status}"),
                                });
                            }
                            calls.sort_by_key(|(seq, _)| *seq);
                            // The child has exited (child.wait() above), so its
                            // stderr pipe is closed and the drain task will reach
                            // EOF. Await it before reading the buffer so trailing
                            // stderr lines are not lost.
                            let _joined = stderr_task.await;
                            // Merge stderr lines (Javy path: redirect_stdout_to_stderr)
                            // with protocol-carried logs. The javy path currently
                            // emits no protocol-carried logs, so `logs` is empty
                            // and stderr supplies the captured output.
                            let mut all_logs = logs;
                            {
                                let stderr_captured = stderr_lines.lock().await;
                                all_logs.extend(stderr_captured.iter().cloned());
                            }

                            // sanitize_tool_text() redacts secrets/control chars.
                            // Apply log caps from config, appending a sentinel when truncated.
                            let all_logs = apply_log_caps(
                                all_logs,
                                max_log_entries,
                                max_log_bytes,
                            );
                            let sanitized_logs = all_logs
                                .into_iter()
                                .map(|line| {
                                    crate::dispatch::gateway::projection::sanitize_tool_text(&line, 4096)
                                })
                                .collect();
                            return Ok(CodeModeExecutionResponse {
                                result,
                                calls: calls.into_iter().map(|(_, call)| call).collect(),
                                logs: sanitized_logs,
                            });
                        }
                        CodeModeRunnerOutput::Error { kind, message } => {
                            if let Ok(status) = child.wait().await {
                                tracing::debug!(
                                    surface = "dispatch",
                                    service = "code_mode",
                                    action = "code_execute",
                                    exit_status = %status,
                                    "runner exited with error"
                                );
                            }
                            return Err(ToolError::Sdk {
                                sdk_kind: kind,
                                message,
                            });
                        }
                    }
                }
                completed = pending_tool_calls.next(), if !pending_tool_calls.is_empty() => {
                    let Some((seq, id, result, elapsed_ms)):
                        Option<(u64, String, Result<Value, ToolError>, u128)> = completed
                    else {
                        continue;
                    };
                    match result {
                        Ok(result) => {
                            calls.push((seq, CodeModeExecutedCall {
                                id,
                                ok: true,
                                elapsed_ms,
                                error_kind: None,
                            }));
                            write_runner_input(
                                &mut stdin,
                                &CodeModeRunnerInput::ToolResult { seq, result },
                            )
                            .await?;
                        }
                        Err(err) => {
                            // Catchable tool errors (Cloudflare parity): a single failed
                            // callTool must NOT abort the run. Reject the in-sandbox promise
                            // with the structured {kind,message} so the user's JS try/catch
                            // can handle it and continue (e.g. partial fan-out). If the
                            // rejection is uncaught, the main promise rejects and the
                            // existing Rejected/Error runner-output path surfaces it as the
                            // final error. Limit/timeout paths still terminate (handled
                            // elsewhere) — only per-call tool errors are caught here.
                            let kind = match &err {
                                ToolError::Sdk { sdk_kind, .. } => sdk_kind.clone(),
                                other => other.kind().to_string(),
                            };
                            // The ToolError settles this seq's promise in-sandbox; do NOT
                            // also send a ToolResult for the same seq.
                            // Use user_message() (the human text), NOT to_string()
                            // (which emits the full JSON envelope) — otherwise the
                            // runner re-wraps it and the in-sandbox rejection message
                            // becomes double-JSON-encoded.
                            write_runner_input(
                                &mut stdin,
                                &CodeModeRunnerInput::ToolError {
                                    seq,
                                    kind: kind.clone(),
                                    message: err.user_message().to_string(),
                                },
                            )
                            .await?;
                            calls.push((seq, CodeModeExecutedCall {
                                id,
                                ok: false,
                                elapsed_ms,
                                error_kind: Some(kind),
                            }));
                        }
                    }
                }
            }
        }
    }
}
