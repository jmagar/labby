//! `CodeModeBroker::run_in_runner`: spawn the runner subprocess and drive the
//! tool-call/log/completion protocol loop.
//!
//! The public entry point is `run_in_runner`, which takes a `RunnerConfig`
//! struct (built by the caller so every runtime parameter is named) and routes
//! to the pooled or one-shot runner path. Each major event arm (`Done`,
//! `ToolCall`, `ArtifactWrite`, `SnippetResolve`, `Error`) is handled by a
//! named async helper to keep the select loop readable.

use std::path::{Path, PathBuf};
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
const MAX_SNIPPET_RESOLVES_PER_RUN: usize = 32;
const MAX_SNIPPET_RESOLVED_BYTES_PER_RUN: usize = 256 * 1024;

/// Default per-run `callTool` fan-out budget (lab-4dcil item 3).
///
/// A single `Promise.all([...thousands of callTool...])` enqueues every future
/// before any settle, amplifying load against upstreams within the wall-clock
/// window. Past this many `callTool` invocations in one run, further calls are
/// rejected with the recoverable `call_budget_exceeded` kind (the in-sandbox
/// promise rejects cleanly) rather than killing the run. Override with
/// `LAB_CODE_MODE_MAX_CALLS_PER_RUN`; hard-clamped to [`MAX_CALLTOOL_PER_RUN_CEILING`]
/// so a misconfigured value cannot re-open the amplification window.
const DEFAULT_MAX_CALLTOOL_PER_RUN: u64 = 512;
/// Hard ceiling on the configurable per-run `callTool` budget.
const MAX_CALLTOOL_PER_RUN_CEILING: u64 = 2048;

/// Default host-side byte ceiling on a single `callTool` RESULT before it enters
/// the runner stdin pipe (lab-y966d item 1).
///
/// A large binary `Uint8Array` returned by an upstream tool would otherwise
/// reach the runner and OOM the 64-MiB QuickJS heap during decode, surfacing as
/// an opaque `server_error`. This pre-flight check on the serialized JSON bytes
/// turns that into a clean, recoverable `result_too_large` kind. Mirrors the
/// artifact content cap (`DEFAULT_ARTIFACT_MAX_MIB`). Override with
/// `LAB_CODE_MODE_CALLTOOL_RESULT_MAX_MIB`; keep it below ~64 to preserve the
/// clean-error boundary.
const DEFAULT_CALLTOOL_RESULT_MAX_MIB: usize = 8;

/// Resolve the per-run `callTool` fan-out budget from the environment, falling
/// back to [`DEFAULT_MAX_CALLTOOL_PER_RUN`] and clamping to
/// [`MAX_CALLTOOL_PER_RUN_CEILING`]. Absent/blank → default silently;
/// present-but-unparseable or `0` → warn and fall back (a 0 budget would reject
/// every call).
fn max_calltool_per_run() -> u64 {
    let Some(raw) = crate::dispatch::helpers::env_non_empty("LAB_CODE_MODE_MAX_CALLS_PER_RUN")
    else {
        return DEFAULT_MAX_CALLTOOL_PER_RUN;
    };
    match raw.trim().parse::<u64>() {
        Ok(value) if value > 0 => value.min(MAX_CALLTOOL_PER_RUN_CEILING),
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                value = %raw,
                default = DEFAULT_MAX_CALLTOOL_PER_RUN,
                "ignoring invalid LAB_CODE_MODE_MAX_CALLS_PER_RUN; using default"
            );
            DEFAULT_MAX_CALLTOOL_PER_RUN
        }
    }
}

/// Resolve the per-result byte ceiling (in bytes) from the environment, falling
/// back to [`DEFAULT_CALLTOOL_RESULT_MAX_MIB`]. The env value is in MiB
/// (`LAB_CODE_MODE_CALLTOOL_RESULT_MAX_MIB=16`); present-but-unparseable or `0`
/// → warn and fall back (a 0 cap would reject every result).
fn calltool_result_max_bytes() -> usize {
    let default_bytes = DEFAULT_CALLTOOL_RESULT_MAX_MIB * 1024 * 1024;
    let Some(raw) =
        crate::dispatch::helpers::env_non_empty("LAB_CODE_MODE_CALLTOOL_RESULT_MAX_MIB")
    else {
        return default_bytes;
    };
    match raw.trim().parse::<usize>() {
        Ok(mib) if mib > 0 => mib.saturating_mul(1024 * 1024),
        _ => {
            tracing::warn!(
                surface = "dispatch",
                service = "code_mode",
                action = "codemode",
                value = %raw,
                default_mib = DEFAULT_CALLTOOL_RESULT_MAX_MIB,
                "ignoring invalid LAB_CODE_MODE_CALLTOOL_RESULT_MAX_MIB; using default"
            );
            default_bytes
        }
    }
}

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
// which stays local in the drive loop so its lifetime is tied to the enclosing
// async fn and not forced to 'static)
// ---------------------------------------------------------------------------

struct DriveState {
    calls: Vec<(u64, CodeModeExecutedCall)>,
    artifacts: Vec<CodeModeArtifactReceipt>,
    artifact_store_pruned: bool,
    artifact_max_bytes: usize,
    artifact_root: PathBuf,
    snippet_resolves: usize,
    snippet_resolved_bytes: usize,
    /// Number of `callTool` invocations enqueued this run. Counts every call the
    /// runner asked for — including ones rejected for budget — so a tight loop
    /// can't reset the counter by being rejected. Drives the per-run fan-out
    /// budget (lab-4dcil item 3).
    calls_enqueued: u64,
    /// Per-run `callTool` fan-out budget, resolved once from the environment.
    max_calls_per_run: u64,
    /// Host-side byte ceiling on a single `callTool` result, resolved once from
    /// the environment (lab-y966d item 1).
    calltool_result_max_bytes: usize,
}

impl DriveState {
    fn new(artifact_run_id: &str) -> Self {
        let artifact_root = code_mode_artifact_root(artifact_run_id);
        let artifact_max_bytes = super::artifacts::artifact_max_bytes();
        Self {
            calls: Vec::new(),
            artifacts: Vec::new(),
            artifact_store_pruned: false,
            artifact_max_bytes,
            artifact_root,
            snippet_resolves: 0,
            snippet_resolved_bytes: 0,
            calls_enqueued: 0,
            max_calls_per_run: max_calltool_per_run(),
            calltool_result_max_bytes: calltool_result_max_bytes(),
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
    /// Runtime params arrive packed in a [`RunnerConfig`] (built at the single
    /// call site so every field is named) and the loop arms are delegated to
    /// named helpers. Timeout and killpg invariants are preserved exactly.
    pub(in crate::dispatch::gateway::code_mode) async fn run_in_runner(
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
                            // Per-run fan-out budget (lab-4dcil item 3). Count
                            // every requested call so a rejected call still
                            // advances the counter; past the budget, reject this
                            // call with a recoverable `call_budget_exceeded`
                            // (the in-sandbox promise rejects cleanly) instead of
                            // enqueuing another upstream-amplifying future.
                            state.calls_enqueued = state.calls_enqueued.saturating_add(1);
                            if state.calls_enqueued > state.max_calls_per_run {
                                if let Err(err) = reject_tool_call_over_budget(
                                    seq,
                                    id,
                                    state.max_calls_per_run,
                                    stdin,
                                    child,
                                    child_pid,
                                    deadline,
                                    &mut state,
                                )
                                .await
                                {
                                    return DriveOutcome::RunnerUnhealthy(err);
                                }
                            } else {
                                enqueue_tool_call(
                                    self,
                                    seq,
                                    id,
                                    params,
                                    deadline,
                                    cfg,
                                    &mut pending_tool_calls,
                                );
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
                        CodeModeRunnerOutput::SnippetResolve { seq, name, input } => {
                            if let Err(err) = handle_snippet_resolve_event(
                                self,
                                seq,
                                name,
                                input,
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
                    if let Err(err) = handle_completed_tool_call(
                        completed, stdin, child, child_pid, deadline, &mut state,
                    )
                    .await
                    {
                        // Failed to relay a tool result back to the runner (pipe
                        // error or write-deadline expiry) — the runner is killed
                        // on the deadline path; evict so a replacement spawns.
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
/// `broker` with the same lifetime as the enclosing `run_in_runner`
/// rather than being forced to `'static`.
fn enqueue_tool_call<'a>(
    broker: &'a CodeModeBroker<'a>,
    seq: u64,
    id: String,
    params: Value,
    deadline: tokio::time::Instant,
    cfg: &RunnerConfig,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'a>>,
) {
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
}

/// Reject a `callTool` that exceeded the per-run fan-out budget (lab-4dcil item
/// 3) by settling its in-sandbox promise with the recoverable
/// `call_budget_exceeded` kind, without enqueuing the upstream call.
///
/// The rejection is recorded in the call trace (`ok: false`,
/// `error_kind: call_budget_exceeded`) so the budget breach is observable, and a
/// single WARN is emitted on the first over-budget call so operators can see
/// the run hit the cap. Writing the error via the deadline-bounded writer means
/// a deadlocked child still gets killed on the wall-clock, mirroring every other
/// in-loop writeback.
#[allow(clippy::too_many_arguments)]
async fn reject_tool_call_over_budget(
    seq: u64,
    id: String,
    budget: u64,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    // Log once, on the first call past the budget, to avoid flooding logs when a
    // huge fan-out keeps tripping the cap.
    if state.calls_enqueued == budget.saturating_add(1) {
        tracing::warn!(
            surface = "dispatch",
            service = "code_mode",
            action = "codemode",
            kind = "call_budget_exceeded",
            budget,
            "Code Mode run exceeded the per-run callTool fan-out budget; rejecting further calls"
        );
    }
    write_runner_input_by_deadline(
        stdin,
        &CodeModeRunnerInput::ToolError {
            seq,
            kind: "call_budget_exceeded".to_string(),
            message: format!(
                "per-run callTool budget of {budget} exceeded; reduce fan-out or split the work across multiple codemode calls"
            ),
        },
        deadline,
        child,
        child_pid,
        &state.calls,
    )
    .await?;
    state.calls.push((
        seq,
        CodeModeExecutedCall {
            id,
            ok: false,
            elapsed_ms: 0,
            params: None,
            error_kind: Some("call_budget_exceeded".to_string()),
        },
    ));
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
    // Retention prune is a best-effort heuristic ("pruning must never fail a
    // run"), and on a large store it walks up to 200 run dirs (concurrency 8)
    // before the write — a stat-storm on the writeArtifact critical path
    // (lab-y966d item 3). Detach it as a fire-and-forget `tokio::spawn` (drop the
    // JoinHandle) on the first write of the run so the writeArtifact promise
    // settles on the write alone, independent of total store size. Concurrency
    // is safe: this run's dir is registered in `active_runs` via the
    // `ActiveArtifactRun` RAII guard *before* any write, so a detached prune
    // (this run's or another concurrent run's) skips it. The `artifact_store_pruned`
    // flag keeps it to one prune per run even under a rapid burst of writes.
    if !state.artifact_store_pruned {
        state.artifact_store_pruned = true;
        let retain = super::artifacts::artifact_retention_runs();
        // Fire-and-forget: dropping the JoinHandle detaches the task. If the
        // runtime shuts down mid-prune the store is simply pruned on the next
        // run — acceptable for a best-effort retention pass.
        drop(tokio::spawn(async move {
            super::artifacts::prune_artifact_runs(retain).await;
        }));
    }

    // Only the write itself is host-side filesystem work on the critical path;
    // bound it by the run deadline (like tool calls) so a hung or slow disk
    // can't outlive `timeout_ms`.
    let artifact_root = state.artifact_root.clone();
    let artifact_max_bytes = state.artifact_max_bytes;
    let trace_params = cfg.trace_params;
    let artifact_op = handle_artifact_write(
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
    );
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

async fn handle_snippet_resolve_event(
    broker: &CodeModeBroker<'_>,
    seq: u64,
    name: String,
    input: Value,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    cfg: &RunnerConfig,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let op = resolve_snippet_for_runner(broker, &name, input, cfg, state);
    match tokio::time::timeout_at(deadline, op).await {
        Ok(Ok((code, input))) => {
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::SnippetResolved { seq, code, input },
                deadline,
                child,
                child_pid,
                &state.calls,
            )
            .await
        }
        Ok(Err(err)) => {
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind: err.kind().to_string(),
                    message: err.user_message().to_string(),
                },
                deadline,
                child,
                child_pid,
                &state.calls,
            )
            .await?;
            Ok(())
        }
        Err(_) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(code_mode_timeout_error(&state.calls))
        }
    }
}

async fn resolve_snippet_for_runner(
    broker: &CodeModeBroker<'_>,
    name: &str,
    input: Value,
    cfg: &RunnerConfig,
    state: &mut DriveState,
) -> Result<(String, Value), ToolError> {
    if !cfg.caller.can_use_snippets() {
        return Err(ToolError::Forbidden {
            message: "codemode.run requires lab:admin or trusted-local Code Mode".to_string(),
            required_scopes: vec!["lab:admin".to_string()],
        });
    }
    if cfg.capability_filter.is_scoped_to_upstreams() {
        return Err(ToolError::Forbidden {
            message: "codemode.run is not available on route-scoped Code Mode surfaces".to_string(),
            required_scopes: vec!["lab:admin".to_string()],
        });
    }
    if broker.gateway_manager.is_none() {
        return Err(ToolError::Sdk {
            sdk_kind: "gateway_unavailable".to_string(),
            message: "codemode.run requires a live gateway manager".to_string(),
        });
    }
    if state.snippet_resolves >= MAX_SNIPPET_RESOLVES_PER_RUN {
        return Err(ToolError::Sdk {
            sdk_kind: "snippet_resolve_limit".to_string(),
            message: "snippet resolve limit exceeded".to_string(),
        });
    }

    state.snippet_resolves = state.snippet_resolves.saturating_add(1);
    let started = std::time::Instant::now();
    let lab_home = crate::dispatch::helpers::lab_home();
    let builtin_dir = crate::dispatch::snippets::store::builtin_snippet_dir();
    let name = name.to_string();
    let resolved = tokio::task::spawn_blocking(move || {
        let resolved =
            crate::dispatch::snippets::store::resolve_snippet(&lab_home, &builtin_dir, &name)?;
        let input = crate::dispatch::snippets::store::merge_snippet_input(&resolved, input)?;
        let code = crate::dispatch::snippets::store::code_for_snippet(&resolved)?;
        Ok::<_, ToolError>((resolved.name, code, input))
    })
    .await
    .map_err(|err| ToolError::internal_message(format!("snippet resolve task failed: {err}")))??;

    let (name, code, input) = resolved;

    state.snippet_resolved_bytes = state.snippet_resolved_bytes.saturating_add(code.len());
    if state.snippet_resolved_bytes > MAX_SNIPPET_RESOLVED_BYTES_PER_RUN {
        return Err(ToolError::Sdk {
            sdk_kind: "snippet_budget_exceeded".to_string(),
            message: "resolved snippet code budget exceeded".to_string(),
        });
    }
    tracing::info!(
        surface = "dispatch",
        service = "code_mode",
        action = "snippet.resolve",
        snippet = %name,
        elapsed_ms = started.elapsed().as_millis(),
        "Code Mode snippet resolved"
    );
    Ok((code, input))
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
        execution_id: None,
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

/// Write a message back to the runner bounded by the execution deadline.
///
/// `write_runner_input`'s bare `write_all` + `flush` can block indefinitely if
/// the child stops draining its stdin while the parent is mid-write — the classic
/// two-pipe deadlock (child flooding stdout, which the parent isn't reading while
/// it's blocked writing a large `ToolResult` to stdin). The read side of the loop
/// is already guarded by `timeout_at(deadline, lines.next())`; without this, the
/// parent→child writeback path was the one reachable *in-loop* `await` the 30 s
/// wall-clock backstop did not cover (the pre-deadline `Start` write is excluded:
/// it runs before the deadline exists and against a freshly-parked child that
/// cannot yet be flooding stdout), so a deadlocked child could hang the drive
/// loop and leak the pool slot forever. On expiry we kill the child (killpg) so
/// the pooled slot respawns, mirroring the read-timeout path, and surface the
/// stable `timeout` kind — carrying the partial call trace like the other
/// timeout paths. A plain write I/O error (not a timeout) propagates without a
/// trace, matching the pre-existing bare-write behavior.
async fn write_runner_input_by_deadline(
    stdin: &mut ChildStdin,
    input: &CodeModeRunnerInput,
    deadline: tokio::time::Instant,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    calls: &[(u64, CodeModeExecutedCall)],
) -> Result<(), CodeModeExecutionError> {
    match tokio::time::timeout_at(deadline, write_runner_input(stdin, input)).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => {
            terminate_code_mode_runner(child, child_pid).await;
            Err(code_mode_timeout_error(calls))
        }
    }
}

/// Handle a completed tool-call future from `pending_tool_calls`.
async fn handle_completed_tool_call(
    completed: Option<(u64, String, Option<Value>, Result<Value, ToolError>, u128)>,
    stdin: &mut ChildStdin,
    child: &mut tokio::process::Child,
    child_pid: Option<u32>,
    deadline: tokio::time::Instant,
    state: &mut DriveState,
) -> Result<(), CodeModeExecutionError> {
    let Some((seq, id, params, result, elapsed_ms)) = completed else {
        return Ok(());
    };
    match result {
        Ok(result) => {
            // Host-side size guard (lab-y966d item 1): a large binary result
            // (e.g. a multi-MiB Uint8Array from an upstream tool) would OOM the
            // runner's 64-MiB QuickJS heap during decode and surface as an
            // opaque `server_error`. Measure the serialized JSON bytes BEFORE the
            // payload enters the stdin pipe and, when oversized, reject the
            // in-sandbox promise with the recoverable `result_too_large` kind
            // instead. Mirrors the artifact content cap. The base64 wrapper
            // inflates binary ~4/3, so this serialized-byte cap is conservative.
            let serialized_len = serde_json::to_vec(&result).map(|v| v.len()).unwrap_or(0);
            if serialized_len > state.calltool_result_max_bytes {
                let max = state.calltool_result_max_bytes;
                write_runner_input_by_deadline(
                    stdin,
                    &CodeModeRunnerInput::ToolError {
                        seq,
                        kind: "result_too_large".to_string(),
                        message: format!(
                            "callTool result is {serialized_len} bytes; maximum is {max} bytes (use writeArtifact for large payloads)"
                        ),
                    },
                    deadline,
                    child,
                    child_pid,
                    &state.calls,
                )
                .await?;
                state.calls.push((
                    seq,
                    CodeModeExecutedCall {
                        id,
                        ok: false,
                        elapsed_ms,
                        params,
                        error_kind: Some("result_too_large".to_string()),
                    },
                ));
                return Ok(());
            }
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
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::ToolResult { seq, result },
                deadline,
                child,
                child_pid,
                &state.calls,
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
            write_runner_input_by_deadline(
                stdin,
                &CodeModeRunnerInput::ToolError {
                    seq,
                    kind: kind.clone(),
                    message: err.user_message().to_string(),
                },
                deadline,
                child,
                child_pid,
                &state.calls,
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

    fn test_config(timeout: Duration) -> RunnerConfig {
        RunnerConfig {
            code_to_run: "async () => 1".to_string(),
            proxy: String::new(),
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
        let broker = CodeModeBroker::new(None);
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

    /// Read back the single framed line a writeback helper just sent to the
    /// runner. The `cat` stub echoes stdin → stdout, so the next stdout line is
    /// exactly the JSON the helper wrote — deserialize it as a runner input.
    async fn read_echoed_input(runner: &mut PooledRunner) -> CodeModeRunnerInput {
        let line = tokio::time::timeout(Duration::from_secs(5), runner.lines.next())
            .await
            .expect("stub echo within 5s")
            .expect("stub produced a line")
            .expect("stub line decodes");
        serde_json::from_str::<CodeModeRunnerInput>(&line).expect("echoed line is a runner input")
    }

    fn drive_state_with_caps(per_run: u64, result_max_bytes: usize) -> DriveState {
        let mut state = DriveState::new(&Ulid::new().to_string());
        state.max_calls_per_run = per_run;
        state.calltool_result_max_bytes = result_max_bytes;
        state
    }

    /// lab-4dcil item 3: past the per-run fan-out budget, a `callTool` is
    /// rejected with the recoverable `call_budget_exceeded` kind written back to
    /// the runner (the in-sandbox promise rejects cleanly) and recorded as a
    /// failed call — the run is NOT killed.
    #[tokio::test]
    async fn fan_out_budget_rejects_calls_past_cap_with_call_budget_exceeded() {
        let mut runner = PooledRunner::spawn_stub().expect("spawn cat stub");
        // Budget of 2: the 1st and 2nd calls enqueue; the 3rd is over budget.
        let mut state = drive_state_with_caps(2, 8 * 1024 * 1024);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let child_pid = runner.child_pid;
        let PooledRunner {
            child,
            stdin,
            lines,
            ..
        } = &mut runner;

        // Simulate three requested calls. Only the rejection path writes to the
        // runner, so drive the counter exactly as the ToolCall arm does.
        let mut rejected = Vec::new();
        for seq in 1..=3u64 {
            state.calls_enqueued = state.calls_enqueued.saturating_add(1);
            if state.calls_enqueued > state.max_calls_per_run {
                reject_tool_call_over_budget(
                    seq,
                    format!("up::tool{seq}"),
                    state.max_calls_per_run,
                    stdin,
                    child,
                    child_pid,
                    deadline,
                    &mut state,
                )
                .await
                .expect("budget rejection writes cleanly");
                rejected.push(seq);
            }
        }
        // Borrow `lines` separately after the &mut on stdin/child ends.
        let echoed = {
            let line = tokio::time::timeout(Duration::from_secs(5), lines.next())
                .await
                .expect("stub echo within 5s")
                .expect("stub produced a line")
                .expect("stub line decodes");
            serde_json::from_str::<CodeModeRunnerInput>(&line).expect("echoed input")
        };

        assert_eq!(
            rejected,
            vec![3],
            "only the 3rd call (past budget=2) rejects"
        );
        match echoed {
            CodeModeRunnerInput::ToolError { seq, kind, .. } => {
                assert_eq!(seq, 3);
                assert_eq!(kind, "call_budget_exceeded");
            }
            other => panic!("expected ToolError call_budget_exceeded, got {other:?}"),
        }
        // The rejected call is recorded as a failed call in the trace.
        assert_eq!(state.calls.len(), 1);
        let (trace_seq, call) = &state.calls[0];
        assert_eq!(*trace_seq, 3);
        assert!(!call.ok);
        assert_eq!(call.error_kind.as_deref(), Some("call_budget_exceeded"));
    }

    /// lab-y966d item 1: an oversized binary `callTool` RESULT is rejected with
    /// the recoverable `result_too_large` kind BEFORE the payload enters the
    /// runner stdin pipe, rather than reaching the runner and OOMing the heap.
    #[tokio::test]
    async fn oversized_tool_result_is_rejected_as_result_too_large() {
        let mut runner = PooledRunner::spawn_stub().expect("spawn cat stub");
        // Tiny result cap so a small JSON payload trips the guard.
        let mut state = drive_state_with_caps(512, 16);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let child_pid = runner.child_pid;
        let PooledRunner { child, stdin, .. } = &mut runner;

        // A result whose serialized JSON exceeds the 16-byte cap.
        let big = json!({ "data": "xxxxxxxxxxxxxxxxxxxxxxxxxxxx" });
        let completed = Some((7u64, "up::tool".to_string(), None, Ok(big), 3u128));
        handle_completed_tool_call(completed, stdin, child, child_pid, deadline, &mut state)
            .await
            .expect("oversized result rejected cleanly, run not killed");

        let echoed = read_echoed_input(&mut runner).await;
        match echoed {
            CodeModeRunnerInput::ToolError { seq, kind, .. } => {
                assert_eq!(seq, 7);
                assert_eq!(kind, "result_too_large");
            }
            other => panic!("expected ToolError result_too_large, got {other:?}"),
        }
        let (_, call) = &state.calls[0];
        assert!(!call.ok);
        assert_eq!(call.error_kind.as_deref(), Some("result_too_large"));
    }

    /// A within-cap result is relayed as a normal `ToolResult` (the guard does
    /// not reject ordinary payloads).
    #[tokio::test]
    async fn within_cap_tool_result_is_relayed_as_tool_result() {
        let mut runner = PooledRunner::spawn_stub().expect("spawn cat stub");
        let mut state = drive_state_with_caps(512, 8 * 1024 * 1024);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let child_pid = runner.child_pid;
        let PooledRunner { child, stdin, .. } = &mut runner;

        let small = json!({ "ok": true });
        let completed = Some((9u64, "up::tool".to_string(), None, Ok(small.clone()), 1u128));
        handle_completed_tool_call(completed, stdin, child, child_pid, deadline, &mut state)
            .await
            .expect("within-cap result relays cleanly");

        let echoed = read_echoed_input(&mut runner).await;
        match echoed {
            CodeModeRunnerInput::ToolResult { seq, result } => {
                assert_eq!(seq, 9);
                assert_eq!(result, small);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        let (_, call) = &state.calls[0];
        assert!(call.ok);
        assert_eq!(call.error_kind, None);
    }

    /// The per-run fan-out budget env override parses and hard-clamps to the
    /// ceiling; invalid/zero values fall back to the default.
    #[test]
    fn max_calltool_per_run_parses_and_clamps() {
        use crate::dispatch::helpers::with_env_override;
        use std::collections::HashMap;

        let parsed = with_env_override(
            HashMap::from([(
                "LAB_CODE_MODE_MAX_CALLS_PER_RUN".to_string(),
                "16".to_string(),
            )]),
            max_calltool_per_run,
        );
        assert_eq!(parsed, 16);

        let clamped = with_env_override(
            HashMap::from([(
                "LAB_CODE_MODE_MAX_CALLS_PER_RUN".to_string(),
                "999999".to_string(),
            )]),
            max_calltool_per_run,
        );
        assert_eq!(clamped, MAX_CALLTOOL_PER_RUN_CEILING);

        for bad in ["0", "nope", "-5"] {
            let fallback = with_env_override(
                HashMap::from([(
                    "LAB_CODE_MODE_MAX_CALLS_PER_RUN".to_string(),
                    bad.to_string(),
                )]),
                max_calltool_per_run,
            );
            assert_eq!(fallback, DEFAULT_MAX_CALLTOOL_PER_RUN, "bad value `{bad}`");
        }
    }
}
