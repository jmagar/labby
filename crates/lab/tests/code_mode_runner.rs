use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn read_protocol_line(reader: &mut BufReader<impl Read>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read runner output");
    assert!(!line.is_empty(), "runner closed stdout");
    serde_json::from_str(&line).expect("runner output must be JSON")
}

fn assert_done_undefined(done: &Value) {
    assert_eq!(done["result"]["state"], json!("undefined"));
    assert!(
        done["result"].get("value").is_none(),
        "undefined results must not carry a value: {}",
        done["result"]
    );
}

fn done_json_result(done: &Value) -> &Value {
    assert_eq!(
        done["result"]["state"],
        json!("json"),
        "done.result must carry a JSON value: {}",
        done["result"]
    );
    &done["result"]["value"]
}

#[test]
fn code_mode_runner_evaluates_js_in_a_minimal_host_environment() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stderr = child.stderr.take().expect("runner stderr");
    let mut stdout = BufReader::new(stdout);
    let code = r#"async () => {
        if (typeof process !== "undefined" || typeof require !== "undefined" ||
            typeof fetch !== "undefined" || typeof Deno !== "undefined" ||
            typeof Bun !== "undefined" || typeof XMLHttpRequest !== "undefined" ||
            typeof connect !== "undefined") {
          throw new Error("ambient host API exposed");
        }
        let dynamicImportWorked = false;
        try {
          await Function("return import('fs')")();
          dynamicImportWorked = true;
        } catch (_e) {}
        if (dynamicImportWorked) {
          throw new Error("dynamic import exposed host modules");
        }
        console.log("runner console check");
        const first = await callTool("lab::gateway.first", {"x": 1});
        if (first.ok) {
          await callTool("lab::gateway.second", {"from": first.value});
        }
        if (false) {
          await callTool("lab::gateway.never", {});
        }
    }"#;

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": code
        })
    )
    .expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "lab::gateway.first",
            "params": {"x": 1}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 0,
            "result": {"ok": true, "value": 42}
        })
    )
    .expect("write first result");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 1,
            "id": "lab::gateway.second",
            "params": {"from": 42}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 1,
            "result": {"ok": true}
        })
    )
    .expect("write second result");

    // Done now carries result (the function return value) and logs.
    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done");
    // The test code has no explicit return.
    assert_done_undefined(&done);
    // logs is always [] until Bead 3 console capture is implemented.
    assert_eq!(done["logs"], json!([]));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
    let mut stderr_text = String::new();
    stderr
        .read_to_string(&mut stderr_text)
        .expect("read runner stderr");
    // Console.log capture routes to stderr on the Javy path.
    assert!(stderr_text.contains("runner console check"));
}

/// The `search` action passes the caller's code to the runner *raw* (no
/// `normalize_user_code`). A non-function search input (e.g. `42`) must surface
/// as `server_error`, preserving the contract the old in-process
/// `evaluate_code_search` enforced. The runner's invoker requires the code to
/// evaluate to a function and throws a TypeError otherwise, which
/// `run_code_mode_runner_stdio` maps to `server_error`.
#[test]
fn code_mode_runner_rejects_non_function_search_input_as_server_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // Raw, un-normalized non-function code with the search-shaped catalog proxy.
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": "42",
            "proxy": "const tools = [];\n"
        })
    )
    .expect("write start");

    let error = read_protocol_line(&mut stdout);
    assert_eq!(error["type"], "error", "expected error, got: {error}");
    assert_eq!(
        error["kind"], "server_error",
        "non-function search input must surface as server_error, got: {error}"
    );

    let status = child.wait().expect("wait for runner");
    assert!(!status.success(), "runner must exit non-zero after error");
}

/// Malformed search JS (a syntax/parse error like `async () => {`) fails at the
/// top-level `cx.eval` before the main promise is ever created. That is a caller
/// mistake, so it must surface as `invalid_param` — matching the contract the old
/// in-process `evaluate_code_search` enforced — not `server_error`. (Contrast with
/// the non-function `42` case above, whose TypeError surfaces as a promise
/// rejection and stays `server_error`.)
#[test]
fn code_mode_runner_rejects_malformed_search_js_as_invalid_param() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // Raw, un-normalized malformed code (unterminated arrow body) with the
    // search-shaped catalog proxy.
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": "async () => {",
            "proxy": "const tools = [];\n"
        })
    )
    .expect("write start");

    let error = read_protocol_line(&mut stdout);
    assert_eq!(error["type"], "error", "expected error, got: {error}");
    assert_eq!(
        error["kind"], "invalid_param",
        "malformed search JS must surface as invalid_param, got: {error}"
    );

    let status = child.wait().expect("wait for runner");
    assert!(!status.success(), "runner must exit non-zero after error");
}

/// An *uncaught* structured rejection — the main promise throws an Error whose
/// message is a `{kind,message}` JSON payload — must preserve that `kind` in the
/// top-level error envelope rather than collapsing to a blanket `server_error`
/// (#2b). Contrast with the non-function `42` case, whose plain TypeError stays
/// `server_error`. (When the same structured error is *caught* inside the user
/// code, the run resolves normally — that path is covered by the fan-out tests.)
#[test]
fn code_mode_runner_preserves_kind_from_uncaught_structured_rejection() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // The main promise throws an Error carrying a structured {kind,message}.
    let code = r#"async () => {
        throw new Error(JSON.stringify({ kind: "rate_limited", message: "slow down" }));
    }"#;
    writeln!(stdin, "{}", json!({ "type": "start", "code": code })).expect("write start");

    let error = read_protocol_line(&mut stdout);
    assert_eq!(error["type"], "error", "expected error, got: {error}");
    assert_eq!(
        error["kind"], "rate_limited",
        "an uncaught structured rejection must preserve its kind, got: {error}"
    );

    let status = child.wait().expect("wait for runner");
    assert!(!status.success(), "runner must exit non-zero after error");
}

#[test]
fn code_mode_runner_tags_typed_array_results_as_base64() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": "async () => ({ bytes: new Uint8Array([1, 2, 255]) })"
        })
    )
    .expect("write start");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done");
    assert_eq!(
        done_json_result(&done)["bytes"],
        json!({
            "__labBinary": "base64",
            "type": "Uint8Array",
            "data": "AQL/"
        })
    );
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

#[test]
fn code_mode_runner_rejects_non_json_serializable_results() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": "async () => BigInt(1)"
        })
    )
    .expect("write start");

    let error = read_protocol_line(&mut stdout);
    assert_eq!(error["type"], "error", "expected error, got: {error}");
    assert_eq!(error["kind"], "invalid_param");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("JSON-serializable")),
        "unexpected error message: {error}"
    );

    let status = child.wait().expect("wait for runner");
    assert!(!status.success(), "runner must exit non-zero after error");
}

#[test]
fn code_mode_runner_preserves_binary_tool_args_and_results() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": "async () => { const bytes = await callTool('test::echo', { bytes: new Uint8Array([1, 2, 3]) }); return { isBytes: bytes instanceof Uint8Array, values: Array.from(bytes) }; }"
        })
    )
    .expect("write start");

    let call = read_protocol_line(&mut stdout);
    assert_eq!(call["type"], "tool_call");
    assert_eq!(
        call["params"]["bytes"],
        json!({
            "__labBinary": "base64",
            "type": "Uint8Array",
            "data": "AQID"
        })
    );
    let seq = call["seq"].as_u64().expect("seq");
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": seq,
            "result": {
                "__labBinary": "base64",
                "type": "Uint8Array",
                "data": "BAUG"
            }
        })
    )
    .expect("write result");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done");
    assert_eq!(
        done_json_result(&done),
        &json!({"isBytes": true, "values": [4, 5, 6]})
    );
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// The JS value codec must (a) honor `toJSON()` so a `Date` round-trips as its
/// ISO string instead of `{}`, (b) tag an `Int16Array` with its real class so
/// the decoder can reconstruct the correct element width, and (c) reconstruct
/// the recorded class from a binary-result sentinel — an `Int16Array` sentinel
/// must decode back into an `Int16Array`, not collapse to `Uint8Array`.
#[test]
fn code_mode_runner_round_trips_date_typed_array_and_array_buffer() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // Args carry a Date, an Int16Array([256, -1]), and an ArrayBuffer.
    let code = r"async () => {
        const buf = new Uint8Array([9, 8, 7]).buffer;
        const echoed = await callTool('test::echo', {
          when: new Date(0),
          ints: new Int16Array([256, -1]),
          raw: buf
        });
        return {
          isInt16: echoed instanceof Int16Array,
          ctor: echoed && echoed.constructor && echoed.constructor.name,
          values: Array.from(echoed)
        };
    }";

    writeln!(stdin, "{}", json!({ "type": "start", "code": code })).expect("write start");

    let call = read_protocol_line(&mut stdout);
    assert_eq!(call["type"], "tool_call");
    // (a) Date honored toJSON() → ISO string, not {}.
    assert_eq!(
        call["params"]["when"],
        json!("1970-01-01T00:00:00.000Z"),
        "Date must encode via toJSON() to its ISO string, got: {}",
        call["params"]["when"]
    );
    // (b) Int16Array tagged with its real class and little-endian bytes.
    assert_eq!(
        call["params"]["ints"],
        json!({ "__labBinary": "base64", "type": "Int16Array", "data": "AAH//w==" })
    );
    // ArrayBuffer tagged as ArrayBuffer.
    assert_eq!(call["params"]["raw"]["__labBinary"], json!("base64"));
    assert_eq!(call["params"]["raw"]["type"], json!("ArrayBuffer"));

    let seq = call["seq"].as_u64().expect("seq");
    // (c) Result is an Int16Array sentinel: bytes [1,0,2,0,3,0] → [1,2,3].
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": seq,
            "result": { "__labBinary": "base64", "type": "Int16Array", "data": "AQACAAMA" }
        })
    )
    .expect("write result");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done");
    assert_eq!(
        done_json_result(&done),
        &json!({ "isInt16": true, "ctor": "Int16Array", "values": [1, 2, 3] }),
        "Int16Array result sentinel must reconstruct as Int16Array, got: {}",
        done["result"]
    );
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

#[test]
fn code_mode_runner_fans_out_promise_all_tool_calls() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);
    let code = r#"async () => {
        const [first, second] = await Promise.all([
          callTool("lab::gateway.first", {"x": 1}),
          callTool("lab::gateway.second", {"x": 2})
        ]);
        await callTool("lab::gateway.after", {"sum": first.value + second.value});
    }"#;

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "start",
            "code": code
        })
    )
    .expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "lab::gateway.first",
            "params": {"x": 1}
        })
    );
    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 1,
            "id": "lab::gateway.second",
            "params": {"x": 2}
        })
    );

    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 1,
            "result": {"value": 20}
        })
    )
    .expect("write second result");
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 0,
            "result": {"value": 10}
        })
    )
    .expect("write first result");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 2,
            "id": "lab::gateway.after",
            "params": {"sum": 30}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({
            "type": "tool_result",
            "seq": 2,
            "result": {"ok": true}
        })
    )
    .expect("write after result");

    // Done now carries result (the function return value) and logs.
    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done");
    // The test code has no explicit return.
    assert_done_undefined(&done);
    // logs is always [] until Bead 3 console capture is implemented.
    assert_eq!(done["logs"], json!([]));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// Verify that Done carries a non-null result when the async function explicitly
/// returns a value. This tests the result field extraction fix (bead lab-y08q1.1).
#[test]
fn code_mode_runner_done_carries_return_value() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // The function fetches one tool result and returns it directly.
    let code = r#"async () => {
        const result = await callTool("test::ping", {"msg": "hello"});
        return result;
    }"#;

    writeln!(stdin, "{}", json!({"type": "start", "code": code})).expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "test::ping",
            "params": {"msg": "hello"}
        })
    );
    writeln!(
        stdin,
        "{}",
        json!({"type": "tool_result", "seq": 0, "result": {"pong": true}})
    )
    .expect("write tool result");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done", "expected done message");
    // The function returned the tool result — should be non-null.
    assert_eq!(
        done_json_result(&done),
        &json!({"pong": true}),
        "done.result must carry the function return value"
    );
    assert_eq!(done["logs"], json!([]), "logs must be empty until Bead 3");
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// Verify that tool errors are rejected with a JSON-encoded CodeModeError object,
/// not a plain "kind: message" string. This tests the error format fix (bead lab-y08q1.1).
#[test]
fn code_mode_runner_tool_error_produces_json_encoded_error() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // The function catches the error and returns the parsed CodeModeError shape.
    // If the rejection is plain text, JSON.parse will throw SyntaxError and
    // the function itself will error, causing Done to never appear.
    let code = r#"async () => {
        try {
            await callTool("test::fail", {});
        } catch (e) {
            const parsed = JSON.parse(String(e.message));
            return {caught: true, kind: parsed.kind, msg: parsed.message};
        }
    }"#;

    writeln!(stdin, "{}", json!({"type": "start", "code": code})).expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "test::fail",
            "params": {}
        })
    );
    // Inject a tool_error — the runner must reject the promise with JSON.
    writeln!(
        stdin,
        "{}",
        json!({"type": "tool_error", "seq": 0, "kind": "server_error", "message": "upstream exploded"})
    )
    .expect("write tool_error");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(
        done["type"], "done",
        "expected done message — if missing, JSON.parse threw SyntaxError"
    );
    // The catch block should have parsed the JSON error and returned the structured result.
    let result = done_json_result(&done);
    assert_eq!(result["caught"], json!(true));
    assert_eq!(result["kind"], json!("server_error"));
    assert_eq!(result["msg"], json!("upstream exploded"));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// Verify that a tool error in the middle of a fan-out does NOT abort the run
/// (bead lab-xvff5). With `Promise.allSettled`, one rejected callTool settles as
/// `rejected` while siblings still resolve, and the function returns normally —
/// the runner must keep processing after the mid-fan-out error and emit Done with
/// both outcomes.
#[test]
fn code_mode_runner_tool_error_does_not_abort_fan_out() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    let code = r#"async () => {
        const settled = await Promise.allSettled([
          callTool("test::fail", {}),
          callTool("test::ok", {})
        ]);
        return settled.map(s => {
          if (s.status === "rejected") {
            const parsed = JSON.parse(String(s.reason.message));
            return {status: s.status, kind: parsed.kind};
          }
          return {status: s.status, value: s.value};
        });
    }"#;

    writeln!(stdin, "{}", json!({"type": "start", "code": code})).expect("write start");

    // Both callTool requests are emitted before either is answered (parallel fan-out).
    let first = read_protocol_line(&mut stdout);
    let second = read_protocol_line(&mut stdout);
    assert_eq!(first["type"], "tool_call");
    assert_eq!(second["type"], "tool_call");
    assert_eq!(first["seq"], json!(0));
    assert_eq!(second["seq"], json!(1));

    // Fail seq 0 mid-fan-out; resolve seq 1 normally.
    writeln!(
        stdin,
        "{}",
        json!({"type": "tool_error", "seq": 0, "kind": "rate_limited", "message": "slow down"})
    )
    .expect("write tool_error");
    writeln!(
        stdin,
        "{}",
        json!({"type": "tool_result", "seq": 1, "result": {"pong": true}})
    )
    .expect("write tool_result");

    // The run must NOT have aborted on the seq-0 failure — Done arrives with both outcomes.
    let done = read_protocol_line(&mut stdout);
    assert_eq!(
        done["type"], "done",
        "a mid-fan-out tool error must not abort the run"
    );
    let result = done_json_result(&done);
    assert_eq!(result[0]["status"], json!("rejected"));
    assert_eq!(result[0]["kind"], json!("rate_limited"));
    assert_eq!(result[1]["status"], json!("fulfilled"));
    assert_eq!(result[1]["value"], json!({"pong": true}));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// Drive the runner with `code` that makes exactly one `callTool` and returns
/// its result. Answers the single tool call with `tool_result`, then asserts
/// Done carries the returned value. Used to prove a given code shape executes
/// end-to-end through the runner's arrow-function wrapper (bead lab-vkwfa).
fn assert_single_call_round_trip(code: &str, expected_result: Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    writeln!(stdin, "{}", json!({"type": "start", "code": code})).expect("write start");

    let call = read_protocol_line(&mut stdout);
    assert_eq!(
        call["type"], "tool_call",
        "expected a tool_call, got: {call}"
    );
    let seq = call["seq"].as_u64().expect("seq");
    writeln!(
        stdin,
        "{}",
        json!({"type": "tool_result", "seq": seq, "result": {"pong": true}})
    )
    .expect("write tool result");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done", "expected done, got: {done}");
    assert_eq!(
        done_json_result(&done),
        &expected_result,
        "done.result must carry the function return value"
    );
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// FIX 1 (bead lab-vkwfa): a `function main` BODY form, after `normalize_user_code`,
/// must execute end-to-end through the runner's arrow-function wrapper. This is
/// non-vacuous: the raw body form is normalized to a wrapper that calls the
/// named function before being piped to the runner, exactly as the broker does.
#[test]
fn normalized_function_main_form_executes_end_to_end() {
    let body = "async function main() { return await callTool(\"test::ping\", {}); }";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    // Guard: normalize must produce a wrapper that invokes the named function,
    // otherwise this test would be vacuous (the raw form happens to wrap too).
    assert!(
        normalized.starts_with("async () => {") && normalized.contains("return main();"),
        "normalize must emit a named-function wrapper, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}

/// lab-12fm5: the runtime `codemode.*` proxy travels through the Start protocol
/// and routes `codemode.demo.ping(...)` to `callTool("demo::ping", ...)`
/// end-to-end. The proxy here is the exact shape `generate_js_proxy` emits.
/// Non-vacuous: with no proxy, `codemode` is undefined and the code would throw.
#[test]
fn codemode_proxy_routes_through_call_tool() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn code mode runner");

    let mut stdin = child.stdin.take().expect("runner stdin");
    let stdout = child.stdout.take().expect("runner stdout");
    let mut stdout = BufReader::new(stdout);

    // Minimal proxy mirroring generate_js_proxy's output shape (var codemode = {};
    // codemode["demo"] = { ping: function(p) { return callTool(...); } };).
    let proxy = "var codemode = {};\n\
        codemode[\"demo\"] = {\n\
          \"ping\": function(p) { return callTool(\"demo::ping\", p == null ? {} : p); }\n\
        };\n";
    // Guard the in-sandbox `codemode` type, then route through the proxy.
    let code = "async () => { \
        if (typeof codemode !== \"object\") { throw new Error(\"codemode not object\"); } \
        return await codemode.demo.ping({x: 1}); \
    }";

    writeln!(
        stdin,
        "{}",
        json!({"type": "start", "code": code, "proxy": proxy})
    )
    .expect("write start");

    // The proxy must have emitted a callTool to the dotted upstream id.
    let call = read_protocol_line(&mut stdout);
    assert_eq!(
        call["type"], "tool_call",
        "expected a tool_call, got: {call}"
    );
    assert_eq!(
        call["id"], "demo::ping",
        "proxy must route to the dotted upstream tool id"
    );
    assert_eq!(call["params"], json!({"x": 1}), "proxy must forward params");
    let seq = call["seq"].as_u64().expect("seq");
    writeln!(
        stdin,
        "{}",
        json!({"type": "tool_result", "seq": seq, "result": {"pong": true}})
    )
    .expect("write tool result");

    let done = read_protocol_line(&mut stdout);
    assert_eq!(done["type"], "done", "expected done, got: {done}");
    assert_eq!(
        done_json_result(&done),
        &json!({"pong": true}),
        "codemode.demo.ping must resolve to the tool result"
    );
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}

/// FIX 1 (bead lab-vkwfa): an `export default async function` form, after
/// `normalize_user_code`, must execute end-to-end. Non-vacuous: the raw form
/// would fail because `export default` is not valid in a script wrapper;
/// normalize now emits an async wrapper that invokes the exported function.
#[test]
fn normalized_export_default_form_executes_end_to_end() {
    let body = "export default async function() { return await callTool(\"test::ping\", {}); }";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    assert!(
        normalized.starts_with("async () =>")
            && normalized.contains("async function")
            && !normalized.contains("export default"),
        "normalize must emit executable script code without export syntax, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}

/// An arrow function in `export default` position *with a prologue*
/// (`const tool = "..."; export default async () => callTool(tool, {})`) must
/// execute end-to-end. Boa's parse_module cannot parse an arrow default export,
/// and its AST arms drop the prologue, so this used to loose-wrap into invalid JS
/// (or lose `tool`). Non-vacuous: the arrow references the prologue binding
/// `tool`, so if the prologue were dropped, `tool` would be undefined and no
/// tool_call would fire (the round-trip helper would fail waiting for one).
#[test]
fn normalized_export_default_arrow_with_prologue_executes_end_to_end() {
    let body = "const tool = \"test::ping\";\n\
                export default async () => await callTool(tool, {});";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    assert!(
        normalized.starts_with("async () =>") && !normalized.contains("export default"),
        "normalize must emit executable script code without export syntax, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}

/// The same prologue-preservation must hold for the *AST* path (a plain — non
/// async — arrow in `export default` position parses as a DefaultAssignmentExpression,
/// so it goes through `normalize_module_code`, not the textual fallback). Boa
/// re-renders the arrow on round-trip, so string assertions can't prove the
/// prologue binding is actually in runtime scope — this runs it. Non-vacuous: the
/// arrow references the prologue `const tool`, so a dropped prologue would leave
/// `tool` undefined and emit no tool_call.
#[test]
fn normalized_export_default_plain_arrow_with_prologue_executes_end_to_end() {
    let body = "const tool = \"test::ping\";\n\
                export default () => callTool(tool, {});";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    assert!(
        normalized.starts_with("async () =>") && !normalized.contains("export default"),
        "normalize must emit executable script code without export syntax, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}

/// The AST *function* arm with a prologue (`const tool = "...";
/// export default async function() { ... }`) goes through `normalize_module_code`
/// → `wrap_default_fn_as_iife` nested inside the prologue wrapper — a different
/// shape (double IIFE) than the arrow arms. Run it to prove the prologue binding
/// is in runtime scope for the exported function too. Non-vacuous: the function
/// references the prologue `const tool`.
#[test]
fn normalized_export_default_function_with_prologue_executes_end_to_end() {
    let body = "const tool = \"test::ping\";\n\
                export default async function() { return await callTool(tool, {}); }";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    assert!(
        normalized.starts_with("async () =>") && !normalized.contains("export default"),
        "normalize must emit executable script code without export syntax, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}

/// A *named* export the default references, with an async-arrow default — the
/// textual fallback path (boa can't parse an async-arrow `export default`, so the
/// whole module fails to parse and the prologue is recovered textually). The
/// named export's `export` keyword must be stripped, otherwise it is a syntax
/// error inside the wrapper and nothing runs. Non-vacuous: the default calls the
/// `tool` binding from `export const`, so a dropped/un-stripped export emits no
/// tool_call.
#[test]
fn normalized_async_arrow_default_with_named_export_executes_end_to_end() {
    let body = "export const tool = \"test::ping\";\n\
                export default async () => await callTool(tool, {});";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    assert!(
        normalized.starts_with("async () =>") && !normalized.contains("export "),
        "normalize must strip every `export` keyword, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}

/// Multiple prologue statements — a function declaration the default closes over
/// plus a `const` computed from it — must all land in runtime scope. Routes
/// through the AST path (plain-arrow default → DefaultAssignmentExpression),
/// exercising the `prologue.join("\n")` rendering rather than the single-`const`
/// shape the other e2e tests use. Non-vacuous: a dropped prologue leaves `mk`/
/// `tool` undefined, so no tool_call fires.
#[test]
fn normalized_export_default_multi_statement_prologue_executes_end_to_end() {
    let body = "function mk() { return \"test::ping\"; }\n\
                const tool = mk();\n\
                export default () => callTool(tool, {});";
    let normalized = labby::dispatch::gateway::code_mode::normalize_user_code(body);
    assert!(
        normalized.starts_with("async () =>") && !normalized.contains("export default"),
        "normalize must emit executable script code without export syntax, got: {normalized}"
    );
    assert_single_call_round_trip(&normalized, json!({"pong": true}));
}
