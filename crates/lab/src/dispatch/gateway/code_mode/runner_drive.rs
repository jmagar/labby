//! `CodeModeBroker::run_in_runner`: spawn the runner subprocess and drive the
//! tool-call/log/completion protocol loop.
//!
//! The public entry point is `run_in_runner`, which accepts the same
//! positional parameters as before (preserving all call sites) but
//! immediately packs them into a `RunnerConfig` struct and delegates to
//! `run_in_runner_with_config`. Each major event arm (`Done`, `ToolCall`,
//! `ArtifactWrite`, `Error`) is handled by a named async helper to keep the
//! select loop readable.

use std::path::Path;
use std::time::Duration;

use futures::{StreamExt, stream::FuturesUnordered};
use serde_json::{Value, json};
use tokio::process::ChildStdin;
use ulid::Ulid;

use crate::dispatch::error::ToolError;

use super::CodeModeBroker;
use super::GatewayManager;
use super::artifacts::{
    ActiveArtifactRun, CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
use super::pool::RunnerPool;
use super::pool::runner_handle::PooledRunner;
use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};
use super::runner_io::{terminate_code_mode_runner, write_runner_input};
use super::truncate::apply_log_caps;
use super::types::{
    CodeModeCaller, CodeModeCapabilityFilter, CodeModeExecutedCall, CodeModeExecutionError,
    CodeModeExecutionResponse, CodeModeSurface,
};

const ARTIFACT_WRITE_CALL_ID: &str = "code_mode::write_artifact";

// Concrete future type for pending tool calls.
// Using Pin<Box<dyn Future>> keeps the FuturesUnordered type concrete so the
// compiler can infer the element type at the declaration site without requiring
// `impl Future` in a non-`async fn` parameter position (which is unsupported).
type ToolCallFut<'a> = std::pin::Pin<
    Box<
        dyn Future<Output = (u64, String, Option<Value>, Result<Value, ToolError>, u128)>
            + Send
            + 'a,
    >,
>;

// ---------------------------------------------------------------------------
// RunnerConfig — collects the 10 positional parameters into one struct
// ---------------------------------------------------------------------------

/// All configuration for a single `run_in_runner` invocation.
///
/// Collecting these into a struct eliminates the 10-positional-argument call
/// site (clippy `too_many_arguments`) and makes each field self-documenting.
pub(in crate::dispatch::gateway::code_mode) struct RunnerConfig {
    pub code_to_run: String,
    pub proxy: String,
    pub max_tool_calls: usize,
    pub timeout: Duration,
    pub caller: CodeModeCaller,
    pub surface: CodeModeSurface,
    pub max_log_entries: usize,
    pub max_log_bytes: usize,
    pub trace_params: bool,
    pub capability_filter: CodeModeCapabilityFilter,
}

// ---------------------------------------------------------------------------
// Drive state — per-run mutable bookkeeping (excludes pending_tool_calls,
// which stays local in run_in_runner_with_config so its lifetime is tied to
// the enclosing async fn and not forced to 'static)
// ---------------------------------------------------------------------------

struct DriveState {
    calls: Vec<(u64, CodeModeExecutedCall)>,
    started_tool_calls: usize,
    started_artifact_writes: usize,
    artifacts: Vec<CodeModeArtifactReceipt>,
    artifact_store_pruned: bool,
    artifact_max_bytes: usize,
    artifact_root: std::path::PathBuf,
}

impl DriveState {
    fn new(artifact_run_id: &str) -> Self {
        let artifact_root = code_mode_artifact_root(artifact_run_id);
        let artifact_max_bytes = super::artifacts::artifact_max_bytes();
        Self {
            calls: Vec::new(),
            started_tool_calls: 0,
            started_artifact_writes: 0,
            artifacts: Vec::new(),
            artifact_store_pruned: false,
            artifact_max_bytes,
            artifact_root,
        }
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

impl CodeModeBroker<'_> {
    /// Spawn the runner subprocess, send the code, and drive the
    /// tool-call/artifact/completion protocol loop until the runner exits
    /// or the wall-clock deadline fires.
    ///
    /// The signature is kept identical to the original (10 positional params)
    /// so all call sites compile unchanged. Internally the params are packed
    /// into [`RunnerConfig`] and the loop arms are delegated to named helpers.
    /// All timeout, killpg, and budget-gate invariants are preserved exactly.
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
        let cfg = RunnerConfig {
            code_to_run,
            proxy,
            max_tool_calls,
            timeout,
            caller,
            surface,
            max_log_entries,
            max_log_bytes,
            trace_params,
            capability_filter,
        };
        self.run_in_runner_with_config(cfg).await
    }

    async fn run_in_runner_with_config(
        &self,
        cfg: RunnerConfig,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let exe = std::env::current_exe().map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to locate current executable for Code Mode runner: {err}"),
        })?;

        // Acquire a runner. With a gateway manager, use the shared warm pool
        // (Perf H1): a pooled runner amortizes the fork/startup cost across
        // executions while still building a fresh `javy::Runtime` per `Start`
        // (runner-side), so JS state isolation holds. Without a manager (some
        // tests / standalone paths), spawn a one-shot runner directly.
        match self
            .gateway_manager
            .map(GatewayManager::code_mode_runner_pool)
        {
            Some(pool) => self.run_via_pool(pool, &exe, cfg).await,
            None => self.run_standalone(&exe, cfg).await,
        }
    }

    /// Run one execution against a runner checked out from the shared pool.
    ///
    /// On a clean completion (`Done`) or a runner-reported execution `Error`,
    /// the runner is parked and returned to the pool — it stayed alive and built
    /// a fresh runtime, so it is safe to reuse. On a crash (EOF/exit), timeout,
    /// or protocol fault the runner is evicted (killed) and the slot respawns on
    /// the next checkout.
    async fn run_via_pool(
        &self,
        pool: &RunnerPool,
        exe: &Path,
        cfg: RunnerConfig,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let mut lease = pool.checkout(exe).await?;
        let outcome = self.drive_runner(lease.runner_mut(), &cfg).await;
        match outcome {
            DriveOutcome::Completed(response) => {
                lease.release().await;
                Ok(response)
            }
            DriveOutcome::ExecutionError(err) => {
                // The runner parked after emitting its `Error` line and is
                // healthy (fresh runtime dropped); reuse it.
                lease.release().await;
                Err(err)
            }
            DriveOutcome::RunnerUnhealthy(err) => {
                // Crash / timeout / protocol fault: discard the runner so the
                // pool respawns a clean replacement.
                lease.evict();
                Err(err)
            }
        }
    }

    /// Run one execution against a freshly-spawned one-shot runner (no pool).
    async fn run_standalone(
        &self,
        exe: &Path,
        cfg: RunnerConfig,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let mut runner = PooledRunner::spawn(exe)?;
        let outcome = self.drive_runner(&mut runner, &cfg).await;
        // The runner handle's Drop kills the process on every path here, so a
        // standalone runner is never leaked or reused.
        match outcome {
            DriveOutcome::Completed(response) => Ok(response),
            DriveOutcome::ExecutionError(err) | DriveOutcome::RunnerUnhealthy(err) => Err(err),
        }
    }

    /// Drive the Start → tool-call/artifact → Done/Error protocol loop against a
    /// single runner. Returns a [`DriveOutcome`] classifying both the result and
    /// whether the runner is safe to reuse.
    async fn drive_runner(&self, runner: &mut PooledRunner, cfg: &RunnerConfig) -> DriveOutcome {
        // Record the stderr buffer position before this execution so we capture
        // only the lines this run produces (a pooled runner's buffer carries
        // prior executions' lines).
        let stderr = runner.stderr.clone();
        let stderr_start = stderr.mark().await;

        if let Err(err) = write_runner_input(
            &mut runner.stdin,
            &CodeModeRunnerInput::Start {
                code: cfg.code_to_run.clone(),
                proxy: cfg.proxy.clone(),
            },
        )
        .await
        {
            // Failed to even send Start — the runner is suspect; evict it.
            return DriveOutcome::RunnerUnhealthy(err.into());
        }

        let deadline = tokio::time::Instant::now() + cfg.timeout;
        let artifact_run_id = Ulid::new().to_string();
        let mut state = DriveState::new(&artifact_run_id);
        // Mark this run active before any artifact dir exists, so a concurrent
        // run's first-write prune can never delete our directory mid-run. The
        // RAII guard clears the id on every exit path (including early returns).
        let _active_artifact_run = ActiveArtifactRun::register(&artifact_run_id);

        // pending_tool_calls lives here (not in DriveState) so its lifetime is
        // tied to this async fn rather than being forced to 'static, allowing
        // futures to capture `self` (a non-'static reference) without error.
        let mut pending_tool_calls: FuturesUnordered<ToolCallFut<'_>> = FuturesUnordered::new();

        // Borrow the runner's components for the loop. The protocol loop owns
        // these references for its duration; the runner is parked afterwards.
        let child = &mut runner.child;
        let child_pid = runner.child_pid;
        let stdin = &mut runner.stdin;
        let lines = &mut runner.lines;

        loop {
            tokio::select! {
                line = tokio::time::timeout_at(deadline, lines.next()) => {
                    let line = match line {
                        Ok(line) => line,
                        Err(_) => {
                            // Wall-clock expiry: kill the runner (do not reuse a
                            // runtime mid-execution) so the pool respawns it.
                            terminate_code_mode_runner(child, child_pid).await;
                            return DriveOutcome::RunnerUnhealthy(
                                code_mode_timeout_error(&state.calls),
                            );
                        }
                    };
                    // `FramedRead::next()` yields `Option<Result<String, LinesCodecError>>`.
                    // `None` = EOF (runner crashed/exited); `Some(Err(_))` = I/O or line-too-long.
                    let Some(line_result) = line else {
                        // EOF: the runner process died unexpectedly. Surface a
                        // clean error and evict so a replacement spawns.
                        drop(child.wait().await);
                        return DriveOutcome::RunnerUnhealthy(
                            CodeModeExecutionError::with_trace(
                                ToolError::Sdk {
                                    sdk_kind: "server_error".to_string(),
                                    message:
                                        "Code Mode runner exited before completion".to_string(),
                                },
                                sorted_calls(&state.calls),
                            ),
                        );
                    };
                    let line = match classify_line_result(line_result) {
                        Ok(line) => line,
                        Err(err) => {
                            terminate_code_mode_runner(child, child_pid).await;
                            return DriveOutcome::RunnerUnhealthy(
                                CodeModeExecutionError::with_trace(err, sorted_calls(&state.calls)),
                            );
                        }
                    };

                    let msg = match serde_json::from_str::<CodeModeRunnerOutput>(&line) {
                        Ok(msg) => msg,
                        Err(err) => {
                            terminate_code_mode_runner(child, child_pid).await;
                            return DriveOutcome::RunnerUnhealthy(
                                CodeModeExecutionError::with_trace(
                                    ToolError::Sdk {
                                        sdk_kind: "internal_error".to_string(),
                                        message: format!(
                                            "Code Mode runner emitted invalid protocol JSON: {err}"
                                        ),
                                    },
                                    sorted_calls(&state.calls),
                                ),
                            );
                        }
                    };

                    match msg {
                        CodeModeRunnerOutput::ToolCall { seq, id, params } => {
                            if let Err(err) = enqueue_tool_call(
                                self,
                                seq,
                                id,
                                params,
                                child,
                                child_pid,
                                deadline,
                                cfg,
                                &mut state,
                                &mut pending_tool_calls,
                            )
                            .await
                            {
                                // The limit gate already terminated the runner.
                                return DriveOutcome::RunnerUnhealthy(err);
                            }
                        }
                        CodeModeRunnerOutput::ArtifactWrite {
                            seq,
                            path,
                            content,
                            content_type,
                        } => {
                            if let Err(err) = handle_artifact_write_event(
                                seq,
                                path,
                                content,
                                content_type,
                                stdin,
                                child,
                                child_pid,
                                deadline,
                                cfg,
                                &mut state,
                            )
                            .await
                            {
                                return DriveOutcome::RunnerUnhealthy(err);
                            }
                        }
                        CodeModeRunnerOutput::Done { result, logs } => {
                            // Preserve original invariant: Done with in-flight
                            // tool calls is a protocol error → evict.
                            if !pending_tool_calls.is_empty() {
                                terminate_code_mode_runner(child, child_pid).await;
                                return DriveOutcome::RunnerUnhealthy(
                                    CodeModeExecutionError::with_trace(
                                        ToolError::Sdk {
                                            sdk_kind: "internal_error".to_string(),
                                            message:
                                                "Code Mode runner completed with pending tool calls"
                                                    .to_string(),
                                        },
                                        sorted_calls(&state.calls),
                                    ),
                                );
                            }
                            let response = finalize_done(result, logs, &state);
                            // Capture only this execution's stderr lines. The
                            // runner is parked (it loops), so do not wait on it;
                            // give the drain a brief window to flush console
                            // output emitted before Done.
                            stderr.flush_settle().await;
                            let mut all_logs = response.logs.clone();
                            all_logs.extend(stderr.take_since_and_clear(stderr_start).await);
                            let all_logs = apply_log_caps(
                                all_logs,
                                cfg.max_log_entries,
                                cfg.max_log_bytes,
                            );
                            let sanitized_logs = all_logs
                                .into_iter()
                                .map(|line| {
                                    crate::dispatch::gateway::projection::sanitize_tool_text(
                                        &line, 4096,
                                    )
                                })
                                .collect();
                            return DriveOutcome::Completed(CodeModeExecutionResponse {
                                logs: sanitized_logs,
                                ..response
                            });
                        }
                        CodeModeRunnerOutput::Error { kind, message } => {
                            // A per-execution error. The runner reset and parked
                            // (it does NOT exit), so it is safe to reuse — return
                            // ExecutionError so the pool releases rather than
                            // evicts.
                            stderr.flush_settle().await;
                            stderr.clear().await;
                            return DriveOutcome::ExecutionError(
                                CodeModeExecutionError::with_trace(
                                    ToolError::Sdk {
                                        sdk_kind: kind,
                                        message,
                                    },
                                    sorted_calls(&state.calls),
                                ),
                            );
                        }
                    }
                }
                completed = pending_tool_calls.next(),
                    if !pending_tool_calls.is_empty() =>
                {
                    if let Err(err) =
                        handle_completed_tool_call(completed, stdin, &mut state).await
                    {
                        // Failed to relay a tool result back to the runner — the
                        // pipe is suspect; evict.
                        return DriveOutcome::RunnerUnhealthy(err);
                    }
                }
            }
        }
    }
}

/// Classification of a single drive: the result plus whether the runner is safe
/// to return to the pool.
enum DriveOutcome {
    /// Clean `Done` — return the response and keep (park) the runner.
    Completed(CodeModeExecutionResponse),
    /// The runner reported a per-execution `Error` and then parked itself; the
    /// process is healthy and may be reused.
    ExecutionError(CodeModeExecutionError),
    /// The runner crashed, timed out, or violated the protocol; it must be
    /// killed and replaced.
    RunnerUnhealthy(CodeModeExecutionError),
}

/// Decode a framed-line read result into either the line text or a structured
/// I/O / protocol-violation error.
fn classify_line_result(
    line_result: Result<String, tokio_util::codec::LinesCodecError>,
) -> Result<String, ToolError> {
    line_result.map_err(|err| {
        use tokio_util::codec::LinesCodecError;
        let max = super::pool::runner_handle::MAX_LINE_BYTES;
        let (sdk_kind, message) = match &err {
            LinesCodecError::MaxLineLengthExceeded => (
                "internal_error",
                format!(
                    "Code Mode runner emitted a protocol line exceeding the \
                     {max}-byte safety cap; possible unbounded output"
                ),
            ),
            LinesCodecError::Io(io_err) => (
                "internal_error",
                format!("failed to read Code Mode runner output: {io_err}"),
            ),
        };
        ToolError::Sdk {
            sdk_kind: sdk_kind.to_string(),
            message,
        }
    })
}

// ---------------------------------------------------------------------------
// Named arm helpers (free functions so they don't capture `self` lifetimes)
// ---------------------------------------------------------------------------

/// Enqueue a `ToolCall` request from the runner into `pending_tool_calls`.
///
/// Free function (not `&self` method) so the returned future can capture
/// `broker` with the same lifetime as the enclosing `run_in_runner_with_config`
/// rather than being forced to `'static`.
async fn enqueue_tool_call<'a>(
    broker: &'a CodeModeBroker<'a>,
    seq: u64,
    id: String,
    params: Value,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    cfg: &RunnerConfig,
    state: &mut DriveState,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
) -> Result<(), CodeModeExecutionError> {
    if let Err(err) = ensure_within_limit(
        state.started_tool_calls,
        cfg.max_tool_calls,
        "tool call",
        &state.calls,
    ) {
        terminate_code_mode_runner(child, child_pid).await;
        return Err(err);
    }
    state.started_tool_calls += 1;
    let call_id = id.clone();
    let redacted_params = super::trace::redact_trace_params(&params, cfg.trace_params);
    let caller = cfg.caller.clone();
    let capability_filter = cfg.capability_filter.clone();
    let surface = cfg.surface;
    pending_tool_calls.push(Box::pin(async move {
        let call_start = std::time::Instant::now();
        let result = broker
            .call_tool_id_before_deadline(
                &id,
                params,
                deadline,
                caller,
                surface,
                &capability_filter,
            )
            .await;
        let elapsed_ms = call_start.elapsed().as_millis();
        (seq, call_id, redacted_params, result, elapsed_ms)
    }));
    Ok(())
}

/// Handle an `ArtifactWrite` event from the runner.
async fn handle_artifact_write_event(
    seq: u64,
    path: String,
    content: String,
    content_type: Option<String>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    cfg: &RunnerConfig,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    if let Err(err) = ensure_within_limit(
        state.started_artifact_writes,
        cfg.max_tool_calls,
        "artifact write",
        &state.calls,
    ) {
        terminate_code_mode_runner(child, child_pid).await;
        return Err(err);
    }
    state.started_artifact_writes += 1;
    // Prune (lazy, once per run) and the write are host-side filesystem work;
    // bound them by the run deadline just like tool calls so a hung or slow
    // disk can't outlive `timeout_ms`.
    let artifact_root = state.artifact_root.clone();
    let artifact_max_bytes = state.artifact_max_bytes;
    let trace_params = cfg.trace_params;
    let artifact_op = async {
        if !state.artifact_store_pruned {
            super::artifacts::prune_artifact_runs(super::artifacts::artifact_retention_runs())
                .await;
            state.artifact_store_pruned = true;
        }
        handle_artifact_write(
            stdin,
            &artifact_root,
            &mut state.artifacts,
            &mut state.calls,
            seq,
            CodeModeArtifactWrite {
                path,
                content,
                content_type,
            },
            trace_params,
            artifact_max_bytes,
        )
        .await
    };
    match tokio::time::timeout_at(deadline, artifact_op).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(err.into())
        }
        Err(_) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(code_mode_timeout_error(&state.calls))
        }
    }
}

/// Assemble the `Done` response. The runner is long-lived (it loops after Done),
/// so this does NOT wait on the child — the process parks for the next `Start`.
/// Logs are merged by the caller from the per-execution stderr slice.
///
/// Cloudflare parity: pure computation (filter, sort, reduce over
/// already-known data) is a valid Code Mode use case. Do not require at
/// least one callTool.
fn finalize_done(
    result: super::protocol::CodeModeRunnerResult,
    logs: Vec<String>,
    state: &DriveState,
) -> CodeModeExecutionResponse {
    let mut sorted = state.calls.clone();
    sorted.sort_by_key(|(seq, _)| *seq);
    CodeModeExecutionResponse {
        result: result.into_response_result(),
        // Widget capture and optional `__ui` unwrapping are applied later in
        // `execute()`; the runner-level response always starts with `ui: None`.
        ui: None,
        calls: sorted.into_iter().map(|(_, call)| call).collect(),
        // Caller merges the per-execution stderr slice into logs.
        logs,
        artifacts: state.artifacts.clone(),
    }
}

/// Handle a completed tool-call future from `pending_tool_calls`.
async fn handle_completed_tool_call(
    completed: Option<(u64, String, Option<Value>, Result<Value, ToolError>, u128)>,
    stdin: &mut ChildStdin,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let Some((seq, id, params, result, elapsed_ms)) = completed else {
        return Ok(());
    };
    match result {
        Ok(result) => {
            state.calls.push((
                seq,
                CodeModeExecutedCall {
                    id,
                    ok: true,
                    elapsed_ms,
                    params,
                    error_kind: None,
                },
            ));
            write_runner_input(stdin, &CodeModeRunnerInput::ToolResult { seq, result }).await?;
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
                stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind: kind.clone(),
                    message: err.user_message().to_string(),
                },
            )
            .await?;
            state.calls.push((
                seq,
                CodeModeExecutedCall {
                    id,
                    ok: false,
                    elapsed_ms,
                    params,
                    error_kind: Some(kind),
                },
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers (unchanged from original)
// ---------------------------------------------------------------------------

fn sorted_calls(calls: &[(u64, CodeModeExecutedCall)]) -> Vec<CodeModeExecutedCall> {
    let mut calls = calls.to_vec();
    calls.sort_by_key(|(seq, _)| *seq);
    calls.into_iter().map(|(_, call)| call).collect()
}

/// The single source of the Code Mode deadline-timeout error, carrying the
/// partial call trace. Used by both the runner-read and artifact-write timeout
/// paths so the stable `timeout` kind/message lives in one place.
fn code_mode_timeout_error(calls: &[(u64, CodeModeExecutedCall)]) -> CodeModeExecutionError {
    CodeModeExecutionError::with_trace(
        ToolError::Sdk {
            sdk_kind: "timeout".to_string(),
            message: "Code Mode execution timed out".to_string(),
        },
        sorted_calls(calls),
    )
}

/// Shared budget gate for the two host-brokered operations. Tool calls and
/// artifact writes each pass their own running count and share the
/// `max_tool_calls` limit, so neither starves the other. `noun` names the
/// operation in the error message; both reuse the `tool_call_limit_exceeded`
/// kind (HTTP 429) per the error contract.
fn ensure_within_limit(
    started: usize,
    limit: usize,
    noun: &str,
    calls: &[(u64, CodeModeExecutedCall)],
) -> Result<(), CodeModeExecutionError> {
    if started < limit {
        return Ok(());
    }

    Err(CodeModeExecutionError::with_trace(
        ToolError::Sdk {
            sdk_kind: "tool_call_limit_exceeded".to_string(),
            message: format!("Code Mode execution exceeded the {noun} limit of {limit}"),
        },
        sorted_calls(calls),
    ))
}

/// Test seam for the shared budget gate. Keeps the trace argument out of the
/// assertion.
#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) fn ensure_call_budget_for_test(
    started: usize,
    limit: usize,
) -> Result<(), CodeModeExecutionError> {
    ensure_within_limit(started, limit, "tool call", &[])
}

async fn handle_artifact_write(
    stdin: &mut ChildStdin,
    artifact_root: &Path,
    artifacts: &mut Vec<CodeModeArtifactReceipt>,
    calls: &mut Vec<(u64, CodeModeExecutedCall)>,
    seq: u64,
    request: CodeModeArtifactWrite,
    trace_params: bool,
    max_bytes: usize,
) -> Result<(), ToolError> {
    let started = std::time::Instant::now();
    let redacted_params = artifact_trace_params(&request, trace_params);

    match write_code_mode_artifact(artifact_root, &request, max_bytes).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ToolRegistry;

    fn test_config(timeout: Duration) -> RunnerConfig {
        RunnerConfig {
            code_to_run: "async () => 1".to_string(),
            proxy: String::new(),
            max_tool_calls: 1,
            timeout,
            caller: CodeModeCaller::TrustedLocal,
            surface: CodeModeSurface::Cli,
            max_log_entries: 100,
            max_log_bytes: 4096,
            trace_params: false,
            capability_filter: CodeModeCapabilityFilter::default(),
        }
    }

    /// The wall-clock deadline path: a runner that never replies is killed when
    /// the deadline fires, the run surfaces the stable `timeout` kind, and the
    /// runner is classified `RunnerUnhealthy` so the pool evicts (never reuses) a
    /// runtime interrupted mid-execution.
    #[tokio::test]
    async fn drive_runner_times_out_and_marks_runner_unhealthy() {
        let registry = ToolRegistry::new();
        let broker = CodeModeBroker::new(&registry, None);
        let mut runner = PooledRunner::spawn_stub_silent().expect("spawn silent stub");
        let outcome = broker
            .drive_runner(&mut runner, &test_config(Duration::from_millis(80)))
            .await;
        match outcome {
            DriveOutcome::RunnerUnhealthy(err) => {
                assert_eq!(
                    err.kind(),
                    "timeout",
                    "wall-clock expiry must surface the `timeout` kind"
                );
            }
            DriveOutcome::Completed(_) | DriveOutcome::ExecutionError(_) => {
                panic!("a never-replying runner must time out as RunnerUnhealthy")
            }
        }
    }
}
