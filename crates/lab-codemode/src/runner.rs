//! Code Mode runner subprocess entry point (`internal code-mode-runner`):
//! the in-process Javy/QuickJS stdio loop.

use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::process::ExitCode;

use serde_json::Value;

use super::protocol::CODE_MODE_STACK_SIZE_LIMIT;
use super::protocol::{
    CodeModeRunnerInput, CodeModeRunnerOutput, CodeModeRunnerResult, CodeModeRunnerState,
    RUNNER_STATE,
};
use super::wrapper::{CODE_MODE_VALUE_CODEC_JS, code_mode_main_invoker};

pub fn run_code_mode_runner_stdio() -> ExitCode {
    // Security: prevent /proc/<pid>/environ readback of the runner process.
    // Must be the very first act — do this before any state is initialized.
    #[cfg(all(unix, target_os = "linux"))]
    {
        use nix::sys::prctl;
        if prctl::set_dumpable(false).is_err() {
            // Non-fatal — execution continues but /proc/<pid>/environ may be readable.
            // This runs inside the runner SUBPROCESS, which has no tracing
            // subscriber installed; tracing::warn! here would be dropped. The
            // parent drains this child's stderr into the response logs, so
            // eprintln! is the channel that actually surfaces the warning.
            #[allow(clippy::print_stderr)]
            {
                eprintln!(
                    "WARNING: prctl(PR_SET_DUMPABLE, 0) failed; runner environment may be readable via /proc"
                );
            }
        }
    }

    RUNNER_STATE.with(|state| {
        *state.borrow_mut() = Some(CodeModeRunnerState {
            reader: BufReader::new(io::stdin()),
            writer: BufWriter::new(io::stdout()),
            next_seq: 0,
        });
    });

    // Warm-runner pool (Perf H1): the runner process is long-lived and serves
    // one execution per `Start` message, building a FRESH `javy::Runtime` per
    // execution so no JS state (globals, `__labPendingToolCalls`, captured data)
    // can leak across callers. After each execution the process parks on the
    // next `read_line`. The parent pools these processes to amortize the fork
    // cost. EOF on the input (parent closed stdin / dropped the handle) ends the
    // loop with a clean exit. A genuine per-execution failure is reported as an
    // `Error` line; the runner then continues to the next `Start` so a single bad
    // snippet does not poison a pooled process (the parent decides whether to
    // recycle it).
    loop {
        match run_code_mode_runner() {
            Ok(RunnerLoopOutcome::Completed) => {
                // Reset per-execution state and park for the next Start.
                reset_runner_seq();
            }
            Ok(RunnerLoopOutcome::InputClosed) => {
                // Parent closed the pipe; shut the process down cleanly.
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                drop(runner_emit(CodeModeRunnerOutput::Error {
                    kind: err.kind,
                    message: err.message,
                }));
                // Reset and continue: the per-execution javy runtime is dropped
                // at the end of `run_code_mode_runner`, so a failed execution
                // leaves no JS state behind. Whether to reuse or recycle this
                // process is the parent pool's decision.
                reset_runner_seq();
            }
        }
    }
}

/// Why the per-execution loop body returned.
enum RunnerLoopOutcome {
    /// An execution ran to a `Done` and the runner is ready for the next Start.
    Completed,
    /// The input stream reached EOF before a Start arrived; the process should
    /// exit cleanly (the parent dropped this pooled runner).
    InputClosed,
}

/// Reset the per-execution sequence counter so the next pooled execution starts
/// from `seq = 0`, matching the spawn-per-execution contract. The javy runtime
/// (and all JS globals) is constructed fresh inside `run_code_mode_runner`, so
/// this is the only thread-local carried across executions that needs clearing.
fn reset_runner_seq() {
    RUNNER_STATE.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.next_seq = 0;
        }
    });
}

thread_local! {
    /// Stable base directory the per-execution jails live under — the runner's
    /// spawn cwd (the per-runner `TempDir` the parent set). Captured lazily on
    /// the first execution so each new jail is anchored here, never nested inside
    /// the previous execution's jail.
    static JAIL_BASE: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
    /// The current per-execution jail subdir, so the next execution can remove
    /// it before creating a fresh one. `None` until the first execution.
    static EXECUTION_JAIL: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Create a fresh empty per-execution working directory and `chdir` into it,
/// removing the previous execution's directory first. Best-effort: on any
/// failure the process is left in a still-valid isolated cwd — the prior jail if
/// it was never touched, otherwise the stable spawn base (the per-runner
/// `TempDir`), since the prior jail is removed up front. See the call site for
/// why this is defense-in-depth rather than a hard containment boundary.
fn reset_execution_jail() {
    // Resolve (and remember) the stable base = the spawn cwd. The first call
    // captures it before we ever chdir into a subdir, so subsequent jails are
    // siblings, not nested.
    let base = JAIL_BASE.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.is_none() {
            *cell = std::env::current_dir().ok();
        }
        cell.clone()
    });
    let Some(base) = base else {
        return;
    };

    EXECUTION_JAIL.with(|cell| {
        let mut cell = cell.borrow_mut();
        // Remove the previous execution's jail (if any) so no file state from a
        // prior caller survives on this pooled process.
        if let Some(previous) = cell.take() {
            drop(std::fs::remove_dir_all(&previous));
        }
        let unique = format!("exec-{}-{}", std::process::id(), next_jail_seq());
        let jail = base.join(unique);
        if std::fs::create_dir(&jail).is_err() {
            // We already removed the previous jail above, so the process must not
            // be left `chdir`'d inside a now-deleted directory. Fall back to the
            // stable base (the per-runner spawn TempDir) so cwd stays valid and
            // isolated. `*cell` is already `None` (taken above).
            drop(std::env::set_current_dir(&base));
            return;
        }
        if std::env::set_current_dir(&jail).is_ok() {
            *cell = Some(jail);
        } else {
            // Could not enter the jail; clean it up and fall back to the stable
            // base rather than the just-removed previous jail.
            drop(std::fs::remove_dir_all(&jail));
            drop(std::env::set_current_dir(&base));
        }
    });
}

fn next_jail_seq() -> u64 {
    thread_local! {
        static JAIL_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    JAIL_SEQ.with(|seq| {
        let next = seq.get();
        seq.set(next.saturating_add(1));
        next
    })
}

/// Runner failure with an explicit error kind so the contract distinguishes a
/// caller mistake (`invalid_param`: malformed JS that fails to parse/eval, or a
/// non-JSON-serializable result) from a genuine backend fault (`server_error`).
///
/// `From<String>` defaults to `server_error`, so every existing
/// `map_err(|e| e.to_string())?` site keeps the previous behavior. The eval site
/// and the main-promise rejection classifier override the kind explicitly.
struct CodeModeRunnerError {
    kind: String,
    message: String,
}

impl From<String> for CodeModeRunnerError {
    fn from(message: String) -> Self {
        Self {
            kind: "server_error".to_string(),
            message,
        }
    }
}

/// Extract a structured `{kind,message}` payload embedded in a rejection message
/// and return its `kind`, scanning the ENTIRE message rather than the first line.
///
/// The `__labSettleToolCall` bridge rejects a failed `callTool` with an `Error`
/// whose `.message` is the *pure JSON* `{kind,message}` of the tool-call error.
/// That pure-JSON shape is a load-bearing contract: caller JS recovers the
/// structured error via `JSON.parse(e.message)` (see the runner integration
/// tests), so the bridge must NOT wrap the message in markers or prose. QuickJS
/// then surfaces an uncaught rejection to the host as `Error: <message>`,
/// optionally followed by a `\n    at ...` stack trace. Rather than depend on
/// that exact prefix/first-line shape, locate the embedded JSON object (first
/// `{` to last `}`) and parse it: `JSON.stringify` escapes any newline inside
/// `message`, so the object stays single-line and a multi-line tool message no
/// longer perturbs recovery, and QuickJS stack frames carry no braces so a
/// trailing stack is ignored. A non-JSON span (e.g. `Error: x is not a
/// function`) fails the parse and falls through to the generic classification.
fn extract_structured_kind(message: &str) -> Option<String> {
    let start = message.find('{')?;
    let end = message.rfind('}')?;
    if end < start {
        return None;
    }
    let json_candidate = &message[start..=end];
    let Value::Object(map) = serde_json::from_str::<Value>(json_candidate).ok()? else {
        return None;
    };
    map.get("kind").and_then(Value::as_str).map(str::to_string)
}

/// Classify a main-promise rejection message into an error kind:
/// 1. If the message carries an embedded structured `{kind,message}` JSON object,
///    preserve that kind (structured tool-error rejections re-raised through the
///    sandbox). See `extract_structured_kind`.
/// 2. Else if it mentions `JSON-serializable`, the result could not be
///    serialized — a caller mistake → `invalid_param`.
/// 3. Otherwise it is a runtime throw (e.g. the non-function TypeError) →
///    `server_error`.
///
/// Note: a caller can deliberately set its own execution's error kind by throwing
/// a structured `{kind,message}` Error (intentional, see the
/// `..._preserves_kind_from_uncaught_structured_rejection` integration test). The
/// extracted kind is the caller's OWN result, not a cross-trust signal, so this
/// is by design rather than a forgery boundary.
fn classify_rejection(message: String) -> CodeModeRunnerError {
    if let Some(kind) = extract_structured_kind(&message) {
        return CodeModeRunnerError { kind, message };
    }
    if message.contains("JSON-serializable") {
        return CodeModeRunnerError {
            kind: "invalid_param".to_string(),
            message,
        };
    }
    let message = add_code_mode_hint("server_error", &message);
    CodeModeRunnerError {
        kind: "server_error".to_string(),
        message,
    }
}

fn add_code_mode_hint(kind: &str, message: &str) -> String {
    let mut hints = Vec::new();
    if kind == "ReferenceError"
        || message.contains(" is not defined")
        || message.contains("not defined")
    {
        hints.push(
            "Available globals: codemode, codemode.run, codemode.search, codemode.describe, codemode.step, callTool, writeArtifact. Node/Deno globals such as require, process, fs, fetch, and Bun are not available in the sandbox.",
        );
    }
    if (message.contains(" is not a function") || message.contains("not a function"))
        && message.contains("codemode")
    {
        hints.push(
            "Use await codemode.search(\"...\") or await codemode.describe(\"...\") to find the exact helper name.",
        );
    }
    if hints.is_empty() {
        message.to_string()
    } else {
        format!("{message}\n\nHint: {}", hints.join(" "))
    }
}

fn run_code_mode_runner() -> Result<RunnerLoopOutcome, CodeModeRunnerError> {
    // Read the next Start. EOF here is the normal pool-shutdown path (the parent
    // dropped this runner), NOT an error — return InputClosed so the caller can
    // exit cleanly without emitting a spurious `Error` line.
    let input = match runner_read_input() {
        Ok(input) => input,
        Err(RunnerReadError::InputClosed) => return Ok(RunnerLoopOutcome::InputClosed),
        Err(RunnerReadError::Other(message)) => return Err(message.into()),
    };
    let CodeModeRunnerInput::Start { code, proxy } = input else {
        return Err("runner expected start message".to_string().into());
    };

    // Per-execution cwd jail (Perf H1 isolation): a pooled runner is long-lived,
    // so its process cwd must not accumulate state across executions. Create a
    // fresh empty subdir under the runner's spawn cwd and chdir into it, after
    // removing the previous execution's subdir. The JS sandbox exposes no fs
    // APIs, so this is defense-in-depth — it guarantees that even a future
    // host-side artifact path bug cannot let one execution observe a prior one's
    // working-directory contents on the same pooled process. Failure is
    // non-fatal: the spawn cwd is already an isolated TempDir.
    reset_execution_jail();

    let mut config = javy::Config::default();
    config
        .redirect_stdout_to_stderr(true)
        .memory_limit(64 * 1024 * 1024)
        .max_stack_size(CODE_MODE_STACK_SIZE_LIMIT);
    let runtime = javy::Runtime::new(config).map_err(|err| err.to_string())?;

    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<()> {
            let globals = cx.globals();
            globals.set(
                "__labEmitToolCall",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_tool_call(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            globals.set(
                "__labEmitArtifactWrite",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_artifact_write(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            globals.set(
                "__labEmitSnippetResolve",
                javy::quickjs::Function::new(
                    cx.clone(),
                    javy::quickjs::prelude::MutFn::new(|cx, args| {
                        javy_emit_snippet_resolve(javy::Args::hold(cx, args))
                    }),
                )?,
            )?;
            Ok(())
        })
        .map_err(javy_error_message)?;

    let wrapped = wrap_code_mode(&code, &proxy);

    // A failure here is a parse/eval error in the caller's code (e.g. the
    // malformed `async () => {`), before the main promise is ever created. That
    // is a caller mistake, not a backend fault → `invalid_param`. (A non-function
    // body like `42` evals fine; its TypeError surfaces later as a promise
    // rejection and is classified by `classify_rejection`.)
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(wrapped))
        .map_err(|err| CodeModeRunnerError {
            kind: "invalid_param".to_string(),
            message: javy_error_message(err),
        })?;

    // Run the event loop until the main promise settles.
    let resolved_result = loop {
        runtime
            .resolve_pending_jobs()
            .map_err(|err| err.to_string())?;
        match javy_main_promise_state(&runtime)? {
            JavyMainPromiseState::Resolved(result) => break result,
            JavyMainPromiseState::Rejected(message) => {
                return Err(classify_rejection(message));
            }
            JavyMainPromiseState::Pending => {
                // Mid-execution EOF means the parent died while a tool call was
                // in flight — a genuine fault, not the clean pool-shutdown path.
                let input = runner_read_input().map_err(RunnerReadError::into_runner_error)?;
                javy_settle_tool_promise(&runtime, &input)?;
            }
        }
    };

    runner_emit(CodeModeRunnerOutput::Done {
        result: CodeModeRunnerResult::from_response_result(resolved_result),
        logs: Vec::new(),
    })
    .map_err(CodeModeRunnerError::from)?;
    Ok(RunnerLoopOutcome::Completed)
}

fn wrap_code_mode(code: &str, proxy: &str) -> String {
    // The execute wrapper body (assign → typeof check → invoke) is shared with
    // the Boa path via `code_mode_main_invoker` so the contract cannot diverge.
    // It is interpolated as a named arg (`{invoker}`) so its literal JS braces
    // are substituted verbatim and need no `{{`/`}}` escaping.
    let invoker = code_mode_main_invoker(code);
    format!(
        r#"
globalThis.__labPendingToolCalls = new Map();
globalThis.__labSnippetStack = [];
globalThis.__labSnippetResolveCount = 0;
globalThis.__labSnippetResolvedBytes = 0;
globalThis.__labSnippetMaxDepth = 8;
globalThis.__labSnippetMaxResolves = 32;
globalThis.__labSnippetMaxBytes = 262144;
{codec}
globalThis.callTool = (id, params = {{}}) => {{
  if (typeof id !== "string" || id.trim() === "") {{
    throw new TypeError("callTool id must be a non-empty string");
  }}
  if (params === null || typeof params !== "object" || Array.isArray(params)) {{
    throw new TypeError("callTool params must be a JSON object");
  }}
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitToolCall(id, __labEncodeResult(params));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "tool", resolve, reject }});
  }});
}};
globalThis.writeArtifact = (path, content, options = {{}}) => {{
  if (typeof path !== "string" || path.trim() === "") {{
    throw new TypeError("writeArtifact path must be a non-empty string");
  }}
  if (typeof content !== "string") {{
    throw new TypeError("writeArtifact content must be a string");
  }}
  if (options === null || typeof options !== "object" || Array.isArray(options)) {{
    throw new TypeError("writeArtifact options must be a JSON object");
  }}
  if (options.contentType !== undefined && typeof options.contentType !== "string") {{
    throw new TypeError("writeArtifact options.contentType must be a string");
  }}
  const contentType = options.contentType ?? null;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitArtifactWrite(path, content, contentType);
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "artifact", resolve, reject }});
  }});
}};
globalThis.__labRunSnippet = (name, input = {{}}) => {{
  if (typeof name !== "string" || name.trim() === "") {{
    return Promise.reject(new Error(JSON.stringify({{kind: "bad_snippet_name", message: "codemode.run name must be a non-empty string"}})));
  }}
  if (input === null || typeof input !== "object" || Array.isArray(input)) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "invalid_param", message: "codemode.run input must be a JSON object"}})));
  }}
  if (globalThis.__labSnippetStack.indexOf(name) !== -1) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "snippet_recursion_limit", message: "snippet recursion detected for `" + name + "`"}})));
  }}
  if (globalThis.__labSnippetStack.length >= globalThis.__labSnippetMaxDepth) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "snippet_depth_exceeded", message: "snippet depth limit exceeded"}})));
  }}
  if (globalThis.__labSnippetResolveCount >= globalThis.__labSnippetMaxResolves) {{
    return Promise.reject(new Error(JSON.stringify({{kind: "snippet_resolve_limit", message: "snippet resolve limit exceeded"}})));
  }}
  globalThis.__labSnippetResolveCount++;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitSnippetResolve(name, __labEncodeResult(input));
    globalThis.__labPendingToolCalls.set(seq, {{ kind: "snippet", name, resolve, reject }});
  }});
}};
globalThis.__labSettlePendingOperation = (message) => {{
  const input = JSON.parse(message);
  const pending = globalThis.__labPendingToolCalls.get(input.seq);
  if (!pending) {{
    throw new Error("runner received a response for an unknown pending operation");
  }}
  globalThis.__labPendingToolCalls.delete(input.seq);
  if (input.type === "tool_result") {{
    pending.resolve(__labDecodeResult(input.result));
    return;
  }}
  if (input.type === "snippet_resolved") {{
    if (pending.kind !== "snippet") {{
      throw new Error("runner received snippet code for a non-snippet operation");
    }}
    if (typeof input.code !== "string") {{
      pending.reject(new Error(JSON.stringify({{kind: "invalid_snippet_resolution", message: "resolved snippet code must be a string"}})));
      return;
    }}
    globalThis.__labSnippetResolvedBytes += input.code.length;
    if (globalThis.__labSnippetResolvedBytes > globalThis.__labSnippetMaxBytes) {{
      pending.reject(new Error(JSON.stringify({{kind: "snippet_budget_exceeded", message: "resolved snippet code budget exceeded"}})));
      return;
    }}
    Promise.resolve().then(async () => {{
      globalThis.__labSnippetStack.push(pending.name);
      try {{
        return await (eval("(" + input.code + ")"))(__labDecodeResult(input.input));
      }} finally {{
        globalThis.__labSnippetStack.pop();
      }}
    }}).then(pending.resolve, pending.reject);
    return;
  }}
  if (input.type === "tool_error") {{
    // Reject with an Error whose message is the *pure JSON* {{kind,message}} of
    // the tool-call error. This is a load-bearing contract: caller JS recovers the
    // structured error via `JSON.parse(e.message)` (see the runner integration
    // tests), so the message must stay valid JSON — do NOT wrap it in markers or
    // prose. The host preserves `kind` by extracting the embedded JSON object from
    // the rejection (see classify_rejection / extract_structured_kind), which is
    // robust to QuickJS's `Error: ` prefix and any appended stack trace because
    // JSON.stringify escapes newlines and stack frames carry no braces.
    pending.reject(new Error(JSON.stringify({{kind: input.kind, message: input.message}})));
    return;
  }}
  throw new Error("runner received unexpected protocol message");
}};
globalThis.__labSettleToolCall = globalThis.__labSettlePendingOperation;
{proxy}
globalThis.__labMainPromise = (async () => {{
{invoker}}})();
"#,
        codec = CODE_MODE_VALUE_CODEC_JS,
        invoker = invoker,
        proxy = proxy,
    )
}

enum JavyMainPromiseState {
    Pending,
    /// The async function returned. `result` is the JSON-serialized return value,
    /// or None when the function returned undefined.
    Resolved(Option<Value>),
    Rejected(String),
}

fn javy_emit_tool_call(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let id_value = args
        .0
        .first()
        .ok_or_else(|| javy_type_error(cx.clone(), "callTool id must be a non-empty string"))?;
    let id = javy::val_to_string(&cx, id_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if id.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "callTool id must be a non-empty string",
        ));
    }

    let params_json = args
        .0
        .get(1)
        .map(|params| cx.json_stringify(params.clone()))
        .transpose()?
        .flatten()
        .map(|params| params.to_string())
        .transpose()?
        .unwrap_or_else(|| "{}".to_string());
    let params: Value = serde_json::from_str(&params_json).map_err(|err| {
        javy_type_error(
            cx.clone(),
            format!("callTool params must be JSON-serializable: {err}"),
        )
    })?;
    if !params.is_object() {
        return Err(javy_type_error(
            cx.clone(),
            "callTool params must be a JSON object",
        ));
    }

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::ToolCall { seq, id, params })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

fn javy_emit_artifact_write(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let path_value = args.0.first().ok_or_else(|| {
        javy_type_error(cx.clone(), "writeArtifact path must be a non-empty string")
    })?;
    let path = javy::val_to_string(&cx, path_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if path.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "writeArtifact path must be a non-empty string",
        ));
    }

    let content_value = args
        .0
        .get(1)
        .ok_or_else(|| javy_type_error(cx.clone(), "writeArtifact content must be a string"))?;
    let content = javy::val_to_string(&cx, content_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;

    let content_type = args
        .0
        .get(2)
        .filter(|value| !value.is_null() && !value.is_undefined())
        .map(|value| javy::val_to_string(&cx, value.clone()))
        .transpose()
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::ArtifactWrite {
        seq,
        path,
        content,
        content_type,
    })
    .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

fn javy_emit_snippet_resolve(args: javy::Args<'_>) -> javy::quickjs::Result<u64> {
    let (cx, args) = args.release();
    let name_value = args
        .0
        .first()
        .ok_or_else(|| javy_type_error(cx.clone(), "snippet name must be a non-empty string"))?;
    let name = javy::val_to_string(&cx, name_value.clone())
        .map_err(|err| javy::to_js_error(cx.clone(), err))?;
    if name.trim().is_empty() {
        return Err(javy_type_error(
            cx.clone(),
            "snippet name must be a non-empty string",
        ));
    }

    let input_json = args
        .0
        .get(1)
        .map(|input| cx.json_stringify(input.clone()))
        .transpose()?
        .flatten()
        .map(|input| input.to_string())
        .transpose()?
        .unwrap_or_else(|| "{}".to_string());
    let input: Value = serde_json::from_str(&input_json).map_err(|err| {
        javy_type_error(
            cx.clone(),
            format!("snippet input must be JSON-serializable: {err}"),
        )
    })?;
    if !input.is_object() {
        return Err(javy_type_error(
            cx.clone(),
            "snippet input must be a JSON object",
        ));
    }

    let seq = next_runner_seq(&cx)?;

    runner_emit(CodeModeRunnerOutput::SnippetResolve { seq, name, input })
        .map_err(|err| javy_type_error(cx, err))?;
    Ok(seq)
}

fn javy_settle_tool_promise(
    runtime: &javy::Runtime,
    input: &CodeModeRunnerInput,
) -> Result<(), String> {
    let message = serde_json::to_string(input).map_err(|err| err.to_string())?;
    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<()> {
            let settle: javy::quickjs::Function<'_> =
                cx.globals().get("__labSettlePendingOperation")?;
            settle.call::<_, ()>((message,))?;
            Ok(())
        })
        .map_err(javy_error_message)?;
    runtime
        .resolve_pending_jobs()
        .map_err(|err| err.to_string())
}

fn javy_main_promise_state(runtime: &javy::Runtime) -> Result<JavyMainPromiseState, String> {
    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<JavyMainPromiseState> {
            let promise: javy::quickjs::Promise<'_> = cx.globals().get("__labMainPromise")?;
            match promise.result::<javy::quickjs::Value<'_>>() {
                None => Ok(JavyMainPromiseState::Pending),
                Some(Ok(val)) => {
                    // Serialize the resolved value to JSON via cx.json_stringify.
                    // undefined cannot be stringified and maps to None (no result).
                    // null is a real JSON value and must round-trip as Some(Null).
                    let result = if val.is_undefined() {
                        None
                    } else {
                        match cx.json_stringify(val) {
                            Ok(Some(json_str)) => {
                                let json_text = json_str.to_string()?;
                                let value: Value = match serde_json::from_str(&json_text) {
                                    Ok(value) => value,
                                    Err(err) => {
                                        return Ok(JavyMainPromiseState::Rejected(format!(
                                            "Code Mode result must be JSON-serializable: {err}"
                                        )));
                                    }
                                };
                                Some(value)
                            }
                            Ok(None) => {
                                return Ok(JavyMainPromiseState::Rejected(
                                    "Code Mode result must be JSON-serializable".to_string(),
                                ));
                            }
                            Err(err) => {
                                return Ok(JavyMainPromiseState::Rejected(format!(
                                    "Code Mode result must be JSON-serializable: {}",
                                    javy::from_js_error(cx.clone(), err)
                                )));
                            }
                        }
                    };
                    Ok(JavyMainPromiseState::Resolved(result))
                }
                Some(Err(err)) => {
                    // Capture a debug representation before `from_js_error`
                    // consumes `err`; used as a fallback when the stringified
                    // JS error is empty (lab-4uele).
                    let debug_fallback = format!("{err:?}");
                    let message = javy::from_js_error(cx.clone(), err).to_string();
                    let message = if message.is_empty() {
                        debug_fallback
                    } else {
                        message
                    };
                    Ok(JavyMainPromiseState::Rejected(message))
                }
            }
        })
        .map_err(javy_error_message)
}

fn javy_type_error(
    message_context: javy::quickjs::Ctx<'_>,
    message: impl Into<String>,
) -> javy::quickjs::Error {
    javy::to_js_error(message_context, anyhow::anyhow!(message.into()))
}

fn next_runner_seq(cx: &javy::quickjs::Ctx<'_>) -> javy::quickjs::Result<u64> {
    RUNNER_STATE
        .with(|state| {
            let mut state = state.borrow_mut();
            let state = state
                .as_mut()
                .ok_or_else(|| "runner state is not initialized".to_string())?;
            let seq = state.next_seq;
            state.next_seq = state.next_seq.saturating_add(1);
            Ok::<_, String>(seq)
        })
        .map_err(|err| javy_type_error(cx.clone(), err))
}

fn javy_error_message(error: javy::quickjs::Error) -> String {
    error.to_string()
}

fn runner_emit(output: CodeModeRunnerOutput) -> Result<(), String> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        serde_json::to_writer(&mut state.writer, &output).map_err(|err| err.to_string())?;
        state
            .writer
            .write_all(b"\n")
            .map_err(|err| err.to_string())?;
        state.writer.flush().map_err(|err| err.to_string())
    })
}

/// Distinguishes a clean end-of-input (EOF on stdin — the parent dropped this
/// pooled runner) from any other read failure. The pool loop treats `InputClosed`
/// as a normal shutdown signal and exits cleanly without emitting an `Error` line.
enum RunnerReadError {
    /// EOF: the parent closed/dropped the input stream.
    InputClosed,
    /// Any other failure (I/O error, malformed protocol JSON, uninitialized state).
    Other(String),
}

impl RunnerReadError {
    /// Collapse to a `CodeModeRunnerError` for mid-execution reads, where an EOF
    /// is a genuine fault (the parent died while a tool call was in flight)
    /// rather than the clean pool-shutdown path.
    fn into_runner_error(self) -> CodeModeRunnerError {
        match self {
            Self::InputClosed => "runner input closed".to_string().into(),
            Self::Other(message) => message.into(),
        }
    }
}

fn runner_read_input() -> Result<CodeModeRunnerInput, RunnerReadError> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| RunnerReadError::Other("runner state is not initialized".to_string()))?;
        let mut line = String::new();
        let read = state
            .reader
            .read_line(&mut line)
            .map_err(|err| RunnerReadError::Other(err.to_string()))?;
        if read == 0 {
            return Err(RunnerReadError::InputClosed);
        }
        serde_json::from_str(&line).map_err(|err| RunnerReadError::Other(err.to_string()))
    })
}
