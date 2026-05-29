use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn read_protocol_line(reader: &mut BufReader<impl Read>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read runner output");
    assert!(!line.is_empty(), "runner closed stdout");
    serde_json::from_str(&line).expect("runner output must be JSON")
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
            typeof Bun !== "undefined") {
          throw new Error("ambient host API exposed");
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
    // The test code has no explicit return — result is None (serialized as null).
    assert!(done["result"].is_null());
    // logs is always [] until Bead 3 console capture is implemented.
    assert_eq!(done["logs"], json!([]));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
    let mut stderr_text = String::new();
    stderr
        .read_to_string(&mut stderr_text)
        .expect("read runner stderr");
    // Console.log capture routes to stderr only on the WASM/Javy path; the
    // Boa path defers console capture to Bead 3 (boa_runtime integration).
    #[cfg(feature = "code_mode_wasm")]
    assert!(stderr_text.contains("runner console check"));
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
    // The test code has no explicit return — result is None (serialized as null).
    assert!(done["result"].is_null());
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
        const result = await callTool("upstream::test::ping", {"msg": "hello"});
        return result;
    }"#;

    writeln!(stdin, "{}", json!({"type": "start", "code": code})).expect("write start");

    assert_eq!(
        read_protocol_line(&mut stdout),
        json!({
            "type": "tool_call",
            "seq": 0,
            "id": "upstream::test::ping",
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
        done["result"],
        json!({"pong": true}),
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
            await callTool("upstream::test::fail", {});
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
            "id": "upstream::test::fail",
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
    assert_eq!(done["result"]["caught"], json!(true));
    assert_eq!(done["result"]["kind"], json!("server_error"));
    assert_eq!(done["result"]["msg"], json!("upstream exploded"));
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
          callTool("upstream::test::fail", {}),
          callTool("upstream::test::ok", {})
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
    assert_eq!(done["result"][0]["status"], json!("rejected"));
    assert_eq!(done["result"][0]["kind"], json!("rate_limited"));
    assert_eq!(done["result"][1]["status"], json!("fulfilled"));
    assert_eq!(done["result"][1]["value"], json!({"pong": true}));
    let status = child.wait().expect("wait for runner");
    assert!(status.success(), "runner exited with {status}");
}
