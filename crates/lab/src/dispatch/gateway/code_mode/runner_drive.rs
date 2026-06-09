//! `CodeModeBroker::run_in_runner`: spawn the runner subprocess and drive the
//! tool-call/log/completion protocol loop.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader as TokioBufReader;
use tokio::process::{ChildStdin, Command};
use ulid::Ulid;

use crate::dispatch::error::ToolError;

use super::CodeModeBroker;
use super::artifacts::{
    ActiveArtifactRun, CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};
use super::runner_io::{terminate_code_mode_runner, write_runner_input};
use super::truncate::apply_log_caps;
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeExecutedCall, CodeModeExecutionError,
    CodeModeExecutionResponse, CodeModeSurface,
};

const ARTIFACT_WRITE_CALL_ID: &str = "code_mode::write_artifact";

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
        trace_params: bool,
        capability_filter: CodeModeCapabilityFilter,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
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
        let artifact_run_id = Ulid::new().to_string();
        let artifact_root = code_mode_artifact_root(&artifact_run_id);
        // Mark this run active before any artifact dir exists, so a concurrent
        // run's first-write prune can never delete our directory mid-run. The
        // RAII guard clears the id on every exit path (including early returns).
        let _active_artifact_run = ActiveArtifactRun::register(&artifact_run_id);
        let mut artifacts: Vec<CodeModeArtifactReceipt> = Vec::new();
        // The store is pruned lazily on the first actual write (below), so runs
        // that never call writeArtifact — including every search — leave
        // `$LAB_HOME/code-mode-artifacts/` untouched.
        let mut artifact_store_pruned = false;

        loop {
            tokio::select! {
                line = tokio::time::timeout_at(deadline, lines.next_line()) => {
                    let line = match line {
                        Ok(line) => line,
                        Err(_) => {
                            terminate_code_mode_runner(&mut child, child_pid).await;
                            return Err(CodeModeExecutionError::with_trace(ToolError::Sdk {
                                sdk_kind: "timeout".to_string(),
                                message: "Code Mode execution timed out".to_string(),
                            }, sorted_calls(&calls)));
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
                        return Err(CodeModeExecutionError::with_trace(ToolError::Sdk {
                            sdk_kind: "server_error".to_string(),
                            message: format!(
                                "Code Mode runner exited before completion with status {status}"
                            ),
                        }, sorted_calls(&calls)));
                    };
                    match serde_json::from_str::<CodeModeRunnerOutput>(&line).map_err(|err| {
                        ToolError::Sdk {
                            sdk_kind: "internal_error".to_string(),
                            message: format!("Code Mode runner emitted invalid protocol JSON: {err}"),
                        }
                        })? {
                        CodeModeRunnerOutput::ToolCall { seq, id, params } => {
                            if let Err(err) = ensure_call_budget(started_tool_calls, max_tool_calls, &calls) {
                                terminate_code_mode_runner(&mut child, child_pid).await;
                                return Err(err);
                            }
                            started_tool_calls += 1;
                            let call_id = id.clone();
                            let redacted_params =
                                super::trace::redact_trace_params(&params, trace_params);
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
                                    (seq, call_id, redacted_params, result, elapsed_ms)
                                }
                                .boxed(),
                            );
                        }
                        CodeModeRunnerOutput::ArtifactWrite {
                            seq,
                            path,
                            content,
                            content_type,
                        } => {
                            if let Err(err) = ensure_call_budget(started_tool_calls, max_tool_calls, &calls) {
                                terminate_code_mode_runner(&mut child, child_pid).await;
                                return Err(err);
                            }
                            started_tool_calls += 1;
                            // Keep the on-disk store bounded — once per run, on the
                            // first write that actually touches it.
                            if !artifact_store_pruned {
                                super::artifacts::prune_artifact_runs(
                                    super::artifacts::artifact_retention_runs(),
                                )
                                .await;
                                artifact_store_pruned = true;
                            }
                            handle_artifact_write(
                                &mut stdin,
                                &artifact_root,
                                &mut artifacts,
                                &mut calls,
                                seq,
                                CodeModeArtifactWrite {
                                    path,
                                    content,
                                    content_type,
                                },
                                trace_params,
                            )
                            .await?;
                        }
                        CodeModeRunnerOutput::Done { result, logs } => {
                            if !pending_tool_calls.is_empty() {
                                terminate_code_mode_runner(&mut child, child_pid).await;
                                return Err(CodeModeExecutionError::with_trace(ToolError::Sdk {
                                    sdk_kind: "internal_error".to_string(),
                                    message: "Code Mode runner completed with pending tool calls".to_string(),
                                }, sorted_calls(&calls)));
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
                                return Err(CodeModeExecutionError::with_trace(ToolError::Sdk {
                                    sdk_kind: "server_error".to_string(),
                                    message: format!("Code Mode runner exited with status {status}"),
                                }, sorted_calls(&calls)));
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
                                result: result.into_response_result(),
                                calls: calls.into_iter().map(|(_, call)| call).collect(),
                                logs: sanitized_logs,
                                artifacts,
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
                            return Err(CodeModeExecutionError::with_trace(ToolError::Sdk {
                                sdk_kind: kind,
                                message,
                            }, sorted_calls(&calls)));
                        }
                    }
                }
                completed = pending_tool_calls.next(), if !pending_tool_calls.is_empty() => {
                    let Some((seq, id, params, result, elapsed_ms)):
                        Option<(u64, String, Option<Value>, Result<Value, ToolError>, u128)> = completed
                    else {
                        continue;
                    };
                    match result {
                        Ok(result) => {
                            calls.push((seq, CodeModeExecutedCall {
                                id,
                                ok: true,
                                elapsed_ms,
                                params,
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
                                params,
                                error_kind: Some(kind),
                            }));
                        }
                    }
                }
            }
        }
    }
}

fn sorted_calls(calls: &[(u64, CodeModeExecutedCall)]) -> Vec<CodeModeExecutedCall> {
    let mut calls = calls.to_vec();
    calls.sort_by_key(|(seq, _)| *seq);
    calls.into_iter().map(|(_, call)| call).collect()
}

fn ensure_call_budget(
    started_tool_calls: usize,
    max_tool_calls: usize,
    calls: &[(u64, CodeModeExecutedCall)],
) -> Result<(), CodeModeExecutionError> {
    if started_tool_calls < max_tool_calls {
        return Ok(());
    }

    Err(CodeModeExecutionError::with_trace(
        ToolError::Sdk {
            sdk_kind: "tool_call_limit_exceeded".to_string(),
            message: format!("Code Mode execution exceeded max_tool_calls={max_tool_calls}"),
        },
        sorted_calls(calls),
    ))
}

/// Test seam for the shared budget gate (tool calls + artifact writes both go
/// through `ensure_call_budget`). Keeps the trace argument out of the assertion.
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) fn ensure_call_budget_for_test(
    started_tool_calls: usize,
    max_tool_calls: usize,
) -> Result<(), CodeModeExecutionError> {
    ensure_call_budget(started_tool_calls, max_tool_calls, &[])
}

async fn handle_artifact_write(
    stdin: &mut ChildStdin,
    artifact_root: &Path,
    artifacts: &mut Vec<CodeModeArtifactReceipt>,
    calls: &mut Vec<(u64, CodeModeExecutedCall)>,
    seq: u64,
    request: CodeModeArtifactWrite,
    trace_params: bool,
) -> Result<(), ToolError> {
    let started = std::time::Instant::now();
    let redacted_params = artifact_trace_params(&request, trace_params);

    match write_code_mode_artifact(artifact_root, &request).await {
        Ok(receipt) => {
            let result = json!(receipt);
            artifacts.push(receipt);
            calls.push(artifact_call(
                seq,
                true,
                started.elapsed().as_millis(),
                redacted_params,
                None,
            ));
            write_runner_input(stdin, &CodeModeRunnerInput::ToolResult { seq, result }).await
        }
        Err(err) => {
            let kind = err.kind().to_string();
            calls.push(artifact_call(
                seq,
                false,
                started.elapsed().as_millis(),
                redacted_params,
                Some(kind.clone()),
            ));
            write_runner_input(
                stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind,
                    message: err.user_message().to_string(),
                },
            )
            .await
        }
    }
}

fn artifact_trace_params(request: &CodeModeArtifactWrite, trace_params: bool) -> Option<Value> {
    super::trace::redact_trace_params(
        &json!({
            "path": request.path.as_str(),
            "content_type": request.content_type.as_deref(),
        }),
        trace_params,
    )
}

fn artifact_call(
    seq: u64,
    ok: bool,
    elapsed_ms: u128,
    params: Option<Value>,
    error_kind: Option<String>,
) -> (u64, CodeModeExecutedCall) {
    (
        seq,
        CodeModeExecutedCall {
            id: ARTIFACT_WRITE_CALL_ID.to_string(),
            ok,
            elapsed_ms,
            params,
            error_kind,
        },
    )
}
