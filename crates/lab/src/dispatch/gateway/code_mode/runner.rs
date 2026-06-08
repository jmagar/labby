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

    let result = run_code_mode_runner();
    if let Err(err) = result {
        drop(runner_emit(CodeModeRunnerOutput::Error {
            kind: err.kind,
            message: err.message,
        }));
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
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

/// Classify a main-promise rejection message into an error kind:
/// 1. If the message is a JSON object carrying a `kind`, preserve that kind
///    (structured tool-error rejections re-raised through the sandbox).
/// 2. Else if it mentions `JSON-serializable`, the result could not be
///    serialized — a caller mistake → `invalid_param`.
/// 3. Otherwise it is a runtime throw (e.g. the non-function TypeError) →
///    `server_error`.
fn classify_rejection(message: String) -> CodeModeRunnerError {
    // An uncaught structured tool-error rejection arrives as the JS Error's
    // stringified form, e.g. `Error: {"kind":"upstream_error","message":"..."}`
    // followed by a `\n    at ...` stack trace that QuickJS appends. Take only
    // the first line and strip the `Error: ` prefix before attempting to recover
    // the structured `{kind,message}` payload — the trailing stack would
    // otherwise make `serde_json::from_str` reject the candidate as having
    // trailing garbage and silently fall through to `server_error`.
    let first_line = message.lines().next().unwrap_or(&message);
    let json_candidate = first_line.strip_prefix("Error: ").unwrap_or(first_line);
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(json_candidate)
        && let Some(kind) = map.get("kind").and_then(Value::as_str)
    {
        return CodeModeRunnerError {
            kind: kind.to_string(),
            message,
        };
    }
    if message.contains("JSON-serializable") {
        return CodeModeRunnerError {
            kind: "invalid_param".to_string(),
            message,
        };
    }
    CodeModeRunnerError {
        kind: "server_error".to_string(),
        message,
    }
}

fn run_code_mode_runner() -> Result<(), CodeModeRunnerError> {
    let CodeModeRunnerInput::Start { code, proxy } = runner_read_input()? else {
        return Err("runner expected start message".to_string().into());
    };

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
                let input = runner_read_input()?;
                javy_settle_tool_promise(&runtime, &input)?;
            }
        }
    };

    runner_emit(CodeModeRunnerOutput::Done {
        result: CodeModeRunnerResult::from_response_result(resolved_result),
        logs: Vec::new(),
    })
    .map_err(CodeModeRunnerError::from)
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
    globalThis.__labPendingToolCalls.set(seq, {{ resolve, reject }});
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
  const contentType = typeof options.contentType === "string" ? options.contentType : null;
  return new Promise((resolve, reject) => {{
    const seq = globalThis.__labEmitArtifactWrite(path, content, contentType);
    globalThis.__labPendingToolCalls.set(seq, {{ resolve, reject }});
  }});
}};
globalThis.__labSettleToolCall = (message) => {{
  const input = JSON.parse(message);
  const pending = globalThis.__labPendingToolCalls.get(input.seq);
  if (!pending) {{
    throw new Error("runner received a response for an unknown tool call");
  }}
  globalThis.__labPendingToolCalls.delete(input.seq);
  if (input.type === "tool_result") {{
    pending.resolve(__labDecodeResult(input.result));
    return;
  }}
  if (input.type === "tool_error") {{
    // Reject with a JS string whose content is JSON-encoded CodeModeError so that
    // JSON.parse(String(e.message)) in the sandbox recovers the structured error.
    // Both the Javy and Boa paths had the same plain-string bug ("kind: message").
    pending.reject(new Error(JSON.stringify({{kind: input.kind, message: input.message}})));
    return;
  }}
  throw new Error("runner received unexpected protocol message");
}};
{proxy}
globalThis.__labMainPromise = (async () => {{
{invoker}}})();
"#,
        codec = CODE_MODE_VALUE_CODEC_JS,
        invoker = invoker,
        proxy = proxy,
    )
}

#[cfg(test)]
pub(in crate::dispatch::gateway::code_mode) fn wrap_code_mode_for_test(
    code: &str,
    proxy: &str,
) -> String {
    wrap_code_mode(code, proxy)
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

fn javy_settle_tool_promise(
    runtime: &javy::Runtime,
    input: &CodeModeRunnerInput,
) -> Result<(), String> {
    let message = serde_json::to_string(input).map_err(|err| err.to_string())?;
    runtime
        .context()
        .with(|cx| -> javy::quickjs::Result<()> {
            let settle: javy::quickjs::Function<'_> = cx.globals().get("__labSettleToolCall")?;
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
            state.next_seq += 1;
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

fn runner_read_input() -> Result<CodeModeRunnerInput, String> {
    RUNNER_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| "runner state is not initialized".to_string())?;
        let mut line = String::new();
        let read = state
            .reader
            .read_line(&mut line)
            .map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("runner input closed".to_string());
        }
        serde_json::from_str(&line).map_err(|err| err.to_string())
    })
}
