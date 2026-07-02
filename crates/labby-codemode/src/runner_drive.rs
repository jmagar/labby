//! `CodeModeBroker::run_in_runner`: spawn the runner subprocess and drive the
//! tool-call/log/completion protocol loop.
//!
//! The public entry point is `run_in_runner`, which packs runtime parameters
//! into a `RunnerConfig` struct and delegates to `run_in_runner_with_config`.
//! Each major event arm (`Done`, `ToolCall`, `ArtifactWrite`,
//! `SnippetResolve`, `Error`) is handled by a named async helper to keep the
//! select loop readable.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use futures::{StreamExt, stream::FuturesUnordered};
use serde_json::{Value, json};
use tokio::process::ChildStdin;
use tokio::sync::Mutex;
use ulid::Ulid;

use crate::error::ToolError;
use crate::git::provider::dispatch_git_method;
use crate::host::CodeModeHost;
use crate::local_provider::{LocalProviderCall, LocalProviderName};
use crate::state::provider::dispatch_state_method;
use crate::state::quota::StateWorkspaceLimits;
use crate::state::workspace::StateWorkspace;

use super::CodeModeBroker;
use super::artifacts::{
    ActiveArtifactRun, CodeModeArtifactReceipt, CodeModeArtifactWrite, code_mode_artifact_root,
    write_code_mode_artifact,
};
use super::config::{
    MAX_SNIPPET_RESOLVED_BYTES_PER_RUN, MAX_SNIPPET_RESOLVES_PER_RUN, calltool_result_max_bytes,
    max_calltool_per_run,
};
use super::pool::RunnerPool;
use super::pool::runner_handle::PooledRunner;
use super::protocol::{CodeModeRunnerInput, CodeModeRunnerOutput};
use super::runner_io::{terminate_code_mode_runner, write_runner_input};
use super::truncate::apply_log_caps;
use super::types::{
    CodeModeCaller, CodeModeExecutedCall, CodeModeExecutionError, CodeModeExecutionResponse,
    CodeModeSurface, ToolScope,
};

static LOCAL_PROVIDER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
pub(crate) struct RunnerConfig {
    pub code_to_run: String,
    pub proxy: String,
    pub timeout: Duration,
    pub caller: CodeModeCaller,
    pub surface: CodeModeSurface,
    pub max_log_entries: usize,
    pub max_log_bytes: usize,
    pub trace_params: bool,
    pub capability_filter: ToolScope,
}

// ---------------------------------------------------------------------------
// Drive state — per-run mutable bookkeeping (excludes pending_tool_calls,
// which stays local in run_in_runner_with_config so its lifetime is tied to
// the enclosing async fn and not forced to 'static)
// ---------------------------------------------------------------------------

struct DriveState {
    calls: Vec<(u64, CodeModeExecutedCall)>,
    artifacts: Vec<CodeModeArtifactReceipt>,
    artifact_store_pruned: bool,
    artifact_max_bytes: usize,
    artifact_root: PathBuf,
    snippet_resolves: usize,
    snippet_resolved_bytes: usize,
    calls_enqueued: u64,
    max_calls_per_run: u64,
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

impl<H: CodeModeHost> CodeModeBroker<'_, H> {
    /// Spawn the runner subprocess, send the code, and drive the
    /// tool-call/artifact/completion protocol loop until the runner exits
    /// or the wall-clock deadline fires.
    ///
    /// The runtime params are packed into [`RunnerConfig`] and the loop arms
    /// are delegated to named helpers. Timeout and killpg invariants are
    /// preserved exactly.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_in_runner(
        &self,
        code_to_run: String,
        proxy: String,
        timeout: Duration,
        caller: CodeModeCaller,
        surface: CodeModeSurface,
        max_log_entries: usize,
        max_log_bytes: usize,
        trace_params: bool,
        capability_filter: ToolScope,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let cfg = RunnerConfig {
            code_to_run,
            proxy,
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
        // Acquire a runner. With a host, use the shared warm pool (Perf H1): a
        // pooled runner amortizes the fork/startup cost across executions while
        // still building a fresh `javy::Runtime` per `Start` (runner-side), so JS
        // state isolation holds. Without a host (some tests / standalone paths),
        // spawn a one-shot runner directly.
        match self.host {
            Some(host) => self.run_via_pool(host.runner_pool(), cfg).await,
            None => self.run_standalone(cfg).await,
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
        cfg: RunnerConfig,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let mut lease = pool.checkout().await?;
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
        cfg: RunnerConfig,
    ) -> Result<CodeModeExecutionResponse, CodeModeExecutionError> {
        let spawn = super::pool::RunnerSpawn::try_default()?;
        let mut runner = PooledRunner::spawn(&spawn)?;
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
                            // Reserved host-internal pseudo-tool calls (see
                            // `execute.rs`'s `LAB_INTERNAL_NAMESPACE`) are
                            // exempt from the per-run call budget and the
                            // call trace — but NOT from dispatch routing:
                            // they still flow through the normal enqueue
                            // path below so their promise settles normally.
                            let is_internal = id.starts_with("__lab_internal::");
                            if !is_internal {
                                state.calls_enqueued = state.calls_enqueued.saturating_add(1);
                            }
                            if !is_internal && state.calls_enqueued > state.max_calls_per_run {
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
                                match crate::local_provider::try_parse_local_provider_call(&id) {
                                    Ok(Some(local)) => {
                                        enqueue_local_provider_call(
                                            seq,
                                            id,
                                            local,
                                            params,
                                            cfg,
                                            &mut pending_tool_calls,
                                        );
                                    }
                                    Ok(None) => {
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
                                    Err(err) => {
                                        enqueue_rejected_tool_call(
                                            seq,
                                            id,
                                            params,
                                            err,
                                            cfg,
                                            &mut pending_tool_calls,
                                        );
                                    }
                                }
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
                                .map(|line| super::truncate::sanitize_log_text(&line, 4096))
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
/// `broker` with the same lifetime as the enclosing `run_in_runner_with_config`
/// rather than being forced to `'static`.
fn enqueue_tool_call<'a, H: CodeModeHost>(
    broker: &'a CodeModeBroker<'a, H>,
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

fn enqueue_local_provider_call(
    seq: u64,
    id: String,
    local: LocalProviderCall,
    params: Value,
    cfg: &RunnerConfig,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'_>>,
) {
    let redacted_params = super::trace::redact_trace_params(&params, cfg.trace_params);
    let caller = cfg.caller.clone();
    let capability_filter = cfg.capability_filter.clone();
    pending_tool_calls.push(Box::pin(async move {
        let call_start = std::time::Instant::now();
        let _guard = LOCAL_PROVIDER_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .await;
        let result = if super::execute::local_providers_allowed(&caller, &capability_filter) {
            dispatch_local_provider_stub(local, params).await
        } else {
            Err(ToolError::Forbidden {
                message: "local Code Mode providers require unscoped lab:admin".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            })
        };
        let elapsed_ms = call_start.elapsed().as_millis();
        (seq, id, redacted_params, result, elapsed_ms)
    }));
}

fn enqueue_rejected_tool_call(
    seq: u64,
    id: String,
    params: Value,
    err: ToolError,
    cfg: &RunnerConfig,
    pending_tool_calls: &mut FuturesUnordered<ToolCallFut<'_>>,
) {
    let redacted_params = super::trace::redact_trace_params(&params, cfg.trace_params);
    pending_tool_calls.push(Box::pin(
        async move { (seq, id, redacted_params, Err(err), 0) },
    ));
}

async fn dispatch_local_provider_stub(
    local: LocalProviderCall,
    params: Value,
) -> Result<Value, ToolError> {
    drop(local.params);
    let provider_name = local.provider.as_str();
    match local.provider {
        LocalProviderName::State => {
            let workspace_root = labby_runtime::lab_home()
                .join("code-mode-workspaces")
                .join("default");
            let workspace = StateWorkspace::new(workspace_root, StateWorkspaceLimits::default())?;
            dispatch_state_method(&workspace, &local.method, params).await
        }
        LocalProviderName::Git => {
            let workspace_root = labby_runtime::lab_home()
                .join("code-mode-workspaces")
                .join("default");
            let workspace = StateWorkspace::new(workspace_root, StateWorkspaceLimits::default())?;
            let _ = provider_name;
            dispatch_git_method(&workspace, &local.method, params).await
        }
    }
}

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

async fn handle_snippet_resolve_event<H: CodeModeHost>(
    broker: &CodeModeBroker<'_, H>,
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

async fn resolve_snippet_for_runner<H: CodeModeHost>(
    broker: &CodeModeBroker<'_, H>,
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
    if cfg.capability_filter.is_scoped() {
        return Err(ToolError::Forbidden {
            message: "codemode.run is not available on route-scoped Code Mode surfaces".to_string(),
            required_scopes: vec!["lab:admin".to_string()],
        });
    }
    let Some(host) = broker.host else {
        return Err(ToolError::Sdk {
            sdk_kind: "tool_source_unavailable".to_string(),
            message: "codemode.run requires a live tool source".to_string(),
        });
    };
    if state.snippet_resolves >= MAX_SNIPPET_RESOLVES_PER_RUN {
        return Err(ToolError::Sdk {
            sdk_kind: "snippet_resolve_limit".to_string(),
            message: "snippet resolve limit exceeded".to_string(),
        });
    }

    state.snippet_resolves = state.snippet_resolves.saturating_add(1);
    let started = std::time::Instant::now();
    let resolved = host.resolve_snippet(name, input).await?;
    let (name, code, input) = (resolved.name, resolved.code, resolved.input);

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
        result_shaping: None,
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
    // Reserved host-internal pseudo-tool calls never appear in the call
    // trace (`state.calls`) — but their ToolResult/ToolError responses are
    // still written back unconditionally so the sandbox's `callTool(...)`
    // promise settles normally.
    let is_internal = id.starts_with("__lab_internal::");
    match result {
        Ok(result) => {
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
                if !is_internal {
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
                }
                return Ok(());
            }
            if !is_internal {
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
            }
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
            if !is_internal {
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
    #![allow(clippy::panic)]
    use super::*;
    use crate::host::NoopHost;

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
            capability_filter: ToolScope::default(),
        }
    }

    /// Budget/trace exclusion for reserved `__lab_internal::` pseudo-tool
    /// calls: an internal call interleaved with exactly `max_calls_per_run`
    /// ordinary calls must (a) never appear in the call trace and (b) never
    /// consume a budget slot — if it did, the last ordinary call would be
    /// rejected with `call_budget_exceeded`.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn drive_runner_excludes_lab_internal_calls_from_budget_and_trace() {
        let budget = crate::config::max_calltool_per_run();
        // The stub emits 1 internal ToolCall + `budget` ordinary ToolCalls,
        // waits for the host to settle them (a background `cat` drains stdin
        // so ToolResult/ToolError writes back to the stub never block), then
        // emits Done.
        let script = format!(
            r#"
exec 3<&0
cat <&3 >/dev/null &
printf '{{"type":"tool_call","seq":1,"id":"__lab_internal::semantic_rank","params":{{"query":"q","limit":5}}}}\n'
i=2
while [ "$i" -le {last_seq} ]; do
  printf '{{"type":"tool_call","seq":%d,"id":"stub::tool","params":{{}}}}\n' "$i"
  i=$((i+1))
done
sleep 2
printf '{{"type":"done"}}\n'
sleep 3600
"#,
            last_seq = budget + 1
        );
        let host = NoopHost::default();
        let broker = CodeModeBroker::new(Some(&host));
        let mut runner = PooledRunner::spawn_stub_script(&script).expect("spawn script stub");
        let outcome = broker
            .drive_runner(&mut runner, &test_config(Duration::from_secs(30)))
            .await;
        let response = match outcome {
            DriveOutcome::Completed(response) => response,
            DriveOutcome::ExecutionError(err) | DriveOutcome::RunnerUnhealthy(err) => {
                panic!("run must complete, got error kind `{}`", err.kind())
            }
        };
        assert!(
            response
                .calls
                .iter()
                .all(|call| !call.id.starts_with("__lab_internal::")),
            "internal calls must not appear in the call trace"
        );
        assert_eq!(
            response.calls.len(),
            usize::try_from(budget).expect("budget fits usize"),
            "every ordinary call must be traced"
        );
        assert!(
            response
                .calls
                .iter()
                .all(|call| call.error_kind.as_deref() != Some("call_budget_exceeded")),
            "the internal call must not consume a budget slot"
        );
    }

    /// The wall-clock deadline path: a runner that never replies is killed when
    /// the deadline fires, the run surfaces the stable `timeout` kind, and the
    /// runner is classified `RunnerUnhealthy` so the pool evicts (never reuses) a
    /// runtime interrupted mid-execution.
    #[tokio::test]
    async fn drive_runner_times_out_and_marks_runner_unhealthy() {
        let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
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
